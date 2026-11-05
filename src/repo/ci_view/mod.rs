//! A commit's builds in the big area: its workflow runs (GitHub) or pipelines (GitLab), the
//! jobs of the one picked as a graph, the steps of the job picked on a time line, and running
//! the failed jobs again, cancelling and the failed jobs' log.

mod graph;
mod page;
mod steps;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{Msg, RepoTab, spawn};
use crate::ci::{self, CheckState, Job, Run};

/// How often a run that's still going is read again.
const POLL: Duration = Duration::from_secs(10);
/// How soon a run is read again after it was re-run or cancelled.
const AFTER_ACTING: Duration = Duration::from_secs(3);

/// Numbers the reads of every tab's view, so an answer for an older one is told apart.
static REQUESTS: AtomicU64 = AtomicU64::new(0);

fn next_request() -> u64 {
    REQUESTS.fetch_add(1, Ordering::Relaxed) + 1
}

/// What the pipeline view asks for.
pub(crate) enum CiAct {
    SelectRun(u64),
    SelectJob(u64),
    Reload,
    RerunFailed,
    Cancel,
    /// Show the log of the failed jobs, or hide it.
    ToggleLog,
}

pub(crate) struct CiView {
    pub(super) sha: String,
    request: u64,
    loading: bool,
    runs: Option<Result<Vec<Run>, String>>,
    /// The run shown, by id.
    run: Option<u64>,
    /// The jobs of a run (by id), the one shown once they're here.
    jobs: Option<(u64, Result<Vec<Job>, String>)>,
    /// The job whose steps are shown, by id.
    job: Option<u64>,
    /// The failed jobs' log, `Some(None)` while it's read.
    log: Option<Option<Result<String, String>>>,
    log_request: u64,
    /// When to read the run again, while it runs.
    next: Option<Instant>,
    /// It was seen running: once it has finished, the dots are read again.
    was_running: bool,
    /// Re-running or cancelling is under way.
    acting: bool,
}

impl CiView {
    fn shown_run(&self) -> Option<&Run> {
        let runs = self.runs.as_ref()?.as_ref().ok()?;
        runs.iter().find(|run| Some(run.id) == self.run)
    }

    /// The jobs of the run shown, once they're here.
    fn shown_jobs(&self) -> Option<&Result<Vec<Job>, String>> {
        self.jobs
            .as_ref()
            .filter(|(run, _)| Some(*run) == self.run)
            .map(|(_, jobs)| jobs)
    }

    fn running(&self) -> bool {
        let jobs = self.shown_jobs().and_then(|jobs| jobs.as_ref().ok());
        self.shown_run()
            .is_some_and(|run| run.state == CheckState::Running)
            || jobs.is_some_and(|jobs| jobs.iter().any(|j| j.state == CheckState::Running))
    }
}

impl RepoTab {
    /// Shows the builds of commit `sha` in the big area, run `run` first if it's given.
    pub(super) fn open_ci(&mut self, sha: String, run: Option<u64>, ctx: &egui::Context) {
        if self.conflict.is_some() || self.rebase.is_some() {
            let editor = if self.conflict.is_some() {
                "conflict editor"
            } else {
                "interactive rebase"
            };
            self.status = Some((format!("Finish or close the {editor} first"), true));
            return;
        }
        self.diff = None;
        self.file_view = None;
        self.ci_view = Some(CiView {
            sha,
            request: 0,
            loading: true,
            runs: None,
            run,
            jobs: None,
            job: None,
            log: None,
            log_request: 0,
            next: None,
            was_running: false,
            acting: false,
        });
        self.load_ci_view(ctx);
    }

    /// Reads the runs of the commit shown (again), then the jobs of the one shown.
    fn load_ci_view(&mut self, ctx: &egui::Context) {
        let project = self.ci_project();
        let Some(view) = &mut self.ci_view else {
            return;
        };
        view.next = None;
        let Some(project) = project else {
            view.loading = false;
            view.runs = Some(Err("This repository isn't on GitHub or GitLab.".to_owned()));
            return;
        };
        view.request = next_request();
        view.loading = true;
        let (request, path, sha) = (view.request, self.path.clone(), view.sha.clone());
        spawn(&self.tx, ctx, move || {
            Msg::CiRuns(request, ci::runs(&path, &project, &sha))
        });
    }

    fn load_ci_jobs(&mut self, ctx: &egui::Context) {
        let project = self.ci_project();
        let Some(view) = &mut self.ci_view else {
            return;
        };
        let (Some(project), Some(run)) = (project, view.run) else {
            return;
        };
        view.loading = true;
        let (request, path) = (view.request, self.path.clone());
        spawn(&self.tx, ctx, move || {
            Msg::CiJobs(request, run, ci::jobs(&path, &project, run))
        });
    }

