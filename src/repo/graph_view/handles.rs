//! The drag handles on the column borders of the graph's header.

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};

use super::{HANDLE_COLOR, HANDLE_GAP, HANDLE_WIDTH};
use crate::repo::RepoTab;
use crate::repo::columns::{Bar, Cols};
use crate::settings::Settings;
use crate::theme::Tokens;

/// Keeps the widths after a drag of `bar` from the preferred `prefs` in the settings.
/// Only what changed: hidden columns keep their width. The graph keeps its fit to the lanes
/// unless its own bar is dragged: dragging another bar in a narrow view would otherwise save
/// the squeezed graph width, and the graph would stay that narrow once the view widens.
fn save_drag(next: &mut Settings, bar: Bar, prefs: &Cols, widths: &Cols) {
    next.last_resized = Some(bar);
    for (col, setting) in [
        (Bar::Refs, &mut next.refs_width),
        (Bar::Graph, &mut next.graph_width),
        (Bar::Hash, &mut next.hash_width),
        (Bar::Author, &mut next.author_width),
        (Bar::When, &mut next.when_width),
        (Bar::Date, &mut next.date_width),
    ] {
        if col == Bar::Graph && bar != Bar::Graph {
            continue;
        }
        if widths.width(col) != prefs.width(col) {
            *setting = widths.width(col);
        }
    }
    next.column_push = widths.push;
}

impl RepoTab {
    /// Drag handles on the column borders of the header. A drag changes the preferred width
    /// of the handle's column in the settings (the app saves them from there); `columns`
    /// works out what that squeezes.
    ///
    /// Call this after the header background is painted, or the fill covers the lines.
    pub(super) fn column_handles(
        &self,
        ui: &mut Ui,
        header: Rect,
        prefs: &Cols,
        cols: &Cols,
        settings: &Settings,
    ) {
        let t = Tokens::get(ui.ctx());
        let mut next = settings.clone();
        // The right-hand columns start their text exactly on the border: keep the line off it.
        let right = header.right() - HANDLE_GAP;
        // (bar, x of the border, shown)
        let borders = [
            (Bar::Refs, header.left() + cols.refs, true),
            (Bar::Graph, header.left() + cols.refs + cols.graph, true),
            (Bar::Hash, right - cols.trailing(), cols.hash > 0.0),
            (
                Bar::Author,
                right - cols.author - cols.when - cols.date,
                cols.author > 0.0,
            ),
            (Bar::When, right - cols.when - cols.date, cols.when > 0.0),
            (Bar::Date, right - cols.date, cols.date > 0.0),
        ];
        for (bar, x, shown) in borders {
            if !shown {
                continue;
            }
            let grab =
                Rect::from_center_size(pos2(x, header.center().y), vec2(10.0, header.height()));
            let resp = ui
                .interact(
                    grab,
                    ui.id().with(("col-handle", bar)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            let active = resp.hovered() || resp.dragged();
            ui.painter().line_segment(
                [pos2(x, header.top() + 5.0), pos2(x, header.bottom() - 5.0)],
                Stroke::new(HANDLE_WIDTH, if active { t.accent } else { HANDLE_COLOR }),
            );
            // A dragged graph width can cut lanes off; a double-click fits it to them again.
            if bar == Bar::Graph && resp.double_clicked() {
                next.graph_width = 0.0;
                continue;
            }
            // Measured from the pointer, not its movement: past a column's limit the bar waits
            // there and only moves again once the pointer comes back to it.
            let dx = resp
                .interact_pointer_pos()
                .map_or(0.0, |pointer| pointer.x - x);
            if !resp.dragged() || dx == 0.0 {
                continue;
            }
            let widths = bar.drag(prefs, settings.last_resized, dx, header.width());
            save_drag(&mut next, bar, prefs, &widths);
        }
        if next != *settings {
            next.set(ui.ctx());
            // The new widths apply from the next frame: make sure there is one.
            ui.ctx().request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_another_bar_keeps_the_graph_fitted_to_its_lanes() {
        let prefs = Cols {
            refs: 200.0,
            graph: 100.0,
            hash: 0.0,
            author: 150.0,
            when: 0.0,
            date: 0.0,
            push: 0.0,
        };
        // A narrow view: the graph is squeezed on screen.
        let widths = Bar::Author.drag(&prefs, None, -10.0, 550.0);
        assert_ne!(widths.graph, prefs.graph);
        let mut next = Settings::default();
        save_drag(&mut next, Bar::Author, &prefs, &widths);
        assert_eq!(next.graph_width, 0.0);
        assert_eq!(next.author_width, widths.author);
        assert_eq!(next.last_resized, Some(Bar::Author));
        // Its own bar still sets it.
        let widths = Bar::Graph.drag(&prefs, None, 10.0, 700.0);
        save_drag(&mut next, Bar::Graph, &prefs, &widths);
        assert_eq!(next.graph_width, 110.0);
    }
}
