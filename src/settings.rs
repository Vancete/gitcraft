//! User settings: stored with the app state, readable from anywhere through the egui context.

use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};

use crate::code_theme::SyntaxStyle;
use crate::repo::widgets::{SliderUnit, slider};
use crate::theme::{self, Tokens};
use crate::vscode;
use crate::workspace::{self, Workspace};

/// The code each theme is shown with.
const SAMPLE: &str = "/// Opens the server on `port`.
pub fn listen(port: u16) -> Result<Server> {
    let addr = format!(\"0.0.0.0:{port}\");
    let socket = TcpListener::bind(&addr)?;
    log!(\"listening on {}\", addr); // ok
    Ok(Server { socket, open: true })
}";

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
    /// Diffs side by side rather than unified.
    pub diff_split: bool,
    /// Diffs show the whole file, not just the lines around the changes.
    pub diff_whole_file: bool,
    /// A commit's files in a tree of their folders rather than a list of paths.
    pub files_as_tree: bool,
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
    /// The theme code is coloured with in diffs and blame.
    pub theme: CodeTheme,
    /// VS Code colour themes imported into the settings.
    pub imported_themes: Vec<vscode::ThemeInfo>,
    pub profiles: Vec<Profile>,
    /// Profiles with their own tabs, folder, identity and accounts (see [`workspace`]).
    pub workspaces: Vec<Workspace>,
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
            diff_split: false,
            diff_whole_file: false,
            files_as_tree: false,
            auto_fetch_minutes: 10,
            avatars: true,
            editor: "code".to_owned(),
            notifications: true,
            check_updates: true,
            theme: CodeTheme::default(),
            imported_themes: Vec::new(),
            profiles: Vec::new(),
            workspaces: Vec::new(),
        }
    }
}

/// The code's theme: one of gitcraft's own, or a VS Code colour theme imported, which also
/// colours the rest of the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CodeTheme {
    Builtin(SyntaxStyle),
    VsCode(String),
}

impl Default for CodeTheme {
    fn default() -> Self {
        CodeTheme::Builtin(SyntaxStyle::default())
    }
}

/// Makes the settings page scroll to its PROFILES section next time it's drawn.
pub fn show_profiles(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("gitcraft-show-profiles"), true));
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
    }

    /// The imported VS Code theme chosen, if one is.
    pub fn vscode_theme(&self) -> Option<&vscode::ThemeInfo> {
        match &self.theme {
            CodeTheme::VsCode(name) => self.imported_themes.iter().find(|t| t.name == *name),
            CodeTheme::Builtin(_) => None,
        }
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
            self.zoom(ui);
            ui.end_row();
            ui.label("Branch column width");
            slider(ui, &mut self.refs_width, 180.0..=480.0, 10.0, SliderUnit::Suffix(" px"));
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

        section(ui, "THEMES");
        self.themes(ui);

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
                slider(
                    ui,
                    &mut self.terminal_font,
                    9.0..=20.0,
                    0.5,
                    SliderUnit::Suffix(" pt"),
                );
                ui.end_row();
            });

        self.workspaces_ui(ui, &section);

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

