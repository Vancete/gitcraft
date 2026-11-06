//! The repository's submodules.

use egui::{Align2, Rect, Response, Ui, pos2, vec2};

use super::ROW_H;
use crate::git::{Submodule, SubmoduleState};
use crate::icons;
use crate::repo::keep_submodules::{update_all, update_one};
use crate::repo::pinned_sections::{Button, Header, Sections};
use crate::repo::{Action, RepoTab, reveal_label, short, widgets};
use crate::theme::{self, Tokens};

impl RepoTab {
    /// The submodules; none at all hides the section.
    pub(super) fn submodule_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        sections: &mut Sections,
    ) {
        if self.submodules.is_empty()
            || !sections.add(
                ui,
                Header {
                    key: "submodules",
                    icon: "package",
                    title: "SUBMODULES",
                    count: self.submodules.len(),
                    default_open: true,
                    button: Some(Button {
                        icon: "refresh-cw",
                        tooltip: "Update all submodules",
                        action: idle.then(update_all),
                    }),
                },
            )
        {
            return;
        }
        for sub in &self.submodules {
            let r = submodule_row(ui, sub).on_hover_text(submodule_tip(sub));
            let cloned = sub.state != SubmoduleState::NotInitialized;
            if r.double_clicked() && cloned {
                acts.push(Action::OpenTab(self.path.join(&sub.path)));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| self.submodule_menu_items(ui, acts, sub, idle, cloned));
        }
    }

    fn submodule_menu_items(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        sub: &Submodule,
        idle: bool,
        cloned: bool,
    ) {
        if ui
            .add_enabled(idle, egui::Button::new("Update"))
            .on_hover_text("Clone it if needed and check out the commit recorded for it")
            .clicked()
        {
            acts.push(update_one(&sub.path));
        }
        let folder = self.path.join(&sub.path);
        if ui
            .add_enabled(cloned, egui::Button::new("Open in a new tab"))
            .on_disabled_hover_text("Update it first to clone it")
            .clicked()
        {
            acts.push(Action::OpenTab(folder.clone()));
        }
        if ui
            .add_enabled(folder.is_dir(), egui::Button::new(reveal_label()))
            .clicked()
        {
            acts.push(Action::OpenPath(sub.path.clone().into()));
        }
        ui.separator();
        if ui.button("Copy path").clicked() {
            ui.ctx().copy_text(sub.path.clone());
        }
        if !sub.url.is_empty() && ui.button("Copy URL").clicked() {
            ui.ctx().copy_text(sub.url.clone());
        }
    }
}

/// A submodule: its path, its commit, and how it stands at the right.
fn submodule_row(ui: &mut Ui, sub: &Submodule) -> Response {
    let t = Tokens::get(ui.ctx());
    // Known by its path, not its place: a menu left open (Update) must stay this submodule's.
    let (rect, resp) = widgets::keyed_row(ui, ROW_H, ("submodule-row", &sub.path));
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    let faint = sub.state == SubmoduleState::NotInitialized;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
        "package",
        13.0,
        t.text_faint,
    );
    let painter = ui.painter_at(rect);
    let (label, color) = match (sub.state, sub.modified) {
        (SubmoduleState::Conflicted, _) => (Some("conflict"), t.danger),
        (SubmoduleState::UpToDate, true) => (Some("modified"), t.warning),
        (state, _) => (state.label(), if faint { t.text_faint } else { t.warning }),
    };
    let mut right = rect.right() - 8.0;
    if let Some(label) = label {
        let r = painter.text(
            pos2(right, y),
            Align2::RIGHT_CENTER,
            label,
            theme::medium(10.5),
            color,
        );
        right = r.left() - 6.0;
    }
    let clip = painter.with_clip_rect(Rect::from_min_max(rect.min, pos2(right, rect.bottom())));
    let name = clip.text(
        pos2(rect.left() + 48.0, y),
        Align2::LEFT_CENTER,
        &sub.path,
        egui::FontId::proportional(12.5),
        if faint { t.text_dim } else { t.text },
    );
    clip.text(
        pos2(name.right() + 8.0, y),
        Align2::LEFT_CENTER,
        short(&sub.hash),
        egui::FontId::proportional(11.0),
        t.text_faint,
    );
    resp
}

/// The hover text of a submodule: where it comes from, its commit, what its state means.
fn submodule_tip(sub: &Submodule) -> String {
    let mut tip = sub.path.clone();
    if !sub.url.is_empty() {
        tip.push_str(&format!("\nFrom {}", sub.url));
    }
    let at = match &sub.describe {
        Some(describe) => format!("{} ({describe})", short(&sub.hash)),
        None => short(&sub.hash).to_owned(),
    };
    tip.push_str(&match sub.state {
        SubmoduleState::UpToDate => format!("\nAt {at}, as recorded"),
        SubmoduleState::NotInitialized => {
            format!("\nNot cloned yet; Update clones it at {at}")
        }
        SubmoduleState::OutOfDate => {
            format!("\nAt {at}, not the commit recorded here; Update checks that one out")
        }
        SubmoduleState::Conflicted => format!("\nIn conflict; at {at}"),
    });
    if sub.modified {
        tip.push_str("\nHas changes of its own");
    }
    if sub.state != SubmoduleState::NotInitialized {
        tip.push_str("\nDouble-click to open in a tab");
    }
    tip
}
