//! Painting one diff row: line numbers, the +/− mark, syntax colours with the changed part and
//! the selected text marked, and the buttons in hunk headers.

use std::ops::Range;
use std::sync::Arc;

use egui::{
    Align2, Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, text::CCursor, text::LayoutJob,
    text::TextFormat, vec2,
};

use super::select::Side;
use super::{DiffData, Kind, LINE_H, NUM_W, Pick};
use crate::theme::{self, Tokens};

pub(super) fn filler(ui: &Ui, rect: Rect, t: &Tokens) {
    ui.painter()
        .rect_filled(rect, 0.0, t.card.gamma_multiply(0.35));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_line(
    ui: &Ui,
    rect: Rect,
    data: &DiffData,
    i: usize,
    side: Side,
    // How far the text is scrolled sideways inside `rect` (the numbers stay).
    scroll_x: f32,
    font: &egui::FontId,
    t: &Tokens,
    selected: bool,
    // Characters of the text selected by dragging.
    text_sel: Option<Range<usize>>,
) -> Option<(Pos2, Arc<egui::Galley>)> {
    let line = data.lines.get(i)?;
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let (bg, fg) = match line.kind {
        Kind::Add => (Some(t.diff_add_bg), t.text),
        Kind::Del => (Some(t.diff_del_bg), t.text),
        Kind::Hunk => (Some(t.accent.gamma_multiply(0.12)), t.accent),
        Kind::Meta => (None, t.text_faint),
        Kind::Context => (None, t.text_dim),
    };
    if let Some(bg) = bg {
        p.rect_filled(rect, 0.0, bg);
    }
    // Picked to stage / unstage / discard: tinted, with a bar on the left.
    if selected {
        p.rect_filled(rect, 0.0, t.accent.gamma_multiply(0.22));
    }
    let y = rect.center().y;
    let numbers: &[(Option<u32>, f32)] = match side {
        Side::Unified => &[(line.old, NUM_W - 8.0), (line.new, NUM_W * 2.0 - 8.0)],
        Side::Old => &[(line.old, NUM_W - 8.0)],
        Side::New => &[(line.new, NUM_W - 8.0)],
        Side::Both => &[],
    };
    let gutter = match side {
        Side::Unified => NUM_W * 2.0,
        Side::Both => 0.0,
        _ => NUM_W,
    };
    if gutter > 0.0 {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(gutter, LINE_H)),
            0.0,
            t.card.gamma_multiply(0.6),
        );
    }
    if selected {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(3.0, LINE_H)),
            0.0,
            t.accent,
        );
    }
    for &(n, x) in numbers {
        if let Some(n) = n {
            p.text(
                pos2(rect.left() + x, y),
                Align2::RIGHT_CENTER,
                n.to_string(),
                theme::mono(11.0),
                t.text_faint,
            );
        }
    }
    let marker = match line.kind {
        Kind::Add => "+",
        Kind::Del => "−",
        _ => "",
    };
    let mark_color = if line.kind == Kind::Add {
        t.success
    } else {
        t.danger
    };
    p.text(
        pos2(rect.left() + gutter + 8.0, y),
        Align2::LEFT_CENTER,
        marker,
        font.clone(),
        mark_color,
    );

    // Text: syntax colours when known, the changed part on a stronger background.
    let strong = match line.kind {
        Kind::Add => t.success.gamma_multiply(0.35),
        Kind::Del => t.danger.gamma_multiply(0.35),
        _ => Color32::TRANSPARENT,
    };
    let changed = data.changed.get(i).copied().flatten();
    let spans: Vec<(Color32, &str)> = match (line.kind, data.spans.get(i).and_then(|s| s.as_ref()))
    {
        (Kind::Add | Kind::Del | Kind::Context, Some(spans)) => {
            spans.iter().map(|(c, s)| (*c, s.as_str())).collect()
        }
        _ => vec![(fg, line.text.as_str())],
    };
    let job = line_job(&spans, changed, font, strong);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let x = rect.left() + gutter + 24.0;
    // Scrolled text goes under the marker column, not over the numbers.
    let p = if scroll_x > 0.0 {
        p.with_clip_rect(p.clip_rect().intersect(rect.with_min_x(x - 4.0)))
    } else {
        p
    };
    let origin = pos2(x - scroll_x, y - galley.size().y / 2.0);
    if let Some(chars) = text_sel {
        let at = |c: usize| origin.x + galley.pos_from_cursor(CCursor::new(c)).min.x;
        let left = at(chars.start);
        // Selected through the end: a little past the text, for the line break.
        let right = if chars.end == usize::MAX {
            origin.x + galley.size().x + 6.0
        } else {
            at(chars.end)
        };
        p.rect_filled(
            Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
            0.0,
            t.accent.gamma_multiply(0.35),
        );
    }
    p.galley(origin, galley.clone(), fg);
    Some((origin, galley))
}

/// A layout job for `spans`, with the chars in `changed` on `strong`.
pub(super) fn line_job(
    spans: &[(Color32, &str)],
    changed: Option<(usize, usize)>,
    font: &egui::FontId,
    strong: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut pos = 0usize;
    for (color, text) in spans {
        let n = text.chars().count();
        let (start, end) = (pos, pos + n);
        // Split the span at the changed range's edges.
        let cuts: Vec<usize> = match changed {
            Some((a, b)) => vec![start, a.clamp(start, end), b.clamp(start, end), end],
            None => vec![start, end],
        };
        for w in cuts.windows(2) {
            let (from, to) = (w[0], w[1]);
            if to <= from {
                continue;
            }
            let piece: String = text.chars().skip(from - start).take(to - from).collect();
            let inside = changed.is_some_and(|(a, b)| from >= a && to <= b);
            job.append(
                &piece,
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: *color,
                    background: if inside { strong } else { Color32::TRANSPARENT },
                    ..Default::default()
                },
            );
        }
        pos = end;
    }
    job
}

/// A small text button in a hunk header, its right edge at `right`; returns it and its left edge.
pub(super) fn hunk_button(
    ui: &mut Ui,
    right: f32,
    y: f32,
    pick: Pick,
    hunk: usize,
) -> (egui::Response, f32) {
    let t = Tokens::get(ui.ctx());
    let label = format!("{} hunk", pick.verb());
    let color = if pick == Pick::Discard {
        t.text_dim
    } else {
        t.accent
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label, theme::medium(11.0), color);
    let r = Rect::from_min_max(
        pos2(right - galley.size().x - 14.0, y - 9.0),
        pos2(right, y + 9.0),
    );
    let resp = ui.interact(
        r,
        ui.id().with(("hunk_btn", hunk, pick.verb())),
        Sense::click(),
    );
    let fill = if resp.hovered() { t.hover } else { t.dock };
    ui.painter().rect_filled(r, t.radius_sm, fill);
    ui.painter().rect_stroke(
        r,
        t.radius_sm,
        Stroke::new(1.0, t.separator),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(r.left() + 7.0, y - galley.size().y / 2.0),
        galley,
        color,
    );
    let tip = match pick {
        Pick::Stage => {
            "Stage this hunk. Click lines to stage only some of them (Shift+click for a range)."
        }
        Pick::Unstage => {
            "Unstage this hunk. Click lines to unstage only some of them (Shift+click for a range)."
        }
        Pick::Discard => "Throw away this hunk's changes (Ctrl+Z brings them back).",
    };
    (resp.on_hover_text(tip), r.left())
}
