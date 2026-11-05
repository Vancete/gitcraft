//! The jobs of a run as a graph: a column per stage (GitLab) or per wave of jobs that started
//! once those before them had finished (GitHub, which doesn't say what a job needs), joined
//! from one column to the next.

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::CiAct;
use crate::ci::{self, CheckState, Job};
use crate::icons;
use crate::repo::Action;
use crate::theme::{self, Tokens};

const CARD_W: f32 = 210.0;
const CARD_H: f32 = 42.0;
const CARD_GAP: f32 = 8.0;
const COLUMN_GAP: f32 = 44.0;
const TITLE_H: f32 = 22.0;
/// A job that started this soon after another finished may well have waited for it.
const SLACK: i64 = 5;

/// One column of the graph: its title and its jobs (indices), what needs attention first.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Column {
    pub(super) title: String,
    pub(super) jobs: Vec<usize>,
}

/// The columns `jobs` are laid out in: by stage when they have stages, else by when they
/// started. `start`: when the run started, which the columns' titles count from.
pub(super) fn columns(jobs: &[Job], start: Option<i64>) -> Vec<Column> {
    let mut columns = if jobs.iter().any(|job| !job.stage.is_empty()) {
        by_stage(jobs)
    } else {
        by_start(jobs, start)
    };
    let rank = |state: CheckState| match state {
        CheckState::Failed => 0,
        CheckState::Running => 1,
        CheckState::Passed => 2,
        CheckState::Skipped => 3,
    };
    for column in &mut columns {
        column.jobs.sort_by(|&a, &b| {
            (rank(jobs[a].state), &jobs[a].name).cmp(&(rank(jobs[b].state), &jobs[b].name))
        });
    }
    columns
}

/// A column per stage, in the order the stages first come in.
fn by_stage(jobs: &[Job]) -> Vec<Column> {
    let mut columns: Vec<Column> = Vec::new();
    for (i, job) in jobs.iter().enumerate() {
        match columns.iter_mut().find(|c| c.title == job.stage) {
            Some(column) => column.jobs.push(i),
            None => columns.push(Column {
                title: job.stage.clone(),
                jobs: vec![i],
            }),
        }
    }
    columns
}

/// A job goes one column after the last of those that had finished when it started; jobs
/// that haven't started wait in a column of their own at the end.
fn by_start(jobs: &[Job], start: Option<i64>) -> Vec<Column> {
    let mut order: Vec<usize> = (0..jobs.len())
        .filter(|&i| jobs[i].started.is_some())
        .collect();
    order.sort_by_key(|&i| jobs[i].started);
    let mut level = vec![0usize; jobs.len()];
    for (n, &i) in order.iter().enumerate() {
        let started = jobs[i].started.unwrap_or_default();
        level[i] = order[..n]
            .iter()
            .filter(|&&before| jobs[before].finished.is_some_and(|f| f <= started + SLACK))
            .filter(|&&before| jobs[before].started.is_some_and(|s| s + SLACK < started))
            .map(|&before| level[before] + 1)
            .max()
            .unwrap_or(0);
    }
    let levels = order.iter().map(|&i| level[i] + 1).max().unwrap_or(0);
    let start = start.or_else(|| order.first().and_then(|&i| jobs[i].started));
    let mut columns: Vec<Column> = (0..levels)
        .map(|n| {
            let first = order
                .iter()
                .find(|&&i| level[i] == n)
                .and_then(|&i| jobs[i].started);
            let offset = first.zip(start).map_or(0, |(f, s)| (f - s).max(0));
            Column {
                title: format!("+{}", ci::duration_label(offset)),
                jobs: order.iter().copied().filter(|&i| level[i] == n).collect(),
            }
        })
        .collect();
    let waiting: Vec<usize> = (0..jobs.len())
        .filter(|&i| jobs[i].started.is_none())
        .collect();
    if !waiting.is_empty() {
        columns.push(Column {
            title: "Not started".to_owned(),
            jobs: waiting,
        });
    }
    columns
}

/// Paints the graph of `jobs`, `selected` outlined; a card clicked selects its job.
pub(super) fn job_graph(
    ui: &mut Ui,
    jobs: &[Job],
    start: Option<i64>,
    selected: Option<u64>,
    now: i64,
    acts: &mut Vec<Action>,
) {
    let t = Tokens::get(ui.ctx());
    let columns = columns(jobs, start);
    let tallest = columns.iter().map(|c| c.jobs.len()).max().unwrap_or(0);
    let width = columns.len() as f32 * (CARD_W + COLUMN_GAP) - COLUMN_GAP;
    let height = TITLE_H + tallest as f32 * (CARD_H + CARD_GAP);
    let (area, _) = ui.allocate_exact_size(vec2(width.max(0.0), height), Sense::hover());
    let card = |col: usize, row: usize| {
        let x = area.left() + col as f32 * (CARD_W + COLUMN_GAP);
        let y = area.top() + TITLE_H + row as f32 * (CARD_H + CARD_GAP);
        Rect::from_min_size(pos2(x, y), vec2(CARD_W, CARD_H))
    };
    let line = Stroke::new(1.5, t.text_faint.gamma_multiply(0.6));
    for (n, column) in columns.iter().enumerate() {
        ui.painter().text(
            pos2(card(n, 0).left() + 2.0, area.top() + TITLE_H / 2.0 - 2.0),
            Align2::LEFT_CENTER,
            &column.title,
            theme::semibold(10.5),
            t.text_faint,
        );
        // Every card of this column joins a bar halfway to the next, which joins every card of
        // that one.
        if let Some(next) = columns.get(n + 1) {
            let bar_x = card(n, 0).right() + COLUMN_GAP / 2.0;
            let ys =
                |col: usize, count: usize| (0..count).map(move |row| card(col, row).center().y);
            let all: Vec<f32> = ys(n, column.jobs.len())
                .chain(ys(n + 1, next.jobs.len()))
                .collect();
            let (top, bottom) = all
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
            ui.painter()
                .line_segment([pos2(bar_x, top), pos2(bar_x, bottom)], line);
            for y in ys(n, column.jobs.len()) {
                ui.painter()
                    .line_segment([pos2(card(n, 0).right(), y), pos2(bar_x, y)], line);
            }
            for y in ys(n + 1, next.jobs.len()) {
                ui.painter()
                    .line_segment([pos2(bar_x, y), pos2(card(n + 1, 0).left(), y)], line);
            }
        }
        for (row, &i) in column.jobs.iter().enumerate() {
            job_card(
                ui,
                card(n, row),
                &jobs[i],
                selected == Some(jobs[i].id),
                now,
                acts,
            );
        }
    }
}

