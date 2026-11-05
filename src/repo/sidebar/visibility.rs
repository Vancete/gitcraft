//! Hiding and soloing branches from the sidebar: the eye on a branch's row, the entries of its
//! menu, and how many branches the graph shows.

use egui::{Rect, Response, RichText, Sense, Ui, pos2, vec2};

use crate::icons;
use crate::repo::branch_visibility::Change;
use crate::repo::{Action, RepoTab};
use crate::theme::Tokens;

impl RepoTab {
    /// Whether the graph shows the branch `full` (`refs/heads/main`); the checked-out one it
    /// always does.
    pub(super) fn branch_shown(&self, full: &str, is_head: bool) -> bool {
        is_head || self.visibility.shows(full)
    }

    /// Hide or show, solo or unsolo the branch `full`; show them all while some are left out.
    pub(super) fn visibility_items(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        full: &str,
        is_head: bool,
    ) {
        let hide = if self.branch_shown(full, is_head) {
            "Hide in the graph"
        } else {
            "Show in the graph"
        };
        if ui
            .add_enabled(!is_head, egui::Button::new(hide))
            .on_disabled_hover_text("The checked-out branch is always shown")
            .clicked()
        {
            acts.push(Action::BranchVisibility(Change::Toggle(full.to_owned())));
        }
        let solo = if self.visibility.solo.contains(full) {
            "Unsolo"
        } else {
            "Solo"
        };
        if ui
            .button(solo)
            .on_hover_text("The graph shows only the soloed branches and the checked-out one")
            .clicked()
        {
            acts.push(Action::BranchVisibility(Change::Solo(full.to_owned())));
        }
        if self.visibility.is_filtering() && ui.button("Show all branches").clicked() {
            acts.push(Action::BranchVisibility(Change::ShowAll));
        }
    }

    /// "Viewing 12 of 55" and a way to show them all, while the graph leaves branches out.
    pub(super) fn visibility_counter(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
        if !self.visibility.is_filtering() {
            return;
        }
        let t = Tokens::get(ui.ctx());
        let (shown, all) = self.visibility.counts(&self.refs);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            icons::inline(ui, "eye", 14.0, t.text_faint);
            let what = if self.visibility.is_soloing() {
                "soloed"
            } else {
                "shown"
            };
            ui.label(RichText::new(format!("Viewing {shown} of {all}")).color(t.text_dim))
                .on_hover_text(format!(
                    "The graph has the commits of the {what} branches and the checked-out one"
                ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                if ui.link("Show all").clicked() {
                    acts.push(Action::BranchVisibility(Change::ShowAll));
                }
            });
        });
        ui.add_space(6.0);
    }
}

/// The eye at the left of the row `row` of the branch `full`: crossed out while the graph leaves
/// the branch out, else only while the row is hovered. Clicking it hides or shows the branch.
pub(super) fn eye(ui: &Ui, row: &Response, full: &str, shown: bool) -> Option<Action> {
    let rect = Rect::from_center_size(
        pos2(row.rect.left() + 14.0, row.rect.center().y),
        vec2(18.0, 18.0),
    );
    let resp = ui.interact(rect, row.id.with("eye"), Sense::click());
    if shown && !row.contains_pointer() && !resp.contains_pointer() {
        return None;
    }
    let t = Tokens::get(ui.ctx());
    let tint = if resp.hovered() { t.text } else { t.text_faint };
    icons::paint(ui, rect, if shown { "eye" } else { "eye-off" }, 13.0, tint);
    let tip = if shown {
        "Hide in the graph"
    } else {
        "Show in the graph"
    };
    resp.on_hover_text(tip)
        .clicked()
        .then(|| Action::BranchVisibility(Change::Toggle(full.to_owned())))
}
