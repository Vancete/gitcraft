//! Central pane for one file: its history (every commit that changed it, following renames),
//! its blame (who last changed each line) or the whole file as it is at a commit. Opened from a
//! file's right-click menu and from the list of all the files of a commit.

mod blame;
mod history;

use egui::{Stroke, Ui};

use super::file_text::{self, FileText};
use super::widgets;
use super::{Action, RepoTab};
use crate::git::FileCommit;
use crate::icons;
use crate::theme::{self, Tokens};

pub(crate) use blame::{BlameData, load_blame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileTab {
    History,
    Blame,
    /// The whole file.
    File,
}

pub(crate) struct FileView {
    pub path: String,
    /// The commit history, blame and the file are at; HEAD and the working tree when `None`.
    pub rev: Option<String>,
    pub tab: FileTab,
    pub history: Option<Result<Vec<FileCommit>, String>>,
    pub blame: Option<Result<BlameData, String>>,
    pub text: Option<Result<FileText, String>>,
}

impl FileView {
    pub fn new(path: &str, rev: Option<String>, tab: FileTab) -> Self {
        Self {
            path: path.to_owned(),
            rev,
            tab,
            history: None,
            blame: None,
            text: None,
        }
    }

    /// Whether `tab` still has to be loaded.
    pub fn needs(&self, tab: FileTab) -> bool {
        match tab {
            FileTab::History => self.history.is_none(),
            FileTab::Blame => self.blame.is_none(),
            FileTab::File => self.text.is_none(),
        }
    }
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
                    let tabs = [
                        (FileTab::History, "History"),
                        (FileTab::Blame, "Blame"),
                        (FileTab::File, "File"),
                    ];
                    for (tab, label) in tabs {
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
            FileTab::History => self.file_history(ui, acts),
            FileTab::Blame => self.blame_lines(ui, acts),
            FileTab::File => {
                file_text::show(ui, ("file_view", &view.path, &view.rev), view.text.as_ref())
            }
        }
    }
}

fn error(ui: &mut Ui, e: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.colored_label(t.danger, e);
    });
}
