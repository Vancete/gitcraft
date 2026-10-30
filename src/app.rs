//! Window shell: git check, browser-like tabs, the start page and cloning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use egui::{Align2, Key, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::git;
use crate::icons;
use crate::repo::RepoTab;
use crate::settings::Settings;
use crate::tab_look::{self, TabLook};
use crate::theme::{self, ThemeKind, Tokens};
use crate::update;

const MAX_RECENT: usize = 12;
const TAB_H: f32 = 40.0;

/// Set by `main` when the window was closed maximized: it opens restored and is maximized once
/// it shows its first frame. Created maximized, winit shows the hidden window for an instant
/// (`ShowWindow(SW_MAXIMIZE)`) before anything is painted, a white flash on Windows.
pub static MAXIMIZE_WHEN_SHOWN: AtomicBool = AtomicBool::new(false);

/// What survives a restart.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    open: Vec<PathBuf>,
    active: usize,
    recent: Vec<PathBuf>,
    theme: ThemeKind,
    clone_parent: Option<PathBuf>,
    settings: Settings,
    /// Unix time of the last update check.
    update_checked: i64,
    /// A release the user chose to skip.
    skipped_version: String,
    /// Repository tabs given their own icon or colour.
    tab_looks: BTreeMap<PathBuf, TabLook>,
}

enum Tab {
    /// The start page ("new tab"): open, create or clone a repository.
    Home,
    Repo(Box<RepoTab>),
}

struct CloneForm {
    url: String,
    parent: String,
    name: String,
    /// The user typed a name; stop deriving it from the URL.
    name_edited: bool,
    error: Option<String>,
    focus: bool,
    /// The remote asked for a username and password / token.
    auth: bool,
    username: String,
    password: String,
}

pub struct App {
    git_version: Result<String, String>,
    tabs: Vec<Tab>,
    active: usize,
    recent: Vec<PathBuf>,
    theme: ThemeKind,
    clone_parent: Option<PathBuf>,
    error: Option<String>,
    was_focused: bool,
    clone_form: Option<CloneForm>,
    /// A clone running in the background: (destination, result).
    cloning: Option<(PathBuf, Receiver<Result<(), String>>)>,
    /// The form of the running clone, back on screen if it needs credentials.
    cloning_form: Option<CloneForm>,
    /// A newer release than this build.
    update: Option<update::Release>,
    update_rx: Option<Receiver<Result<update::Release, String>>>,
    update_checked: i64,
    skipped_version: String,
    tab_looks: BTreeMap<PathBuf, TabLook>,
    settings: Settings,
    settings_open: bool,
    manual_open: bool,
    palette: Option<Palette>,
    toasts: Vec<Toast>,
}

/// The command palette (Ctrl+P): type to filter, Enter runs.
struct Palette {
    query: String,
    selected: usize,
    focus: bool,
}

struct Toast {
    text: String,
    error: bool,
    until: std::time::Instant,
}

/// What a palette entry does outside the open repository.
enum AppCommand {
    NewTab,
    Open,
    Create,
    Clone,
    Settings,
    Manual,
    Theme(ThemeKind),
}

enum Entry {
    App(AppCommand),
    /// Index into the active repository's palette items.
    Repo(usize),
}

