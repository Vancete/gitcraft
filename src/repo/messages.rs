//! Results of background work coming back to the tab, and the per-frame polling that takes
//! them in (with file-system events, the background fetch and the debounced search).

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use super::load::Loaded;
use super::{CompareData, OpKind, Refresh, RepoTab, diff, file_view, ops, pr_review, quick_edit};
use crate::forge::PullRequest;
use crate::git::{self, Commit, CommitDetail, WorkStatus};
use crate::settings::Settings;

/// Results coming back from background git calls.
pub(super) enum Msg {
    Loaded(Result<Box<Loaded>, String>),
    /// A further page of history, read while `history_gen` was so: a reload since may have
    /// moved the commits it skipped, so then it no longer fits.
    More(u64, Result<Vec<Commit>, String>),
    Status(Result<WorkStatus, String>),
    Detail(String, Result<CommitDetail, String>),
    /// The answer to the diff request numbered so (see [`DiffView::next_request`]).
    ///
    /// [`DiffView::next_request`]: super::DiffView::next_request
    Diff(u64, Result<diff::DiffData, String>),
    FileHistory(String, Option<String>, Result<Vec<git::FileCommit>, String>),
    Blame(String, Option<String>, Result<file_view::BlameData, String>),
    RebaseCommits {
        onto: String,
        label: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    },
    /// Results for a query, searched in the code (`true`) or in the messages.
    Search(String, bool, Result<Vec<String>, String>),
    /// A comparison's files, for the request numbered so.
    Compare(u64, Result<CompareData, String>),
    Prs(Result<Vec<PullRequest>, String>),
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
    },
    /// Mergiraf's take on a conflicted file.
    Mergiraf(String, Result<String, String>),
}

/// Runs `job` on a background thread and wakes the UI when its message is sent.
pub(super) fn spawn(
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    job: impl FnOnce() -> Msg + Send + 'static,
) {
    let tx = tx.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(job());
        ctx.request_repaint();
    });
}

impl RepoTab {
    /// Drains finished background work and file-system events.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg, ctx);
        }

        // Changes on disk, debounced.
        if let Some(rx) = &self.fs_rx {
            while let Ok(git_dir) = rx.try_recv() {
                let was = self.fs_pending.map(|(g, _)| g).unwrap_or(false);
                self.fs_pending = Some((was || git_dir, Instant::now()));
            }
        }
        if let Some((git_dir, at)) = self.fs_pending
            && at.elapsed() > Duration::from_millis(400)
            && !self.loading
            && self.busy_op.is_none()
        {
            self.fs_pending = None;
            if git_dir {
                self.reload(ctx);
            } else {
                self.reload_status(ctx);
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
            Msg::More(generation, _) if generation != self.history_gen => {}
            Msg::More(_, result) => self.more_loaded(result, ctx),
            Msg::Status(result) => match result {
                Ok(work) => {
                    self.work = work;
                    self.rebuild(ctx);
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
            Msg::Diff(request, result) => {
                if let Some(view) = &mut self.diff {
                    view.arrived(request, result);
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
            Msg::Search(query, code, result) => self.search_done(query, code, result, ctx),
            Msg::Prs(result) => match result {
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
            } => self.op_done(label, result, refresh, kind, quiet, ctx),
        }
    }
}
