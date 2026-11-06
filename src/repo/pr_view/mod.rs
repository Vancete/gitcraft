//! A pull request in the big area: its description, build status, comments and commits. The
//! right pane lists the files it changes, which open in the normal diff.

mod page;
mod text;
mod timeline;

use std::collections::HashMap;

use egui_commonmark::CommonMarkCache;

use super::{DiffKey, Msg, RepoTab, spawn};
use crate::pull_request::{self as pr, Block, PrDetails, PrFiles};

pub(super) use text::{blocks, only_web_links};

pub(crate) struct PrView {
    pub number: u64,
    request: u64,
    loading: bool,
    pub(super) details: Option<Result<PrDetails, String>>,
    /// The latest commit of the pull request as last read, kept through a reading that failed.
    seen_head: Option<String>,
    /// The description and what the timeline says, made safe to show.
    body: Vec<Block>,
    pub(super) entries: Vec<Vec<Block>>,
    /// What the comments on lines say, by comment.
    pub(super) thread_bodies: HashMap<String, Vec<Block>>,
    /// The suggested change of each comment that has one the reader can see.
    pub(super) suggestions: HashMap<String, String>,
    pub(super) files: Option<Result<PrFiles, String>>,
    pub(super) markdown: CommonMarkCache,
}

impl PrView {
    /// Makes the texts of `details` safe to show, to show them from now on.
    fn prepare_texts(&mut self, details: &PrDetails) {
        let host = pr::repo_of_url(&details.url).map_or(String::new(), |(host, _)| host);
        let prepare = |text: &str| pr::safe_blocks(text, &host);
        self.body = prepare(&details.body);
        self.entries = details
            .timeline
            .iter()
            .map(|entry| prepare(&entry.body))
            .collect();
        self.thread_bodies = details
            .extras
            .threads
            .iter()
            .flat_map(|thread| &thread.comments)
            .map(|comment| (comment.id.clone(), prepare(&comment.body)))
            .collect();
        self.suggestions = details
            .extras
            .threads
            .iter()
            .flat_map(|thread| &thread.comments)
            .filter_map(|comment| {
                crate::pr_post::shown_suggestion(&comment.body, &host)
                    .map(|text| (comment.id.clone(), text))
            })
            .collect();
    }
}

impl RepoTab {
    /// Shows the pull request numbered `number` in the big area.
    pub(super) fn open_pr(&mut self, number: u64, ctx: &egui::Context) {
        // Those editors hold choices written nowhere yet, and take the big area until they're
        // done: the pull request waits rather than throwing them away.
        if let Some(why) = editor_in_the_way(self.conflict.is_some(), self.rebase.is_some()) {
            self.status = Some((why.to_owned(), true));
            return;
        }
        self.leave_pr();
        self.diff = None;
        self.file_view = None;
        self.compare = None;
        self.pr_view = Some(PrView {
            number,
            request: 0,
            loading: true,
            details: None,
            seen_head: None,
            body: Vec::new(),
            entries: Vec::new(),
            thread_bodies: HashMap::new(),
            suggestions: HashMap::new(),
            files: None,
            markdown: CommonMarkCache::default(),
        });
        self.load_pr(ctx);
    }

    /// Reads the pull request shown (again). What is shown stays until the answer comes.
    pub(super) fn load_pr(&mut self, ctx: &egui::Context) {
        self.pr_requests += 1;
        let request = self.pr_requests;
        let Some(view) = &mut self.pr_view else {
            return;
        };
        view.request = request;
        view.loading = true;
        let (path, number) = (self.path.clone(), view.number);
        spawn(&self.tx, ctx, move || {
            Msg::PrDetails(request, Box::new(pr::details(&path, number)))
        });
    }

    pub(super) fn pr_details_arrived(
        &mut self,
        request: u64,
        result: Result<PrDetails, String>,
        ctx: &egui::Context,
    ) {
        let Some(view) = self.pr_view.as_mut().filter(|v| v.request == request) else {
            return;
        };
        view.loading = false;
        // Against the last head read, not the last answer: a reading that failed in between
        // (say because the pull request moved while it was read) mustn't hide the move.
        let moved = match &result {
            Ok(now) => head_moved(view.seen_head.as_deref(), &now.head_oid),
            Err(_) => false,
        };
        if let Ok(now) = &result {
            view.seen_head = Some(now.head_oid.clone());
        }
        if let Some(draft) = self.pr_drafts.get_mut(&view.number) {
            if moved {
                draft.head_moved();
            }
            // With details come the files, which is when it's settled. Without, nothing comes.
            if result.is_err() && std::mem::take(&mut draft.settling) {
                draft.posting = false;
            }
        }
        if let Ok(details) = &result {
            view.prepare_texts(details);
            let details = details.clone();
            let (path, remotes, credentials) = (
                self.path.clone(),
                self.remotes.clone(),
                self.credentials.clone(),
            );
            spawn(&self.tx, ctx, move || {
                let files = pr::files(&path, &details, &remotes, credentials.as_ref());
                Msg::PrFiles(request, files)
            });
        }
        view.details = Some(result);
    }