impl App {
    /// `open_arg`: a repository passed on the command line, opened (or focused) on start.
    pub fn new(cc: &eframe::CreationContext<'_>, open_arg: Option<PathBuf>) -> Self {
        let saved: Saved = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        let ctx = &cc.egui_ctx;
        egui_extras::install_image_loaders(ctx);
        theme::install_fonts(ctx);
        theme::apply(ctx, saved.theme);
        saved.settings.set(ctx);
        crate::desktop_entry::refresh();

        let mut app = Self {
            git_version: git::version(),
            tabs: Vec::new(),
            active: 0,
            recent: saved.recent,
            theme: saved.theme,
            clone_parent: saved.clone_parent,
            settings: saved.settings.clone(),
            settings_open: false,
            manual_open: false,
            palette: None,
            toasts: Vec::new(),
            error: None,
            was_focused: true,
            clone_form: None,
            cloning: None,
            cloning_form: None,
            update: None,
            update_rx: None,
            update_checked: saved.update_checked,
            skipped_version: saved.skipped_version,
            tab_looks: saved.tab_looks,
        };
        if saved.settings.check_updates && git::now() - saved.update_checked > update::INTERVAL {
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(update::latest());
                ctx2.request_repaint();
            });
            app.update_rx = Some(rx);
        }
        if app.git_version.is_ok() {
            for path in saved.open {
                if path.is_dir() {
                    app.tabs.push(Tab::Repo(Box::new(RepoTab::new(path, ctx))));
                }
            }
        }
        if app.tabs.is_empty() {
            app.tabs.push(Tab::Home);
        }
        app.active = saved.active.min(app.tabs.len() - 1);
        if let Some(path) = open_arg
            && app.git_version.is_ok()
        {
            app.open_repo(&path, ctx);
        }
        app
    }

    /// Opens `path` as the start page's "Open" button does (for `examples/ui_shot.rs`).
    pub fn open_tab(&mut self, path: &Path, ctx: &egui::Context) {
        self.open_repo(path, ctx);
    }

    /// Opens `path` in the active tab if it's a start page, otherwise in a new tab.
    fn open_repo(&mut self, path: &Path, ctx: &egui::Context) {
        let root = match git::toplevel(path) {
            Ok(root) => root,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let root = strip_verbatim(root);
        self.remember(&root);
        self.error = None;

        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::Repo(r) if r.path == root))
        {
            self.active = i;
            return;
        }
        let tab = Tab::Repo(Box::new(RepoTab::new(root, ctx)));
        if matches!(self.tabs.get(self.active), Some(Tab::Home)) {
            self.tabs[self.active] = tab;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
    }

    fn remember(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_owned());
        self.recent.truncate(MAX_RECENT);
    }

    fn pick_existing(&mut self, ctx: &egui::Context) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Open a git repository")
            .pick_folder()
        {
            self.open_repo(&dir, ctx);
        }
    }

    fn pick_new(&mut self, ctx: &egui::Context) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose a folder for the new repository")
            .pick_folder()
        else {
            return;
        };
        // Already the root of a repository: just open it rather than re-initialising.
        if let Ok(root) = git::toplevel(&dir)
            && same_path(&root, &dir)
        {
            self.open_repo(&dir, ctx);
            return;
        }
        match git::init(&dir) {
            Ok(()) => self.open_repo(&dir, ctx),
            Err(e) => self.error = Some(format!("git init failed: {e}")),
        }
    }

    fn start_clone(&mut self) {
        let parent = self
            .clone_parent
            .clone()
            .or_else(|| {
                self.recent
                    .first()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf)
            })
            .or_else(home_dir)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.clone_form = Some(CloneForm {
            url: String::new(),
            parent,
            name: String::new(),
            name_edited: false,
            error: None,
            focus: true,
            auth: false,
            username: String::new(),
            password: String::new(),
        });
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::Home);
        }
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    fn new_tab(&mut self) {
        self.tabs.push(Tab::Home);
        self.active = self.tabs.len() - 1;
    }

    fn set_theme(&mut self, ctx: &egui::Context, kind: ThemeKind) {
        self.theme = kind;
        theme::apply(ctx, kind);
    }

    /// PhotoCraft-style document tabs on a dark strip, `+` after them, the theme menu at the end.
    fn tab_strip(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let mut select = None;
        let mut close = None;
        let mut add = false;
        let mut restyle = None;
        let default_look = TabLook::default();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for (i, tab) in self.tabs.iter().enumerate() {
                let active = i == self.active;
                let (title, tooltip, busy) = match tab {
                    Tab::Home => (
                        "New tab".to_owned(),
                        "Open, create or clone a repository".to_owned(),
                        false,
                    ),
                    Tab::Repo(r) => (r.name.clone(), r.path.display().to_string(), r.is_busy()),
                };
                let font = if active {
                    theme::medium(12.5)
                } else {
                    egui::FontId::proportional(12.5)
                };
                let color = if active { t.text } else { t.text_dim };
                let galley = ui.painter().layout_no_wrap(title, font, color);
                let w = (galley.size().x + 66.0).clamp(110.0, 260.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(w, TAB_H), Sense::click());
                let hovered = resp.hovered();
                let look = match tab {
                    Tab::Repo(r) => self.tab_looks.get(&r.path).unwrap_or(&default_look),
                    Tab::Home => &default_look,
                };
                let accent = look.color.color(&t);
                let p = ui.painter();
                if active {
                    p.rect_filled(rect, 0.0, t.chrome);
                    p.line_segment(
                        [
                            rect.left_top() + vec2(0.0, 1.0),
                            rect.right_top() + vec2(0.0, 1.0),
                        ],
                        Stroke::new(2.0, accent),
                    );
                } else if hovered {
                    p.rect_filled(rect, 0.0, t.hover.gamma_multiply(0.6));
                }
                let y = rect.center().y;
                let icon = if matches!(tab, Tab::Home) {
                    "plus"
                } else {
                    look.icon()
                };
                // A chosen colour also marks the tab when it is in the background.
                let icon_tint = if active {
                    accent
                } else if look.color != tab_look::TabColor::default() {
                    accent.gamma_multiply(0.75)
                } else {
                    t.text_faint
                };
                icons::paint(
                    ui,
                    Rect::from_center_size(pos2(rect.left() + 18.0, y), vec2(16.0, 16.0)),
                    icon,
                    14.0,
                    icon_tint,
                );
                let text_clip = Rect::from_min_max(
                    pos2(rect.left() + 32.0, rect.top()),
                    pos2(rect.right() - 28.0, rect.bottom()),
                );
                ui.painter().with_clip_rect(text_clip).galley(
                    pos2(text_clip.left(), y - galley.size().y / 2.0),
                    galley,
                    color,
                );

                let close_rect =
                    Rect::from_center_size(pos2(rect.right() - 15.0, y), vec2(18.0, 18.0));
                if busy {
                    egui::Spinner::new().size(11.0).paint_at(ui, close_rect);
                } else if active || hovered {
                    let c = ui.interact(close_rect, resp.id.with("close"), Sense::click());
                    if c.hovered() {
                        ui.painter().rect_filled(close_rect, t.radius_sm, t.pressed);
                    }
                    icons::paint(
                        ui,
                        close_rect,
                        "x",
                        12.0,
                        if c.hovered() { t.text } else { t.text_faint },
                    );
                    if c.clicked() {
                        close = Some(i);
                    }
                    c.on_hover_text("Close tab (Ctrl+W)");
                }
                if let Tab::Repo(r) = tab {
                    egui::Popup::context_menu(&resp)
                        .style(crate::theme::menu_style)
                        .show(|ui| {
                            if let Some(new) = tab_look::menu(ui, look) {
                                restyle = Some((r.path.clone(), new));
                            }
                        });
                }
                let resp = resp.on_hover_text(tooltip);
                if resp.clicked() && close.is_none() {
                    select = Some(i);
                }
                if resp.middle_clicked() {
                    close = Some(i);
                }
            }
            ui.add_space(4.0);
            if icons::small_button(ui, "plus", "New tab (Ctrl+T)").clicked() {
                add = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                if icons::bar_button(ui, "settings", "Settings").clicked() {
                    self.settings_open = true;
                }
                if icons::bar_button(ui, "command", "Command palette (Ctrl+P)").clicked() {
                    self.open_palette();
                }
                let resp = icons::bar_button(ui, "palette", "Theme");
                egui::Popup::menu(&resp)
                    .style(crate::theme::menu_style)
                    .show(|ui| {
                        ui.label(
                            RichText::new("THEME")
                                .font(theme::semibold(10.5))
                                .color(t.text_faint),
                        );
                        for kind in ThemeKind::ALL {
                            if ui.radio(self.theme == kind, kind.label()).clicked() {
                                self.set_theme(ui.ctx(), kind);
                            }
                        }
                    });
                if icons::bar_button(ui, "book-open", "Manual and keyboard shortcuts (F1)")
                    .clicked()
                {
                    self.manual_open = true;
                }
                // A newer release: first in the row, before the icons.
                if let Some(release) = self.update.clone() {
                    ui.add_space(4.0);
                    let pill = ui.add(
                        egui::Button::new(
                            RichText::new(format!("v{} available", release.version))
                                .color(t.accent_text)
                                .font(theme::medium(11.5)),
                        )
                        .fill(t.accent)
                        .corner_radius(10.0),
                    );
                    egui::Popup::menu(&pill)
                        .style(crate::theme::menu_style)
                        .show(|ui| {
                            // Menus have no side margin (their buttons pad themselves); the
                            // text gets the buttons' own.
                            egui::Frame::new()
                                .inner_margin(egui::Margin {
                                    left: 16,
                                    right: 16,
                                    top: 4,
                                    bottom: 8,
                                })
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "gitcraft {} is out (you have {}).",
                                            release.version,
                                            update::CURRENT
                                        ))
                                        .color(t.text_dim),
                                    );
                                });
                            if ui.button("Open the release page").clicked() {
                                let _ = open::that_detached(&release.url);
                            }
                            if ui.button("Skip this version").clicked() {
                                self.skipped_version = release.version.clone();
                                self.update = None;
                            }
                        });
                }
            });
        });
        if let Some(i) = select {
            self.active = i;
        }
        if let Some((path, look)) = restyle {
            if look == TabLook::default() {
                self.tab_looks.remove(&path);
            } else {
                self.tab_looks.insert(path, look);
            }
        }
        if let Some(i) = close {
            self.close_tab(i);
        }
        if add {
            self.new_tab();
        }
    }

    fn home(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        let mut open = None;
        let mut forget = None;
        let mut action = None;

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(640.0);
                    ui.add_space((ui.available_height() * 0.10).max(32.0));
                    ui.horizontal(|ui| {
                        icons::inline(ui, "git-merge", 34.0, t.accent);
                        ui.label(RichText::new("gitcraft").font(theme::semibold(30.0)));
                    });
                    ui.add_space(4.0);
                    if let Ok(v) = &self.git_version {
                        ui.horizontal(|ui| {
                            icons::inline(ui, "check", 14.0, t.success);
                            ui.label(RichText::new(v).color(t.text_dim));
                        });
                    }
                    if let Some(appimage) = crate::desktop_entry::appimage()
                        && !crate::desktop_entry::installed()
                    {
                        ui.add_space(4.0);
                        let link = ui
                            .link("Add gitcraft to the applications menu")
                            .on_hover_text(appimage.display().to_string());
                        if link.clicked() {
                            self.error = crate::desktop_entry::install(&appimage).err();
                        }
                    }
                    ui.add_space(26.0);

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        let w = (ui.available_width() - 24.0) / 3.0;
                        for (i, (icon, title, sub)) in [
                            ("folder-open", "Open", "An existing local repository"),
                            ("folder-plus", "Create", "A new repository (git init)"),
                            ("log-in", "Clone", "Copy a remote repository"),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if action_card(ui, w, icon, title, sub).clicked() {
                                action = Some(i);
                            }
                        }
                    });

                    if let Some(err) = &self.error {
                        ui.add_space(12.0);
                        ui.colored_label(t.danger, err);
                    }
                    if let Some((dest, _)) = &self.cloning {
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(format!("Cloning into {}…", dest.display()));
                        });
                    }

                    if !self.recent.is_empty() {
                        ui.add_space(30.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.label(
                                RichText::new("RECENT")
                                    .font(theme::semibold(10.5))
                                    .color(t.text_faint),
                            );
                            ui.add_space(4.0);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for (i, path) in self.recent.iter().enumerate() {
                                match recent_row(ui, path) {
                                    Some(RecentClick::Open) => open = Some(path.clone()),
                                    Some(RecentClick::Forget) => forget = Some(i),
                                    None => {}
                                }
                            }
                        });
                    }
                });
            });

        match action {
            Some(0) => self.pick_existing(&ctx),
            Some(1) => self.pick_new(&ctx),
            Some(_) => self.start_clone(),
            None => {}
        }
        if let Some(path) = open {
            self.open_repo(&path, &ctx);
        }
        if let Some(i) = forget {
            self.recent.remove(i);
        }
    }

    fn clone_dialog(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.clone_form else {
            return;
        };
        let t = Tokens::get(ctx);
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::same(18))
            .shadow(ctx.global_style().visuals.window_shadow);
        let mut submit = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::new("clone-dialog"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(110))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Clone repository").font(theme::semibold(15.0)));
                ui.add_space(10.0);
                ui.label(RichText::new("URL").color(t.text_dim).small());
                let url = ui.add(
                    egui::TextEdit::singleline(&mut form.url)
                        .hint_text("https://github.com/user/repo.git")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 6)),
                );
                if form.focus {
                    url.request_focus();
                    form.focus = false;
                }
                if url.changed() && !form.name_edited {
                    form.name = git::clone_dir_name(&form.url);
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Into folder").color(t.text_dim).small());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut form.parent)
                            .desired_width(ui.available_width() - 90.0)
                            .margin(egui::Margin::symmetric(8, 6)),
                    );
                    if ui
                        .add(egui::Button::new("Browse…").min_size(vec2(80.0, 28.0)))
                        .clicked()
                        && let Some(dir) =
                            rfd::FileDialog::new().set_title("Clone into").pick_folder()
                    {
                        form.parent = dir.display().to_string();
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new("Name").color(t.text_dim).small());
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut form.name)
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(8, 6)),
                    )
                    .changed()
                {
                    form.name_edited = true;
                }
                let dest = PathBuf::from(form.parent.trim()).join(form.name.trim());
                ui.label(
                    RichText::new(dest.display().to_string())
                        .color(t.text_faint)
                        .small(),
                );
                if form.auth {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("The remote wants to know who you are. Most hosts take a personal access token as the password.")
                            .color(t.text_dim)
                            .small(),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut form.username)
                            .hint_text("Username")
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(8, 6)),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut form.password)
                            .password(true)
                            .hint_text("Password or token")
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(8, 6)),
                    );
                }
                if let Some(err) = &form.error {
                    ui.colored_label(t.danger, err);
                }
                ui.add_space(12.0);
                let valid = !form.url.trim().is_empty()
                    && !form.parent.trim().is_empty()
                    && !form.name.trim().is_empty();
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    if crate::repo::primary_button(ui, "Clone", valid, 90.0).clicked()
                        || (enter && valid)
                    {
                        submit = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(vec2(80.0, 30.0)))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        if resp.should_close() {
            cancel = true;
        }

        if submit {
            let dest = PathBuf::from(form.parent.trim()).join(form.name.trim());
            let occupied = dest.exists()
                && std::fs::read_dir(&dest)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(true);
            if occupied {
                form.error = Some(format!("{} already exists and isn't empty", dest.display()));
                return;
            }
            let url = form.url.trim().to_owned();
            let creds = (form.auth && !form.username.trim().is_empty()).then(|| git::Credentials {
                username: form.username.trim().to_owned(),
                password: form.password.clone(),
            });
            self.clone_parent = Some(PathBuf::from(form.parent.trim()));
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            let dest2 = dest.clone();
            std::thread::spawn(move || {
                let _ = tx.send(git::clone(&url, &dest2, creds.as_ref()));
                ctx2.request_repaint();
            });
            self.cloning = Some((dest, rx));
            self.cloning_form = self.clone_form.take();
            self.error = None;
        } else if cancel {
            self.clone_form = None;
        }
    }

    fn poll_update(&mut self) {
        let Some(rx) = &self.update_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.update_rx = None;
        // A failed check (offline…) is tried again next start.
        if let Ok(release) = result {
            self.update_checked = git::now();
            if update::newer(&release.version, update::CURRENT)
                && release.version != self.skipped_version
            {
                self.update = Some(release);
            }
        }
    }

    fn poll_clone(&mut self, ctx: &egui::Context) {
        let Some((dest, rx)) = &self.cloning else {
            return;
        };
        let Ok(result) = rx.try_recv() else { return };
        let dest = dest.clone();
        self.cloning = None;
        match result {
            Ok(()) => {
                // Show it in a start page tab if one is active, otherwise in a new tab.
                self.open_repo(&dest, ctx);
                // The credentials that cloned it also fetch and push in its tab.
                if let Some(form) = self.cloning_form.take().filter(|f| f.auth)
                    && let Some(Tab::Repo(repo)) = self.tabs.get_mut(self.active)
                {
                    repo.set_credentials(git::Credentials {
                        username: form.username.trim().to_owned(),
                        password: form.password,
                    });
                }
            }
            // Back to the form, asking for credentials.
            Err(e) if git::is_auth_error(&e) => match self.cloning_form.take() {
                Some(mut form) => {
                    form.error = Some(if form.auth {
                        "Those credentials were refused.".to_owned()
                    } else {
                        "This repository needs you to sign in.".to_owned()
                    });
                    form.auth = true;
                    form.password.clear();
                    self.clone_form = Some(form);
                }
                None => self.error = Some(format!("Clone failed: {e}")),
            },
            Err(e) => self.error = Some(format!("Clone failed: {e}")),
        }
        self.cloning_form = None;
    }

    fn git_missing(&mut self, ui: &mut Ui, err: &str) {
        let t = Tokens::get(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            icons::inline(ui, "circle-alert", 40.0, t.warning);
            ui.add_space(8.0);
            ui.label(RichText::new("Git was not found").font(theme::semibold(24.0)));
            ui.add_space(8.0);
            ui.label("gitcraft uses the git command line. Install git and make sure `git` is on your PATH.");
            ui.add_space(4.0);
            ui.label(RichText::new(err).color(t.text_faint).small());
            ui.add_space(16.0);
            if crate::repo::primary_button(ui, "Check again", true, 140.0).clicked() {
                self.git_version = git::version();
            }
            ui.add_space(8.0);
            ui.hyperlink_to("Download git", "https://git-scm.com/downloads");
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);

        // eframe shows the window after the first frame; maximize it on the next one.
        if MAXIMIZE_WHEN_SHOWN.load(Ordering::Relaxed) {
            if ctx.cumulative_frame_nr() >= 1 {
                MAXIMIZE_WHEN_SHOWN.store(false, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            } else {
                ctx.request_repaint();
            }
        }

        if let Err(err) = self.git_version.clone() {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(t.dock))
                .show(ui, |ui| self.git_missing(ui, &err));
            return;
        }
        self.poll_clone(&ctx);
        self.poll_update();

        // Shortcuts.
        // A focused text field or the terminal keeps these keys for itself.
        let modal_open = self.clone_form.is_some()
            || self.palette.is_some()
            || self.settings_open
            || self.manual_open
            || ctx.egui_wants_keyboard_input();
        let (new_tab, close, refresh, focused, palette, manual) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(Key::T),
                i.modifiers.command && i.key_pressed(Key::W),
                i.key_pressed(Key::F5),
                i.focused,
                i.modifiers.command && i.key_pressed(Key::P),
                i.key_pressed(Key::F1),
            )
        });
        if manual && self.clone_form.is_none() && !self.settings_open && self.palette.is_none() {
            self.manual_open = !self.manual_open;
        }
        if palette && self.clone_form.is_none() && !self.settings_open {
            if self.palette.is_some() {
                self.palette = None;
            } else {
                self.open_palette();
            }
        }
        if new_tab && !modal_open {
            self.new_tab();
        }
        if close && !modal_open {
            self.close_tab(self.active);
        }
        // Coming back to the window (e.g. after using git elsewhere) refreshes the open repo.
        let regained = focused && !self.was_focused;
        self.was_focused = focused;
        if let Some(Tab::Repo(r)) = self.tabs.get_mut(self.active)
            && ((refresh && !modal_open) || regained)
        {
            r.reload(&ctx);
        }

        // Background repos still need their results drained.
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if let Tab::Repo(r) = tab
                && i != self.active
            {
                r.poll(&ctx);
            }
        }

        egui::Panel::top("tabs")
            .frame(
                egui::Frame::new()
                    .fill(t.tab_strip)
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| self.tab_strip(ui));

        let active = self.active;
        match self.tabs.get_mut(active) {
            Some(Tab::Repo(repo)) => {
                repo.keys_blocked = self.palette.is_some()
                    || self.settings_open
                    || self.manual_open
                    || self.clone_form.is_some();
                repo.ui(ui);
                // A worktree asked to be opened in its own tab.
                if let Some(path) = repo.open_request.take() {
                    self.open_repo(&path, &ctx);
                }
            }
            _ => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(t.dock))
                    .show(ui, |ui| self.home(ui));
            }
        }
        self.clone_dialog(&ctx);
        self.palette_ui(&ctx);
        self.settings_ui(&ctx);
        self.manual_ui(&ctx);

        // Long operations that ended: a toast, and a desktop notification when away.
        for tab in &mut self.tabs {
            if let Tab::Repo(r) = tab
                && let Some((text, error)) = r.finished.take()
            {
                if !focused && self.settings.notifications {
                    notify(&text);
                }
                self.toasts.push(Toast {
                    text,
                    error,
                    until: std::time::Instant::now() + std::time::Duration::from_secs(6),
                });
            }
        }
        self.toasts_ui(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let open: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter_map(|t| match t {
                Tab::Repo(r) => Some(r.path.clone()),
                Tab::Home => None,
            })
            .collect();
        // Index of the active tab among the saved (repo) tabs.
        let active = self
            .tabs
            .iter()
            .take(self.active + 1)
            .filter(|t| matches!(t, Tab::Repo(_)))
            .count()
            .saturating_sub(1);
        let saved = Saved {
            open,
            active,
            recent: self.recent.clone(),
            theme: self.theme,
            clone_parent: self.clone_parent.clone(),
            settings: self.settings.clone(),
            update_checked: self.update_checked,
            skipped_version: self.skipped_version.clone(),
            tab_looks: self.tab_looks.clone(),
        };
        eframe::set_value(storage, eframe::APP_KEY, &saved);
    }
}

