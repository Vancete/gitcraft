//! Selecting commits and stashes, opening diffs, comparisons and file views, and the commit
//! search.

use super::{
    Compare, CompareData, DiffKey, DiffView, MAX_HISTORY, Msg, RepoTab, WIP_HASH, conflict, diff,
    file_text, file_view, spawn,
};
use crate::git;
use crate::settings::Settings;
use crate::theme::Tokens;

impl RepoTab {
    pub(super) fn is_wip(&self, row: usize) -> bool {
        self.commits.get(row).is_some_and(|c| c.hash == WIP_HASH)
    }

    /// The stash a graph row stands for, if it's one.
    pub(super) fn stash_at(&self, row: usize) -> Option<usize> {
        let hash = &self.commits.get(row)?.hash;
        self.stashes.iter().position(|s| &s.hash == hash)
    }

    pub(super) fn selected_hash(&self) -> Option<String> {
        self.selected
            .and_then(|r| self.commits.get(r))
            .map(|c| c.hash.clone())
    }

    pub(super) fn select(&mut self, row: usize, ctx: &egui::Context) {
        let Some(commit) = self.commits.get(row) else {
            return;
        };
        let stash = self.stash_at(row);
        if self.selected == Some(row) && self.stash_view == stash {
            return;
        }
        let hash = commit.hash.clone();
        self.selected = Some(row);
        self.stash_view = stash;
        self.file_view = None;
        self.compare = None;
        if matches!(
            self.diff,
            Some(DiffView {
                key: DiffKey::Commit { .. } | DiffKey::Range { .. },
                ..
            })
        ) || hash != WIP_HASH
        {
            self.diff = None;
            self.conflict = None;
        }
        if hash == WIP_HASH {
            self.detail = None;
            self.detail_hash = None;
            return;
        }
        self.load_detail(hash, ctx);
    }

