//! The file view's Blame tab: who last changed each line, newer changes marked brighter.

use std::path::Path;

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::error;
use crate::git::{self, BlameLine};
use crate::repo::highlight::{self, Spans};
use crate::repo::{Action, RepoTab};
use crate::theme::{self, Tokens};

const LINE_H: f32 = 19.0;
const GUTTER_W: f32 = 270.0;
const NUM_W: f32 = 46.0;

pub(crate) struct BlameData {
    lines: Vec<BlameLine>,
    spans: Option<Vec<Spans>>,
    /// Oldest and newest commit time, for the age colour.
    span: (i64, i64),
    /// Characters in the longest line, for the width to scroll (counted once, not every frame).
    longest: usize,
}

/// Runs `git blame` and colours the lines; on a background thread.
pub(crate) fn load_blame(
    repo: &Path,
    rev: Option<&str>,
    path: &str,
    dark: bool,
) -> Result<BlameData, String> {
    let lines = git::blame(repo, rev, path)?;
    let spans = highlight::highlight(path, lines.iter().map(|l| l.text.as_str()), dark);
    let times = lines.iter().filter(|l| !l.uncommitted()).map(|l| l.time);
    let span = (times.clone().min().unwrap_or(0), times.max().unwrap_or(0));
    // A tab is drawn four spaces wide (the diff expands them; blame keeps them).
    let longest = lines
        .iter()
        .map(|l| l.text.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum())
        .max()
        .unwrap_or(0);
    Ok(BlameData {
        lines,
        spans,
        span,
        longest,
    })
}

impl RepoTab {
    pub(super) fn blame_lines(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &self.file_view else { return };
        let data = match &view.blame {
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            Some(Err(e)) => {
                error(ui, e);
                return;
            }
            Some(Ok(d)) => d,
        };
        let now = git::now();
        let font = theme::mono(12.0);
        let char_w = ui
            .painter()
            .layout_no_wrap("M".to_owned(), font.clone(), t.text)
            .size()
            .x;
        let longest = data.longest as f32;
        let width = (GUTTER_W + NUM_W + 16.0 + longest * char_w + 24.0).max(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::both()
            .id_salt(("file_blame", &view.path, &view.rev))
            .auto_shrink(false)
            .show_rows(ui, LINE_H, data.lines.len(), |ui, range| {
                for i in range {
                    let Some(line) = data.lines.get(i) else {
                        continue;
                    };
                    let (rect, _) = ui.allocate_exact_size(vec2(width, LINE_H), Sense::hover());
                    let clip = rect.intersect(ui.clip_rect());
                    let p = ui.painter().with_clip_rect(clip);
                    let first =
                        i == 0 || data.lines.get(i - 1).map(|l| &l.hash) != Some(&line.hash);
                    // Each change starts with a thin rule, so blocks read as one.
                    if first && i > 0 {
                        p.hline(rect.x_range(), rect.top(), Stroke::new(1.0, t.separator));
                    }
                    let gutter = Rect::from_min_size(rect.min, vec2(GUTTER_W, LINE_H));
                    p.rect_filled(gutter, 0.0, t.card);
                    p.rect_filled(
                        Rect::from_min_size(rect.min, vec2(3.0, LINE_H)),
                        0.0,
                        age_color(&t, line, data.span),
                    );
                    let mid = rect.center().y;
                    let gutter_resp =
                        ui.interact(gutter, ui.id().with(("blame", i)), Sense::click());
                    if first {
                        let small = FontId::proportional(11.5);
                        if line.uncommitted() {
                            p.text(
                                pos2(gutter.left() + 10.0, mid),
                                Align2::LEFT_CENTER,
                                "Not committed yet",
                                small,
                                t.warning,
                            );
                        } else {
                            let hash = p.text(
                                pos2(gutter.left() + 10.0, mid),
                                Align2::LEFT_CENTER,
                                line.hash.get(..7).unwrap_or(&line.hash),
                                theme::mono(11.0),
                                if gutter_resp.hovered() {
                                    t.accent
                                } else {
                                    t.text_faint
                                },
                            );
                            let when = p.text(
                                pos2(gutter.right() - 8.0, mid),
                                Align2::RIGHT_CENTER,
                                git::relative_time(line.time, now),
                                small.clone(),
                                t.text_faint,
                            );
                            p.with_clip_rect(Rect::from_min_max(
                                pos2(hash.right() + 8.0, rect.top()),
                                pos2(when.left() - 8.0, rect.bottom()),
                            ))
                            .text(
                                pos2(hash.right() + 10.0, mid),
                                Align2::LEFT_CENTER,
                                &line.author,
                                small,
                                t.text_dim,
                            );
                        }
                    }
                    if !line.uncommitted() {
                        let gutter_resp = gutter_resp.on_hover_text(format!(
                            "{} · {}\n{}\nClick to show the commit",
                            line.author,
                            git::relative_time(line.time, now),
                            line.summary
                        ));
                        if gutter_resp.clicked() {
                            acts.push(Action::CloseFileView);
                            acts.push(Action::SelectHash(line.hash.clone()));
                        }
                    }
                    p.text(
                        pos2(gutter.right() + NUM_W - 8.0, mid),
                        Align2::RIGHT_CENTER,
                        (i + 1).to_string(),
                        theme::mono(11.0),
                        t.text_faint,
                    );
                    let x = gutter.right() + NUM_W + 8.0;
                    match data.spans.as_ref().and_then(|s| s.get(i)) {
                        Some(spans) => {
                            let mut job = egui::text::LayoutJob::default();
                            for (color, text) in spans {
                                job.append(
                                    text,
                                    0.0,
                                    egui::TextFormat {
                                        font_id: font.clone(),
                                        color: *color,
                                        ..Default::default()
                                    },
                                );
                            }
                            let galley = ui.painter().layout_job(job);
                            p.galley(pos2(x, mid - galley.size().y / 2.0), galley, t.text);
                        }
                        None => {
                            p.text(
                                pos2(x, mid),
                                Align2::LEFT_CENTER,
                                &line.text,
                                font.clone(),
                                t.text,
                            );
                        }
                    }
                }
            });
    }
}

/// Newer lines get a stronger accent bar; uncommitted ones the warning colour.
fn age_color(t: &Tokens, line: &BlameLine, (oldest, newest): (i64, i64)) -> Color32 {
    if line.uncommitted() {
        return t.warning;
    }
    let f = if newest > oldest {
        (line.time - oldest) as f32 / (newest - oldest) as f32
    } else {
        1.0
    };
    t.accent.gamma_multiply(0.15 + 0.85 * f)
}