/// The window and taskbar icon (the `.exe` gets the same picture from `build.rs`).
pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon/gitcraft-256.png"))
        .unwrap_or_default()
}

/// A large clickable card on the start page.
fn action_card(ui: &mut Ui, width: f32, icon: &str, title: &str, subtitle: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 116.0), Sense::click());
    let fill = if resp.hovered() { t.hover } else { t.card };
    let stroke = if resp.hovered() {
        t.accent
    } else {
        t.field_border
    };
    let p = ui.painter();
    p.rect(
        rect,
        t.radius_lg,
        fill,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.center().x, rect.top() + 34.0), vec2(28.0, 28.0)),
        icon,
        26.0,
        t.accent,
    );
    p.text(
        pos2(rect.center().x, rect.top() + 68.0),
        Align2::CENTER_CENTER,
        title,
        theme::semibold(14.0),
        t.text,
    );
    p.text(
        pos2(rect.center().x, rect.top() + 90.0),
        Align2::CENTER_CENTER,
        subtitle,
        egui::FontId::proportional(11.5),
        t.text_dim,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

enum RecentClick {
    Open,
    Forget,
}

fn recent_row(ui: &mut Ui, path: &Path) -> Option<RecentClick> {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, t.radius, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 20.0, y), vec2(18.0, 18.0)),
        "folder-git-2",
        16.0,
        t.icon,
    );
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Leave room for the remove button; long paths are cut at the row's edge.
    let p = ui.painter_at(rect.with_max_x(rect.right() - 36.0));
    p.text(
        pos2(rect.left() + 40.0, y - 8.0),
        Align2::LEFT_CENTER,
        name,
        theme::medium(12.5),
        t.text,
    );
    p.text(
        pos2(rect.left() + 40.0, y + 8.0),
        Align2::LEFT_CENTER,
        path.display().to_string(),
        egui::FontId::proportional(11.0),
        t.text_faint,
    );
    let mut forget = false;
    if hovered || ui.rect_contains_pointer(rect) {
        let x_rect = Rect::from_center_size(pos2(rect.right() - 18.0, y), vec2(20.0, 20.0));
        let x = ui
            .interact(x_rect, resp.id.with("forget"), Sense::click())
            .on_hover_text("Remove from list");
        if x.hovered() {
            ui.painter().rect_filled(x_rect, t.radius_sm, t.pressed);
        }
        icons::paint(ui, x_rect, "x", 12.0, t.text_faint);
        forget = x.clicked();
    }
    if forget {
        Some(RecentClick::Forget)
    } else if resp.clicked() {
        Some(RecentClick::Open)
    } else {
        None
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `canonicalize` on Windows returns `\\?\C:\…`; show and store the familiar form.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

impl App {
    fn open_palette(&mut self) {
        self.palette = Some(Palette {
            query: String::new(),
            selected: 0,
            focus: true,
        });
    }

    /// Every palette entry: (label, hint, what it runs).
    fn palette_entries(&self) -> Vec<(String, String, Entry)> {
        let mut list = vec![
            (
                "New tab".to_owned(),
                "Ctrl+T".to_owned(),
                Entry::App(AppCommand::NewTab),
            ),
            (
                "Open repository…".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Open),
            ),
            (
                "Create repository…".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Create),
            ),
            (
                "Clone repository…".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Clone),
            ),
            (
                "Settings".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Settings),
            ),
            (
                "Manual and keyboard shortcuts".to_owned(),
                "F1".to_owned(),
                Entry::App(AppCommand::Manual),
            ),
        ];
        for kind in ThemeKind::ALL {
            list.push((
                format!("Theme: {}", kind.label()),
                "theme".to_owned(),
                Entry::App(AppCommand::Theme(kind)),
            ));
        }
        if let Some(Tab::Repo(repo)) = self.tabs.get(self.active) {
            for (i, (label, hint, _)) in repo.palette_items().into_iter().enumerate() {
                list.push((label, hint, Entry::Repo(i)));
            }
        }
        list
    }

    fn run_entry(&mut self, entry: Entry, ctx: &egui::Context) {
        match entry {
            Entry::App(AppCommand::NewTab) => self.new_tab(),
            Entry::App(AppCommand::Open) => self.pick_existing(ctx),
            Entry::App(AppCommand::Create) => self.pick_new(ctx),
            Entry::App(AppCommand::Clone) => self.start_clone(),
            Entry::App(AppCommand::Settings) => self.settings_open = true,
            Entry::App(AppCommand::Manual) => self.manual_open = true,
            Entry::App(AppCommand::Theme(kind)) => self.set_theme(ctx, kind),
            Entry::Repo(i) => {
                if let Some(Tab::Repo(repo)) = self.tabs.get_mut(self.active)
                    && let Some((_, _, action)) = repo.palette_items().into_iter().nth(i)
                {
                    repo.queue(action);
                    ctx.request_repaint();
                }
            }
        }
    }

    fn palette_ui(&mut self, ctx: &egui::Context) {
        if self.palette.is_none() {
            return;
        }
        let t = Tokens::get(ctx);
        let entries = self.palette_entries();
        let Some(palette) = &mut self.palette else {
            return;
        };
        let query = palette.query.to_lowercase();
        let shown: Vec<usize> = (0..entries.len())
            .filter(|&i| fuzzy(&entries[i].0.to_lowercase(), &query))
            .take(60)
            .collect();
        palette.selected = palette.selected.min(shown.len().saturating_sub(1));
        let (up, down, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        if down {
            palette.selected = (palette.selected + 1).min(shown.len().saturating_sub(1));
        }
        if up {
            palette.selected = palette.selected.saturating_sub(1);
        }
        let mut run = None;
        let mut close = esc;
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::same(10))
            .shadow(ctx.global_style().visuals.window_shadow);
        let area = egui::Area::new(egui::Id::new("palette"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 70.0));
        let resp = area.show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(560.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut palette.query)
                        .hint_text("Type a command or a branch…")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(10, 8))
                        .font(theme::medium(14.0)),
                );
                if std::mem::take(&mut palette.focus) {
                    edit.request_focus();
                }
                if edit.changed() {
                    palette.selected = 0;
                }
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (k, &i) in shown.iter().enumerate() {
                            let (label, hint, _) = &entries[i];
                            let (rect, r) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 28.0),
                                Sense::click(),
                            );
                            if k == palette.selected {
                                ui.painter().rect_filled(rect, t.radius_sm, t.row_selected);
                                if up || down {
                                    ui.scroll_to_rect(rect, None);
                                }
                            } else if r.hovered() {
                                ui.painter().rect_filled(rect, t.radius_sm, t.hover);
                            }
                            ui.painter().text(
                                pos2(rect.left() + 10.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                label,
                                egui::FontId::proportional(13.0),
                                t.text,
                            );
                            ui.painter().text(
                                pos2(rect.right() - 10.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                hint,
                                egui::FontId::proportional(11.0),
                                t.text_faint,
                            );
                            if r.clicked() {
                                run = Some(i);
                            }
                        }
                        if shown.is_empty() {
                            ui.label(RichText::new("Nothing matches").color(t.text_faint));
                        }
                    });
            });
        });
        if enter && let Some(&i) = shown.get(palette.selected) {
            run = Some(i);
        }
        if resp.response.clicked_elsewhere()
            && ctx.input(|i| i.pointer.any_pressed())
            && run.is_none()
        {
            close = true;
        }
        if close || run.is_some() {
            self.palette = None;
        }
        if let Some(i) = run
            && let Some((_, _, entry)) = entries.into_iter().nth(i)
        {
            self.run_entry(entry, ctx);
        }
    }

    fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let t = Tokens::get(ctx);
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::same(18))
            .shadow(ctx.global_style().visuals.window_shadow);
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::new("settings"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(110))
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").font(theme::semibold(16.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close").clicked() {
                            close = true;
                        }
                    });
                });
                egui::ScrollArea::vertical()
                    .max_height(560.0)
                    .show(ui, |ui| {
                        if self.settings.ui(ui) {
                            self.settings.set(ui.ctx());
                        }
                    });
            });
        if close || resp.should_close() {
            self.settings_open = false;
        }
    }

    fn manual_ui(&mut self, ctx: &egui::Context) {
        if !self.manual_open {
            return;
        }
        let t = Tokens::get(ctx);
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::same(18))
            .shadow(ctx.global_style().visuals.window_shadow);
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::new("manual"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(110))
            .show(ctx, |ui| {
                ui.set_width(640.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Manual").font(theme::semibold(16.0)));
                    ui.label(
                        RichText::new(format!("gitcraft {}", update::CURRENT)).color(t.text_faint),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
                let max = (ctx.content_rect().height() - 140.0).max(240.0);
                egui::ScrollArea::vertical()
                    .max_height(max)
                    .min_scrolled_height(max)
                    .show(ui, crate::manual::ui);
            });
        if close || resp.should_close() {
            self.manual_open = false;
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let t = Tokens::get(ctx);
        egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0))
            .show(ctx, |ui| {
                for toast in &self.toasts {
                    egui::Frame::new()
                        .fill(t.card)
                        .stroke(Stroke::new(
                            1.0,
                            if toast.error { t.danger } else { t.success },
                        ))
                        .corner_radius(t.radius_lg)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .shadow(ctx.global_style().visuals.popup_shadow)
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                let icon = if toast.error { "circle-alert" } else { "check" };
                                icons::inline(
                                    ui,
                                    icon,
                                    16.0,
                                    if toast.error { t.danger } else { t.success },
                                );
                                ui.add(egui::Label::new(&toast.text).wrap());
                            });
                        });
                    ui.add_space(6.0);
                }
            });
    }
}

