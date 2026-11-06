//! The sidebar's section headers, which never leave the pane.
//!
//! A header scrolled out of view stops at the edge, on top of the ones already stopped there,
//! and becomes a jump mark: clicking it scrolls its section up to the top of the pane.
//!
//! While the content is laid out, a header only reserves its place. The headers are drawn over
//! the content afterwards, when it is known where they all are.

use egui::{Align2, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::Action;
use crate::icons;
use crate::theme::{self, Tokens};

const HEADER_H: f32 = 26.0;

/// The icon button on a header.
pub(super) struct Button {
    pub icon: &'static str,
    pub tooltip: &'static str,
    /// What it does; `None` while that is not possible.
    pub action: Option<Action>,
}

/// A header: `▸ ICON TITLE  count`.
pub(super) struct Header {
    pub key: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    pub count: usize,
    pub default_open: bool,
    pub button: Option<Button>,
}

struct Placed {
    id: Id,
    header: Header,
    /// Its place in the content, on screen.
    natural: Rect,
    open: bool,
}

/// The headers of one pane, in order.
pub(super) struct Sections {
    /// Identifies the pane; their open state is saved under this.
    id: Id,
    placed: Vec<Placed>,
    /// The key of a section to open and scroll to, as its jump mark does.
    reveal: Option<&'static str>,
}

impl Sections {
    pub fn new(id: Id, reveal: Option<&'static str>) -> Self {
        Self {
            id,
            placed: Vec::new(),
            reveal,
        }
    }

    /// Reserves the header's place in the content. Returns whether its section is open.
    pub fn add(&mut self, ui: &mut Ui, header: Header) -> bool {
        let id = self.id.with(header.key);
        let (_, natural) = ui.allocate_space(vec2(ui.available_width(), HEADER_H));
        if self.reveal == Some(header.key) {
            ui.data_mut(|d| d.insert_persisted(id, true));
        }
        let open = ui
            .data_mut(|d| d.get_persisted::<bool>(id))
            .unwrap_or(header.default_open);
        self.placed.push(Placed {
            id,
            header,
            natural,
            open,
        });
        open
    }

    /// Draws the headers over the content of a pane scrolled to `offset`. Returns the offset to
    /// scroll to, when a jump mark was clicked.
    pub fn show(self, ui: &mut Ui, pane: Rect, offset: f32, acts: &mut Vec<Action>) -> Option<f32> {
        let count = self.placed.len();
        let mut jump = None;
        for (index, mut placed) in self.placed.into_iter().enumerate() {
            let natural = placed.natural.top();
            let y = pinned_y(natural, index, count, pane.top(), pane.bottom(), HEADER_H);
            let rect = Rect::from_min_size(pos2(placed.natural.left(), y), placed.natural.size());
            if self.reveal == Some(placed.header.key) {
                jump = Some(jump_offset(natural, index, pane.top(), offset, HEADER_H));
            }
            let clicked = header_row(ui, rect, natural, &mut placed, acts);
            if clicked {
                let is_pinned = (y - natural).abs() > 0.5;
                // A jump mark opens its section too: it is there to get to what is in it.
                let open = is_pinned || !placed.open;
                ui.data_mut(|d| d.insert_persisted(placed.id, open));
                if is_pinned {
                    jump = Some(jump_offset(natural, index, pane.top(), offset, HEADER_H));
                }
                ui.ctx().request_repaint();
            }
        }
        jump
    }
}

/// Draws a header at `rect`. Returns whether it was clicked, other than on its button.
fn header_row(
    ui: &mut Ui,
    rect: Rect,
    natural: f32,
    placed: &mut Placed,
    acts: &mut Vec<Action>,
) -> bool {
    let t = Tokens::get(ui.ctx());
    let header = &placed.header;
    let resp = ui.interact(rect, placed.id, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, t.card);
    if resp.hovered() {
        painter.rect_filled(rect, 0.0, t.hover);
    }
    // A line on the side the content passes under.
    let separator = Stroke::new(1.0, t.separator);
    if rect.top() > natural + 0.5 {
        painter.hline(rect.x_range(), rect.bottom(), separator);
    } else if rect.top() < natural - 0.5 {
        painter.hline(rect.x_range(), rect.top(), separator);
    }
    let y = rect.center().y;
    let chevron = if placed.open {
        "chevron-down"
    } else {
        "chevron-right"
    };
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 14.0, y), vec2(14.0, 14.0)),
        chevron,
        12.0,
        t.text_faint,
    );
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 32.0, y), vec2(14.0, 14.0)),
        header.icon,
        14.0,
        t.icon,
    );
    let painter = ui.painter();
    painter.text(
        pos2(rect.left() + 46.0, y),
        Align2::LEFT_CENTER,
        header.title,
        theme::semibold(11.5),
        t.text_dim,
    );
    painter.text(
        pos2(rect.right() - 10.0, y),
        Align2::RIGHT_CENTER,
        header.count.to_string(),
        theme::medium(11.0),
        t.text_faint,
    );
    if let Some(button) = placed.header.button.as_mut() {
        let r = Rect::from_center_size(pos2(rect.right() - 36.0, y), vec2(20.0, 20.0));
        let b = ui
            .interact(r, placed.id.with("button"), Sense::click())
            .on_hover_text(button.tooltip);
        if b.hovered() {
            ui.painter().rect_filled(r, t.radius_sm, t.pressed);
        }
        icons::paint(
            ui,
            r,
            button.icon,
            13.0,
            if b.hovered() { t.text } else { t.icon },
        );
        if b.clicked()
            && let Some(action) = button.action.take()
        {
            acts.push(action);
        }
    }
    resp.clicked()
}

