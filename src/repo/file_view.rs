//! Central pane for one file: its history (every commit that changed it, following renames) or
//! its blame (who last changed each line). Opened from a file's right-click menu.

use std::path::Path;

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::highlight::{self, Spans};
use super::widgets;
use super::{Action, DiffKey, RepoTab};
use crate::git::{self, BlameLine, FileCommit};
use crate::icons;
use crate::theme::{self, Tokens};

const ROW_H: f32 = 30.0;
const LINE_H: f32 = 19.0;
const GUTTER_W: f32 = 270.0;
const NUM_W: f32 = 46.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileTab {
    History,
    Blame,
}

pub(crate) struct FileView {
    pub path: String,
    /// The commit history and blame start at; HEAD and the working tree when `None`.
    pub rev: Option<String>,
    pub tab: FileTab,
    pub history: Option<Result<Vec<FileCommit>, String>>,
    pub blame: Option<Result<BlameData, String>>,
}

pub(crate) struct BlameData {
    lines: Vec<BlameLine>,
    spans: Option<Vec<Spans>>,
    /// Oldest and newest commit time, for the age colour.
    span: (i64, i64),
    /// Characters in the longest line, for the width to scroll (counted once, not every frame).
    longest: usize,
}

impl FileView {
    pub fn new(path: &str, rev: Option<String>, tab: FileTab) -> Self {
        Self {
            path: path.to_owned(),
            rev,
            tab,
            history: None,
            blame: None,
        }
    }

    /// Whether `tab` still has to be loaded.
    pub fn needs(&self, tab: FileTab) -> bool {
        match tab {
            FileTab::History => self.history.is_none(),
            FileTab::Blame => self.blame.is_none(),
        }
    }
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
    pub(super) fn file_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &self.file_view else { return };

        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "history", 16.0, t.icon);
                    ui.label(egui::RichText::new(&view.path).font(theme::medium(13.0)));
                    if let Some(rev) = &view.rev {
                        widgets::chip(
                            ui,
                            &format!("at {}", rev.get(..8).unwrap_or(rev)),
                            t.text_dim,
                        );
                    }
                    ui.add_space(12.0);
                    for (tab, label) in [(FileTab::History, "History"), (FileTab::Blame, "Blame")] {
                        let on = view.tab == tab;
                        let b = ui.add(
                            egui::Button::new(egui::RichText::new(label).color(if on {
                                t.text
                            } else {
                                t.text_dim
                            }))
                            .frame_when_inactive(on),
                        );
                        if on {
                            widgets::active_underline(ui, b.rect, t.accent);
                        }
                        if b.clicked() && !on {
                            acts.push(Action::FileView {
                                path: view.path.clone(),
                                rev: view.rev.clone(),
                                tab,
                            });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::CloseFileView);
                        }
                    });
                });
            });

        match view.tab {
            FileTab::History => self.history_list(ui, acts),
            FileTab::Blame => self.blame_lines(ui, acts),
        }
    }

    fn history_list(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &self.file_view else { return };
        let commits = match &view.history {
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            Some(Err(e)) => {
                error(ui, e);
                return;
            }
            Some(Ok(c)) => c,
        };
        if commits.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("No commits touch this file yet.").color(t.text_dim));
            });
            return;
        }
        let now = git::now();
        let open = match self.diff.as_ref().map(|d| &d.key) {
            Some(DiffKey::Commit { hash, .. }) => Some(hash.as_str()),
            _ => None,
        };
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical()
            .id_salt(("file_history", &view.path))
            .auto_shrink(false)
            .show_rows(ui, ROW_H, commits.len(), |ui, range| {
                for i in range {
                    let Some(c) = commits.get(i) else { continue };
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    let p = ui.painter_at(rect);
                    if open == Some(c.hash.as_str()) {
                        p.rect_filled(rect, 0.0, t.row_selected);
                    } else if resp.hovered() {
                        p.rect_filled(rect, 0.0, t.hover);
                    }
                    let mid = rect.center().y;
                    widgets::avatar(ui, pos2(rect.left() + 22.0, mid), 10.0, &c.author, &c.email);
                    let right = rect.right() - 12.0;
                    let hash = p.text(
                        pos2(right, mid),
                        Align2::RIGHT_CENTER,
                        c.hash.get(..7).unwrap_or(&c.hash),
                        theme::mono(11.0),
                        t.text_faint,
                    );
                    let when = p.text(
                        pos2(hash.left() - 16.0, mid),
                        Align2::RIGHT_CENTER,
                        git::relative_time(c.time, now),
                        FontId::proportional(11.5),
                        t.text_dim,
                    );
                    let author = p.text(
                        pos2(when.left() - 16.0, mid),
                        Align2::RIGHT_CENTER,
                        &c.author,
                        FontId::proportional(11.5),
                        t.text_dim,
                    );
                    let mut summary = c.summary.clone();
                    if c.path != view.path {
                        summary = format!("{summary}  ·  as {}", c.path);
                    }
                    p.with_clip_rect(Rect::from_min_max(
                        rect.min,
                        pos2(author.left() - 16.0, rect.bottom()),
                    ))
                    .text(
                        pos2(rect.left() + 42.0, mid),
                        Align2::LEFT_CENTER,
                        summary,
                        FontId::proportional(12.5),
                        t.text,
                    );
                    let resp = resp.on_hover_text(
                        "Click: this commit's changes to the file\nDouble-click: show the commit in the graph",
                    );
                    if resp.double_clicked() {
                        acts.push(Action::CloseFileView);
                        acts.push(Action::SelectHash(c.hash.clone()));
                    } else if resp.clicked() {
                        acts.push(Action::OpenDiff(DiffKey::Commit {
                            hash: c.hash.clone(),
                            path: c.path.clone(),
                        }));
                    }
                }
            });
    }

    fn blame_lines(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
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

fn error(ui: &mut Ui, e: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.colored_label(t.danger, e);
    });
}
