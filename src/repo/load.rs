//! Reading the repository in the background and rebuilding the graph rows from what was read.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::branch_visibility::Visibility;
use super::{DiffKey, DiffView, MAX_HISTORY, Msg, RepoTab, WIP_HASH, spawn};
use crate::forge::{self, Forge};
use crate::git::{self, Commit, Identity, Refs, Remote, Stash, Submodule, WorkStatus, Worktree};
use crate::graph;
use crate::settings::Settings;

pub(super) struct Loaded {
    pub(super) refs: Refs,
    pub(super) history: Vec<Arc<Commit>>,
    pub(super) work: WorkStatus,
    pub(super) worktrees: Vec<Worktree>,
    pub(super) stashes: Vec<Stash>,
    pub(super) remotes: Vec<Remote>,
    pub(super) submodules: Vec<Submodule>,
    /// It keeps files in Git LFS, and git-lfs is installed.
    pub(super) lfs: bool,
    pub(super) identity: Identity,
    /// How many commits were asked for (more exist when that many came back).
    pub(super) requested: usize,
    /// What `history` was logged from.
    pub(super) log_from: Option<Vec<String>>,
    /// Which read this is (see `RepoTab::reads`).
    pub(super) read: u64,
}

impl RepoTab {
    /// Re-reads refs, history, working tree, worktrees, stashes, remotes and submodules in the
    /// background.
    pub fn reload(&mut self, ctx: &egui::Context) {
        // Changes seen on disk so far are read by this reload (or the one queued after the
        // current one): reloading for them again would read the same.
        self.fs_pending = None;
        self.deferred = false;
        if self.loading {
            // What changed may have been read before it changed: read again once this is in.
            self.reload_again = true;
            return;
        }
        self.loading = true;
        let read = self.next_read();
        let path = self.path.clone();
        // Keep the pages already loaded.
        let count = self.history.len().clamp(git::LOG_PAGE, MAX_HISTORY);
        let mut visibility = Visibility::load(ctx, self.repo_key());
        spawn(&self.tx, ctx, move || {
            Msg::Loaded((|| {
                let refs = git::refs(&path)?;
                visibility.prune(&refs);
                let log_from = visibility.revisions(&refs);
                // Independent reads, side by side: a reload waits for the slowest of them
                // rather than for all of them in turn.
                let path = path.as_path();
                let (history, work, worktrees, stashes, remotes, submodules, lfs, identity) =
                    std::thread::scope(|scope| {
                        let history = scope.spawn(|| git::log(path, log_from.as_deref(), 0, count));
                        let work_trees = scope.spawn(|| {
                            let work = git::status(path)?;
                            let changes = Some(work.changed_files());
                            // Very old gits have no `worktree list`; the section just stays
                            // empty.
                            let worktrees = git::worktrees(path, changes).unwrap_or_default();
                            Ok::<_, String>((work, worktrees))
                        });
                        let stashes = scope.spawn(|| git::stashes(path).unwrap_or_default());
                        let remotes = scope.spawn(|| git::remotes(path).unwrap_or_default());
                        let submodules = scope.spawn(|| git::submodules(path).unwrap_or_default());
                        let lfs = scope.spawn(|| git::uses_lfs(path) && git::lfs_installed());
                        let identity = git::identity(path);
                        let join = |e| format!("Reading the repository failed: {e:?}");
                        let history: Vec<Arc<Commit>> = history
                            .join()
                            .map_err(join)??
                            .into_iter()
                            .map(Arc::new)
                            .collect();
                        let (work, worktrees) = work_trees.join().map_err(join)??;
                        Ok::<_, String>((
                            history,
                            work,
                            worktrees,
                            stashes.join().map_err(join)?,
                            remotes.join().map_err(join)?,
                            submodules.join().map_err(join)?,
                            lfs.join().map_err(join)?,
                            identity,
                        ))
                    })?;
                Ok(Box::new(Loaded {
                    refs,
                    history,
                    work,
                    worktrees,
                    stashes,
                    remotes,
                    submodules,
                    lfs,
                    identity,
                    requested: count,
                    log_from,
                    read,
                }))
            })())
        });
    }