fn job_card(ui: &mut Ui, rect: Rect, job: &Job, selected: bool, now: i64, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(rect, ui.id().with(("ci-job", job.id)), Sense::click());
    let fill = if resp.hovered() { t.hover } else { t.card };
    let painter = ui.painter();
    painter.rect_filled(rect, t.radius, fill);
    let border = if selected {
        Stroke::new(1.5, t.accent)
    } else if job.state == CheckState::Failed {
        Stroke::new(1.0, t.danger.gamma_multiply(0.7))
    } else {
        Stroke::new(1.0, t.separator)
    };
    painter.rect_stroke(rect, t.radius, border, egui::StrokeKind::Inside);
    let icon = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(14.0, 14.0));
    state_icon(ui, icon, job.state);
    let text = ui.painter_at(rect.shrink2(vec2(6.0, 0.0)));
    text.text(
        pos2(rect.left() + 32.0, rect.center().y - 7.0),
        Align2::LEFT_CENTER,
        &job.name,
        theme::medium(12.0),
        t.text,
    );
    let when = match ci::duration(job.started, job.finished, now) {
        Some(seconds) => ci::duration_label(seconds),
        None => crate::repo::ci_status::state_word(job.state).to_owned(),
    };
    text.text(
        pos2(rect.left() + 32.0, rect.center().y + 8.0),
        Align2::LEFT_CENTER,
        when,
        FontId::proportional(11.0),
        t.text_faint,
    );
    let resp = resp.on_hover_text(format!(
        "{}\nDouble-click to open it in the browser",
        job.name
    ));
    if resp.double_clicked() {
        acts.push(Action::OpenUrl(job.url.clone()));
    } else if resp.clicked() {
        acts.push(Action::Ci(CiAct::SelectJob(job.id)));
    }
}

/// The icon of a state, as the checks of a pull request have it: a spinner while it runs.
pub(super) fn state_icon(ui: &Ui, rect: Rect, state: CheckState) {
    let t = Tokens::get(ui.ctx());
    let color: Color32 = crate::repo::ci_status::state_color(&t, state);
    match state {
        CheckState::Passed => icons::paint(ui, rect, "check", 13.0, color),
        CheckState::Failed => icons::paint(ui, rect, "x", 13.0, color),
        CheckState::Skipped => icons::paint(ui, rect, "minus", 13.0, color),
        CheckState::Running => {
            egui::Spinner::new()
                .size(11.0)
                .color(color)
                .paint_at(ui, rect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(name: &str, stage: &str, state: CheckState, times: Option<(i64, i64)>) -> Job {
        Job {
            name: name.to_owned(),
            stage: stage.to_owned(),
            state,
            started: times.map(|(s, _)| s),
            finished: times.map(|(_, f)| f),
            ..Job::default()
        }
    }

    fn names(jobs: &[Job], columns: &[Column]) -> Vec<(String, Vec<String>)> {
        columns
            .iter()
            .map(|c| {
                (
                    c.title.clone(),
                    c.jobs.iter().map(|&i| jobs[i].name.clone()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn stages_are_columns_in_their_order_with_failures_on_top() {
        let jobs = [
            job("compile", "build", CheckState::Passed, None),
            job("unit", "test", CheckState::Passed, None),
            job("lint", "test", CheckState::Failed, None),
            job("pages", "deploy", CheckState::Skipped, None),
        ];
        let columns = columns(&jobs, None);
        let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            names(&jobs, &columns),
            [
                ("build".to_owned(), s(&["compile"])),
                ("test".to_owned(), s(&["lint", "unit"])),
                ("deploy".to_owned(), s(&["pages"])),
            ]
        );
    }

    #[test]
    fn jobs_that_waited_for_others_come_after_them() {
        let jobs = [
            job("lint", "", CheckState::Passed, Some((100, 160))),
            job("test", "", CheckState::Passed, Some((101, 400))),
            // Started once lint was done: it needed lint.
            job("docs", "", CheckState::Passed, Some((162, 200))),
            // After test, so after docs too.
            job("release", "", CheckState::Running, Some((402, 500))),
            job("deploy", "", CheckState::Running, None),
        ];
        let columns = columns(&jobs, Some(90));
        let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            names(&jobs, &columns),
            [
                ("+10s".to_owned(), s(&["lint", "test"])),
                ("+1m 12s".to_owned(), s(&["docs"])),
                ("+5m 12s".to_owned(), s(&["release"])),
                ("Not started".to_owned(), s(&["deploy"])),
            ]
        );
    }

    #[test]
    fn no_jobs_no_columns() {
        assert!(columns(&[], None).is_empty());
    }
}
