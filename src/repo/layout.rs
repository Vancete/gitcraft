//! The tab's layout: toolbar on top, the foldable side panels, the terminal and the central
//! view; and its keyboard shortcuts.

use egui::{Key, Ui};

use super::{Action, RepoTab, pr_review, widgets};
use crate::icons;
use crate::settings::Settings;
use crate::theme::Tokens;

impl RepoTab {
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // Shown: its first read can't wait any more.
        self.start_deferred(&ctx);
        self.poll(&ctx);
        let t = Tokens::get(&ctx);
        if self.opening() {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(t.dock))
                .show(ui, |ui| self.opening_screen(ui));
            return;
        }
        let mut acts: Vec<Action> = std::mem::take(&mut self.queued);
        self.shortcuts(&ctx, &mut acts);

        egui::Panel::top("repo_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(t.chrome)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .stroke(egui::Stroke::new(1.0, t.separator)),
            )
            .show(ui, |ui| self.toolbar(ui, &mut acts));
        self.side_panels(ui, &mut acts);
        if self.terminal_open {
            egui::Panel::bottom("repo_terminal")
                .resizable(true)
                .default_size(260.0)
                .size_range(90.0..=900.0)
                .frame(egui::Frame::new().fill(t.card))
                .show(ui, |ui| self.terminal_panel(ui, &mut acts));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.dock))
            .show(ui, |ui| {
                if self.conflict.is_some() {
                    self.conflict_view(ui, &mut acts);
                } else if self.rebase.is_some() {
                    self.rebase_view(ui, &mut acts);
                } else if self.diff.is_some() && !self.history_beside_diff() {
                    self.diff_view(ui, &mut acts);
                } else if self.file_view.is_some() {
                    self.file_view(ui, &mut acts);
                } else if self.ci_view.is_some() {
                    self.ci_view_ui(ui, &mut acts);
                } else if self.pr_view.is_some() {
                    self.pr_view_ui(ui, &mut acts);
                } else {
                    self.graph_view(ui, &mut acts);
                }
            });

        self.dialogs(&ctx, &mut acts);
        for action in acts {
            self.apply(action, &ctx);
        }
    }

    /// Until the first read is in: rather than branches, history and changes that all look
    /// empty, the repository being opened. Nothing at all for the first moments, so that a
    /// repository read in an instant doesn't flash it.
    fn opening_screen(&self, ui: &mut Ui) {
        const QUIET: std::time::Duration = std::time::Duration::from_millis(150);
        let waited = self.opened.elapsed();
        if waited < QUIET {
            ui.ctx().request_repaint_after(QUIET - waited);
            return;
        }
        let t = Tokens::get(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 70.0).max(0.0));
            icons::inline(ui, "folder-git-2", 40.0, t.text_faint);
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(format!("Opening {}", self.name))
                    .font(crate::theme::semibold(18.0))
                    .color(t.text),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(self.path.display().to_string())
                    .size(12.0)
                    .color(t.text_faint),
            );
            ui.add_space(18.0);
            ui.add(egui::Spinner::new().size(22.0));
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("Reading branches and history…")
                    .size(12.5)
                    .color(t.text_dim),
            );
        });
    }

    /// Escape closes the innermost view; Ctrl+Z / Ctrl+Y undo and redo, Ctrl+F searches, F7 and
    /// Shift+F7 (or Alt+↓ / Alt+↑) go to the next and previous change of a diff.
    fn shortcuts(&mut self, ctx: &egui::Context, acts: &mut Vec<Action>) {
        let free_keys =
            self.dialog.is_none() && !self.keys_blocked && !ctx.egui_wants_keyboard_input();
        if free_keys {
            let (esc, undo, redo, find) = ctx.input(|i| {
                (
                    i.key_pressed(Key::Escape),
                    i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::Z),
                    i.modifiers.command
                        && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                    i.modifiers.command && i.key_pressed(Key::F),
                )
            });
            if esc && self.diff.is_some() && self.line_comment_open() {
                acts.push(Action::Pr(pr_review::PrAct::CloseLine));
            } else if esc && self.diff.is_some() {
                acts.push(Action::CloseDiff);
            } else if esc && self.rebase.is_some() {
                acts.push(Action::CloseRebase);
            } else if esc && self.file_view.is_some() {
                acts.push(Action::CloseFileView);
            } else if esc && self.ci_view.is_some() {
                acts.push(Action::CloseCi);
            } else if esc && self.pr_view.is_some() {
                acts.push(Action::ClosePr);
            } else if esc && self.compare.is_some() {
                acts.push(Action::CloseCompare);
            } else if esc && self.search.open {
                acts.push(Action::ToggleSearch);
            }
            let diff_shown =
                self.diff.is_some() && self.conflict.is_none() && self.rebase.is_none();
            if let Some(delta) = ctx.input(change_step).filter(|_| diff_shown) {
                acts.push(Action::DiffStep(delta));
            }
            if undo {
                acts.push(Action::Undo);
            }
            if redo {
                acts.push(Action::Redo);
            }
            if find {
                self.search.open = true;
                self.search.focus = true;
            }
        } else if self.search.open && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
            self.search.focus = true;
        }
    }

    /// The branches panel on the left and the detail panel on the right.
    fn side_panels(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        let side = egui::Frame::new().fill(t.card);
        // Both side panels fold down to a strip holding their footer's toggle, also when an
        // edge is dragged past its minimum; dragging the strip out opens them again.
        let mut settings = Settings::get(&ctx);
        let (mut show_branches, mut show_detail) = (settings.show_branches, settings.show_detail);
        let area = ui.available_rect_before_wrap();
        egui::Panel::show_switched(
            ui,
            &mut show_branches,
            egui::Panel::left("repo_branches_folded")
                .resizable(true)
                .exact_size(widgets::FOLDED_W)
                .frame(side),
            egui::Panel::left("repo_branches")
                .resizable(true)
                .default_size(270.0)
                .size_range(170.0..=520.0)
                .frame(side),
            |ui, open| {
                widgets::panel_footer(ui, "repo_branches_footer", area.left(), false, |ui| {
                    if panel_toggle(ui, "panel-left", "branches", open) {
                        acts.push(Action::ToggleBranches);
                    }
                });
                if open {
                    self.sidebar(ui, acts);
                } else {
                    self.folded_sidebar(ui, area.left(), acts);
                }
            },
        );
        egui::Panel::show_switched(
            ui,
            &mut show_detail,
            egui::Panel::right("repo_detail_folded")
                .resizable(true)
                .exact_size(widgets::FOLDED_W)
                .frame(side),
            egui::Panel::right("repo_detail")
                .resizable(true)
                .default_size(420.0)
                .size_range(280.0..=900.0)
                .frame(side),
            |ui, open| {
                widgets::panel_footer(ui, "repo_detail_footer", area.right(), true, |ui| {
                    if panel_toggle(ui, "panel-right", "detail", open) {
                        acts.push(Action::ToggleDetail);
                    }
                });
                if open {
                    self.detail_panel(ui, acts);
                }
            },
        );
        if (show_branches, show_detail) != (settings.show_branches, settings.show_detail) {
            settings.show_branches = show_branches;
            settings.show_detail = show_detail;
            settings.set(&ctx);
        }
    }
}

/// F7 or Alt+↓ for the next change of a diff (`1`), Shift+F7 or Alt+↑ for the previous one.
fn change_step(i: &egui::InputState) -> Option<i32> {
    let alt = i.modifiers.alt && !i.modifiers.command;
    if (i.key_pressed(Key::F7) && !i.modifiers.shift) || (alt && i.key_pressed(Key::ArrowDown)) {
        Some(1)
    } else if (i.key_pressed(Key::F7) && i.modifiers.shift) || (alt && i.key_pressed(Key::ArrowUp))
    {
        Some(-1)
    } else {
        None
    }
}

/// The footer button folding a side panel down to its strip, or opening it back up.
fn panel_toggle(ui: &mut Ui, icon: &str, name: &str, open: bool) -> bool {
    let tip = if open {
        format!("Hide the {name} panel")
    } else {
        format!("Show the {name} panel")
    };
    icons::tool_button(ui, icon, "", true)
        .on_hover_text(tip)
        .clicked()
}
