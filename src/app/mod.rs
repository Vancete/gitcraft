//! Window shell: git check, browser-like tabs, the start page and cloning.

mod clone;
mod home;
mod palette;
mod panels;
mod tab_strip;
mod tabs;
#[cfg(test)]
mod tests;
mod worktrees;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use egui::{Key, Ui};
use serde::{Deserialize, Serialize};

use crate::git;
use crate::repo::{OpenRequest, RepoTab};
use crate::settings::Settings;
use crate::tab_look::TabLook;
use crate::theme::{self, ThemeKind, Tokens};
use crate::update;
use clone::CloneForm;
use palette::Palette;
use panels::Toast;
use tab_strip::TabDrag;

const MAX_RECENT: usize = 12;

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
    Settings,
}

pub struct App {
    git_version: Result<String, String>,
    tabs: Vec<Tab>,
    /// What is typed in the worktree switcher.
    switcher_query: String,
    /// The switcher just opened: its search field takes the keyboard.
    switcher_focus: bool,
    /// The worktree picked in the switcher, to open after the tab strip is drawn.
    switch_to: Option<PathBuf>,
    /// The active tab's worktree is to be closed, after the tab strip is drawn.
    close_worktree: bool,
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
    manual_open: bool,
    palette: Option<Palette>,
    toasts: Vec<Toast>,
    /// The folders of the last repository tabs closed, the last one at the end.
    closed: Vec<PathBuf>,
    tab_drag: Option<TabDrag>,
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
            switcher_query: String::new(),
            switcher_focus: false,
            switch_to: None,
            close_worktree: false,
            active: 0,
            recent: saved.recent,
            theme: saved.theme,
            clone_parent: saved.clone_parent,
            settings: saved.settings.clone(),
            manual_open: false,
            palette: None,
            toasts: Vec::new(),
            closed: Vec::new(),
            tab_drag: None,
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
        let mut kept = Vec::new();
        if app.git_version.is_ok() {
            for path in saved.open {
                let is_dir = path.is_dir();
                kept.push(is_dir);
                if is_dir {
                    app.tabs.push(Tab::Repo(Box::new(RepoTab::new(path, ctx))));
                }
            }
        }
        if app.tabs.is_empty() {
            app.tabs.push(Tab::Home);
        }
        app.active = active_among_kept(&kept, saved.active).min(app.tabs.len() - 1);
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
        let root = match repo_root(path) {
            Ok(root) => root,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
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
        // A worktree parked in some tab comes out with everything it had.
        let parked = self.take_parked(&root);
        let came_back = parked.is_some();
        let mut view = parked.unwrap_or_else(|| Box::new(RepoTab::new(root, ctx)));
        if came_back {
            view.resume(ctx);
        }
        let tab = Tab::Repo(view);
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

    fn set_theme(&mut self, ctx: &egui::Context, kind: ThemeKind) {
        self.theme = kind;
        theme::apply(ctx, kind);
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

    /// Keyboard shortcuts, and the reload on coming back to the window. Returns whether the
    /// window has the focus.
    fn shortcuts(&mut self, ctx: &egui::Context) -> bool {
        // A focused text field or the terminal keeps these keys for itself.
        let modal_open = self.clone_form.is_some()
            || self.palette.is_some()
            || self.manual_open
            || ctx.egui_wants_keyboard_input();
        let (new_tab, reopen, close, refresh, focused, palette, manual) = ctx.input(|i| {
            let t = i.modifiers.command && i.key_pressed(Key::T);
            (
                t && !i.modifiers.shift,
                t && i.modifiers.shift,
                i.modifiers.command && i.key_pressed(Key::W),
                i.key_pressed(Key::F5),
                i.focused,
                i.modifiers.command && i.key_pressed(Key::P),
                i.key_pressed(Key::F1),
            )
        });
        if manual && self.clone_form.is_none() && self.palette.is_none() {
            self.manual_open = !self.manual_open;
        }
        if palette && self.clone_form.is_none() {
            if self.palette.is_some() {
                self.palette = None;
            } else {
                self.open_palette();
            }
        }
        if new_tab && !modal_open {
            self.new_tab();
        }
        if reopen && !modal_open {
            self.reopen_closed_tab(ctx);
        }
        if close && !modal_open {
            self.close_tab(self.active, ctx);
        }
        // Coming back to the window (e.g. after using git elsewhere) refreshes the open repo.
        let regained = focused && !self.was_focused;
        self.was_focused = focused;
        if let Some(Tab::Repo(r)) = self.tabs.get_mut(self.active)
            && ((refresh && !modal_open) || regained)
        {
            r.reload(ctx);
        }
        focused
    }

    /// The active tab's page under the strip.
    fn active_tab_ui(&mut self, ui: &mut Ui, t: &Tokens) {
        let ctx = ui.ctx().clone();
        let active = self.active;
        match self.tabs.get_mut(active) {
            Some(Tab::Repo(repo)) => {
                repo.keys_blocked =
                    self.palette.is_some() || self.manual_open || self.clone_form.is_some();
                repo.ui(ui);
                // A tab asked for a folder to be opened.
                match repo.open_request.take() {
                    Some(OpenRequest::NewTab(path)) => self.open_repo(&path, &ctx),
                    Some(OpenRequest::Worktree(path)) => self.open_worktree(&path, &ctx),
                    None => {}
                }
            }
            Some(Tab::Settings) => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(t.dock))
                    .show(ui, |ui| self.settings_page(ui));
            }
            _ => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(t.dock))
                    .show(ui, |ui| self.home(ui));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        // Column widths dragged in a graph header live in the context; keep them for saving.
        self.settings = Settings::get(&ctx);

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
        let focused = self.shortcuts(&ctx);

        // Background repos still need their results drained.
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if let Tab::Repo(r) = tab
                && i != self.active
            {
                r.poll(&ctx);
            }
        }

        // Worktrees parked in any tab keep going too.
        for tab in &mut self.tabs {
            if let Tab::Repo(r) = tab {
                r.poll_parked(&ctx);
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
        if let Some(path) = self.switch_to.take() {
            self.open_worktree(&path, &ctx);
        }
        if std::mem::take(&mut self.close_worktree) {
            self.close_current_worktree(&ctx);
        }

        self.active_tab_ui(ui, &t);
        self.clone_dialog(&ctx);
        self.palette_ui(&ctx);
        self.manual_ui(&ctx);
        self.toast_finished(focused);
        self.toasts_ui(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let open: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter_map(|t| match t {
                Tab::Repo(r) => Some(r.path.clone()),
                Tab::Home | Tab::Settings => None,
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

/// Where the tab that was active at `active` is once the tabs not `kept` (their folder is gone)
/// are left out: a gone one passes to the tab before it.
fn active_among_kept(kept: &[bool], active: usize) -> usize {
    kept.iter()
        .take(active.saturating_add(1))
        .filter(|&&k| k)
        .count()
        .saturating_sub(1)
}

/// The top of the repository `path` is in, in the form tabs are told apart by.
fn repo_root(path: &Path) -> Result<PathBuf, String> {
    let root = git::toplevel(path)?;
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    Ok(strip_verbatim(root))
}

/// The window and taskbar icon (the `.exe` gets the same picture from `build.rs`).
pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../assets/app-icon/gitcraft-256.png"))
        .unwrap_or_default()
}

/// `canonicalize` on Windows returns `\\?\C:\…`; show and store the familiar form.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}
