//! Window shell: git check, browser-like tabs, the start page and cloning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use egui::{Align2, Key, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::git;
use crate::icons;
use crate::repo::worktree_tab::{self, Chip};
use crate::repo::{OpenRequest, RepoTab};
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
    Settings,
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
    tab_drag: Option<TabDrag>,
}

/// A tab being dragged along the strip.
#[derive(Clone, Copy)]
struct TabDrag {
    /// Its index when the drag started.
    from: usize,
    /// Where the pointer holds it, from its left edge.
    grab: f32,
}

/// Seconds tabs take to slide into their place.
const TAB_SLIDE: f32 = 0.15;

/// The command palette (Ctrl+P): type to filter, Enter runs.
struct Palette {
    query: String,
    selected: usize,
    focus: bool,
}

/// Wide enough for a repository, its branch's chip and the badge.
const MAX_TAB_W: f32 = 380.0;
/// The badge counting a tab's worktrees with work in the background.
const BADGE_W: f32 = 22.0;
/// The widest a worktree chip gets: what a tab at its widest leaves next to its icon, its close
/// button and the badge.
const MAX_CHIP_W: f32 = MAX_TAB_W - 32.0 - 28.0 - BADGE_W;

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
                kept.push(path.is_dir());
                if path.is_dir() {
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

    /// Moves the active tab to a worktree of its repository, when the settings say so, parking
    /// what it was showing. A worktree open in another tab just gets that tab shown.
    fn open_worktree(&mut self, path: &Path, ctx: &egui::Context) {
        if !self.settings.worktrees_in_same_tab {
            self.open_repo(path, ctx);
            return;
        }
        let root = match repo_root(path) {
            Ok(root) => root,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::Repo(r) if r.path == root))
        {
            self.active = i;
            return;
        }
        let parked = self.take_parked(&root);
        let Some(Tab::Repo(current)) = self.tabs.get_mut(self.active) else {
            self.open_repo(&root, ctx);
            return;
        };
        // The tab keeps the look it was given.
        if let Some(look) = self.tab_looks.get(&current.path).cloned() {
            self.tab_looks.entry(root.clone()).or_insert(look);
        }
        let came_back = parked.is_some();
        let mut next = parked.unwrap_or_else(|| Box::new(RepoTab::new(root, ctx)));
        next.continue_from(current);
        if came_back {
            next.resume(ctx);
        }
        let old = std::mem::replace(current, next);
        current.adopt_parked(old);
    }

    /// Closes the worktree the active tab shows and goes back to the repository's main one. What
    /// was going on in it ends, as when closing a tab; the worktree itself stays on disk.
    fn close_current_worktree(&mut self, ctx: &egui::Context) {
        let main = match self.tabs.get(self.active) {
            Some(Tab::Repo(r)) => r.main_worktree().and_then(|main| repo_root(main).ok()),
            _ => return,
        };
        // No main worktree to show (a bare repository): this is the end of the tab.
        let Some(root) = main else {
            self.close_tab(self.active, ctx);
            return;
        };
        let shows_main = |t: &Tab| matches!(t, Tab::Repo(r) if r.path == root);
        if self.tabs.iter().any(shows_main) {
            self.close_tab(self.active, ctx);
            if let Some(i) = self.tabs.iter().position(shows_main) {
                self.active = i;
            }
            return;
        }
        let parked = self.take_parked(&root);
        let Some(Tab::Repo(current)) = self.tabs.get_mut(self.active) else {
            return;
        };
        let came_back = parked.is_some();
        let mut next = parked.unwrap_or_else(|| Box::new(RepoTab::new(root.clone(), ctx)));
        next.continue_from(current);
        next.inherit_parked(current);
        if came_back {
            next.resume(ctx);
        }
        *current = next;
    }

    /// Takes the parked view of the folder `path` out of whichever tab has it.
    fn take_parked(&mut self, path: &Path) -> Option<Box<RepoTab>> {
        self.tabs.iter_mut().find_map(|tab| match tab {
            Tab::Repo(r) => r.take_parked(path),
            Tab::Home | Tab::Settings => None,
        })
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

    fn close_tab(&mut self, i: usize, ctx: &egui::Context) {
        if i >= self.tabs.len() {
            return;
        }
        // Work still going on in a parked worktree isn't closed with a click on what is showing.
        if let Some(Tab::Repo(r)) = self.tabs.get_mut(i)
            && let Some(next) = r.successor_with_work()
        {
            *r = next;
            r.resume(ctx);
            let name = r.label(true);
            let name = name.branch.unwrap_or(name.repo);
            self.active = i;
            self.toasts.push(Toast {
                text: format!("{name} still has work going on, so the tab now shows it"),
                error: false,
                until: std::time::Instant::now() + std::time::Duration::from_secs(6),
            });
            return;
        }
        self.tabs.remove(i);
        // A drag going on holds a tab by its index, which now names another one.
        self.tab_drag = None;
        if self.tabs.is_empty() {
            self.tabs.push(Tab::Home);
        }
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    /// Moves tab `from` to index `to`; the same tab stays active.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = if self.active == from {
            to
        } else if from < self.active && to >= self.active {
            self.active - 1
        } else if from > self.active && to <= self.active {
            self.active + 1
        } else {
            self.active
        };
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
    /// A tab can be dragged along the strip, the others sliding out of its way.
    fn tab_strip(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let mut select = None;
        let mut close = None;
        let mut add = false;
        let mut restyle = None;
        let default_look = TabLook::default();
        let mut moved = None;
        ui.horizontal(|ui| {
            const GAP: f32 = 1.0;
            ui.spacing_mut().item_spacing.x = GAP;
            let ctx = ui.ctx().clone();
            let n = self.tabs.len();
            // Each tab's title and width.
            let titles: Vec<_> = self
                .tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| {
                    let active = i == self.active;
                    let (title, chip, background): (String, Option<Chip>, usize) = match tab {
                        Tab::Home => ("New tab".to_owned(), None, 0),
                        Tab::Repo(r) => {
                            let label = r.label(self.settings.worktrees_in_same_tab);
                            let chip = worktree_tab::chip_for(ui, &label, MAX_CHIP_W);
                            (label.repo, chip, r.background_work().len())
                        }
                        Tab::Settings => ("Settings".to_owned(), None, 0),
                    };
                    let font = if active {
                        theme::medium(12.5)
                    } else {
                        egui::FontId::proportional(12.5)
                    };
                    let color = if active { t.text } else { t.text_dim };
                    let galley = ui.painter().layout_no_wrap(title, font, color);
                    let extras = chip.as_ref().map_or(0.0, |c| c.width + 6.0)
                        + if background > 0 { BADGE_W } else { 0.0 };
                    let w = (galley.size().x + 66.0 + extras).clamp(110.0, MAX_TAB_W);
                    (galley, color, w, chip, background)
                })
                .collect();
            let width = |i: usize| titles.get(i).map_or(0.0, |g| g.2);
            let total: f32 = titles.iter().map(|g| g.2 + GAP).sum::<f32>() - GAP;
            let (strip, _) = ui.allocate_exact_size(vec2(total.max(0.0), TAB_H), Sense::hover());

            // The dragged tab follows the pointer; the slot it would drop into is where its
            // middle is among the others.
            let dragged = self.tab_drag.filter(|d| d.from < n).and_then(|d| {
                let x = ctx.pointer_interact_pos()?.x - d.grab;
                let max = (strip.right() - width(d.from)).max(strip.left());
                Some((d.from, x.clamp(strip.left(), max)))
            });
            let mut order: Vec<usize> = (0..n).collect();
            if let Some((from, x)) = dragged {
                let middle = x + width(from) / 2.0;
                let mut left = strip.left();
                let mut slot = 0;
                for j in (0..n).filter(|&j| j != from) {
                    if left + width(j) / 2.0 < middle {
                        slot += 1;
                    }
                    left += width(j) + GAP;
                }
                order.retain(|&j| j != from);
                order.insert(slot, from);
                if !ctx.input(|i| i.pointer.primary_down()) {
                    moved = Some((from, slot));
                }
            }
            let mut place = vec![strip.left(); n];
            let mut x = strip.left();
            for &j in &order {
                if let Some(p) = place.get_mut(j) {
                    *p = x;
                }
                x += width(j) + GAP;
            }

            // The dragged tab is drawn last, over the others.
            let dragging = dragged.map(|(from, _)| from);
            let paint_order = (0..n).filter(|&j| Some(j) != dragging).chain(dragging);
            let mut homes = 0;
            let ids: Vec<egui::Id> = self
                .tabs
                .iter()
                .map(|tab| match tab {
                    Tab::Repo(r) => egui::Id::new(("tab", &r.path)),
                    Tab::Home => {
                        homes += 1;
                        egui::Id::new(("tab-home", homes))
                    }
                    Tab::Settings => egui::Id::new("tab-settings"),
                })
                .collect();
            for i in paint_order {
                let (Some(tab), Some((galley, color, w, chip, background)), Some(&id)) =
                    (self.tabs.get(i), titles.get(i), ids.get(i))
                else {
                    continue;
                };
                let (galley, color, w, background) = (galley.clone(), *color, *w, *background);
                let active = i == self.active;
                let (tooltip, busy) = match tab {
                    Tab::Home => ("Open, create or clone a repository".to_owned(), false),
                    Tab::Repo(r) => (r.path.display().to_string(), r.is_busy()),
                    Tab::Settings => ("Settings".to_owned(), false),
                };
                let x = match dragged {
                    Some((from, x)) if from == i => {
                        ctx.animate_value_with_time(id.with("x"), x, 0.0)
                    }
                    _ => ctx.animate_value_with_time(
                        id.with("x"),
                        place.get(i).copied().unwrap_or(strip.left()),
                        TAB_SLIDE,
                    ),
                };
                let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(w, TAB_H));
                let resp = ui.interact(rect, id, Sense::click_and_drag());
                let hovered = resp.hovered() && dragging.is_none();
                let look = match tab {
                    Tab::Repo(r) => self.tab_looks.get(&r.path).unwrap_or(&default_look),
                    Tab::Home | Tab::Settings => &default_look,
                };
                let accent = look.color.color(&t);
                let p = ui.painter();
                if dragging == Some(i) && !active {
                    // Opaque, so the tabs it passes over don't show through.
                    p.rect_filled(rect, 0.0, t.hover);
                }
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
                let icon = match tab {
                    Tab::Home => "plus",
                    Tab::Settings => "settings",
                    Tab::Repo(_) => look.icon(),
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
                // The name, then the chip right after it, then the badge. A name too long for
                // the tab gives way to them.
                let badge_w = if background > 0 { BADGE_W } else { 0.0 };
                let chip_w = chip.as_ref().map_or(0.0, |c| c.width);
                let right = rect.right() - 28.0;
                let chip_left =
                    (rect.left() + 32.0 + galley.size().x + 6.0).min(right - chip_w - badge_w);
                let text_right = if chip.is_some() || background > 0 {
                    chip_left - 4.0
                } else {
                    right
                };
                let text_clip = Rect::from_min_max(
                    pos2(rect.left() + 32.0, rect.top()),
                    pos2(text_right, rect.bottom()),
                );
                ui.painter().with_clip_rect(text_clip).galley(
                    pos2(text_clip.left(), y - galley.size().y / 2.0),
                    galley,
                    color,
                );

                if let Some(chip) = chip {
                    let chip_rect =
                        Rect::from_min_size(pos2(chip_left, y - 10.0), vec2(chip.width, 20.0));
                    if let (true, Tab::Repo(r)) = (active, tab) {
                        // The chip opens the switcher, its end closes the worktree.
                        let body =
                            chip_rect.with_max_x(chip_rect.right() - worktree_tab::CHIP_CLOSE_W);
                        let c = ui.interact(body, id.with("worktree"), Sense::click());
                        worktree_tab::paint_chip(ui, chip_rect, chip, true, c.hovered());
                        let close_rect = Rect::from_center_size(
                            pos2(chip_rect.right() - worktree_tab::CHIP_CLOSE_W / 2.0, y),
                            vec2(18.0, 18.0),
                        );
                        let x = ui.interact(close_rect, id.with("close-worktree"), Sense::click());
                        if x.hovered() {
                            ui.painter().rect_filled(close_rect, t.radius_sm, t.pressed);
                        }
                        icons::paint(
                            ui,
                            close_rect,
                            "x",
                            11.0,
                            if x.hovered() { t.text } else { t.text_faint },
                        );
                        if x.clicked() {
                            self.close_worktree = true;
                        }
                        x.on_hover_text(
                            "Close this worktree and go back to the main one. It stays on disk.",
                        );
                        if c.clicked() {
                            self.switcher_query.clear();
                            self.switcher_focus = true;
                        }
                        let popup = egui::Popup::menu(&c)
                            .style(crate::theme::menu_style)
                            .width(worktree_tab::SWITCHER_W)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                        popup.show(|ui| {
                            let picked = worktree_tab::switcher(
                                ui,
                                r,
                                &mut self.switcher_query,
                                &mut self.switcher_focus,
                            );
                            if picked.is_some() {
                                self.switch_to = picked;
                                ui.close();
                            }
                        });
                        c.on_hover_text(format!("{}\nSwitch worktree", chip.full_name));
                    } else {
                        worktree_tab::paint_chip(ui, chip_rect, chip, false, false);
                    }
                }
                if background > 0 {
                    let at = pos2(chip_left + chip_w + 7.0, y);
                    ui.painter().circle_filled(at, 3.0, t.accent);
                    ui.painter().text(
                        at + vec2(5.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        background.to_string(),
                        theme::medium(10.5),
                        t.text_dim,
                    );
                    let badge = Rect::from_center_size(at + vec2(4.0, 0.0), vec2(BADGE_W, 20.0));
                    ui.interact(badge, id.with("background"), Sense::hover())
                        .on_hover_text(format!(
                            "{background} worktree(s) with work in the background"
                        ));
                }

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
                    if dragging.is_none() {
                        c.on_hover_text("Close tab (Ctrl+W)");
                    }
                }
                if resp.drag_started_by(egui::PointerButton::Primary)
                    && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
                {
                    self.tab_drag = Some(TabDrag {
                        from: i,
                        grab: origin.x - rect.left(),
                    });
                    select = Some(i);
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
                if dragging.is_none() {
                    resp.clone().on_hover_text(tooltip);
                }
                if resp.clicked() && close.is_none() {
                    select = Some(i);
                }
                if resp.middle_clicked() {
                    close = Some(i);
                }
            }
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.tab_drag = None;
            }
            ui.add_space(4.0);
            if icons::small_button(ui, "plus", "New tab (Ctrl+T)").clicked() {
                add = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                if icons::bar_button(ui, "settings", "Settings").clicked() {
                    self.open_settings();
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
        // A tab closed as another is dropped: the indices are from before either, so the drop
        // is left out rather than moving the wrong tab or closing one that moved.
        if let Some((from, to)) = moved
            && close.is_none()
        {
            self.move_tab(from, to);
        }
        if let Some((path, look)) = restyle {
            if look == TabLook::default() {
                self.tab_looks.remove(&path);
            } else {
                self.tab_looks.insert(path, look);
            }
        }
        if let Some(i) = close {
            self.close_tab(i, ui.ctx());
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
            // One clone at a time: a second would take the first one's place, whose result
            // (and its tab) would never come.
            if let Some((running, _)) = &self.cloning {
                form.error = Some(format!(
                    "Wait for the clone into {} to finish",
                    running.display()
                ));
                return;
            }
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
                // The credentials that cloned it also fetch and push in its tab, and in no
                // other: when it couldn't be opened, the active tab is another repository's.
                let root = repo_root(&dest).ok();
                if let Some(form) = self.cloning_form.take().filter(|f| f.auth)
                    && let Some(Tab::Repo(repo)) = self.tabs.get_mut(self.active)
                    && root.as_ref() == Some(&repo.path)
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

        // Shortcuts.
        // A focused text field or the terminal keeps these keys for itself.
        let modal_open = self.clone_form.is_some()
            || self.palette.is_some()
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
        if close && !modal_open {
            self.close_tab(self.active, &ctx);
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
        self.clone_dialog(&ctx);
        self.palette_ui(&ctx);
        self.manual_ui(&ctx);

        // Long operations that ended: a toast, and a desktop notification when away.
        for tab in &mut self.tabs {
            let Tab::Repo(r) = tab else { continue };
            for (text, error) in r.finished.drain(..) {
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
            Entry::App(AppCommand::Settings) => self.open_settings(),
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

    /// Focuses the settings tab, opening it after the others when there is none.
    fn open_settings(&mut self) {
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Settings)) {
            self.active = i;
        } else {
            self.tabs.push(Tab::Settings);
            self.active = self.tabs.len() - 1;
        }
    }

    /// The settings tab: the form in a centred column, scrolling as a whole.
    fn settings_page(&mut self, ui: &mut Ui) {
        const WIDTH: f32 = 640.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let side = ((ui.available_width() - WIDTH) / 2.0).max(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(WIDTH.min(ui.available_width() - 24.0));
                        ui.add_space(32.0);
                        ui.label(RichText::new("Settings").font(theme::semibold(22.0)));
                        ui.add_space(4.0);
                        if self.settings.ui(ui) {
                            self.settings.set(ui.ctx());
                        }
                        ui.add_space(32.0);
                    });
                });
            });
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
    use super::{Saved, active_among_kept, fuzzy, window_icon};
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

    /// A tab whose folder is gone doesn't make the one after the active tab active.
    #[test]
    fn active_tab_survives_a_gone_folder() {
        assert_eq!(active_among_kept(&[true, true, true], 2), 2);
        assert_eq!(active_among_kept(&[false, true, true], 2), 1);
        assert_eq!(active_among_kept(&[true, false, true], 2), 1);
        assert_eq!(active_among_kept(&[true, true, false], 1), 1);
        assert_eq!(active_among_kept(&[true, false, true], 1), 0);
        assert_eq!(active_among_kept(&[false, true], 0), 0);
        assert_eq!(active_among_kept(&[], 3), 0);
        assert_eq!(active_among_kept(&[true, true], usize::MAX), 1);
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
