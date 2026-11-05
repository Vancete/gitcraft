//! Selecting text in a diff by dragging over it, on one side, and copying it.

use std::ops::Range;

use egui::Ui;

use super::{DiffLine, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    Unified,
    Old,
    New,
    /// A hunk header across both halves of the split view.
    Both,
}

impl Side {
    /// Whether a line of `kind` shows on this side.
    pub(super) fn shows(self, kind: Kind) -> bool {
        match self {
            Side::Unified => matches!(kind, Kind::Context | Kind::Add | Kind::Del),
            Side::Old => matches!(kind, Kind::Context | Kind::Del),
            Side::New => matches!(kind, Kind::Context | Kind::Add),
            Side::Both => false,
        }
    }
}

/// A place in the text of a line where it shows: (line, side, character).
pub(super) type Hit = (usize, Side, usize);

/// Text selected in a diff by dragging, on one side of it: from where the drag started to
/// where the pointer is, as (line, character) positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::repo) struct TextSel {
    pub(super) side: Side,
    pub(super) anchor: (usize, usize),
    pub(super) cursor: (usize, usize),
}

impl TextSel {
    /// Whether it was made in the side-by-side view (`split`) or the unified one.
    pub(super) fn made_in(&self, split: bool) -> bool {
        (self.side == Side::Unified) != split
    }

    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    /// The characters of line `i` selected where it shows on `side` (`usize::MAX`: through
    /// the end of the line, line break included).
    pub(super) fn chars_of(&self, i: usize, side: Side) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if side != self.side || i < start.0 || i > end.0 {
            return None;
        }
        let from = if i == start.0 { start.1 } else { 0 };
        let to = if i == end.0 { end.1 } else { usize::MAX };
        (from < to).then_some(from..to)
    }

    /// The selected text, one line of the side per line.
    pub(super) fn text(&self, lines: &[DiffLine]) -> String {
        let (start, end) = self.ordered();
        let mut out: Vec<String> = Vec::new();
        for i in start.0..=end.0 {
            let Some(line) = lines.get(i).filter(|l| self.side.shows(l.kind)) else {
                continue;
            };
            let from = if i == start.0 { start.1 } else { 0 };
            let to = if i == end.0 { end.1 } else { usize::MAX };
            out.push(
                line.text
                    .chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect(),
            );
        }
        out.join("\n")
    }
}

/// The selection after this frame's drag over the lines: started where the press hit the text,
/// its end moved to the pointer on the side it started on, or gone after a click.
/// `pointer_hits`: each with whether the pointer is over that line's half.
pub(super) fn drag_selection(
    sel: Option<TextSel>,
    drag: &egui::Response,
    press_hit: Option<Hit>,
    pointer_hits: &[(Hit, bool)],
) -> Option<TextSel> {
    let mut sel = sel;
    if drag.drag_started() {
        sel = press_hit.map(|(i, side, c)| TextSel {
            side,
            anchor: (i, c),
            cursor: (i, c),
        });
    } else if drag.dragged()
        && let Some(s) = &mut sel
        && let Some(((i, _, c), _)) = pointer_hits
            .iter()
            .find(|((_, side, _), _)| *side == s.side)
    {
        s.cursor = (*i, *c);
    } else if drag.clicked() {
        sel = None;
    }
    sel
}

/// Ctrl+C and the "Copy" of the context menu on the lines (`drag`) copy the selected text.
pub(super) fn copy_selection(
    ui: &Ui,
    drag: &egui::Response,
    sel: Option<TextSel>,
    lines: &[DiffLine],
) {
    let selected_text = sel.map(|s| s.text(lines)).filter(|s| !s.is_empty());
    if let Some(text) = &selected_text {
        let copy = !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if copy {
            ui.ctx().copy_text(text.clone());
        }
    }
    egui::Popup::context_menu(drag)
        .style(crate::theme::menu_style)
        .show(|ui| {
            if ui
                .add_enabled(selected_text.is_some(), egui::Button::new("Copy"))
                .clicked()
                && let Some(text) = &selected_text
            {
                ui.ctx().copy_text(text.clone());
            }
        });
}
