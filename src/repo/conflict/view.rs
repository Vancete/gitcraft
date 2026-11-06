//! The conflict editor's pane: header with the whole-file actions, each conflict with its choice
//! buttons, and the editable result below.

use egui::{RichText, Sense, Stroke, Ui, vec2};

use super::{Choice, ConflictEditor, Segment, render};
use crate::icons;
use crate::repo::{Action, Refresh, RepoTab, ops, widgets};
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(in crate::repo) fn conflict_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        let Some(ed) = &mut self.conflict else { return };
        let (ours_name, theirs_name) = ed.side_names();
        let left = ed.unresolved();

        ed.header(ui, &t, idle, left, acts);
        ed.result_panel(ui, &t);
        ed.conflict_list(ui, &t, (&ours_name, &theirs_name));
    }
}

impl ConflictEditor {
    /// Header: the file, how many conflicts are resolved, save, the whole-file actions, close.
    fn header(&self, ui: &mut Ui, t: &Tokens, idle: bool, left: usize, acts: &mut Vec<Action>) {
        let total = self.choices.len();
        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "git-merge", 16.0, t.danger);
                    ui.label(RichText::new(&self.path).font(theme::medium(13.0)));
                    widgets::chip(
                        ui,
                        &format!(
                            "{} of {total} resolved",
                            total.saturating_sub(left).min(total)
                        ),
                        if left == 0 { t.success } else { t.warning },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::CloseDiff);
                        }
                        if widgets::primary_button(
                            ui,
                            "Save and mark resolved",
                            idle && left == 0,
                            0.0,
                        )
                        .on_disabled_hover_text("Resolve every conflict first")
                        .clicked()
                        {
                            acts.push(Action::SaveConflict);
                        }
                        self.whole_file_buttons(ui, idle, acts);
                    });
                });
            });
    }

    /// Mergiraf when it's set up, and the whole file from either side.
    fn whole_file_buttons(&self, ui: &mut Ui, idle: bool, acts: &mut Vec<Action>) {
        // Whole file: git's own sides (swapped during a rebase).
        let (mine, theirs) = if self.rebase {
            ("--theirs", "--ours")
        } else {
            ("--ours", "--theirs")
        };
        let offer_mergiraf = crate::settings::Settings::get(ui.ctx()).mergiraf
            && crate::mergiraf::found(ui.ctx()).is_some();
        if offer_mergiraf {
            if self.solving {
                ui.spinner();
            } else if ui
                .add_enabled(idle, egui::Button::new("Solve with Mergiraf"))
                .on_hover_text(
                    "Let Mergiraf solve the conflicts it can from the syntax \
                     of the code. Nothing is saved before you check the result.",
                )
                .clicked()
            {
                acts.push(Action::SolveWithMergiraf);
            }
        }
        for (label, side, tip) in [
            ("Use theirs", theirs, "Take their version of the whole file"),
            ("Use mine", mine, "Keep your version of the whole file"),
        ] {
            if ui
                .add_enabled(idle, egui::Button::new(label))
                .on_hover_text(tip)
                .clicked()
            {
                acts.push(Action::Ops {
                    label: format!("{label} for {}", self.path),
                    ops: ops(&[
                        &["--literal-pathspecs", "checkout", side, "--", &self.path],
                        &["--literal-pathspecs", "add", "--", &self.path],
                    ]),
                    refresh: Refresh::Status,
                });
            }
        }
    }

    /// Bottom: the result, editable by hand.
    fn result_panel(&mut self, ui: &mut Ui, t: &Tokens) {
        egui::Panel::bottom("conflict_result")
            .resizable(true)
            .default_size(ui.available_height() * 0.42)
            .size_range(90.0..=f32::INFINITY)
            .frame(
                egui::Frame::new()
                    .fill(t.card)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("RESULT")
                            .font(theme::semibold(10.5))
                            .color(t.text_faint),
                    );
                    if self.manual {
                        ui.label(RichText::new("edited by hand").color(t.warning).small())
                            .on_hover_text(
                                "Choices no longer change the result: \"Back to choices\" \
                                 applies them, and drops the edits",
                            );
                        if ui.small_button("Back to choices").clicked() {
                            self.manual = false;
                            self.result = render(
                                &self.segments,
                                &self.choices,
                                (&self.ours_label, &self.theirs_label),
                            );
                        }
                    }
                });
                egui::ScrollArea::both()
                    .id_salt("conflict_result_scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let edit = ui.add(
                            egui::TextEdit::multiline(&mut self.result)
                                .font(theme::mono(12.0))
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(8),
                        );
                        if edit.changed() {
                            self.manual = true;
                        }
                    });
            });
    }

    /// Top: each conflict, ours | theirs, with the choice buttons. `names`: the two sides'.
    fn conflict_list(&mut self, ui: &mut Ui, t: &Tokens, names: (&str, &str)) {
        let total = self.choices.len();
        egui::ScrollArea::vertical()
            .id_salt("conflict_list")
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut k = 0;
                let mut pick = None;
                for seg in &self.segments {
                    let Segment::Conflict { ours, theirs, .. } = seg else {
                        continue;
                    };
                    let chosen = self.choices.get(k).copied().flatten();
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("Conflict {} of {total}", k + 1))
                                        .font(theme::semibold(12.0)),
                                );
                                ui.add_space(8.0);
                                if let Some(choice) = choice_buttons(ui, t, chosen, names) {
                                    pick = Some((k, choice));
                                }
                            });
                            ui.add_space(4.0);
                            ui.columns(2, |cols| {
                                for (col, (name, text, side)) in cols.iter_mut().zip([
                                    (names.0, ours, Choice::Ours),
                                    (names.1, theirs, Choice::Theirs),
                                ]) {
                                    let picked = matches!(
                                        (chosen, side),
                                        (Some(Choice::Ours), Choice::Ours)
                                            | (Some(Choice::Theirs), Choice::Theirs)
                                    ) || matches!(
                                        chosen,
                                        Some(Choice::OursThenTheirs | Choice::TheirsThenOurs)
                                    );
                                    side_box(col, t, (k, side), name, text, picked);
                                }
                            });
                        });
                    let (sep, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, t.separator);
                    k += 1;
                }
                if let Some((k, choice)) = pick {
                    self.choose(k, choice);
                }
            });
    }
}

