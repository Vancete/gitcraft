//! Reading the repository in the background and rebuilding the graph rows from what was read.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::{DiffKey, DiffView, MAX_HISTORY, Msg, RepoTab, WIP_HASH, spawn};
use crate::forge::{self, Forge};
use crate::git::{self, Commit, Identity, Refs, Remote, Stash, WorkStatus, Worktree};
use crate::graph;
use crate::settings::Settings;

pub(super) struct Loaded {
    pub(super) refs: Refs,
    pub(super) history: Vec<Commit>,
    pub(super) work: WorkStatus,
    pub(super) worktrees: Vec<Worktree>,
    pub(super) stashes: Vec<Stash>,
    pub(super) remotes: Vec<Remote>,
    pub(super) identity: Identity,
    /// How many commits were asked for (more exist when that many came back).
    pub(super) requested: usize,
}

impl RepoTab {
    /// Re-reads refs, history, working tree, worktrees, stashes and remotes in the background.
    pub fn reload(&mut self, ctx: &egui::Context) {
        if self.loading {
            // What changed may have been read before it changed: read again once this is in.
            self.reload_again = true;
            return;
        }
        self.loading = true;
        let path = self.path.clone();
        // Keep the pages already loaded.
        let count = self.history.len().clamp(git::LOG_PAGE, MAX_HISTORY);
        spawn(&self.tx, ctx, move || {
            Msg::Loaded((|| {
                let refs = git::refs(&path)?;
                let history = git::log(&path, 0, count)?;
                let work = git::status(&path)?;
                // Very old gits have no `worktree list`; the section just stays empty.
                let worktrees = git::worktrees(&path).unwrap_or_default();
                let stashes = git::stashes(&path).unwrap_or_default();
                let remotes = git::remotes(&path).unwrap_or_default();
                let identity = git::identity(&path);
                Ok(Box::new(Loaded {
                    refs,
                    history,
                    work,
                    worktrees,
                    stashes,
                    remotes,
                    identity,
                    requested: count,
                }))
            })())
        });
    }

