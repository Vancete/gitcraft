//! User settings: stored with the app state, readable from anywhere through the egui context.

use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};

use crate::theme::{self, Tokens};

/// A name and email to commit with, applied per repository.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub label: String,
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Interface zoom (1.0 = 100 %).
    pub ui_scale: f32,
    pub terminal_font: f32,
    /// Program of the default shell; empty = the platform's default.
    pub shell: String,
    /// Width of the branch/tag column of the graph.
    pub refs_width: f32,
    pub show_author: bool,
    /// An absolute date column (tips show a relative date either way).
    pub show_date: bool,
    pub show_hash: bool,
    /// Background fetch interval in minutes; 0 turns it off.
    pub auto_fetch_minutes: u32,
    /// Load avatars: GitHub / GitLab accounts on repositories hosted there, Gravatar otherwise.
    pub avatars: bool,
    /// Command that opens a folder or file in the user's editor.
    pub editor: String,
    /// Desktop notifications when a long operation ends while the window is in the background.
    pub notifications: bool,
    /// Ask GitHub every 12 hours whether a newer release exists.
    pub check_updates: bool,
    /// Colour code in diffs and blame with VS Code's current theme, when it has one.
    pub vscode_colors: bool,
    pub profiles: Vec<Profile>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            terminal_font: 12.5,
            shell: String::new(),
            refs_width: 270.0,
            show_author: true,
            show_date: false,
            show_hash: false,
            auto_fetch_minutes: 10,
            avatars: true,
            editor: "code".to_owned(),
            notifications: true,
            check_updates: true,
            vscode_colors: true,
            profiles: Vec::new(),
        }
    }
}

fn id() -> egui::Id {
    egui::Id::new("gitcraft-settings")
}

impl Settings {
    pub fn get(ctx: &egui::Context) -> Settings {
        ctx.data(|d| d.get_temp::<Settings>(id()))
            .unwrap_or_default()
    }

    pub fn set(&self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_temp(id(), self.clone()));
        ctx.set_zoom_factor(self.ui_scale.clamp(0.7, 2.0));
        crate::vscode_theme::set_enabled(self.vscode_colors);
    }

    /// The settings form. Returns true when something changed.
    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let before = self.clone();
        let section = |ui: &mut Ui, title: &str| {
            ui.add_space(10.0);
            ui.label(
                RichText::new(title)
                    .font(theme::semibold(10.5))
                    .color(t.text_faint),
            );
            ui.add_space(2.0);
        };

        section(ui, "INTERFACE");
        egui::Grid::new("settings-ui").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.ui_scale, 0.7..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
            ui.end_row();
            ui.label("Branch column width");
            ui.add(egui::Slider::new(&mut self.refs_width, 180.0..=480.0).step_by(10.0).suffix(" px"));
            ui.end_row();
            ui.label("Graph columns");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.show_author, "Author");
                ui.checkbox(&mut self.show_date, "Date");
                ui.checkbox(&mut self.show_hash, "Hash");
            });
            ui.end_row();
            ui.label("Avatars");
            ui.checkbox(&mut self.avatars, "Load from GitHub, GitLab or Gravatar")
                .on_hover_text("On a GitHub repository, its commit list says which account each author is; on GitLab, the host is asked by email. Other authors: an MD5 hash of the email goes to gravatar.com. Cached on disk.");
            ui.end_row();
            ui.label("Code colours");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.vscode_colors, "Use VS Code's colour theme");
                if self.vscode_colors {
                    let note = match crate::vscode_theme::current() {
                        Some(found) if found.dark == t.dark() => found.name.clone(),
                        Some(found) => format!(
                            "{} is {}: gitcraft's colours meanwhile",
                            found.name,
                            if found.dark { "dark" } else { "light" }
                        ),
                        None => "No VS Code theme found".to_owned(),
                    };
                    ui.label(RichText::new(note).color(t.text_faint).small());
                }
            });
            ui.end_row();
            ui.label("Notifications");
            ui.checkbox(&mut self.notifications, "When a long operation ends in the background");
            ui.end_row();
            if let Some(appimage) = crate::desktop_entry::appimage() {
                ui.label("Applications menu");
                ui.horizontal(|ui| {
                    if crate::desktop_entry::installed() {
                        if ui.button("Remove gitcraft").clicked() {
                            let _ = crate::desktop_entry::uninstall();
                        }
                    } else if ui.button("Add gitcraft").clicked() {
                        let _ = crate::desktop_entry::install(&appimage);
                    }
                    ui.label(
                        RichText::new("Starts this AppImage; follows it when you replace it")
                            .color(t.text_faint)
                            .small(),
                    );
                });
                ui.end_row();
            }
            ui.label("Updates");
            ui.checkbox(&mut self.check_updates, "Check GitHub for a new version every 12 hours");
            ui.end_row();
        });

        section(ui, "GIT");
        egui::Grid::new("settings-git")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Fetch in the background");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.auto_fetch_minutes)
                            .range(0..=240)
                            .suffix(" min"),
                    );
                    ui.label(RichText::new("0 = off").color(t.text_faint).small());
                });
                ui.end_row();
                ui.label("Editor command");
                ui.add(
                    egui::TextEdit::singleline(&mut self.editor)
                        .hint_text("code")
                        .desired_width(200.0),
                )
                .on_hover_text("Run with a folder or file path, e.g. code, subl, idea");
                ui.end_row();
            });

        section(ui, "TERMINAL");
        egui::Grid::new("settings-term")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Default shell");
                let shells = crate::terminal::available_shells();
                let current = shells
                    .iter()
                    .find(|s| s.program == self.shell)
                    .map(|s| s.label.clone())
                    .unwrap_or_else(|| "System default".to_owned());
                egui::ComboBox::from_id_salt("settings-shell")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.shell, String::new(), "System default");
                        for s in &shells {
                            ui.selectable_value(&mut self.shell, s.program.clone(), &s.label);
                        }
                    });
                ui.end_row();
                ui.label("Font size");
                ui.add(egui::Slider::new(&mut self.terminal_font, 9.0..=20.0).step_by(0.5));
                ui.end_row();
            });

        section(ui, "IDENTITIES");
        ui.label(
            RichText::new(
                "Name and email pairs you can apply to a repository from its commit box.",
            )
            .color(t.text_dim)
            .small(),
        );
        let mut remove = None;
        for (i, p) in self.profiles.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut p.label)
                        .hint_text("Work")
                        .desired_width(80.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut p.name)
                        .hint_text("Name")
                        .desired_width(140.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut p.email)
                        .hint_text("email@example.com")
                        .desired_width(190.0),
                );
                if crate::icons::small_button(ui, "x", "Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.profiles.remove(i);
        }
        if ui.button("Add identity").clicked() {
            self.profiles.push(Profile::default());
        }
        *self != before
    }
}