/// The four ways to resolve one conflict, the `chosen` one lit; returns the one clicked.
fn choice_buttons(
    ui: &mut Ui,
    t: &Tokens,
    chosen: Option<Choice>,
    (ours_name, theirs_name): (&str, &str),
) -> Option<Choice> {
    let mut clicked = None;
    for (choice, label) in [
        (Choice::Ours, ours_name),
        (Choice::Theirs, theirs_name),
        (Choice::OursThenTheirs, "Both (ours first)"),
        (Choice::TheirsThenOurs, "Both (theirs first)"),
    ] {
        let on = chosen == Some(choice);
        let text = if on {
            RichText::new(label).color(t.accent_text)
        } else {
            RichText::new(label)
        };
        let b = egui::Button::new(text).fill(if on { t.accent } else { t.field });
        if ui.add(b).clicked() {
            clicked = Some(choice);
        }
    }
    clicked
}

/// One side of conflict `k`, its border lit when that side is part of the result.
fn side_box(
    ui: &mut Ui,
    t: &Tokens,
    (k, side): (usize, Choice),
    name: &str,
    text: &str,
    picked: bool,
) {
    let border = if picked { t.accent } else { t.field_border };
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(t.radius)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(name).color(t.text_faint).small());
            let body = if text.is_empty() {
                "(nothing)"
            } else {
                text.trim_end_matches(['\r', '\n'])
            };
            // No wrapping: wrapped code loses its indentation.
            egui::ScrollArea::horizontal()
                .id_salt(("conflict-side", k, side == Choice::Ours))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(body).font(theme::mono(12.0))).extend());
                });
        });
}
