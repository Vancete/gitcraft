//! One tab on the strip: its icon, name, worktree chip, badge and close button.

use egui::{Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::{BADGE_W, Placed, TabDrag, TabEnv};
use crate::app::Tab;
use crate::icons;
use crate::repo::RepoTab;
use crate::repo::worktree_tab::{self, Chip};
use crate::tab_look::{self, TabLook};
use crate::theme::{self, Tokens};

/// Draws one tab and records what was done to it: selecting, closing, dragging, its look.
pub(super) fn tab_ui(ui: &mut Ui, placed: Placed<'_>, env: &mut TabEnv<'_>) {
    let Placed {
        index: i,
        tab,
        title,
        rect,
        id,
    } = placed;
    let t = env.t;
    let tab_looks = env.tab_looks;
    let ctx = ui.ctx().clone();
    let active = i == env.active;
    let (tooltip, busy) = match tab {
        Tab::Home => ("Open, create or clone a repository".to_owned(), false),
        Tab::Repo(r) => (r.path.display().to_string(), r.is_busy()),
        Tab::Settings => ("Settings".to_owned(), false),
    };
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let hovered = resp.hovered() && env.dragging.is_none();
    let default_look = TabLook::default();
    let look = match tab {
        Tab::Repo(r) => tab_looks.get(&r.path).unwrap_or(&default_look),
        Tab::Home | Tab::Settings => &default_look,
    };
    let accent = look.color.color(t);
    let p = ui.painter();
    if env.dragging == Some(i) && !active {
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
    let background = title.background;
    let badge_w = if background > 0 { BADGE_W } else { 0.0 };
    let chip_w = title.chip.as_ref().map_or(0.0, |c| c.width);
    let right = rect.right() - 28.0;
    let chip_left =
        (rect.left() + 32.0 + title.galley.size().x + 6.0).min(right - chip_w - badge_w);
    let text_right = if title.chip.is_some() || background > 0 {
        chip_left - 4.0
    } else {
        right
    };
    let text_clip = Rect::from_min_max(
        pos2(rect.left() + 32.0, rect.top()),
        pos2(text_right, rect.bottom()),
    );
    ui.painter().with_clip_rect(text_clip).galley(
        pos2(text_clip.left(), y - title.galley.size().y / 2.0),
        title.galley.clone(),
        title.color,
    );

    if let Some(chip) = &title.chip {
        let chip_rect = Rect::from_min_size(pos2(chip_left, y - 10.0), vec2(chip.width, 20.0));
        if let (true, Tab::Repo(r)) = (active, tab) {
            active_chip(ui, chip_rect, y, chip, r, id, env);
        } else {
            worktree_tab::paint_chip(ui, chip_rect, chip, false, false);
        }
    }
    if background > 0 {
        badge(ui, pos2(chip_left + chip_w + 7.0, y), background, id, t);
    }

    let close_rect = Rect::from_center_size(pos2(rect.right() - 15.0, y), vec2(18.0, 18.0));
    if busy {
        egui::Spinner::new().size(11.0).paint_at(ui, close_rect);
    } else if (active || hovered) && close_button(ui, close_rect, resp.id, t, env.dragging) {
        env.actions.close = Some(i);
    }
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
    {
        env.actions.drag = Some(TabDrag {
            from: i,
            grab: origin.x - rect.left(),
        });
        env.actions.select = Some(i);
    }
    if let Tab::Repo(r) = tab {
        egui::Popup::context_menu(&resp)
            .style(theme::menu_style)
            .show(|ui| {
                if let Some(new) = tab_look::menu(ui, look) {
                    env.actions.restyle = Some((r.path.clone(), new));
                }
            });
    }
    if env.dragging.is_none() {
        resp.clone().on_hover_text(tooltip);
    }
    if resp.clicked() && env.actions.close.is_none() {
        env.actions.select = Some(i);
    }
    if resp.middle_clicked() {
        env.actions.close = Some(i);
    }
}

/// The active tab's worktree chip: it opens the switcher, its end closes the worktree.
fn active_chip(
    ui: &mut Ui,
    chip_rect: Rect,
    y: f32,
    chip: &Chip,
    r: &RepoTab,
    id: egui::Id,
    env: &mut TabEnv<'_>,
) {
    let t = env.t;
    let body = chip_rect.with_max_x(chip_rect.right() - worktree_tab::CHIP_CLOSE_W);
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
        env.actions.close_worktree = true;
    }
    x.on_hover_text("Close this worktree and go back to the main one. It stays on disk.");
    if c.clicked() {
        env.switcher_query.clear();
        *env.switcher_focus = true;
    }
    let popup = egui::Popup::menu(&c)
        .style(theme::menu_style)
        .width(worktree_tab::SWITCHER_W)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    popup.show(|ui| {
        let picked = worktree_tab::switcher(ui, r, env.switcher_query, env.switcher_focus);
        if picked.is_some() {
            env.actions.switch_to = picked;
            ui.close();
        }
    });
    c.on_hover_text(format!("{}\nSwitch worktree", chip.full_name));
}

/// The dot and count of worktrees with work in the background, at `at`.
fn badge(ui: &mut Ui, at: Pos2, background: usize, id: egui::Id, t: &Tokens) {
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

/// The close button over a tab; true when clicked.
fn close_button(
    ui: &mut Ui,
    rect: Rect,
    tab_id: egui::Id,
    t: &Tokens,
    dragging: Option<usize>,
) -> bool {
    let c = ui.interact(rect, tab_id.with("close"), Sense::click());
    if c.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.pressed);
    }
    icons::paint(
        ui,
        rect,
        "x",
        12.0,
        if c.hovered() { t.text } else { t.text_faint },
    );
    let clicked = c.clicked();
    if dragging.is_none() {
        c.on_hover_text("Close tab (Ctrl+W)");
    }
    clicked
}
