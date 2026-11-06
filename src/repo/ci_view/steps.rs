//! The steps of a job as bars on a time line, so the slow one and the one that failed stand
//! out; and the failed jobs' log.

use egui::{Align2, FontId, Rect, RichText, Sense, Ui, pos2, vec2};

use super::graph::state_icon;
use crate::ci::{self, CheckState, Job};
use crate::repo::ci_status::state_color;
use crate::theme::{self, Tokens};

const ROW_H: f32 = 22.0;
const NAME_W: f32 = 230.0;
const TIME_W: f32 = 64.0;

/// Where on the time line a step is: from `start` to `end` of `span`, as fractions of it.
pub(super) fn bar(
    span: (i64, i64),
    started: Option<i64>,
    finished: Option<i64>,
    now: i64,
) -> Option<(f32, f32)> {
    let (from, to) = span;
    let started = started?;
    let length = (to - from).max(1) as f32;
    let end = finished.unwrap_or(now).clamp(started, to.max(started));
    let a = ((started - from) as f32 / length).clamp(0.0, 1.0);
    let b = ((end - from) as f32 / length).clamp(a, 1.0);
    Some((a, b))
}

/// The time line of `job`'s steps.
pub(super) fn timeline(ui: &mut Ui, job: &Job, now: i64) {
    let t = Tokens::get(ui.ctx());
    if job.steps.is_empty() {
        let text = if !job.stage.is_empty() {
            "GitLab doesn't list the steps of a job: its log is on its page."
        } else if job.state == CheckState::Running {
            "This job has no steps to show yet."
        } else {
            "This job ran no steps."
        };
        ui.label(RichText::new(text).color(t.text_faint).italics());
        return;
    }
    let Some(from) = job
        .started
        .or_else(|| job.steps.iter().find_map(|s| s.started))
    else {
        ui.label(
            RichText::new("Not started yet.")
                .color(t.text_faint)
                .italics(),
        );
        return;
    };
    let to = job.finished.unwrap_or(now).max(from + 1);
    let running = job.state == CheckState::Running;
    for step in &job.steps {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        if resp.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        }
        let y = rect.center().y;
        let icon = Rect::from_center_size(pos2(rect.left() + 10.0, y), vec2(14.0, 14.0));
        state_icon(ui, icon, step.state);
        let started = step.started.is_some();
        let name_clip =
            Rect::from_min_max(rect.min, pos2(rect.left() + NAME_W - 8.0, rect.bottom()));
        ui.painter().with_clip_rect(name_clip).text(
            pos2(rect.left() + 24.0, y),
            Align2::LEFT_CENTER,
            &step.name,
            FontId::proportional(12.0),
            if started { t.text } else { t.text_faint },
        );
        let track = Rect::from_min_max(
            pos2(rect.left() + NAME_W, y - 5.0),
            pos2(
                (rect.right() - TIME_W).max(rect.left() + NAME_W + 20.0),
                y + 5.0,
            ),
        );
        ui.painter().rect_filled(track, t.radius_sm, t.field);
        if let Some((a, b)) = bar((from, to), step.started, step.finished, now) {
            let x = |f: f32| track.left() + f * track.width();
            let fill = Rect::from_min_max(
                pos2(x(a), track.top()),
                pos2(x(b).max(x(a) + 2.0), track.bottom()),
            );
            ui.painter()
                .rect_filled(fill, t.radius_sm, state_color(&t, step.state));
        }
        if let Some(seconds) = ci::duration(step.started, step.finished, now) {
            ui.painter().text(
                pos2(rect.right() - 4.0, y),
                Align2::RIGHT_CENTER,
                ci::duration_label(seconds),
                theme::mono(11.0),
                t.text_dim,
            );
        }
        let span = match (step.started, step.finished) {
            (Some(_), None) => "running".to_owned(),
            (None, _) => "not started".to_owned(),
            _ => crate::repo::ci_status::state_word(step.state).to_owned(),
        };
        resp.on_hover_text(format!("{}\n{span}", step.name));
    }
    if running {
        // The bar of the step that runs grows.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }
}

/// The log of the failed jobs, read-only and selectable.
pub(super) fn log(ui: &mut Ui, log: &str) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::new()
        .fill(t.field)
        .corner_radius(t.radius)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("ci-log")
                .max_height(420.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let mut text = log;
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .font(theme::mono(11.0))
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE),
                    );
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_sit_where_their_steps_ran() {
        assert_eq!(bar((100, 200), Some(150), Some(200), 300), Some((0.5, 1.0)));
        assert_eq!(bar((100, 200), Some(100), Some(110), 300), Some((0.0, 0.1)));
        // Still running: up to now.
        assert_eq!(bar((100, 200), Some(120), None, 140), Some((0.2, 0.4)));
        assert_eq!(bar((100, 200), None, None, 140), None);
    }
}
