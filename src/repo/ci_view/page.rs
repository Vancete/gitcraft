//! The page of the pipeline view: which run, how it went, its jobs and the steps of one.

use egui::{RichText, Ui};

use super::{CiAct, CiView, graph, steps};
use crate::ci::{self, CheckState, Job, Run};
use crate::forge::Forge;
use crate::git;
use crate::icons;
use crate::repo::ci_status::{state_color, state_word};
use crate::repo::{Action, RepoTab, short, widgets};
use crate::theme::{self, Tokens};

const MARGIN_X: i8 = 24;

impl RepoTab {
    /// The big area.
    pub(in crate::repo) fn ci_view_ui(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &self.ci_view else {
            return;
        };
        let noun = match self.forge {
            Some(Forge::GitLab) => "Pipelines",
            _ => "Workflow runs",
        };
        page_header(ui, view, &format!("{noun} of {}", short(&view.sha)), acts);
        let runs = match &view.runs {
            None => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
                return;
            }
            Some(Err(e)) => {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.colored_label(t.danger, e));
                return;
            }
            Some(Ok(runs)) => runs,
        };
        egui::ScrollArea::vertical()
            .id_salt(("ci-view", &view.sha))
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(MARGIN_X, 18))
                    .show(ui, |ui| {
                        if runs.is_empty() {
                            ui.label(
                                RichText::new("No CI ran on this commit.")
                                    .color(t.text_faint)
                                    .italics(),
                            );
                            return;
                        }
                        page(ui, view, runs, acts);
                    });
            });
    }
}

/// The title bar: back, open the run in the browser, load again.
fn page_header(ui: &mut Ui, view: &CiView, title: &str, acts: &mut Vec<Action>) {
    let url = view.shown_run().map(|run| run.url.clone());
    widgets::panel_header(ui, title, |ui| {
        if icons::small_button(ui, "x", "Back (Esc)").clicked() {
            acts.push(Action::CloseCi);
        }
        if let Some(url) = url
            && icons::small_button(ui, "external-link", "Open in the browser").clicked()
        {
            acts.push(Action::OpenUrl(url));
        }
        if view.loading {
            ui.spinner();
        } else if icons::small_button(ui, "refresh-cw", "Load again").clicked() {
            acts.push(Action::Ci(CiAct::Reload));
        }
    });
}

fn page(ui: &mut Ui, view: &CiView, runs: &[Run], acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let now = git::now();
    if runs.len() > 1 {
        run_picker(ui, runs, view.run, acts);
        ui.add_space(14.0);
    }
    let Some(run) = view.shown_run() else {
        return;
    };
    run_header(ui, run, now, acts);
    let jobs = view.shown_jobs();
    let ok_jobs = jobs.and_then(|jobs| jobs.as_ref().ok());
    ui.add_space(10.0);
    run_actions(ui, view, run, ok_jobs.map_or(&[], Vec::as_slice), acts);
    section(ui, "JOBS");
    match jobs {
        None => {
            ui.spinner();
        }
        Some(Err(e)) => {
            ui.colored_label(t.danger, e);
        }
        Some(Ok(jobs)) if jobs.is_empty() => {
            ui.label(RichText::new("No jobs.").color(t.text_faint).italics());
        }
        Some(Ok(jobs)) => {
            egui::ScrollArea::horizontal()
                .id_salt(("ci-graph", run.id))
                .show(ui, |ui| {
                    graph::job_graph(ui, jobs, run.started, view.job, now, acts)
                });
            if let Some(job) = jobs.iter().find(|job| Some(job.id) == view.job) {
                section(ui, &format!("STEPS OF {}", job.name.to_uppercase()));
                steps::timeline(ui, job, now);
            }
        }
    }
    if let Some(log) = &view.log {
        section(ui, "LOG OF THE FAILED JOBS");
        match log {
            None => {
                ui.spinner();
            }
            Some(Err(e)) => {
                ui.colored_label(t.danger, e);
            }
            Some(Ok(text)) if text.trim().is_empty() => {
                ui.label(
                    RichText::new("The log is empty.")
                        .color(t.text_faint)
                        .italics(),
                );
            }
            Some(Ok(text)) => steps::log(ui, text),
        }
    }
}

