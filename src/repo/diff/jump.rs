//! Jumping from change to change in a diff: with the previous / next buttons, their keys, or a
//! click on a change's mark beside the scrollbar.

use std::ops::Range;

use super::LINE_H;
use super::map::ChangeRuns;
use crate::repo::DiffView;

/// Rows kept in view above a change jumped to.
const ABOVE: usize = 3;

/// The changes in the rows shown, in order: the runs of removed and of added rows, those that
/// touch or overlap made one.
pub(super) fn changes(runs: &ChangeRuns) -> Vec<Range<usize>> {
    let mut all: Vec<Range<usize>> = runs.removed.iter().chain(&runs.added).cloned().collect();
    all.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for run in all {
        match out.last_mut() {
            Some(last) if run.start <= last.end => last.end = last.end.max(run.end),
            _ => out.push(run),
        }
    }
    out
}

/// The scroll offset that shows a change starting at row `start`, a few rows below the top.
pub(super) fn offset_of(start: usize) -> f32 {
    start.saturating_sub(ABOVE) as f32 * LINE_H
}

/// The change `delta` changes away from the view scrolled to `offset`. `last`: the change
/// jumped to last and the offset that left; while the view is still there, the step is from
/// that change, else from the row a change jumped to would be at. `None` past either end.
pub(super) fn step(
    changes: &[Range<usize>],
    offset: f32,
    last: Option<(usize, f32)>,
    delta: i32,
) -> Option<usize> {
    if let Some((i, at)) = last
        && (at - offset).abs() < 1.0
        && i < changes.len()
    {
        let to = i as i64 + i64::from(delta);
        return usize::try_from(to).ok().filter(|&to| to < changes.len());
    }
    let row = offset / LINE_H + ABOVE as f32;
    if delta > 0 {
        changes.iter().position(|c| c.start as f32 > row + 0.5)
    } else {
        changes.iter().rposition(|c| (c.start as f32) < row - 0.5)
    }
}

/// The change whose rows are at row `row`, or within `slack` rows of it.
pub(super) fn change_at(changes: &[Range<usize>], row: f32, slack: f32) -> Option<usize> {
    changes
        .iter()
        .position(|c| row >= c.start as f32 - slack && row <= c.end as f32 + slack)
}

impl DiffView {
    /// Scrolls to the change `delta` changes away on the next frame (`split`: in the rows of the
    /// side-by-side view).
    pub(in crate::repo) fn step_change(&mut self, split: bool, delta: i32) {
        let Some(Ok(data)) = &self.data else { return };
        let (_, changes) = data.runs(split);
        if let Some(i) = step(changes, self.scroll, self.at_change, delta) {
            self.jump = Some(i);
        }
    }
}
