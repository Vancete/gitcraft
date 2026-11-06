//! The terminal panel's tab strip.

use egui::{Rect, Sense, Ui, pos2, vec2};

use super::top_rounded;
use crate::icons;
use crate::repo::Action;
use crate::terminal::Terminal;
use crate::theme::{self, Tokens};

/// The terminal panel's strip: one tab per shell, `+` with a shell menu, and close.
pub fn terminal_tabs(ui: &mut Ui, terminals: &[Terminal], active: usize, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(strip, 0.0, t.tab_strip);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(strip.shrink2(vec2(4.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Max)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 2.0;
    for (i, term) in terminals.iter().enumerate() {
        terminal_tab(ui, &t, i, term, i == active, acts);
    }
    ui.add_space(4.0);
    let plus = icons::small_button(ui, "plus", "New terminal");
    if plus.clicked() {
        acts.push(Action::NewTerminal {
            shell: None,
            cwd: None,
        });
    }
    egui::Popup::context_menu(&plus)
        .style(crate::theme::menu_style)
        .show(|ui| shell_items(ui, acts));
    let more = icons::small_button(ui, "chevron-down", "New terminal with…");
    egui::Popup::menu(&more)
        .style(crate::theme::menu_style)
        .show(|ui| shell_items(ui, acts));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        if icons::small_button(ui, "x", "Hide the panel (shells keep running)").clicked() {
            acts.push(Action::ToggleTerminal);
        }
    });
}

/// The tab of shell `i`, with its close button.
fn terminal_tab(
    ui: &mut Ui,
    t: &Tokens,
    i: usize,
    term: &Terminal,
    selected: bool,
    acts: &mut Vec<Action>,
) {
    let label = format!("{} {}", i + 1, term.shell);
    let font = if selected {
        theme::semibold(11.5)
    } else {
        theme::medium(11.5)
    };
    let galley =
        ui.painter()
            .layout_no_wrap(label, font, if selected { t.text } else { t.text_dim });
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 44.0, 26.0), Sense::click());
    let shape = top_rounded(t.radius as u8);
    if selected {
        ui.painter().rect_filled(rect, shape, t.card);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, shape, t.hover);
    }
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(14.0, 14.0)),
        "square-terminal",
        12.0,
        t.text_faint,
    );
    ui.painter().galley(
        pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0),
        galley,
        t.text,
    );
    let x_rect =
        Rect::from_center_size(pos2(rect.right() - 11.0, rect.center().y), vec2(16.0, 16.0));
    let x = ui
        .interact(x_rect, resp.id.with("close"), Sense::click())
        .on_hover_text("Close (ends the shell)");
    if x.hovered() {
        ui.painter().rect_filled(x_rect, t.radius_sm, t.pressed);
    }
    icons::paint(ui, x_rect, "x", 11.0, t.text_faint);
    if x.clicked() {
        acts.push(Action::CloseTerminal(i));
    } else if resp.clicked() {
        acts.push(Action::SelectTerminal(i));
    }
}

/// One entry per shell found on this computer, each opening a new terminal with it.
fn shell_items(ui: &mut Ui, acts: &mut Vec<Action>) {
    for shell in crate::terminal::available_shells() {
        if ui.button(&shell.label).clicked() {
            acts.push(Action::NewTerminal {
                shell: Some(shell),
                cwd: None,
            });
        }
    }
}
