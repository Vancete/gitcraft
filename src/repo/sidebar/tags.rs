//! The tags.

use egui::{Align2, Rect, Response, Sense, Ui, pos2, vec2};

use super::{ROW_H, matches};
use crate::git::Branch;
use crate::icons;
use crate::repo::pinned_sections::{Header, Sections};
use crate::repo::{Action, Dialog, Refresh, RepoTab, Undo, ops};
use crate::theme::Tokens;

impl RepoTab {
    /// The tags passing `filter`; none at all hides the section.
    pub(super) fn tags_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        filter: &str,
        sections: &mut Sections,
    ) {
        let tags: Vec<&Branch> = self
            .refs
            .tags
            .iter()
            .filter(|b| matches(filter, b))
            .collect();
        if tags.is_empty()
            || !sections.add(
                ui,
                Header {
                    key: "tags",
                    icon: "tag",
                    title: "TAGS",
                    count: tags.len(),
                    default_open: false,
                    button: None,
                },
            )
        {
            return;
        }
        for tag in tags {
            let r = plain_row(ui, "tag", &tag.name, "");
            if r.clicked() {
                acts.push(Action::SelectHash(tag.target.clone()));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| self.tag_menu_items(ui, acts, tag, idle));
        }
    }

    fn tag_menu_items(&self, ui: &mut Ui, acts: &mut Vec<Action>, tag: &Branch, idle: bool) {
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(tag.name.clone());
        }
        if let Some(remote) = self.default_remote() {
            if ui
                .add_enabled(idle, egui::Button::new(format!("Push to {remote}")))
                .clicked()
            {
                acts.push(Action::Ops {
                    label: format!("Push tag {}", tag.name),
                    ops: ops(&[&["push", &remote, &format!("refs/tags/{}", tag.name)]]),
                    refresh: Refresh::All,
                });
            }
            if ui
                .add_enabled(idle, egui::Button::new(format!("Delete from {remote}…")))
                .clicked()
            {
                acts.push(Action::Dialog(Dialog::confirm(
                    "Delete remote tag",
                    format!("Delete the tag “{}” from {remote}?", tag.name),
                    "Delete",
                    ops(&[&[
                        "push",
                        &remote,
                        "--delete",
                        &format!("refs/tags/{}", tag.name),
                    ]]),
                    None,
                )));
            }
        }
        ui.separator();
        if ui
            .add_enabled(idle, egui::Button::new("Delete tag…"))
            .clicked()
        {
            acts.push(Action::Dialog(
                Dialog::confirm(
                    "Delete tag",
                    format!("Delete the local tag “{}”?", tag.name),
                    "Delete",
                    ops(&[&["tag", "-d", "--", &tag.name]]),
                    None,
                )
                .undo(Undo::Custom {
                    undo: ops(&[&["tag", "--", &tag.name, &tag.target]]),
                    redo: ops(&[&["tag", "-d", "--", &tag.name]]),
                }),
            ));
        }
    }
}

/// An icon, a name and optional faint text after it.
fn plain_row(ui: &mut Ui, icon: &str, name: &str, sub: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
        icon,
        13.0,
        t.text_faint,
    );
    let p = ui.painter_at(rect);
    let r = p.text(
        pos2(rect.left() + 48.0, y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        t.text,
    );
    if !sub.is_empty() {
        p.text(
            pos2(r.right() + 8.0, y),
            Align2::LEFT_CENTER,
            sub,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
    }
    resp
}
