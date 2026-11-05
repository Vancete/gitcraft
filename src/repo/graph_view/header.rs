//! The strip above the graph: the names of its columns, and its menu.

use egui::{Align2, Rect, Response, Stroke, Ui, pos2};

use crate::repo::columns::Cols;
use crate::settings::{self, Settings};
use crate::theme::{self, Tokens};

/// The names of the columns, in the strip `header` above the graph.
pub(super) fn column_headers(ui: &Ui, t: &Tokens, header: Rect, cols: &Cols) {
    let p = ui.painter();
    p.rect_filled(header, 0.0, t.chrome);
    p.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, t.separator),
    );
    let y = header.center().y;
    let font = theme::semibold(10.5);
    let right = header.right();
    let mut labels = vec![
        (header.left() + 10.0, "BRANCH / TAG"),
        (header.left() + cols.refs + 10.0, "GRAPH"),
        (header.left() + cols.refs + cols.graph + 6.0, "DESCRIPTION"),
    ];
    if cols.hash > 0.0 {
        labels.push((right - cols.trailing(), "COMMIT"));
    }
    if cols.author > 0.0 {
        labels.push((right - cols.author - cols.when - cols.date, "AUTHOR"));
    }
    if cols.when > 0.0 {
        labels.push((right - cols.when - cols.date, "WHEN"));
    }
    if cols.date > 0.0 {
        labels.push((right - cols.date, "DATE"));
    }
    // Each label is clipped where the next column starts, so a narrow window
    // cuts DESCRIPTION short instead of running into AUTHOR.
    for (i, (x, label)) in labels.iter().enumerate() {
        let limit = labels
            .get(i + 1)
            .map_or(header.right(), |(next, _)| next - 8.0);
        let clip = Rect::from_min_max(pos2(*x, header.top()), pos2(limit.max(*x), header.bottom()));
        p.with_clip_rect(clip).text(
            pos2(*x, y),
            Align2::LEFT_CENTER,
            label,
            font.clone(),
            t.text_faint,
        );
    }
}

/// The header's right-click menu: which columns show, a compact graph column, and the widths
/// back as they were at first.
pub(super) fn header_menu(resp: &Response, settings: &Settings) {
    egui::Popup::context_menu(resp)
        .style(theme::menu_style)
        .show(|ui| {
            let mut next = settings.clone();
            ui.checkbox(&mut next.show_hash, "Commit");
            ui.checkbox(&mut next.show_author, "Author");
            ui.checkbox(&mut next.show_when, "When");
            ui.checkbox(&mut next.show_date, "Date");
            ui.separator();
            let mut compact = next.compact_graph;
            if ui
                .checkbox(&mut compact, "Compact graph column")
                .on_hover_text(settings::COMPACT_TIP)
                .changed()
            {
                next.set_compact_graph(compact);
            }
            if ui
                .add_enabled(
                    next.graph_width > 0.0,
                    egui::Button::new("Fit graph column to its lanes"),
                )
                .on_hover_text("As double-clicking its border does")
                .clicked()
            {
                next.graph_width = 0.0;
            }
            ui.separator();
            if ui
                .button("Reset columns")
                .on_hover_text("Every column back to its first width")
                .clicked()
            {
                next.reset_columns();
            }
            if next != *settings {
                next.set(ui.ctx());
            }
        });
}