/// Where to show the header number `index` of `count`, whose place in the scrolled content is
/// at `natural`, in a pane from `top` to `bottom`.
pub(super) fn pinned_y(
    natural: f32,
    index: usize,
    count: usize,
    top: f32,
    bottom: f32,
    height: f32,
) -> f32 {
    let highest = top + index as f32 * height;
    let lowest = bottom - (count - index) as f32 * height;
    // In this order, so that a pane too short for all of them fills from the top.
    natural.min(lowest).max(highest)
}

/// The scroll offset that puts the header number `index` right under the headers above it.
/// `natural` is its place on screen with the content at `offset`, in a pane starting at `top`.
pub(super) fn jump_offset(natural: f32, index: usize, top: f32, offset: f32, height: f32) -> f32 {
    (natural - top + offset - index as f32 * height).max(0.0)
}

/// The room to leave in a pane `pane_h` high so that content `content_h` high, without that
/// room, ends at its bottom edge. None once the content does not fit, or is not measured yet.
pub(super) fn bottom_gap(pane_h: f32, content_h: f32) -> f32 {
    (pane_h - content_h).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f32 = 26.0;
    const TOP: f32 = 100.0;
    const BOTTOM: f32 = 500.0;

    fn pinned(natural: f32, index: usize) -> f32 {
        pinned_y(natural, index, 3, TOP, BOTTOM, H)
    }

    #[test]
    fn a_header_in_view_stays_at_its_place() {
        assert_eq!(pinned(250.0, 1), 250.0);
    }

    #[test]
    fn a_header_above_the_pane_stops_at_the_top() {
        assert_eq!(pinned(-900.0, 0), TOP);
    }

    #[test]
    fn headers_above_the_pane_stack_in_order() {
        assert_eq!(pinned(-900.0, 1), TOP + H);
        assert_eq!(pinned(-800.0, 2), TOP + 2.0 * H);
    }

    #[test]
    fn a_header_below_the_pane_stops_at_the_bottom() {
        assert_eq!(pinned(2000.0, 2), BOTTOM - H);
    }

    #[test]
    fn headers_below_the_pane_stack_in_order() {
        assert_eq!(pinned(2000.0, 1), BOTTOM - 2.0 * H);
        assert_eq!(pinned(3000.0, 0), BOTTOM - 3.0 * H);
    }

    #[test]
    fn a_header_stops_where_the_stack_begins_not_at_the_edge() {
        // Header 1 has header 0 above it, so it stops one header below the top.
        assert_eq!(pinned(TOP + 1.0, 1), TOP + H);
        // It has header 2 below it, so it stops two headers above the bottom.
        assert_eq!(pinned(BOTTOM - 1.0, 1), BOTTOM - 2.0 * H);
    }

    #[test]
    fn a_pane_too_short_for_all_headers_keeps_them_in_order_from_the_top() {
        let y = |index| pinned_y(300.0, index, 3, 0.0, 40.0, H);
        assert_eq!((y(0), y(1), y(2)), (0.0, H, 2.0 * H));
    }

    #[test]
    fn jumping_to_the_first_header_scrolls_it_to_the_top_of_the_pane() {
        // At offset 50 a header on screen at 600 is 550 into the content.
        assert_eq!(jump_offset(600.0, 0, TOP, 50.0, H), 550.0);
    }

    #[test]
    fn jumping_leaves_room_for_the_headers_above() {
        assert_eq!(jump_offset(600.0, 2, TOP, 50.0, H), 550.0 - 2.0 * H);
    }

    #[test]
    fn jumping_lands_where_the_header_is_no_longer_pinned() {
        let offset = jump_offset(600.0, 1, TOP, 50.0, H);
        // The content moved by the difference in offset.
        let natural = 600.0 - (offset - 50.0);
        assert_eq!(pinned(natural, 1), natural);
        assert_eq!(natural, TOP + H);
    }

    #[test]
    fn jumping_never_asks_for_a_negative_offset() {
        assert_eq!(jump_offset(110.0, 2, TOP, 0.0, H), 0.0);
    }

    #[test]
    fn content_shorter_than_the_pane_leaves_the_difference() {
        assert_eq!(bottom_gap(400.0, 250.0), 150.0);
    }

    #[test]
    fn content_as_tall_as_the_pane_leaves_no_gap() {
        assert_eq!(bottom_gap(400.0, 400.0), 0.0);
    }

    #[test]
    fn content_taller_than_the_pane_leaves_no_gap_not_a_negative_one() {
        assert_eq!(bottom_gap(400.0, 900.0), 0.0);
        assert_eq!(bottom_gap(0.0, 1.0), 0.0);
    }

    #[test]
    fn content_not_measured_yet_leaves_no_gap() {
        assert_eq!(bottom_gap(400.0, f32::INFINITY), 0.0);
    }
}