impl Settings {
    /// PROFILES: each with its tabs and recent list (kept by the app), folder, identity,
    /// GitHub account and environment.
    fn workspaces_ui(&mut self, ui: &mut Ui, section: &dyn Fn(&mut Ui, &str)) {
        let t = Tokens::get(ui.ctx());
        let scroll_id = egui::Id::new("gitcraft-show-profiles");
        if ui
            .data_mut(|d| d.remove_temp::<bool>(scroll_id))
            .unwrap_or(false)
        {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        section(ui, "PROFILES");
        ui.label(
            RichText::new(
                "Each profile has its own open tabs and recent repositories; switch between them from the button at the left of the tabs. Empty fields leave your git setup as it is.",
            )
            .color(t.text_dim)
            .small(),
        );
        ui.add_space(4.0);
        let mut remove = None;
        for (i, ws) in self.workspaces.iter_mut().enumerate() {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, t.field_border))
                .corner_radius(t.radius)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    egui::Grid::new(("workspace", ws.id))
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut ws.name)
                                        .hint_text("Work")
                                        .desired_width(200.0),
                                );
                                if crate::icons::small_button(ui, "x", "Remove this profile")
                                    .clicked()
                                {
                                    remove = Some(i);
                                }
                            });
                            ui.end_row();

                            ui.label("Projects folder");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut ws.folder)
                                        .hint_text("~/projects/work (optional)")
                                        .desired_width(300.0),
                                );
                                if ui.button("Browse…").clicked()
                                    && let Some(dir) = rfd::FileDialog::new()
                                        .set_title("Projects folder")
                                        .pick_folder()
                                {
                                    ws.folder = dir.display().to_string();
                                }
                            });
                            ui.end_row();

                            ui.label("Commit as");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut ws.git_name)
                                        .hint_text("Name")
                                        .desired_width(140.0),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut ws.git_email)
                                        .hint_text("email@example.com")
                                        .desired_width(200.0),
                                );
                            });
                            ui.end_row();

                            ui.label("GitHub account");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut ws.github_user)
                                        .hint_text("login (gh auth status)")
                                        .desired_width(160.0),
                                );
                                let id = ui.id().with(("gh-check", ws.id));
                                let user = ws.github_user.trim().to_owned();
                                if ui
                                    .add_enabled(!user.is_empty(), egui::Button::new("Check"))
                                    .on_hover_text("Asks gh for this account's token")
                                    .clicked()
                                {
                                    let result = workspace::github_token(&user)
                                        .map(|_| format!("gh is signed in as {user}"));
                                    ui.data_mut(|d| d.insert_temp(id, (user.clone(), result)));
                                }
                                if let Some((checked, result)) =
                                    ui.data(|d| d.get_temp::<(String, Result<String, String>)>(id))
                                    && checked == user
                                {
                                    match result {
                                        Ok(msg) => ui.colored_label(t.success, msg),
                                        Err(e) => ui.colored_label(t.danger, e),
                                    };
                                }
                            });
                            ui.end_row();

                            ui.label("Environment").on_hover_text(
                                "KEY=VALUE per line, for git, gh and terminals of this profile",
                            );
                            ui.add(
                                egui::TextEdit::multiline(&mut ws.env)
                                    .hint_text(
                                        "GIT_SSH_COMMAND=ssh -i ~/.ssh/id_work\nAZURE_CONFIG_DIR=~/.azure-work",
                                    )
                                    .font(egui::TextStyle::Monospace)
                                    .desired_rows(2)
                                    .desired_width(360.0),
                            );
                            ui.end_row();
                        });
                });
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            self.workspaces.remove(i);
        }
        if ui.button("Add profile").clicked() {
            let id = workspace::next_id(&self.workspaces);
            self.workspaces.push(Workspace {
                id,
                name: format!("Profile {id}"),
                ..Default::default()
            });
        }
    }

    /// The zoom slider and its Apply button: the zoom changes once applied, not while the
    /// slider moves (the slider would move under the pointer as the window rescales).
    fn zoom(&mut self, ui: &mut Ui) {
        let id = ui.id().with("zoom-draft");
        let mut draft = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(self.ui_scale);
        ui.horizontal(|ui| {
            slider(ui, &mut draft, 0.7..=2.0, 0.05, SliderUnit::Percent);
            let pending = (draft - self.ui_scale).abs() > 0.001;
            if ui
                .add_enabled(pending, egui::Button::new("Apply"))
                .clicked()
            {
                self.ui_scale = draft;
            }
            if pending && ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                draft = self.ui_scale;
            }
        });
        ui.data_mut(|d| d.insert_temp(id, draft));
    }

    /// gitcraft's code styles as cards, the VS Code themes imported below them.
    fn themes(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        egui::Grid::new("settings-themes")
            .num_columns(2)
            .spacing([12.0, 12.0])
            .show(ui, |ui| {
                for (i, style) in SyntaxStyle::ALL.into_iter().enumerate() {
                    let chosen = self.theme == CodeTheme::Builtin(style);
                    let card = Card {
                        label: style.label(),
                        theme: style.theme(t.dark()),
                        background: t.dock,
                        text: t.text,
                    };
                    if card.ui(ui, chosen, false).0.clicked() {
                        self.theme = CodeTheme::Builtin(style);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(12.0);
        self.import_button(ui);

        if self.imported_themes.is_empty() {
            return;
        }
        ui.add_space(14.0);
        ui.label(
            RichText::new("VS CODE THEMES")
                .font(theme::semibold(10.5))
                .color(t.text_faint),
        );
        ui.label(
            RichText::new("A VS Code theme colours the whole window, not just the code.")
                .color(t.text_faint)
                .small(),
        );
        ui.add_space(4.0);
        let mut remove = None;
        egui::Grid::new("settings-vscode-themes")
            .num_columns(2)
            .spacing([12.0, 12.0])
            .show(ui, |ui| {
                for (i, info) in self.imported_themes.iter().enumerate() {
                    let name = &info.name;
                    let chosen = self.theme == CodeTheme::VsCode(name.clone());
                    let (resp, removed) = match vscode::color_theme(info) {
                        Some(vs) => Card {
                            label: name,
                            theme: &vs.syntax,
                            background: vs.color("editor.background").unwrap_or(t.dock),
                            text: vs.color("editor.foreground").unwrap_or(t.text),
                        }
                        .ui(ui, chosen, true),
                        None => missing_card(ui, name),
                    };
                    if resp.clicked() {
                        self.theme = CodeTheme::VsCode(name.clone());
                    }
                    if removed {
                        remove = Some(i);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        if let Some(i) = remove {
            let info = self.imported_themes.remove(i);
            if self.theme == CodeTheme::VsCode(info.name) {
                self.theme = CodeTheme::default();
            }
        }
    }

    /// "Import VS Code themes": a list of the colour themes installed in VS Code; picking one
    /// adds it to the VS Code themes.
    fn import_button(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let resp = ui.button("Import VS Code themes…");
        egui::Popup::menu(&resp)
            .style(theme::menu_style)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                let installed = vscode::color_themes();
                if installed.is_empty() {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(16, 4))
                        .show(ui, |ui| {
                            ui.label(RichText::new("No VS Code themes found").color(t.text_faint));
                        });
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        ui.set_width(280.0);
                        for info in installed {
                            let imported = self.imported_themes.iter().any(|t| t.name == info.name);
                            let kind = if info.dark { "dark" } else { "light" };
                            let label = ui.add_enabled(
                                !imported,
                                egui::Button::new(&info.name)
                                    .right_text(RichText::new(kind).color(t.text_faint).small())
                                    .frame(false)
                                    .min_size(egui::vec2(280.0, 0.0)),
                            );
                            if label.clicked() {
                                self.imported_themes.push(info);
                            }
                        }
                    });
            });
    }
}

/// A theme's preview in the settings: its name over the sample code in its colours.
struct Card<'a> {
    label: &'a str,
    theme: &'a syntect::highlighting::Theme,
    background: egui::Color32,
    text: egui::Color32,
}

const CARD_W: f32 = 312.0;

impl Card<'_> {
    /// Draws it, the chosen one outlined; with `removable`, a button to remove it shows on
    /// hover. Returns its response and whether it was removed.
    fn ui(&self, ui: &mut Ui, chosen: bool, removable: bool) -> (egui::Response, bool) {
        let t = Tokens::get(ui.ctx());
        let font = egui::FontId::monospace(11.0);
        let mut job = egui::text::LayoutJob::default();
        let lines = crate::repo::highlight::highlight_with(
            "sample.rs",
            self.theme,
            SAMPLE.lines(),
            self.text,
        )
        .unwrap_or_default();
        for (n, spans) in lines.iter().enumerate() {
            if n > 0 {
                job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), self.text));
            }
            for (color, text) in spans {
                job.append(text, 0.0, egui::TextFormat::simple(font.clone(), *color));
            }
        }
        let galley = ui.painter().layout_job(job);
        let size = egui::vec2(CARD_W, galley.size().y + 42.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        card_frame(ui, rect, &resp, chosen, self.background);
        let p = ui.painter();
        let label = if chosen {
            self.text
        } else {
            self.text.gamma_multiply(0.75)
        };
        p.with_clip_rect(rect.shrink(2.0)).text(
            rect.left_top() + egui::vec2(12.0, 10.0),
            egui::Align2::LEFT_TOP,
            self.label,
            theme::semibold(12.0),
            label,
        );
        p.with_clip_rect(rect.shrink(2.0)).galley(
            rect.left_top() + egui::vec2(12.0, 32.0),
            galley,
            self.text,
        );
        let mut removed = false;
        if removable && ui.rect_contains_pointer(rect) {
            let close = egui::Rect::from_center_size(
                rect.right_top() + egui::vec2(-16.0, 16.0),
                egui::vec2(20.0, 20.0),
            );
            let x = ui.interact(close, resp.id.with("remove"), egui::Sense::click());
            let color = if x.hovered() { self.text } else { t.text_faint };
            crate::icons::paint(ui, close, "x", 14.0, color);
            removed = x.on_hover_text("Remove").clicked();
        }
        (resp, removed)
    }
}

