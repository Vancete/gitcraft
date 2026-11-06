//! The file view's History tab: every commit that changed the file, and the one clicked beside
//! them, as its diff or the whole file.

use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};

use super::{FileTab, error};
use crate::git;
use crate::repo::{Action, DiffKey, RepoTab, widgets};
use crate::theme::{self, Tokens};

const ROW_H: f32 = 30.0;

impl RepoTab {
    /// Whether the open diff is of a commit in the file history shown, which then lists the
    /// commits beside it.
    pub(in crate::repo) fn history_beside_diff(&self) -> bool {
        let Some(view) = &self.file_view else {
            return false;
        };
        let Some(DiffKey::Commit { hash, .. }) = self.diff.as_ref().map(|d| &d.key) else {
            return false;
        };
        view.tab == FileTab::History
            && matches!(&view.history, Some(Ok(commits)) if commits.iter().any(|c| &c.hash == hash))
    }

    /// The commits, with the diff of the one clicked on their right.
    pub(super) fn file_history(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        if !self.history_beside_diff() {
            self.history_list(ui, acts);
            return;
        }
        let t = Tokens::get(ui.ctx());
        let width = ui.available_width();
        egui::Panel::left("file_history_list")
            .resizable(true)
            .default_size((width * 0.35).max(260.0))
            .size_range(200.0..=(width * 0.7).max(200.0))
            .frame(egui::Frame::new().fill(t.dock))
            .show(ui, |ui| self.history_list(ui, acts));
        self.diff_view(ui, acts);
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
                        "Click: this commit's changes to the file, or the file as it was\nDouble-click: show the commit in the graph",
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
}
