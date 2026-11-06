//! The tab strip: document tabs that drag to reorder, `+`, and the buttons at its end.

mod tab;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Galley, Rect, RichText, Sense, Ui, pos2, vec2};

use super::{App, Tab};
use crate::icons;
use crate::repo::worktree_tab::{self, Chip};
use crate::tab_look::TabLook;
use crate::theme::{self, ThemeKind, Tokens};
use crate::update;

const TAB_H: f32 = 40.0;
/// Space between tabs.
const GAP: f32 = 1.0;
/// Seconds tabs take to slide into their place.
const TAB_SLIDE: f32 = 0.15;
/// Wide enough for a repository, its branch's chip and the badge.
const MAX_TAB_W: f32 = 380.0;
/// The badge counting a tab's worktrees with work in the background.
const BADGE_W: f32 = 22.0;
/// The widest a worktree chip gets: what a tab at its widest leaves next to its icon, its close
/// button and the badge.
const MAX_CHIP_W: f32 = MAX_TAB_W - 32.0 - 28.0 - BADGE_W;

/// A tab being dragged along the strip.
#[derive(Clone, Copy)]
pub(super) struct TabDrag {
    /// Its index when the drag started.
    from: usize,
    /// Where the pointer holds it, from its left edge.
    grab: f32,
}

/// What was done on the strip this frame, applied once it is drawn.
#[derive(Default)]
struct StripActions {
    select: Option<usize>,
    close: Option<usize>,
    add: bool,
    restyle: Option<(PathBuf, TabLook)>,
    /// A dragged tab dropped: (from, to).
    moved: Option<(usize, usize)>,
    drag: Option<TabDrag>,
    /// The worktree picked in the switcher.
    switch_to: Option<PathBuf>,
    /// The active tab's worktree is to be closed.
    close_worktree: bool,
    /// The tab closed last is to be opened again.
    reopen: bool,
}

/// A tab's name, laid out, its colour, the tab's width, its worktree chip and how many of its
/// worktrees have work in the background.
struct Title {
    galley: Arc<Galley>,
    color: Color32,
    width: f32,
    chip: Option<Chip>,
    background: usize,
}

/// One tab where it is drawn this frame.
struct Placed<'a> {
    index: usize,
    tab: &'a Tab,
    title: &'a Title,
    rect: Rect,
    id: egui::Id,
}

/// What drawing a tab needs besides the tab: the app's state and what the strip collects.
struct TabEnv<'a> {
    active: usize,
    /// The tab being dragged, if any.
    dragging: Option<usize>,
    /// A closed tab can be opened again.
    can_reopen: bool,
    tab_looks: &'a BTreeMap<PathBuf, TabLook>,
    switcher_query: &'a mut String,
    switcher_focus: &'a mut bool,
    t: &'a Tokens,
    actions: &'a mut StripActions,
}