    pub(super) fn ci_runs_arrived(
        &mut self,
        request: u64,
        result: Result<Vec<Run>, String>,
        ctx: &egui::Context,
    ) {
        let Some(view) = self.ci_view.as_mut().filter(|v| v.request == request) else {
            return;
        };
        view.loading = false;
        if let Ok(runs) = &result {
            // The run shown stays; else the one asked for, or the newest.
            let keep = view.run.filter(|id| runs.iter().any(|run| run.id == *id));
            view.run = keep.or_else(|| runs.first().map(|run| run.id));
        }
        let load_jobs = result.as_ref().is_ok_and(|runs| !runs.is_empty());
        view.runs = Some(result);
        if load_jobs {
            self.load_ci_jobs(ctx);
        }
    }

    pub(super) fn ci_jobs_arrived(
        &mut self,
        request: u64,
        run: u64,
        result: Result<Vec<Job>, String>,
        ctx: &egui::Context,
    ) {
        let Some(view) = self
            .ci_view
            .as_mut()
            .filter(|v| v.request == request && v.run == Some(run))
        else {
            return;
        };
        view.loading = false;
        if let Ok(jobs) = &result {
            // The job shown stays; else the first that failed, or the first.
            let keep = view.job.filter(|id| jobs.iter().any(|job| job.id == *id));
            let failed = jobs.iter().find(|job| job.state == CheckState::Failed);
            view.job = keep.or(failed.or(jobs.first()).map(|job| job.id));
        }
        view.jobs = Some((run, result));
        if view.running() {
            view.next = Some(Instant::now() + POLL);
            view.was_running = true;
        } else if std::mem::take(&mut view.was_running) {
            // It has finished: the dots should say so too.
            self.load_ci(ctx);
        }
    }

    pub(super) fn ci_log_arrived(&mut self, request: u64, result: Result<String, String>) {
        if let Some(view) = self.ci_view.as_mut().filter(|v| v.log_request == request)
            && let Some(log) = &mut view.log
        {
            *log = Some(result);
        }
    }

    /// A re-run or a cancel went through, or didn't.
    pub(super) fn ci_acted(&mut self, label: String, result: Result<(), String>) {
        self.status = Some(match result {
            Ok(()) => (label, false),
            Err(e) => (format!("{label}: {e}"), true),
        });
        if let Some(view) = &mut self.ci_view {
            view.acting = false;
            view.next = Some(Instant::now() + AFTER_ACTING);
        }
    }

    /// Reads the run shown again while it runs; called every frame.
    pub(super) fn poll_ci_view(&mut self, ctx: &egui::Context) {
        let Some(next) = self.ci_view.as_ref().and_then(|v| v.next) else {
            return;
        };
        let now = Instant::now();
        if now >= next {
            self.load_ci_view(ctx);
        } else {
            ctx.request_repaint_after(next - now);
        }
    }

    pub(super) fn ci_act(&mut self, act: CiAct, ctx: &egui::Context) {
        let project = self.ci_project();
        let Some(view) = &mut self.ci_view else {
            return;
        };
        match act {
            CiAct::SelectRun(id) => {
                if view.run != Some(id) {
                    view.run = Some(id);
                    view.job = None;
                    view.log = None;
                    self.load_ci_jobs(ctx);
                }
            }
            CiAct::SelectJob(id) => view.job = Some(id),
            CiAct::Reload => self.load_ci_view(ctx),
            CiAct::RerunFailed | CiAct::Cancel => {
                let (Some(project), Some(run)) = (project, view.shown_run().cloned()) else {
                    return;
                };
                if view.acting {
                    return;
                }
                view.acting = true;
                let rerun = matches!(act, CiAct::RerunFailed);
                let path = self.path.clone();
                spawn(&self.tx, ctx, move || {
                    let what = format!("{} #{}", run.name, run.number);
                    let (label, result) = if rerun {
                        let result = ci::rerun_failed(&path, &project, run.id);
                        (format!("Re-ran the failed jobs of {what}"), result)
                    } else {
                        (
                            format!("Cancelled {what}"),
                            ci::cancel(&path, &project, run.id),
                        )
                    };
                    Msg::CiActed(label, result)
                });
            }
            CiAct::ToggleLog => {
                if view.log.take().is_some() {
                    return;
                }
                let (Some(project), Some(run)) = (project, view.run) else {
                    return;
                };
                let jobs = match view.shown_jobs() {
                    Some(Ok(jobs)) => jobs.clone(),
                    _ => Vec::new(),
                };
                view.log = Some(None);
                view.log_request = next_request();
                let (request, path) = (view.log_request, self.path.clone());
                spawn(&self.tx, ctx, move || {
                    Msg::CiLog(request, ci::failed_log(&path, &project, run, &jobs))
                });
            }
        }
    }
}