/// `needle`'s characters appear in `haystack` in order ("cmain" → "Check out main").
fn fuzzy(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|n| chars.any(|h| h == n))
}

/// A desktop notification, on its own thread (it can take a moment to show).
fn notify(text: &str) {
    let text = text.to_owned();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname("gitcraft")
            .summary("gitcraft")
            .body(&text)
            .show();
    });
}

#[cfg(test)]
mod tests {
    use super::{Saved, fuzzy, window_icon};
    use crate::tab_look::{TabColor, TabLook};

    /// A storage that keeps strings in memory, as eframe's own does on disk.
    #[derive(Default)]
    struct Memory(std::collections::HashMap<String, String>);

    impl eframe::Storage for Memory {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn tab_looks_survive_a_restart() {
        let look = TabLook {
            icon: Some("rocket".to_owned()),
            color: TabColor::Purple,
        };
        let mut saved = Saved::default();
        saved.tab_looks.insert("C:/code/app".into(), look.clone());
        let mut storage = Memory::default();
        eframe::set_value(&mut storage, eframe::APP_KEY, &saved);
        let back: Option<Saved> = eframe::get_value(&storage, eframe::APP_KEY);
        assert!(
            back.is_some_and(
                |b| b.tab_looks.get(std::path::Path::new("C:/code/app")) == Some(&look)
            )
        );
    }

    #[test]
    fn window_icon_decodes() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert!(
            icon.rgba.chunks(4).any(|p| p[3] > 0),
            "the icon isn't blank"
        );
    }

    #[test]
    fn fuzzy_matching() {
        assert!(fuzzy("check out main", "cmain"));
        assert!(fuzzy("check out main", "co main"));
        assert!(!fuzzy("fetch", "push"));
        assert!(fuzzy("anything", ""));
    }
}
