//! The tab strip's chip for a linked worktree, the switcher it opens, and how a worktree with
//! uncommitted changes is marked.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align2, Galley, Rect, Response, Sense, Ui, pos2, vec2};

use super::{TabLabel, fit_branch, picker_entries};
use crate::git::Worktree;
use crate::icons;
use crate::repo::RepoTab;
use crate::theme::{self, Tokens};

pub(crate) const SWITCHER_W: f32 = 340.0;
const SWITCHER_ROW_H: f32 = 26.0;
const SWITCHER_LIST_H: f32 = 340.0;

/// The chip after a tab's name while it is on a linked worktree: the branch, then the button
/// that closes the worktree.
#[derive(Clone)]
pub(crate) struct Chip {
    branch: Arc<Galley>,
    pub full_name: String,
    pub width: f32,
}

/// Width of the end of the chip with the button that closes the worktree.
pub(crate) const CHIP_CLOSE_W: f32 = 20.0;

/// The chip for `label`, if it has one: the main worktree has none. It is at most `max_w` wide,
/// however wide the branch's characters are.
pub(crate) fn chip_for(ui: &Ui, label: &TabLabel, max_w: f32) -> Option<Chip> {
    let name = label.branch.as_deref()?;
    let t = Tokens::get(ui.ctx());
    let layout = |text: String| {
        ui.painter()
            .layout_no_wrap(text, egui::FontId::proportional(11.5), t.text_dim)
    };
    let short = fit_branch(name, max_w - 22.0 - CHIP_CLOSE_W, |text| {
        layout(text.to_owned()).size().x
    });
    let branch = layout(short);
    let width = branch.size().x + 22.0 + CHIP_CLOSE_W;
    Some(Chip {
        branch,
        full_name: name.to_owned(),
        width,
    })
}

/// Draws the chip at `rect`. Only the active tab's chip reacts: `hovered` is about that.
pub(crate) fn paint_chip(ui: &Ui, rect: Rect, chip: &Chip, active: bool, hovered: bool) {
    let t = Tokens::get(ui.ctx());
    let fill = if hovered {
        t.pressed
    } else if active {
        t.field
    } else {
        t.hover.gamma_multiply(0.5)
    };
    ui.painter().rect_filled(rect, t.radius_sm, fill);
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 12.0, y), vec2(12.0, 12.0)),
        "git-branch",
        11.0,
        t.text_faint,
    );
    ui.painter().galley(
        pos2(rect.left() + 21.0, y - chip.branch.size().y / 2.0),
        chip.branch.clone(),
        t.text_dim,
    );
}

/// The switcher's contents: a search field and the worktrees. Returns the one picked.
pub(crate) fn switcher(
    ui: &mut Ui,
    tab: &RepoTab,
    query: &mut String,
    wants_focus: &mut bool,
) -> Option<PathBuf> {
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
    let field = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(query)
                    .hint_text("Search worktrees")
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(8, 4)),
            )
        })
        .inner;
    if *wants_focus {
        field.request_focus();
        *wants_focus = false;
    }
    let background = tab.background_work();
    let entries = picker_entries(&tab.worktrees, background, query);
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        picked = entries.iter().find(|w| !w.current).map(|w| w.path.clone());
    }
    if entries.is_empty() {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("No worktree matches").color(t.text_dim));
            });
        return picked;
    }
    egui::ScrollArea::vertical()
        .id_salt("worktree-switcher")
        .max_height(SWITCHER_LIST_H)
        .auto_shrink([false, true])
        .show_rows(ui, SWITCHER_ROW_H, entries.len(), |ui, rows| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for wt in &entries[rows] {
                let work = background
                    .iter()
                    .find(|(path, _)| *path == wt.path)
                    .map(|(_, work)| *work);
                if switcher_row(ui, wt, work).clicked() && !wt.current {
                    picked = Some(wt.path.clone());
                }
            }
        });
    picked
}

/// How many files with uncommitted changes `wt` has, when it has any and isn't the worktree shown,
/// whose changes are in the list on the right already.
pub(in crate::repo) fn changes_elsewhere(wt: &Worktree) -> Option<usize> {
    wt.changes.filter(|&n| n > 0 && !wt.current)
}

/// The dot that stands for a worktree's uncommitted changes, in place of its icon.
pub(in crate::repo) fn paint_dirty(painter: &egui::Painter, t: &Tokens, center: egui::Pos2) {
    painter.circle_filled(center, 4.0, t.warning);
}

/// What the hover text of a worktree says about its uncommitted changes, if it has any.
pub(in crate::repo) fn changes_line(wt: &Worktree) -> Option<String> {
    match changes_elsewhere(wt)? {
        1 => Some("1 file with uncommitted changes".to_owned()),
        n => Some(format!("{n} files with uncommitted changes")),
    }
}

fn switcher_row(ui: &mut Ui, wt: &Worktree, work: Option<&str>) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), SWITCHER_ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    if wt.current {
        icons::paint(
            ui,
            Rect::from_center_size(pos2(rect.left() + 18.0, y), vec2(14.0, 14.0)),
            "check",
            13.0,
            t.accent,
        );
    } else if changes_elsewhere(wt).is_some() {
        paint_dirty(ui.painter(), &t, pos2(rect.left() + 18.0, y));
    }
    let mut right = rect.right() - 10.0;
    let tag = work.or(wt.main.then_some("main"));
    if let Some(tag) = tag {
        let color = if work.is_some() {
            t.accent
        } else {
            t.text_faint
        };
        let text = ui.painter().text(
            pos2(right, y),
            Align2::RIGHT_CENTER,
            tag,
            theme::medium(11.0),
            color,
        );
        right = text.left() - 8.0;
        if work.is_some() {
            ui.painter()
                .circle_filled(pos2(right + 2.0, y), 3.0, t.accent);
            right -= 6.0;
        }
    }
    let name = wt
        .branch
        .clone()
        .unwrap_or_else(|| format!("{} ({})", wt.name(), wt.head.get(..7).unwrap_or(&wt.head)));
    let font = if wt.current {
        theme::semibold(12.5)
    } else {
        egui::FontId::proportional(12.5)
    };
    ui.painter().with_clip_rect(rect.with_max_x(right)).text(
        pos2(rect.left() + 32.0, y),
        Align2::LEFT_CENTER,
        name,
        font,
        t.text,
    );
    let mut tip = wt.path.display().to_string();
    if let Some(line) = changes_line(wt) {
        tip.push_str(&format!("\n{line}"));
    }
    resp.on_hover_text(tip)
}