    pub(super) fn reload_status(&mut self, ctx: &egui::Context) {
        let read = self.next_read();
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || Msg::Status(read, git::status(&path)));
    }

    /// The watcher is there now. What changed while it was being set up (it walks the whole
    /// worktree, which takes a while) made no event: whatever was read before may have missed
    /// it. Not for a first read yet to start (it reads after this), nor for a parked view
    /// (it reloads when shown).
    pub(super) fn watcher_arrived(&mut self, ctx: &egui::Context) {
        if self.deferred || !self.shown {
            return;
        }
        if self.loading {
            self.recheck_after_load = true;
        } else {
            self.recheck(ctx);
        }
    }

    /// Reads the working tree and submodules again, and the refs to see whether anything
    /// moved: rather than a whole reload, which on a large repository is far longer.
    fn recheck(&mut self, ctx: &egui::Context) {
        self.reload_status(ctx);
        self.reload_submodules(ctx);
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || Msg::RefsCheck(git::refs(&path)));
    }

    /// The number of a read starting now.
    fn next_read(&mut self) -> u64 {
        self.reads += 1;
        self.reads
    }

    /// Whether an answer of `read` is newer than what is shown from the read `shown`; if so,
    /// it becomes what is shown.
    pub(super) fn newer(read: u64, shown: &mut u64) -> bool {
        let newer = read > *shown;
        if newer {
            *shown = read;
        }
        newer
    }

    /// Re-reads the submodules: the commits they should be at are in the index, which can
    /// change without any ref moving (`git add` of a submodule, `git submodule add`).
    pub(super) fn reload_submodules(&mut self, ctx: &egui::Context) {
        let read = self.next_read();
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            Msg::Submodules(read, git::submodules(&path).unwrap_or_default())
        });
    }

    pub(super) fn load_more(&mut self, ctx: &egui::Context) {
        if self.loading_more || !self.has_more || self.history.len() >= MAX_HISTORY {
            return;
        }
        self.loading_more = true;
        let path = self.path.clone();
        let skip = self.history.len();
        let generation = self.history_gen;
        let from = self.log_from.clone();
        let stream = Arc::clone(&self.log_stream);
        spawn(&self.tx, ctx, move || {
            let page = (|| {
                let mut slot = stream
                    .lock()
                    .map_err(|_| "Reading older history failed".to_owned())?;
                // The first page of this history (since it was last reloaded) starts the
                // `git log` the next ones are read from.
                if slot
                    .as_ref()
                    .is_none_or(|(made_for, _)| *made_for != generation)
                {
                    *slot = None;
                    *slot = Some((
                        generation,
                        git::LogStream::start(&path, from.as_deref(), skip)?,
                    ));
                }
                let page = match slot.as_mut() {
                    Some((_, log)) => log.next_page(git::LOG_PAGE),
                    None => Ok(Vec::new()),
                };
                // At the end of history (or after an error): nothing more to read from it.
                if !matches!(&page, Ok(p) if p.len() == git::LOG_PAGE) {
                    *slot = None;
                }
                page
            })();
            Msg::More(generation, page)
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
        } else if std::mem::take(&mut self.recheck_after_load) {
            self.recheck(ctx);
        }
    }

    fn take_loaded(&mut self, loaded: Loaded, ctx: &egui::Context) {
        self.load_error = None;
        // A page asked for before this was read past the old history, which
        // this one replaces: it would leave a gap.
        self.history_gen += 1;
        // The `git log` older pages came from read the history replaced: let it go (once a
        // page being read from it is in).
        self.log_stream = Default::default();
        self.loading_more = false;
        self.refs = loaded.refs;
        self.has_more = loaded.history.len() >= loaded.requested;
        self.history = loaded.history;
        // A status read since this reload started is newer.
        if Self::newer(loaded.read, &mut self.work_read) {
            self.work = loaded.work;
        }
        self.worktrees = loaded.worktrees;
        // This worktree's count is that of the working tree kept, which may be newer.
        let changes = self.work.changed_files();
        if let Some(here) = self.worktrees.iter_mut().find(|wt| wt.current) {
            here.changes = Some(changes);
        }
        self.take_visibility(loaded.log_from, ctx);
        // Stash numbers shift when one is pushed or dropped; follow the commit.
        let viewed = self
            .stash_view
            .and_then(|i| self.stashes.get(i))
            .map(|s| s.hash.clone());
        self.stashes = loaded.stashes;
        self.stash_view = viewed.and_then(|h| self.stashes.iter().position(|s| s.hash == h));
        self.identity = loaded.identity;
        if Self::newer(loaded.read, &mut self.submodules_read) {
            self.submodules = loaded.submodules;
        }
        self.lfs = loaded.lfs;
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
        self.refresh_ci(ctx);
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
                let fresh: Vec<Arc<Commit>> = page
                    .into_iter()
                    .filter(|c| !known.contains(c.hash.as_str()))
                    .map(Arc::new)
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

    /// After a status read that leaves the rows as they were (the changes row neither appeared
    /// nor went away): what `rebuild` does besides laying out the graph again.
    pub(super) fn refresh_work(&mut self, ctx: &egui::Context) {
        if let Some(wip) = self.commits.first_mut()
            && wip.hash == WIP_HASH
        {
            Arc::make_mut(wip).time = git::now();
        }
        self.take_prepared_message();
        self.refresh_work_diff(ctx);
    }

    /// The WIP pseudo-commit (when the tree is dirty), then `history` with its stashes, laid out
    /// as the graph.
    fn rebuild_rows(&mut self) {
        let old_rows = std::mem::take(&mut self.commits);
        if !self.work.is_clean() {
            self.commits.push(Arc::new(Commit {
                hash: WIP_HASH.to_owned(),
                parents: if self.refs.head.is_empty() {
                    Vec::new()
                } else {
                    vec![self.refs.head.clone()]
                },
                time: git::now(),
                ..Commit::default()
            }));
        }
        // Each stash sits right above the commit it was made on, as a commit on no branch.
        let mut stashes_on: HashMap<&str, Vec<&Stash>> = HashMap::new();
        for stash in &self.stashes {
            stashes_on
                .entry(stash.base.as_str())
                .or_default()
                .push(stash);
        }
        self.commits
            .reserve(self.history.len() + self.stashes.len());
        for commit in &self.history {
            for stash in stashes_on.get(commit.hash.as_str()).into_iter().flatten() {
                self.commits.push(Arc::new(Commit {
                    hash: stash.hash.clone(),
                    parents: vec![stash.base.clone()],
                    author: stash.author.clone(),
                    email: stash.email.clone(),
                    time: stash.time,
                    date: stash.date.clone(),
                    summary: stash.message.clone(),
                    ..Commit::default()
                }));
            }
            // Shared with `history`, not copied: tens of thousands of commits may be loaded.
            self.commits.push(Arc::clone(commit));
        }
        let graph = graph::layout(&self.commits);
        let index = self
            .commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.hash.clone(), i))
            .collect();
        let old_graph = std::mem::replace(&mut self.graph, graph);
        let old_index = std::mem::replace(&mut self.index, index);
        // Freeing tens of thousands of commits (those a reload replaced), rows and index entries
        // takes a while: not on the UI thread.
        if old_rows.len() > 5000 {
            std::thread::spawn(move || drop((old_rows, old_graph, old_index)));
        }
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
        let Some((remote, (host, path))) = self
            .remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| self.remotes.first())
            .and_then(|r| Some((r.url.clone(), forge::location(&r.url)?)))
        else {
            return;
        };
        let gitlab = gitlab_authority(&remote).unwrap_or_else(|| host.clone());
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
                        crate::avatars::hint(&email, crate::avatars::Hint::GitLab(gitlab.clone()));
                    }
                }
            }
            ctx.request_repaint();
        });
    }
}

/// Where a self-hosted GitLab's API is, from the remote `url`: its host, with the port of an
/// `https://host:8443/…` remote (its web pages and API are on that port too).
fn gitlab_authority(url: &str) -> Option<String> {
    let web = forge::web_url(url)?;
    let (_, rest) = web.split_once("://")?;
    rest.split('/').next().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::gitlab_authority;

    #[test]
    fn gitlab_avatars_are_asked_on_the_remotes_port() {
        let at = |url: &str| gitlab_authority(url);
        assert_eq!(
            at("https://gitlab.example.com:8443/g/p.git").as_deref(),
            Some("gitlab.example.com:8443")
        );
        assert_eq!(at("git@gitlab.com:g/p.git").as_deref(), Some("gitlab.com"));
        // An SSH port says nothing about the web pages'.
        assert_eq!(
            at("ssh://git@gitlab.example.com:2222/g/p.git").as_deref(),
            Some("gitlab.example.com")
        );
    }
}
