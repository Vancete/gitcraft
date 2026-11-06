//! The strip of changes beside the diff's scrollbar, to see where to scroll to.

use std::ops::Range;

use egui::{Rect, Ui, pos2};

use super::{DiffData, Kind, LINE_H};
use crate::theme::Tokens;

/// The rows of removed and of added lines, in runs: from flags (removed, added) per row.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ChangeRuns {
    pub(super) removed: Vec<Range<usize>>,
    pub(super) added: Vec<Range<usize>>,
}

pub(super) fn change_runs(rows: impl Iterator<Item = (bool, bool)>) -> ChangeRuns {
    let mut runs = ChangeRuns::default();
    let extend = |list: &mut Vec<Range<usize>>, row: usize| match list.last_mut() {
        Some(last) if last.end == row => last.end = row + 1,
        _ => list.push(row..row + 1),
    };
    for (row, (removed, added)) in rows.enumerate() {
        if removed {
            extend(&mut runs.removed, row);
        }
        if added {
            extend(&mut runs.added, row);
        }
    }
    runs
}

/// The runs of changes in the rows shown, side by side (`split`) or unified.
pub(super) fn runs_of(data: &DiffData, split: bool) -> ChangeRuns {
    if split {
        change_runs(data.split.iter().map(|&(l, r)| {
            let kind = |i: Option<usize>| i.and_then(|i| data.lines.get(i)).map(|x| x.kind);
            (kind(l) == Some(Kind::Del), kind(r) == Some(Kind::Add))
        }))
    } else {
        change_runs(
            data.lines
                .iter()
                .map(|l| (l.kind == Kind::Del, l.kind == Kind::Add)),
        )
    }
}

/// The strip beside the diff's scrollbar: a mark for each run of changes where it sits in the
/// file (side by side: removals on the left half, additions on the right), and the part on
/// screen. `view`: (scroll offset, visible height, content height).
pub(super) fn change_map(
    ui: &Ui,
    strip: Rect,
    runs: &ChangeRuns,
    split: bool,
    view: (f32, f32, f32),
    t: &Tokens,
) {
    let p = ui.painter_at(strip);
    p.rect_filled(strip, 0.0, t.chrome);
    let (offset, visible, total) = view;
    if total <= 0.0 {
        return;
    }
    let y = |px: f32| strip.top() + px / total * strip.height();
    let rows = total / LINE_H;
    let span = |run: &Range<usize>, left: f32, right: f32| {
        let top = y(run.start as f32 * LINE_H);
        // Thin runs stay visible however long the file.
        let bottom = y(run.end as f32 * LINE_H).max(top + 2.0);
        Rect::from_min_max(pos2(left, top), pos2(right, bottom))
    };
    let mid = if split {
        strip.center().x
    } else {
        strip.right()
    };
    let added_left = if split { mid } else { strip.left() };
    // Runs that meet on the strip make one mark: a long file can have tens of thousands.
    let marks = |list: &[Range<usize>], left: f32, right: f32, color| {
        let mut mark: Option<Rect> = None;
        for run in list {
            let next = span(run, left, right);
            match &mut mark {
                Some(m) if next.top() <= m.bottom() => m.max.y = m.max.y.max(next.bottom()),
                _ => {
                    if let Some(m) = mark.replace(next) {
                        p.rect_filled(m, 0.0, color);
                    }
                }
            }
        }
        if let Some(m) = mark {
            p.rect_filled(m, 0.0, color);
        }
    };
    marks(&runs.removed, strip.left(), mid, t.danger);
    marks(&runs.added, added_left, strip.right(), t.success);
    // The part on screen, when there is more than fits.
    if visible < total && rows > 0.0 {
        let seen = Rect::from_min_max(
            pos2(strip.left(), y(offset)),
            pos2(strip.right(), y(offset + visible).min(strip.bottom())),
        );
        p.rect_filled(seen, 0.0, t.text.gamma_multiply(0.12));
    }
}
