//! Results of background work coming back to the tab, and the per-frame polling that takes
//! them in (with file-system events, the background fetch and the debounced search).

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use super::file_text::FileText;
use super::load::Loaded;
use super::watch::Reread;
use super::{CompareData, OpKind, Refresh, RepoTab, diff, file_view, ops, pr_review, quick_edit};
use crate::forge::PullRequest;
use crate::git::{self, Commit, CommitDetail, WorkStatus};
use crate::settings::Settings;

/// Results coming back from background git calls.
pub(super) enum Msg {
    Loaded(Result<Box<Loaded>, String>),
    /// The submodules, from the read numbered so (see `RepoTab::reads`).
    Submodules(u64, Vec<git::Submodule>),
    /// The refs as they are now, to compare with those read (see `RepoTab::recheck`).
    RefsCheck(Result<git::Refs, String>),
    /// The worktree's watcher, set up in the background (`None` when it couldn't be).
    Watcher(Option<Box<notify::RecommendedWatcher>>),
    /// A further page of history, read while `history_gen` was so: a reload since may have
    /// moved the commits it skipped, so then it no longer fits.
    More(u64, Result<Vec<Commit>, String>),
    /// The working tree, from the read numbered so (see `RepoTab::reads`).
    Status(u64, Result<WorkStatus, String>),
    Detail(String, Result<CommitDetail, String>),
    /// The answer to the diff request numbered so (see [`DiffView::next_request`]).
    ///
    /// [`DiffView::next_request`]: super::DiffView::next_request
    Diff(u64, Result<Box<diff::DiffData>, String>),
    /// The whole file of the diff, read along with the diff request numbered so.
    DiffFile(u64, Result<FileText, String>),
    FileHistory(String, Option<String>, Result<Vec<git::FileCommit>, String>),
    Blame(String, Option<String>, Result<file_view::BlameData, String>),
    /// A file as it is at a revision, for the file view.
    FileText(String, Option<String>, Result<FileText, String>),
    /// Every file in the tree of a commit.
    TreeFiles(String, Result<Vec<String>, String>),
    RebaseCommits {
        onto: String,
        label: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    },
    /// Results for a query, searched in the code (`true`) or in the messages.
    Search(String, bool, Result<Vec<String>, String>),
    /// A comparison's files, for the request numbered so.
    Compare(u64, Result<CompareData, String>),
    /// The open pull requests, as asked for at that moment (see `RepoTab::prs_loaded`).
    Prs(Instant, Result<Vec<PullRequest>, String>),
    /// Build statuses by commit; `true` when new commits were asked about.
    CiStatuses(bool, Result<HashMap<String, crate::ci::Status>, String>),
    /// The runs of the commit the pipeline view shows, for the read numbered so.
    CiRuns(u64, Result<Vec<crate::ci::Run>, String>),
    /// The jobs of a run (by id), for the read numbered so.
    CiJobs(u64, u64, Result<Vec<crate::ci::Job>, String>),
    CiLog(u64, Result<String, String>),
    /// A run was re-run or cancelled, or wasn't.
    CiActed(String, Result<(), String>),
    /// A pull request in full, for the request numbered so.
    PrDetails(u64, Box<Result<crate::pull_request::PrDetails, String>>),
    PrFiles(u64, Result<crate::pull_request::PrFiles, String>),
    /// Something said on a pull request went out, or didn't.
    PrPosted {
        number: u64,
        what: pr_review::Posted,
        result: Result<(), String>,
    },
    /// The commits to replay when rewording `hash`.
    RewordCommits {
        hash: String,
        message: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    },
    /// A commit's patch was written to the file, or wasn't.
    PatchSaved(std::path::PathBuf, Result<(), String>),
    /// The commits to replay for a drop or move from a commit's menu.
    QuickEditCommits {
        edit: quick_edit::Edit,
        hash: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    },
    OpDone {
        label: String,
        result: Result<String, String>,
        refresh: Refresh,
        kind: OpKind,
        quiet: bool,
        /// The step it stopped at: those before it ran.
        stopped: usize,
    },
    /// Mergiraf's take on a conflicted file.
    Mergiraf(String, Result<String, String>),
    /// Work started with [`spawn`] panicked (a bug, perhaps in a dependency): what it said.
    JobPanicked(String),
}

