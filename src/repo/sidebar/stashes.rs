//! The stashes, and the menu a stash has here and in the detail pane.

use egui::{Align2, Rect, Sense, Ui, pos2, vec2};

use crate::git;
use crate::icons;
use crate::repo::pinned_sections::{Button, Header, Sections};
use crate::repo::{Action, Dialog, Refresh, RepoTab, ops};
use crate::theme::Tokens;

impl RepoTab {
    pub(super) fn stash_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        sections: &mut Sections,
    ) {
        let t = Tokens::get(ui.ctx());
        let can_stash = idle && !self.work.is_clean();
        let open = sections.add(
            ui,
            Header {
                key: "stashes",
                icon: "archive",
                title: "STASHES",
                count: self.stashes.len(),
                default_open: true,
                button: Some(Button {
                    icon: "plus",
                    tooltip: "Stash changes…",
                    action: can_stash.then(|| Action::Dialog(Dialog::stash())),
                }),
            },
        );
        if !open {
            return;
        }
        let now = git::now();
        for (i, stash) in self.stashes.iter().enumerate() {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
            if self.stash_view == Some(i) {
                ui.painter().rect_filled(rect, 0.0, t.row_selected);
            } else if r.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let y = rect.center().y;
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
                "archive",
                13.0,
                t.text_faint,
            );
            let p = ui.painter_at(rect);
            p.text(
                pos2(rect.left() + 48.0, y - 7.0),
                Align2::LEFT_CENTER,
                &stash.message,
                egui::FontId::proportional(12.5),
                t.text,
            );
            p.text(
                pos2(rect.left() + 48.0, y + 8.0),
                Align2::LEFT_CENTER,
                format!("{} · {}", stash.name, git::relative_time(stash.time, now)),
                egui::FontId::proportional(11.0),
                t.text_faint,
            );
            if r.clicked() {
                acts.push(Action::SelectStash(i));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| stash_menu_items(ui, acts, &stash.name, idle));
        }
    }
}

/// Apply / pop / drop for a stash (sidebar and detail pane).
pub(in crate::repo) fn stash_menu_items(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    name: &str,
    idle: bool,
) {
    if ui
        .add_enabled(idle, egui::Button::new("Apply"))
        .on_hover_text("Bring the changes back and keep the stash")
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Apply {name}"),
            ops: ops(&[&["stash", "apply", "--index", name]]),
            refresh: Refresh::All,
        });
    }
    if ui
        .add_enabled(idle, egui::Button::new("Pop"))
        .on_hover_text("Bring the changes back and drop the stash")
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Pop {name}"),
            ops: ops(&[&["stash", "pop", "--index", name]]),
            refresh: Refresh::All,
        });
    }
    ui.separator();
    if ui.add_enabled(idle, egui::Button::new("Drop…")).clicked() {
        acts.push(Action::Dialog(Dialog::confirm(
            "Drop stash",
            format!("Delete {name}? Its changes are gone for good."),
            "Drop",
            ops(&[&["stash", "drop", name]]),
            None,
        )));
    }
}