    pub(super) fn load_detail(&mut self, hash: String, ctx: &egui::Context) {
        self.detail = None;
        self.reword = None;
        self.detail_error = None;
        self.detail_hash = Some(hash.clone());
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let detail = git::commit_detail(&path, &hash);
            Msg::Detail(hash, detail)
        });
    }

    pub(super) fn select_stash(&mut self, i: usize, ctx: &egui::Context) {
        let Some(stash) = self.stashes.get(i) else {
            return;
        };
        let hash = stash.hash.clone();
        self.stash_view = Some(i);
        self.selected = self.index.get(&hash).copied();
        self.scroll_to = self.selected;
        self.file_view = None;
        self.compare = None;
        self.diff = None;
        self.conflict = None;
        self.load_detail(hash, ctx);
    }

    pub(super) fn select_hash(&mut self, hash: &str, ctx: &egui::Context) {
        match self.index.get(hash).copied() {
            Some(row) => {
                self.select(row, ctx);
                self.scroll_to = Some(row);
            }
            None => {
                self.status = Some((
                    format!(
                        "Commit {} is not in the loaded history",
                        hash.get(..8).unwrap_or(hash)
                    ),
                    true,
                ));
            }
        }
    }

    /// Selects `hash`, loading more history first if it's further down.
    pub(super) fn goto(&mut self, hash: &str, ctx: &egui::Context) {
        if self.index.contains_key(hash) {
            self.pending_goto = None;
            self.select_hash(hash, ctx);
        } else if self.has_more && self.history.len() < MAX_HISTORY {
            self.pending_goto = Some(hash.to_owned());
            self.load_more(ctx);
        } else {
            self.pending_goto = None;
            self.select_hash(hash, ctx);
        }
    }

    /// Shows a diff (or the conflict editor) in the central area; asking for the open one
    /// closes it.
    pub(super) fn open_diff(&mut self, key: DiffKey, ctx: &egui::Context) {
        if self.diff.as_ref().is_some_and(|d| d.key == key) {
            self.diff = None;
            self.conflict = None;
            return;
        }
        self.conflict = match &key {
            DiffKey::Work(file) if file.area == git::Area::Conflicted => {
                conflict::ConflictEditor::open(&self.path, &file.path, self.work.in_progress)
            }
            _ => None,
        };
        self.diff = Some(DiffView::new(key));
        self.refresh_diff(ctx);
    }

    /// Reads the open diff again, showing the old one meanwhile.
    pub(super) fn refresh_diff(&mut self, ctx: &egui::Context) {
        let Some(view) = &mut self.diff else { return };
        let request = view.next_request();
        let key = view.key.clone();
        let path = self.path.clone();
        let opts = git::DiffOptions {
            ignore_ws: self.diff_ignore_ws,
            whole_file: Settings::get(ctx).diff_whole_file,
        };
        let dark = Tokens::get(ctx).dark();
        spawn(&self.tx, ctx, move || {
            let result = diff::load(&path, &key, opts, dark).map(Box::new);
            Msg::Diff(request, result)
        });
        self.load_diff_file(ctx);
    }

    /// Reads the whole file of the open diff, when diffs show the file.
    pub(super) fn load_diff_file(&mut self, ctx: &egui::Context) {
        let Some(view) = self.diff.as_ref().filter(|_| self.diff_as_file) else {
            return;
        };
        let (request, key, path) = (view.request, view.key.clone(), self.path.clone());
        let dark = Tokens::get(ctx).dark();
        spawn(&self.tx, ctx, move || {
            let result = file_text::load_for_diff(&path, &key, dark);
            Msg::DiffFile(request, result)
        });
    }

    /// Shows diffs as the whole file, or as the changes.
    pub(super) fn show_diff_as_file(&mut self, on: bool, ctx: &egui::Context) {
        self.diff_as_file = on;
        if self.diff.as_ref().is_some_and(|d| d.file.is_none()) {
            self.load_diff_file(ctx);
        }
    }

    pub(super) fn reopen_diff(&mut self, ctx: &egui::Context) {
        if let Some(view) = self.diff.take() {
            self.conflict = None;
            self.open_diff(view.key, ctx);
        }
    }

    /// Closes the diff of a file between two commits (opened from a comparison).
    pub(super) fn close_range_diff(&mut self) {
        if matches!(
            self.diff,
            Some(DiffView {
                key: DiffKey::Range { .. },
                ..
            })
        ) {
            self.diff = None;
        }
    }

    /// Starts comparing two points of history (replacing the comparison shown).
    pub(super) fn compare(&mut self, compare: Compare, ctx: &egui::Context) {
        self.close_pr();
        self.close_range_diff();
        self.file_view = None;
        self.compare = Some(compare);
        self.reload_compare(ctx);
    }

    /// Reads the comparison's files again (the working tree or a branch moved).
    pub(super) fn reload_compare(&mut self, ctx: &egui::Context) {
        self.compare_requests += 1;
        let request = self.compare_requests;
        let Some(c) = &mut self.compare else { return };
        c.request = request;
        let (path, base, target, since_fork) = (
            self.path.clone(),
            c.base.clone(),
            c.target.clone(),
            c.since_fork,
        );
        spawn(&self.tx, ctx, move || {
            let result = (|| {
                let base = git::resolve(&path, &base)?;
                let to = target.map(|t| git::resolve(&path, &t)).transpose()?;
                // The working tree goes with HEAD's history.
                let other = match &to {
                    Some(to) => to.clone(),
                    None => git::resolve(&path, "HEAD")?,
                };
                let from = if since_fork {
                    git::merge_base(&path, &base, &other)?
                } else {
                    base.clone()
                };
                Ok(CompareData {
                    files: git::diff_files(&path, &from, to.as_deref())?,
                    apart: git::count_apart(&path, &base, &other)?,
                    from,
                    to,
                })
            })();
            Msg::Compare(request, result)
        });
    }

    /// Swaps the comparison's two ends.
    pub(super) fn swap_compare(&mut self, ctx: &egui::Context) {
        // The working tree stays the newer end.
        if let Some(c) = self.compare.take_if(|c| c.target.is_some())
            && let Some(target) = c.target
        {
            self.compare(
                Compare {
                    base: target,
                    base_label: c.target_label,
                    target: Some(c.base),
                    target_label: c.base_label,
                    data: None,
                    ..c
                },
                ctx,
            );
        }
    }

    /// Shows `path`'s history or blame, loading what that tab needs.
    pub(super) fn open_file_view(
        &mut self,
        path: String,
        rev: Option<String>,
        tab: file_view::FileTab,
        ctx: &egui::Context,
    ) {
        self.diff = None;
        self.conflict = None;
        let view = match self.file_view.take() {
            Some(mut v) if v.path == path && v.rev == rev => {
                v.tab = tab;
                v
            }
            _ => file_view::FileView::new(&path, rev.clone(), tab),
        };
        let needs = view.needs(tab);
        self.file_view = Some(view);
        if !needs {
            return;
        }
        let repo = self.path.clone();
        match tab {
            file_view::FileTab::History => spawn(&self.tx, ctx, move || {
                let result = git::file_history(&repo, rev.as_deref(), &path);
                Msg::FileHistory(path, rev, result)
            }),
            file_view::FileTab::Blame => {
                let dark = Tokens::get(ctx).dark();
                spawn(&self.tx, ctx, move || {
                    let result = file_view::load_blame(&repo, rev.as_deref(), &path, dark);
                    Msg::Blame(path, rev, result)
                })
            }
            file_view::FileTab::File => {
                let dark = Tokens::get(ctx).dark();
                spawn(&self.tx, ctx, move || {
                    let result = file_text::load(&repo, rev.as_deref(), &path, dark);
                    Msg::FileText(path, rev, result)
                })
            }
        }
    }

    /// Reads every file of commit `hash`, for the commit pane's "View all files".
    pub(super) fn load_tree_files(&mut self, hash: String, ctx: &egui::Context) {
        self.all_files.wait_for(&hash);
        let repo = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let result = git::tree_files(&repo, &hash);
            Msg::TreeFiles(hash, result)
        });
    }

    pub(super) fn run_search(&mut self, ctx: &egui::Context) {
        let query = self.search.query.trim().to_owned();
        self.search.edited = None;
        if query.is_empty() {
            // One still being read answers a query that is gone.
            self.search.running = false;
            self.search.results.clear();
            self.search.matches.clear();
            self.search.searched.clear();
            return;
        }
        self.search.running = true;
        let path = self.path.clone();
        let code = self.search.code;
        // In the branches the graph shows.
        let from = self.log_from.clone();
        // Answered all the same if it panics: the search runs until it is.
        let failed = {
            let query = query.clone();
            move |e| Msg::Search(query, code, Err(e))
        };
        super::messages::spawn_or(&self.tx, ctx, failed, move || {
            let result = if code {
                git::search_code(&path, from.as_deref(), &query)
            } else {
                git::search(&path, from.as_deref(), &query)
            };
            Msg::Search(query, code, result)
        });
    }

    /// Takes in the commits a search found (if it's still the search asked for), and goes to
    /// the first.
    pub(super) fn search_done(
        &mut self,
        query: String,
        code: bool,
        result: Result<Vec<String>, String>,
        ctx: &egui::Context,
    ) {
        // Closed since, the search shows nothing (nor jumps to its first find).
        if !self.search.open || query != self.search.query.trim() || code != self.search.code {
            return;
        }
        self.search.running = false;
        self.search.searched = query;
        match result {
            Ok(hashes) => {
                self.search.matches = hashes.iter().cloned().collect();
                self.search.results = hashes;
                self.search.current = 0;
                if let Some(first) = self.search.results.first().cloned() {
                    self.goto(&first, ctx);
                }
            }
            Err(e) => self.status = Some((e, true)),
        }
    }

    pub(super) fn search_step(&mut self, delta: i32, ctx: &egui::Context) {
        let n = self.search.results.len();
        if n == 0 {
            return;
        }
        let cur = self.search.current as i32 + delta;
        self.search.current = cur.rem_euclid(n as i32) as usize;
        if let Some(hash) = self.search.results.get(self.search.current).cloned() {
            self.goto(&hash, ctx);
        }
    }

    pub(super) fn toggle_search(&mut self) {
        self.search.open = !self.search.open;
        self.search.focus = self.search.open;
        if !self.search.open {
            self.search.running = false;
            self.search.matches.clear();
            self.search.results.clear();
            self.search.searched.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RepoTab;

    fn tab() -> RepoTab {
        let ctx = egui::Context::default();
        let mut tab = RepoTab::new_deferred("/nonexistent/search-test".into(), &ctx);
        tab.search.open = true;
        tab
    }

    /// Clearing the query while a search runs: its answer is for a query that is gone, and
    /// "searching…" does not stay up.
    #[test]
    fn clearing_the_query_ends_the_search_running() {
        let ctx = egui::Context::default();
        let mut tab = tab();
        tab.search.query = "foo".to_owned();
        tab.search.running = true;
        tab.search.query.clear();
        tab.run_search(&ctx);
        tab.search_done("foo".to_owned(), false, Ok(vec!["a".to_owned()]), &ctx);
        assert!(!tab.search.running);
        assert!(tab.search.results.is_empty());
    }

    /// A search closed before it answered shows no finds.
    #[test]
    fn a_closed_search_takes_no_answer() {
        let ctx = egui::Context::default();
        let mut tab = tab();
        tab.search.query = "foo".to_owned();
        tab.search.running = true;
        tab.toggle_search();
        tab.search_done("foo".to_owned(), false, Ok(vec!["a".to_owned()]), &ctx);
        assert!(!tab.search.running);
        assert!(tab.search.matches.is_empty());
        assert!(tab.pending_goto.is_none());
    }
}