/// A tab per run of the commit: its state, workflow and number.
fn run_picker(ui: &mut Ui, runs: &[Run], shown: Option<u64>, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        for run in runs {
            let text = RichText::new(format!("    {} #{}", run.name, run.number)).color(
                if Some(run.id) == shown {
                    t.text
                } else {
                    t.text_dim
                },
            );
            let resp = ui
                .selectable_label(Some(run.id) == shown, text)
                .on_hover_text(format!("{}\n{}", run.title, state_word(run.state)));
            // A dot of the run's colour in the room left before its name.
            let dot = resp.rect.left_center() + egui::vec2(10.0, 0.0);
            ui.painter()
                .circle_filled(dot, 3.5, state_color(&t, run.state));
            if resp.clicked() {
                acts.push(Action::Ci(CiAct::SelectRun(run.id)));
            }
        }
    });
}

/// What the run is, how it went and how long it took.
fn run_header(ui: &mut Ui, run: &Run, now: i64, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&run.name).font(theme::semibold(18.0)));
        ui.label(
            RichText::new(format!("#{}", run.number))
                .font(egui::FontId::proportional(18.0))
                .color(t.text_faint),
        );
    });
    if !run.title.is_empty() && run.title != run.branch {
        ui.label(RichText::new(&run.title).color(t.text_dim));
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let state = match state_word(run.state) {
            "skipped" => "skipped or cancelled",
            word => word,
        };
        widgets::chip(ui, state, state_color(&t, run.state));
        if !run.event.is_empty() {
            widgets::chip(ui, &run.event, t.text_dim);
        }
        if run.attempt > 1 {
            widgets::chip(ui, &format!("attempt {}", run.attempt), t.text_dim);
        }
        if !run.branch.is_empty() {
            ui.label(RichText::new(&run.branch).font(theme::mono(11.5)));
        }
        if !run.sha.is_empty()
            && ui
                .link(RichText::new(short(&run.sha)).font(theme::mono(11.5)))
                .on_hover_text("Show the commit in the graph")
                .clicked()
        {
            acts.push(Action::SelectHash(run.sha.clone()));
        }
        if let Some(seconds) = ci::duration(run.started, run.finished, now) {
            let took = if run.finished.is_some() {
                format!("took {}", ci::duration_label(seconds))
            } else {
                format!("running for {}", ci::duration_label(seconds))
            };
            ui.label(RichText::new(took).color(t.text_faint));
        }
        if let Some(started) = run.started {
            ui.label(
                RichText::new(format!("· started {}", git::relative_time(started, now)))
                    .color(t.text_faint),
            );
        }
    });
}

/// Run the failed jobs again, cancel, and the failed jobs' log.
fn run_actions(ui: &mut Ui, view: &CiView, run: &Run, jobs: &[Job], acts: &mut Vec<Action>) {
    let failed = jobs.iter().any(|job| job.state == CheckState::Failed);
    let running = run.state == CheckState::Running;
    ui.horizontal(|ui| {
        // A run can fail with no job failed (one was cancelled): those count as failed then.
        if (failed || run.state == CheckState::Failed) && !running {
            let rerun = ui
                .add_enabled(!view.acting, egui::Button::new("Re-run failed jobs"))
                .on_hover_text("Runs the jobs that failed or were cancelled again");
            if rerun.clicked() {
                acts.push(Action::Ci(CiAct::RerunFailed));
            }
        }
        if running
            && ui
                .add_enabled(!view.acting, egui::Button::new("Cancel"))
                .clicked()
        {
            acts.push(Action::Ci(CiAct::Cancel));
        }
        if failed {
            let label = if view.log.is_some() {
                "Hide the log"
            } else {
                "Show the failed log"
            };
            if ui.button(label).clicked() {
                acts.push(Action::Ci(CiAct::ToggleLog));
            }
        }
        if view.acting {
            ui.spinner();
        }
    });
}

fn section(ui: &mut Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(22.0);
    ui.label(
        RichText::new(title)
            .font(theme::semibold(10.5))
            .color(t.text_faint),
    );
    ui.add_space(6.0);
}