    pub(super) fn pr_files_arrived(
        &mut self,
        request: u64,
        result: Result<PrFiles, String>,
        ctx: &egui::Context,
    ) {
        let Some(view) = self.pr_view.as_mut().filter(|v| v.request == request) else {
            return;
        };
        if let Some(draft) = self.pr_drafts.get_mut(&view.number)
            && std::mem::take(&mut draft.settling)
        {
            draft.posting = false;
        }
        let before = view.files.take().and_then(Result::ok);
        view.files = Some(result);
        let (Some(before), Some(Ok(now))) = (before, view.files.as_ref()) else {
            return;
        };
        let next = self
            .diff
            .as_ref()
            .and_then(|diff| followed(&diff.key, &before, now));
        match next {
            Some(Some(key)) => self.open_diff(key, ctx),
            Some(None) => self.diff = None,
            None => {}
        }
    }

    /// What a pull request calls a commit of a diff opened from it.
    pub(super) fn pr_name_of(&self, rev: &str) -> Option<String> {
        let view = self.pr_view.as_ref()?;
        let details = view.details.as_ref()?.as_ref().ok()?;
        let files = view.files.as_ref()?.as_ref().ok()?;
        if files.to == rev {
            Some(details.head_branch.clone())
        } else if files.from == rev {
            Some(format!("{} (merge base)", details.base_branch))
        } else {
            None
        }
    }
}

/// Why a pull request can't take the big area now, if it can't.
fn editor_in_the_way(conflict: bool, rebase: bool) -> Option<&'static str> {
    if conflict {
        Some("Finish or close the conflict editor first")
    } else if rebase {
        Some("Finish or cancel the interactive rebase first")
    } else {
        None
    }
}

/// Whether the pull request read with `now` as its latest commit moved on from `seen`, the one
/// last read (none yet: nothing to have moved from).
fn head_moved(seen: Option<&str>, now: &str) -> bool {
    seen.is_some_and(|seen| seen != now)
}

/// What becomes of the diff open as `open` when the changes of the pull request go from
/// `before` to `now`, say by a suggestion applied or the base branch rewritten: a file of the
/// pull request follows them (`Some(None)` if it's no longer changed). `None` leaves it be.
fn followed(open: &DiffKey, before: &PrFiles, now: &PrFiles) -> Option<Option<DiffKey>> {
    let DiffKey::Range {
        from,
        to: Some(to),
        path,
    } = open
    else {
        return None;
    };
    let was_the_pull_request = *from == before.from && *to == before.to;
    let moved = before.from != now.from || before.to != now.to;
    if !was_the_pull_request || !moved {
        return None;
    }
    let still_changed = now.files.iter().any(|file| file.path == *path);
    Some(still_changed.then(|| DiffKey::Range {
        from: now.from.clone(),
        to: Some(now.to.clone()),
        path: path.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    fn files(from: &str, to: &str, paths: &[&str]) -> PrFiles {
        PrFiles {
            from: from.to_owned(),
            to: to.to_owned(),
            files: paths
                .iter()
                .map(|path| git::ChangedFile {
                    status: 'M',
                    path: (*path).to_owned(),
                    old_path: None,
                    added: None,
                    removed: None,
                })
                .collect(),
        }
    }

    fn range(from: &str, to: &str, path: &str) -> DiffKey {
        DiffKey::Range {
            from: from.to_owned(),
            to: Some(to.to_owned()),
            path: path.to_owned(),
        }
    }

    #[test]
    fn an_open_file_follows_new_commits() {
        let before = files("base", "h0", &["a.rs"]);
        let now = files("base", "h1", &["a.rs"]);
        assert_eq!(
            followed(&range("base", "h0", "a.rs"), &before, &now),
            Some(Some(range("base", "h1", "a.rs")))
        );
    }

    #[test]
    fn an_open_file_follows_a_new_merge_base() {
        let before = files("base0", "h0", &["a.rs"]);
        let now = files("base1", "h0", &["a.rs"]);
        assert_eq!(
            followed(&range("base0", "h0", "a.rs"), &before, &now),
            Some(Some(range("base1", "h0", "a.rs")))
        );
    }

    #[test]
    fn a_file_no_longer_changed_closes() {
        let before = files("base", "h0", &["a.rs"]);
        let now = files("base", "h1", &["b.rs"]);
        assert_eq!(
            followed(&range("base", "h0", "a.rs"), &before, &now),
            Some(None)
        );
    }

    #[test]
    fn other_diffs_and_unchanged_ones_stay() {
        let before = files("base", "h0", &["a.rs"]);
        let now = files("base", "h1", &["a.rs"]);
        // Not the pull request's comparison.
        assert_eq!(followed(&range("other", "h0", "a.rs"), &before, &now), None);
        // Nothing moved.
        assert_eq!(
            followed(&range("base", "h0", "a.rs"), &before, &before),
            None
        );
    }

    #[test]
    fn a_move_is_seen_across_a_reading_that_failed() {
        // Read at h0, then a failed reading, then h1: the yes given for h0 doesn't carry over.
        assert!(head_moved(Some("h0"), "h1"));
        assert!(!head_moved(Some("h0"), "h0"));
        assert!(!head_moved(None, "h0"));
    }

    #[test]
    fn an_editor_open_keeps_its_place() {
        assert!(editor_in_the_way(true, false).is_some());
        assert!(editor_in_the_way(false, true).is_some());
        assert_eq!(editor_in_the_way(false, false), None);
    }
}