    pub(super) fn reload_status(&mut self, ctx: &egui::Context) {
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || Msg::Status(git::status(&path)));
    }

    pub(super) fn load_more(&mut self, ctx: &egui::Context) {
        if self.loading_more || !self.has_more || self.history.len() >= MAX_HISTORY {
            return;
        }
        self.loading_more = true;
        let path = self.path.clone();
        let skip = self.history.len();
        let generation = self.history_gen;
        spawn(&self.tx, ctx, move || {
            Msg::More(generation, git::log(&path, skip, git::LOG_PAGE))
        });
    }

    pub(super) fn load_prs(&mut self, ctx: &egui::Context) {
        let Some(forge) = self.forge else { return };
        self.prs_loaded = Some(Instant::now());
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || Msg::Prs(forge::list(&path, forge)));
    }

    /// Takes in what `reload` read, then reads again if a reload was asked for meanwhile.
    pub(super) fn loaded(&mut self, result: Result<Box<Loaded>, String>, ctx: &egui::Context) {
        self.loading = false;
        match result {
            Ok(loaded) => self.take_loaded(*loaded, ctx),
            Err(e) => self.load_error = Some(e),
        }
        if std::mem::take(&mut self.reload_again) {
            self.reload(ctx);
        }
    }

    fn take_loaded(&mut self, loaded: Loaded, ctx: &egui::Context) {
        self.load_error = None;
        // A page asked for before this was read past the old history, which
        // this one replaces: it would leave a gap.
        self.history_gen += 1;
        self.loading_more = false;
        self.refs = loaded.refs;
        self.has_more = loaded.history.len() >= loaded.requested;
        self.history = loaded.history;
        self.work = loaded.work;
        self.worktrees = loaded.worktrees;
        // Stash numbers shift when one is pushed or dropped; follow the commit.
        let viewed = self
            .stash_view
            .and_then(|i| self.stashes.get(i))
            .map(|s| s.hash.clone());
        self.stashes = loaded.stashes;
        self.stash_view = viewed.and_then(|h| self.stashes.iter().position(|s| s.hash == h));
        self.identity = loaded.identity;
        let forge = forge::detect(&loaded.remotes);
        self.remotes = loaded.remotes;
        if forge != self.forge {
            self.forge = forge;
            self.prs.clear();
            self.prs_loaded = None;
        }
        if self
            .prs_loaded
            .is_none_or(|t| t.elapsed() > Duration::from_secs(120))
        {
            self.load_prs(ctx);
        }
        self.rebuild(ctx);
        self.hint_avatars(ctx);
        self.reload_compare(ctx);
    }

    /// Appends a page of older history that `load_more` read.
    pub(super) fn more_loaded(&mut self, result: Result<Vec<Commit>, String>, ctx: &egui::Context) {
        self.loading_more = false;
        match result {
            Ok(page) => {
                self.has_more = page.len() == git::LOG_PAGE;
                // A reload in between may have loaded these already.
                let known: HashSet<&str> = self.history.iter().map(|c| c.hash.as_str()).collect();
                let fresh: Vec<Commit> = page
                    .into_iter()
                    .filter(|c| !known.contains(c.hash.as_str()))
                    .collect();
                self.history.extend(fresh);
                self.rebuild(ctx);
            }
            Err(e) => self.status = Some((e, true)),
        }
    }

    /// Rebuilds the graph rows from `history` and `work`, keeping the selection.
    pub(super) fn rebuild(&mut self, ctx: &egui::Context) {
        let previous = self.selected_hash();
        self.rebuild_rows();
        self.restore_selection(previous, ctx);
        self.take_prepared_message();
        self.refresh_work_diff(ctx);
        if let Some(hash) = self.pending_goto.clone() {
            self.goto(&hash, ctx);
        }
    }

    /// The WIP pseudo-commit (when the tree is dirty), then `history` with its stashes, laid out
    /// as the graph.
    fn rebuild_rows(&mut self) {
        self.commits.clear();
        if !self.work.is_clean() {
            self.commits.push(Commit {
                hash: WIP_HASH.to_owned(),
                parents: if self.refs.head.is_empty() {
                    Vec::new()
                } else {
                    vec![self.refs.head.clone()]
                },
                time: git::now(),
                ..Commit::default()
            });
        }
        // Each stash sits right above the commit it was made on, as a commit on no branch.
        for commit in &self.history {
            for stash in self.stashes.iter().filter(|s| s.base == commit.hash) {
                self.commits.push(Commit {
                    hash: stash.hash.clone(),
                    parents: vec![stash.base.clone()],
                    author: stash.author.clone(),
                    email: stash.email.clone(),
                    time: stash.time,
                    date: stash.date.clone(),
                    summary: stash.message.clone(),
                    ..Commit::default()
                });
            }
            self.commits.push(commit.clone());
        }
        self.graph = graph::layout(&self.commits);
        self.index = self
            .commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.hash.clone(), i))
            .collect();
    }

    /// Keeps the selection; otherwise starts on the changes, or HEAD.
    fn restore_selection(&mut self, previous: Option<String>, ctx: &egui::Context) {
        let mut keep = previous.as_ref().and_then(|h| self.index.get(h).copied());
        if std::mem::take(&mut self.focus_wip) && self.index.contains_key(WIP_HASH) {
            keep = None;
            self.stash_view = None;
        }
        if self.stash_view.is_some() {
            self.selected = self
                .stash_view
                .and_then(|i| self.index.get(&self.stashes.get(i)?.hash).copied());
            return;
        }
        let target = keep
            .or_else(|| self.index.get(WIP_HASH).copied())
            .or_else(|| self.index.get(&self.refs.head).copied());
        match target {
            Some(row) if keep == Some(row) => self.selected = Some(row),
            Some(row) => {
                self.selected = None;
                self.select(row, ctx);
                self.scroll_to = Some(row);
            }
            None => {
                self.selected = None;
                self.detail = None;
            }
        }
    }

    /// A merge or cherry-pick leaves a message to commit with.
    fn take_prepared_message(&mut self) {
        if let Some(msg) = &self.work.prepared_message
            && self.commit_summary.is_empty()
            && self.commit_body.is_empty()
        {
            let (summary, body) = msg.split_once('\n').unwrap_or((msg.as_str(), ""));
            self.commit_summary = summary.trim().to_owned();
            self.commit_body = body.trim().to_owned();
        }
    }

    /// The diff of a file that's no longer listed (staged, committed…) is stale.
    fn refresh_work_diff(&mut self, ctx: &egui::Context) {
        let Some(DiffView {
            key: DiffKey::Work(file),
            ..
        }) = &self.diff
        else {
            return;
        };
        let still = self
            .work
            .staged
            .iter()
            .chain(&self.work.unstaged)
            .chain(&self.work.conflicted)
            .any(|f| f == file);
        let key = DiffKey::Work(file.clone());
        // The conflict editor keeps what was chosen (or Mergiraf's answer) while the file
        // is as it was opened.
        let editing = self
            .conflict
            .as_ref()
            .is_some_and(|ed| ed.path == file.path && !ed.changed_on_disk(&self.path));
        if still && (file.area != git::Area::Conflicted || editing) {
            // Shown as it was until the new diff is read (no flicker after staging lines).
            self.refresh_diff(ctx);
        } else {
            self.diff = None;
            self.conflict = None;
            if still {
                self.open_diff(key, ctx);
            }
        }
    }

    /// Once per tab, on a GitHub / GitLab repository: tells the avatar loader where its authors'
    /// account pictures are.
    pub(super) fn hint_avatars(&mut self, ctx: &egui::Context) {
        if self.avatars_hinted || !Settings::get(ctx).avatars {
            return;
        }
        let Some(forge) = self.forge else { return };
        let Some((host, path)) = self
            .remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| self.remotes.first())
            .and_then(|r| forge::location(&r.url))
        else {
            return;
        };
        self.avatars_hinted = true;
        let repo = self.path.clone();
        let mut emails: Vec<String> = self.history.iter().map(|c| c.email.clone()).collect();
        emails.sort();
        emails.dedup();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            match forge {
                Forge::GitHub => {
                    for (email, url) in forge::github_avatars(&repo, &host, &path) {
                        crate::avatars::hint(&email, crate::avatars::Hint::Url(url));
                    }
                }
                Forge::GitLab => {
                    for email in emails {
                        crate::avatars::hint(&email, crate::avatars::Hint::GitLab(host.clone()));
                    }
                }
            }
            ctx.request_repaint();
        });
    }
}