/// An imported theme VS Code no longer has: a card saying so, to remove it.
fn missing_card(ui: &mut Ui, name: &str) -> (egui::Response, bool) {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(CARD_W, 60.0), egui::Sense::hover());
    card_frame(ui, rect, &resp, false, t.dock);
    ui.painter().text(
        rect.left_top() + egui::vec2(12.0, 10.0),
        egui::Align2::LEFT_TOP,
        format!("{name}\nnot found in VS Code"),
        egui::FontId::proportional(12.0),
        t.text_faint,
    );
    let close = egui::Rect::from_center_size(
        rect.right_top() + egui::vec2(-16.0, 16.0),
        egui::vec2(20.0, 20.0),
    );
    let x = ui.interact(close, resp.id.with("remove"), egui::Sense::click());
    crate::icons::paint(ui, close, "x", 14.0, t.text_faint);
    let removed = x.on_hover_text("Remove").clicked();
    (resp, removed)
}

fn card_frame(ui: &Ui, rect: egui::Rect, resp: &egui::Response, chosen: bool, fill: egui::Color32) {
    let t = Tokens::get(ui.ctx());
    let stroke = if chosen {
        egui::Stroke::new(2.0, t.accent)
    } else if resp.hovered() {
        egui::Stroke::new(1.0, t.text_faint)
    } else {
        egui::Stroke::new(1.0, t.field_border)
    };
    ui.painter()
        .rect(rect, t.radius, fill, stroke, egui::StrokeKind::Inside);
}
