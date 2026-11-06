//! The search bar above the graph: messages or code, in all history.

use egui::{Key, Stroke, Ui};

use crate::icons;
use crate::repo::{Action, RepoTab};
use crate::theme::Tokens;

impl RepoTab {
    pub(super) fn search_bar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "search", 15.0, t.text_faint);
                    let code = self.search.code;
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.search.query)
                            .hint_text(if code {
                                "Search code added or removed by a commit, in all history"
                            } else {
                                "Search messages, authors and hashes in all history"
                            })
                            .desired_width(ui.available_width() - 290.0)
                            .margin(egui::Margin::symmetric(8, 4)),
                    );
                    // Where to look: commit messages or the code they changed.
                    for (label, on, tip) in [
                        (
                            "Messages",
                            false,
                            "Search commit messages, authors and hashes",
                        ),
                        (
                            "Code",
                            true,
                            "Search the commits that added or removed this text in a file \
                             (git log -S)",
                        ),
                    ] {
                        if ui
                            .selectable_label(code == on, label)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            acts.push(Action::SearchCode(on));
                        }
                    }
                    if std::mem::take(&mut self.search.focus) {
                        edit.request_focus();
                    }
                    if edit.changed() {
                        self.search.edited = Some(std::time::Instant::now());
                    }
                    let (enter, shift, esc) = ui.input(|i| {
                        (
                            i.key_pressed(Key::Enter),
                            i.modifiers.shift,
                            i.key_pressed(Key::Escape),
                        )
                    });
                    if edit.lost_focus() && enter {
                        acts.push(Action::SearchStep(if shift { -1 } else { 1 }));
                        edit.request_focus();
                    }
                    if edit.has_focus() && esc {
                        acts.push(Action::ToggleSearch);
                    }
                    ui.label(
                        egui::RichText::new(self.search_summary())
                            .color(t.text_dim)
                            .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::ToggleSearch);
                        }
                        if icons::small_button(ui, "chevron-down", "Next (Enter)").clicked() {
                            acts.push(Action::SearchStep(1));
                        }
                        if icons::small_button(ui, "chevron-up", "Previous (Shift+Enter)").clicked()
                        {
                            acts.push(Action::SearchStep(-1));
                        }
                    });
                });
            });
    }

    /// Where the search stands: running, no results, or which result is shown.
    fn search_summary(&self) -> String {
        let n = self.search.results.len();
        if self.search.running {
            "searching…".to_owned()
        } else if self.search.searched.is_empty() {
            String::new()
        } else if n == 0 {
            "no results".to_owned()
        } else {
            format!("{} of {n}", self.search.current + 1)
        }
    }
}