/// Runs `job` on a background thread and wakes the UI when its message is sent. Should it
/// panic, [`Msg::JobPanicked`] comes instead: for work the tab waits for (an operation, a
/// reload…), [`spawn_or`] says what to send then, so that the tab doesn't wait forever.
pub(super) fn spawn(
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    job: impl FnOnce() -> Msg + Send + 'static,
) {
    spawn_or(tx, ctx, Msg::JobPanicked, job);
}

/// [`spawn`], sending what `failed` makes of what `job` said if it panics: the message it
/// would have sent had it failed, so that whatever waits for that message (the busy
/// operation, the loading flag…) is let go of.
pub(super) fn spawn_or(
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    failed: impl FnOnce(String) -> Msg + Send + 'static,
    job: impl FnOnce() -> Msg + Send + 'static,
) {
    let tx = tx.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        // What the job owned goes with it; a lock it held is left poisoned, which those
        // taking it handle.
        let msg = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
            .unwrap_or_else(|payload| failed(panic_text(payload.as_ref())));
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
}

/// What a panic said, as an error to show.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let said = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no details".to_owned());
    format!("Internal error: {said}")
}

impl RepoTab {
    /// Drains finished background work and file-system events.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.poll_as(ctx, true);
    }

    /// [`Self::poll`] for a worktree view parked behind the one shown: its own work is seen
    /// through, but it doesn't fetch, follow builds or reload for changes on disk. The view
    /// shown does that for the repository they share, and this one reloads when shown again.
    pub(crate) fn poll_parked_view(&mut self, ctx: &egui::Context) {
        self.poll_as(ctx, false);
    }

    fn poll_as(&mut self, ctx: &egui::Context, shown: bool) {
        self.shown = shown;
        // Changes on disk, debounced. Taken in before the finished work, so that a reload
        // that work starts covers what it wrote itself (see `reload`).
        if let Some(rx) = &self.fs_rx {
            while let Ok(change) = rx.try_recv() {
                let was = self.fs_pending.map(|(c, _)| c);
                self.fs_pending = Some((was.max(Some(change)).unwrap_or(change), Instant::now()));
            }
        }

        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg, ctx);
        }
        if !shown {
            return;
        }
        if let Some((change, at)) = self.fs_pending
            && at.elapsed() > Duration::from_millis(400)
            && !self.loading
            && self.busy_op.is_none()
        {
            self.fs_pending = None;
            for reread in change.rereads() {
                match reread {
                    Reread::All => self.reload(ctx),
                    Reread::Status => self.reload_status(ctx),
                    Reread::Submodules => self.reload_submodules(ctx),
                }
            }
        }

        // Background fetch.
        let minutes = Settings::get(ctx).auto_fetch_minutes;
        if minutes > 0
            && !self.remotes.is_empty()
            && self.busy_op.is_none()
            && self.last_fetch.elapsed() > Duration::from_secs(u64::from(minutes) * 60)
        {
            self.last_fetch = Instant::now();
            self.run_ops_ex(
                "Background fetch",
                ops(&[&["fetch", "--all", "--prune", "--quiet"]]),
                Refresh::All,
                OpKind::plain(),
                true,
                ctx,
            );
        }
        if minutes > 0 {
            ctx.request_repaint_after(Duration::from_secs(30));
        }

        self.poll_ci(ctx);
        self.poll_ci_view(ctx);

        // Debounced search while typing.
        if let Some(at) = self.search.edited
            && at.elapsed() > Duration::from_millis(350)
        {
            self.run_search(ctx);
        } else if self.search.edited.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn handle(&mut self, msg: Msg, ctx: &egui::Context) {
        match msg {
            Msg::Loaded(result) => self.loaded(result, ctx),
            Msg::Submodules(read, list) => {
                if Self::newer(read, &mut self.submodules_read) {
                    self.submodules = list;
                }
            }
            Msg::Watcher(watcher) => {
                self.watcher = watcher.map(|w| *w);
                self.watcher_arrived(ctx);
            }
            Msg::RefsCheck(refs) => {
                // Moved before the watcher could see it.
                if refs.is_ok_and(|refs| refs != self.refs) {
                    self.reload(ctx);
                }
            }
            Msg::More(generation, _) if generation != self.history_gen => {}
            Msg::More(_, result) => self.more_loaded(result, ctx),
            // Older than the working tree shown (a reload started since came in first).
            Msg::Status(read, _) if read <= self.work_read => {}
            Msg::Status(read, result) => match result {
                Ok(work) => {
                    self.work_read = read;
                    // This worktree's count of changes, as the worktree list shows it.
                    let changes = work.changed_files();
                    if let Some(here) = self.worktrees.iter_mut().find(|wt| wt.current) {
                        here.changes = Some(changes);
                    }
                    let wip_row_changed = work.is_clean() != self.work.is_clean();
                    self.work = work;
                    if wip_row_changed {
                        self.rebuild(ctx);
                    } else {
                        // The rows are the same: only what shows the working tree needs
                        // refreshing, not the whole graph.
                        self.refresh_work(ctx);
                    }
                    if self.compare.as_ref().is_some_and(|c| c.target.is_none()) {
                        self.reload_compare(ctx);
                    }
                }
                Err(e) => self.status = Some((e, true)),
            },
            Msg::Detail(hash, result) => {
                if self.detail_hash.as_deref() == Some(hash.as_str()) {
                    match result {
                        Ok(d) => {
                            self.detail = Some(d);
                            self.fill_reword();
                        }
                        Err(e) => self.detail_error = Some(e),
                    }
                }
            }
            Msg::RebaseCommits {
                onto,
                label,
                result,
            } => self.open_rebase(onto, label, result),
            Msg::RewordCommits {
                hash,
                message,
                result,
            } => self.reword_older(hash, message, result, ctx),
            Msg::PatchSaved(file, result) => self.patch_saved(file, result),
            Msg::QuickEditCommits { edit, hash, result } => {
                self.run_quick_edit(edit, hash, result, ctx)
            }
            Msg::FileHistory(path, rev, result) => {
                // Only the history of the revision shown: an older read may answer last.
                if let Some(view) = &mut self.file_view
                    && view.path == path
                    && view.rev == rev
                {
                    view.history = Some(result);
                }
            }
            Msg::Blame(path, rev, result) => {
                if let Some(view) = &mut self.file_view
                    && view.path == path
                    && view.rev == rev
                {
                    view.blame = Some(result);
                }
            }
            Msg::FileText(path, rev, result) => {
                if let Some(view) = &mut self.file_view
                    && view.path == path
                    && view.rev == rev
                {
                    view.text = Some(result);
                }
            }
            Msg::TreeFiles(hash, result) => self.all_files.arrived(&hash, result),
            Msg::Diff(request, result) => {
                if let Some(view) = &mut self.diff {
                    view.arrived(request, result.map(|data| *data));
                }
            }
            Msg::DiffFile(request, result) => {
                if let Some(view) = &mut self.diff {
                    view.file_arrived(request, result);
                }
            }
            Msg::PrDetails(request, result) => self.pr_details_arrived(request, *result, ctx),
            Msg::PrFiles(request, result) => self.pr_files_arrived(request, result, ctx),
            Msg::PrPosted {
                number,
                what,
                result,
            } => self.pr_posted(number, what, result, ctx),
            Msg::Compare(request, result) => {
                if let Some(c) = &mut self.compare
                    && c.request == request
                {
                    c.data = Some(result);
                }
            }
            Msg::Mergiraf(path, result) => self.mergiraf_done(path, result),
            Msg::JobPanicked(e) => self.status = Some((e, true)),
            Msg::Search(query, code, result) => self.search_done(query, code, result, ctx),
            // Asked before the list asked for last (one read before a merge answering after
            // the one read since), or of the forge the remotes no longer point to.
            Msg::Prs(asked, _) if self.prs_loaded != Some(asked) => {}
            Msg::Prs(_, result) => match result {
                Ok(prs) => {
                    self.prs = prs;
                    self.pr_error = None;
                    self.refresh_ci(ctx);
                }
                Err(e) => self.pr_error = Some(e),
            },
            Msg::CiStatuses(fresh, result) => self.ci_arrived(fresh, result),
            Msg::CiRuns(request, result) => self.ci_runs_arrived(request, result, ctx),
            Msg::CiJobs(request, run, result) => self.ci_jobs_arrived(request, run, result, ctx),
            Msg::CiLog(request, result) => self.ci_log_arrived(request, result),
            Msg::CiActed(label, result) => self.ci_acted(label, result),
            Msg::OpDone {
                label,
                result,
                refresh,
                kind,
                quiet,
                stopped,
            } => {
                if result.is_err() && !quiet {
                    self.keep_unfinished(stopped);
                }
                self.op_done(label, result, refresh, kind, quiet, ctx)
            }
        }
    }
}