impl App {
    /// PhotoCraft-style document tabs on a dark strip, `+` after them, the theme menu at the end.
    /// A tab can be dragged along the strip, the others sliding out of its way.
    pub(super) fn tab_strip(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let mut actions = StripActions::default();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            self.tabs_row(ui, &t, &mut actions);
            ui.add_space(4.0);
            if icons::small_button(ui, "plus", "New tab (Ctrl+T)").clicked() {
                actions.add = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.bar_buttons(ui, &t);
            });
        });
        self.apply_strip_actions(actions, ui.ctx());
    }

    /// The tabs in their places, the dragged one following the pointer over the others.
    fn tabs_row(&mut self, ui: &mut Ui, t: &Tokens, actions: &mut StripActions) {
        let ctx = ui.ctx().clone();
        let n = self.tabs.len();
        let titles = self.titles(ui, t);
        let width = |i: usize| titles.get(i).map_or(0.0, |g| g.width);
        let total: f32 = titles.iter().map(|g| g.width + GAP).sum::<f32>() - GAP;
        let (strip, _) = ui.allocate_exact_size(vec2(total.max(0.0), TAB_H), Sense::hover());

        // The dragged tab follows the pointer; the slot it would drop into is where its
        // middle is among the others.
        let dragged = self.tab_drag.filter(|d| d.from < n).and_then(|d| {
            let x = ctx.pointer_interact_pos()?.x - d.grab;
            let max = (strip.right() - width(d.from)).max(strip.left());
            Some((d.from, x.clamp(strip.left(), max)))
        });
        let (place, slot) = places(strip.left(), n, width, dragged);
        if let (Some((from, _)), Some(slot)) = (dragged, slot)
            && !ctx.input(|i| i.pointer.primary_down())
        {
            actions.moved = Some((from, slot));
        }

        // The dragged tab is drawn last, over the others.
        let dragging = dragged.map(|(from, _)| from);
        let paint_order = (0..n).filter(|&j| Some(j) != dragging).chain(dragging);
        let ids = self.tab_ids();
        let mut env = TabEnv {
            active: self.active,
            dragging,
            can_reopen: !self.closed.is_empty(),
            tab_looks: &self.tab_looks,
            switcher_query: &mut self.switcher_query,
            switcher_focus: &mut self.switcher_focus,
            t,
            actions,
        };
        for i in paint_order {
            let (Some(tab), Some(title), Some(&id)) = (self.tabs.get(i), titles.get(i), ids.get(i))
            else {
                continue;
            };
            let x = match dragged {
                Some((from, x)) if from == i => ctx.animate_value_with_time(id.with("x"), x, 0.0),
                _ => ctx.animate_value_with_time(
                    id.with("x"),
                    place.get(i).copied().unwrap_or(strip.left()),
                    TAB_SLIDE,
                ),
            };
            let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(title.width, TAB_H));
            let placed = Placed {
                index: i,
                tab,
                title,
                rect,
                id,
            };
            tab::tab_ui(ui, placed, &mut env);
        }
        if let Some(drag) = env.actions.drag.take() {
            self.tab_drag = Some(drag);
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.tab_drag = None;
        }
    }

    /// Each tab's title and width.
    fn titles(&self, ui: &Ui, t: &Tokens) -> Vec<Title> {
        self.tabs
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
                let width = (galley.size().x + 66.0 + extras).clamp(110.0, MAX_TAB_W);
                Title {
                    galley,
                    color,
                    width,
                    chip,
                    background,
                }
            })
            .collect()
    }

    /// Ids that stay with the tabs as they move.
    fn tab_ids(&self) -> Vec<egui::Id> {
        let mut homes = 0;
        self.tabs
            .iter()
            .map(|tab| match tab {
                Tab::Repo(r) => egui::Id::new(("tab", &r.path)),
                Tab::Home => {
                    homes += 1;
                    egui::Id::new(("tab-home", homes))
                }
                Tab::Settings => egui::Id::new("tab-settings"),
            })
            .collect()
    }

    /// Settings, command palette, theme menu and manual, the update pill first in the row.
    fn bar_buttons(&mut self, ui: &mut Ui, t: &Tokens) {
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
            .style(theme::menu_style)
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
        if icons::bar_button(ui, "book-open", "Manual and keyboard shortcuts (F1)").clicked() {
            self.manual_open = true;
        }
        // A newer release: first in the row, before the icons.
        if let Some(release) = self.update.clone() {
            ui.add_space(4.0);
            self.update_pill(ui, t, &release);
        }
    }

    /// "vX available", with a menu to open the release page or skip that version.
    fn update_pill(&mut self, ui: &mut Ui, t: &Tokens, release: &update::Release) {
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
            .style(theme::menu_style)
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

    fn apply_strip_actions(&mut self, actions: StripActions, ctx: &egui::Context) {
        if let Some(i) = actions.select {
            self.active = i;
        }
        // A tab closed as another is dropped: the indices are from before either, so the drop
        // is left out rather than moving the wrong tab or closing one that moved.
        if let Some((from, to)) = actions.moved
            && actions.close.is_none()
        {
            self.move_tab(from, to);
        }
        if let Some((path, look)) = actions.restyle {
            if look == TabLook::default() {
                self.tab_looks.remove(&path);
            } else {
                self.tab_looks.insert(path, look);
            }
        }
        if let Some(i) = actions.close {
            self.close_tab(i, ctx);
        }
        if actions.add {
            self.new_tab();
        }
        if actions.reopen {
            self.reopen_closed_tab(ctx);
        }
        // Opened and closed after the strip is drawn.
        if actions.switch_to.is_some() {
            self.switch_to = actions.switch_to;
        }
        if actions.close_worktree {
            self.close_worktree = true;
        }
    }
}

/// Each tab's left edge from `left` on, the dragged one (index, left edge) moved to the slot
/// its middle is over, and that slot.
fn places(
    left: f32,
    n: usize,
    width: impl Fn(usize) -> f32,
    dragged: Option<(usize, f32)>,
) -> (Vec<f32>, Option<usize>) {
    let mut order: Vec<usize> = (0..n).collect();
    let mut slot = None;
    if let Some((from, x)) = dragged {
        let middle = x + width(from) / 2.0;
        let mut edge = left;
        let mut s = 0;
        for j in (0..n).filter(|&j| j != from) {
            if edge + width(j) / 2.0 < middle {
                s += 1;
            }
            edge += width(j) + GAP;
        }
        order.retain(|&j| j != from);
        order.insert(s, from);
        slot = Some(s);
    }
    let mut place = vec![left; n];
    let mut x = left;
    for &j in &order {
        if let Some(p) = place.get_mut(j) {
            *p = x;
        }
        x += width(j) + GAP;
    }
    (place, slot)
}
