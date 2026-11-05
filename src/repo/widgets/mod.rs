//! Small painted widgets shared by the repository panes.

mod file_menu;
mod file_row;
mod terminal_tabs;

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Ui, pos2, vec2};

use crate::theme::{self, Tokens};

pub use file_menu::file_menu;
pub use file_row::{RowExtra, file_row};
pub use terminal_tabs::terminal_tabs;

pub const ROW_H: f32 = 24.0;

pub fn toolbar_separator(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t.separator);
    ui.add_space(4.0);
}

/// Width of a folded side panel: its footer's toggle and the footer's margins.
pub const FOLDED_W: f32 = 36.0;
/// Height of [`panel_header`].
pub const PANEL_HEADER_H: f32 = 30.0;

/// How far a side panel whose outer edge belongs at `outer_edge` (the right one for a panel
/// on the right) is slid out of place. A panel folding or opening slides toward its outer
/// edge, where the part still in view is: what is moved back by this stays put there instead
/// of sliding out of sight and back.
pub fn slide_offset(ui: &Ui, outer_edge: f32, right: bool) -> f32 {
    let panel = ui.max_rect();
    if right {
        outer_edge - panel.right()
    } else {
        outer_edge - panel.left()
    }
}

/// The row of buttons along the bottom of a side panel, starting from its outer edge (the
/// right one for a panel on the right), which lies at `outer_edge`. Call it before the
/// panel's other contents.
pub fn panel_footer(
    ui: &mut Ui,
    id: &'static str,
    outer_edge: f32,
    right: bool,
    add_buttons: impl FnOnce(&mut Ui),
) {
    let t = Tokens::get(ui.ctx());
    let slid = slide_offset(ui, outer_edge, right);
    egui::Panel::bottom(id)
        .frame(
            egui::Frame::new()
                .fill(t.tab_strip)
                .inner_margin(egui::Margin::same(4)),
        )
        .show(ui, |ui| {
            let layout = if right {
                egui::Layout::right_to_left(egui::Align::Center)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            };
            // A child the parent doesn't grow by: the slid rect lies partly outside the panel.
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(ui.max_rect().translate(vec2(slid, 0.0)))
                    .layout(layout),
            );
            row.spacing_mut().item_spacing.x = 2.0;
            add_buttons(&mut row);
            ui.allocate_exact_size(vec2(0.0, row.min_rect().height()), Sense::hover());
        });
}

/// A rounded label with a tinted background.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = theme::medium(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = vec2(galley.size().x + 12.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.18));
    ui.painter().galley(
        pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// A tab's shape: rounded at the top only.
fn top_rounded(radius: u8) -> CornerRadius {
    CornerRadius {
        nw: radius,
        ne: radius,
        sw: 0,
        se: 0,
    }
}

/// The strip at the top of a side panel, like PhotoCraft's panel tab strips: a tab with the
/// title, and room on the right for buttons.
pub fn panel_header(ui: &mut Ui, title: &str, add_buttons: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), PANEL_HEADER_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, t.tab_strip);
    let font = theme::semibold(11.5);
    let galley = painter.layout_no_wrap(title.to_owned(), font, t.text);
    let tab = Rect::from_min_size(
        pos2(rect.left() + 4.0, rect.top() + 4.0),
        vec2(galley.size().x + 24.0, rect.height() - 4.0),
    );
    painter.rect_filled(tab, top_rounded(t.radius as u8), t.card);
    painter.galley(
        pos2(tab.left() + 12.0, tab.center().y - galley.size().y / 2.0),
        galley,
        t.text,
    );
    let buttons = Rect::from_min_max(
        pos2(tab.right() + 4.0, rect.top()),
        rect.max - vec2(6.0, 0.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(buttons)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    add_buttons(&mut child);
}

pub fn status_color(t: &Tokens, code: char) -> Color32 {
    match code {
        'A' | '?' => t.success,
        'D' => t.danger,
        'R' | 'C' => t.accent,
        'U' => t.danger,
        _ => t.warning,
    }
}

/// A primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let fill = if enabled { t.accent } else { t.hover };
    let text_color = if enabled { t.accent_text } else { t.text_faint };
    ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(text)
                .color(text_color)
                .font(theme::medium(12.5)),
        )
        .fill(fill)
        .min_size(vec2(width, 30.0)),
    )
}

/// A round avatar: the author's picture when one is loaded (see `avatars`), their initials in
/// a colour of their own until then.
pub fn avatar(ui: &Ui, center: egui::Pos2, radius: f32, name: &str, email: &str) {
    let rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
    if crate::avatars::paint(ui, rect, email) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter();
    painter.circle_filled(center, radius, t.lane(name_hash(name)));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initials(name),
        theme::semibold(radius * 0.8),
        Color32::WHITE,
    );
}

fn initials(name: &str) -> String {
    let mut parts = name.split_whitespace().filter_map(|w| w.chars().next());
    let first = parts.next().unwrap_or('?');
    let last = parts.next_back();
    let mut s: String = first.to_uppercase().collect();
    if let Some(l) = last {
        s.extend(l.to_uppercase());
    }
    s
}

fn name_hash(name: &str) -> usize {
    name.bytes()
        .fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}

/// Labels of which one is on, like tabs. Returns the one clicked when it wasn't on.
pub fn switch(ui: &mut Ui, labels: &[&str], on: usize) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let mut order: Vec<usize> = (0..labels.len()).collect();
    // Laid out right to left, the first label goes last.
    if ui.layout().prefer_right_to_left() {
        order.reverse();
    }
    let mut picked = None;
    for i in order {
        let active = i == on;
        let color = if active { t.text } else { t.text_dim };
        let b = ui.add(
            egui::Button::new(egui::RichText::new(labels[i]).color(color))
                .frame_when_inactive(active),
        );
        if active {
            active_underline(ui, b.rect, t.accent);
        }
        if b.clicked() && !active {
            picked = Some(i);
        }
    }
    picked
}

/// The accent line under an active toolbar toggle.
pub fn active_underline(ui: &Ui, button: Rect, color: Color32) {
    ui.painter().line_segment(
        [
            button.left_bottom() + vec2(6.0, -1.0),
            button.right_bottom() + vec2(-6.0, -1.0),
        ],
        egui::Stroke::new(2.0, color),
    );
}
