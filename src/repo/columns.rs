//! Widths of the graph's columns.
//!
//! The settings hold each column's preferred width. What is shown comes from those and the
//! space there is: when they do not fit, the description gives way first, then the other
//! columns, the one dragged last only when nothing else can give (see [`give_way_order`]).
//! Dragging a bar only changes the preferred width of its own column, so dragging back gives
//! every squeezed column its width back, whichever bar it is. A right-hand column dragged
//! narrower than its minimum stays at it and pushes the columns right of it instead ([`Cols::push`]).

use serde::{Deserialize, Serialize};

pub(super) const MIN_REFS_W: f32 = 210.0;
pub(super) const MIN_GRAPH_W: f32 = 60.0;
pub(super) const MIN_COL_W: f32 = 50.0;
const MIN_DESCRIPTION_W: f32 = 140.0;

/// Column widths; a hidden column is 0 wide. The description takes what is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cols {
    pub refs: f32,
    pub graph: f32,
    pub hash: f32,
    pub author: f32,
    pub date: f32,
    /// How far the bar dragged last was pulled past its column's minimum, towards the right:
    /// the columns right of it give that much (a right-hand column only).
    pub push: f32,
}

/// The columns right of the description, left to right.
const TRAILING: [Bar; 3] = [Bar::Hash, Bar::Author, Bar::Date];

/// A draggable bar, named after the column it resizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Bar {
    Refs,
    Graph,
    Hash,
    Author,
    Date,
}

/// The order columns give way in once the description is at its minimum: the graph, the
/// branch column, then the columns on the right, left to right. The column dragged last
/// gives way last of all, so it keeps the width it was dragged to; the branch and graph
/// columns grow to the right, so after them the columns on the right give way first.
fn give_way_order(last: Option<Bar>) -> Vec<Bar> {
    use Bar::*;
    let mut order = match last {
        Some(Refs | Graph) => vec![Hash, Author, Date, Graph, Refs],
        _ => vec![Graph, Refs, Hash, Author, Date],
    };
    if let Some(last) = last {
        order.retain(|&b| b != last);
        order.push(last);
    }
    order
}

impl Cols {
    /// Width of everything right of the description.
    pub fn trailing(&self) -> f32 {
        self.hash + self.author + self.date
    }

    pub fn width(&self, bar: Bar) -> f32 {
        match bar {
            Bar::Refs => self.refs,
            Bar::Graph => self.graph,
            Bar::Hash => self.hash,
            Bar::Author => self.author,
            Bar::Date => self.date,
        }
    }

    fn width_mut(&mut self, bar: Bar) -> &mut f32 {
        match bar {
            Bar::Refs => &mut self.refs,
            Bar::Graph => &mut self.graph,
            Bar::Hash => &mut self.hash,
            Bar::Author => &mut self.author,
            Bar::Date => &mut self.date,
        }
    }

    /// The widths to show when these preferred ones get `available` in total; `last` is the
    /// bar dragged last.
    pub fn squeezed(self, available: f32, last: Option<Bar>) -> Cols {
        let mut out = Cols { push: 0.0, ..self };
        // A hidden column pushes nothing: the columns right of it get their width back.
        if let Some(bar) = last.filter(|&bar| self.width(bar) > 0.0) {
            out.give_push(bar, self.push);
        }
        let mut overflow = out.refs + out.graph + out.trailing() + MIN_DESCRIPTION_W - available;
        for bar in give_way_order(last) {
            let width = out.width_mut(bar);
            let taken = overflow.clamp(0.0, (*width - bar.min()).max(0.0));
            overflow -= taken;
            *width -= taken;
        }
        out
    }

    /// Space the description has beyond its minimum.
    fn room(&self, available: f32) -> f32 {
        (available - self.refs - self.graph - self.trailing() - MIN_DESCRIPTION_W).max(0.0)
    }

    /// The shown columns right of `bar` give `push` between them, nearest first.
    fn give_push(&mut self, bar: Bar, mut push: f32) {
        for col in TRAILING.iter().skip_while(|&&c| c != bar).skip(1) {
            let width = self.width_mut(*col);
            let taken = push.clamp(0.0, (*width - MIN_COL_W).max(0.0));
            push -= taken;
            *width -= taken;
        }
    }

    /// How much the columns right of `bar` can still shrink.
    fn slack_right_of(&self, bar: Bar) -> f32 {
        TRAILING
            .iter()
            .skip_while(|&&c| c != bar)
            .skip(1)
            .map(|&c| (self.width(c) - MIN_COL_W).max(0.0))
            .sum()
    }

    /// How much the columns on the right can still shrink.
    fn slack(&self) -> f32 {
        [self.hash, self.author, self.date]
            .iter()
            .map(|w| (w - MIN_COL_W).max(0.0))
            .sum()
    }
}

impl Bar {
    fn min(self) -> f32 {
        match self {
            Bar::Refs => MIN_REFS_W,
            Bar::Graph => MIN_GRAPH_W,
            _ => MIN_COL_W,
        }
    }

    /// The preferred widths after dragging this bar by `dx`; `last` is the bar dragged before.
    /// Only this bar's column changes, except when another bar was dragged last: then what is
    /// on screen becomes the preferred widths first, so nothing jumps when the order columns
    /// give way in changes.
    pub fn drag(self, prefs: &Cols, last: Option<Bar>, dx: f32, available: f32) -> Cols {
        let shown = prefs.squeezed(available, last);
        let mut next = if last == Some(self) { *prefs } else { shown };
        // Where the bar is, counting how far it was pulled past the column's minimum.
        let own = shown.width(self) - next.push;
        (*next.width_mut(self), next.push) = self.dragged(&shown, own, dx, available);
        next
    }

    /// The new width of this bar's column (`own` now) after a drag by `dx`, from the widths on
    /// screen, and how far that is past the column's minimum. The columns on the right grow to
    /// the left, as far as the columns left of them can give way, and shrink to the right
    /// past their minimum by pushing the columns right of them. The branch and graph columns
    /// grow to the right, into the description and then the columns on the right.
    fn dragged(self, shown: &Cols, own: f32, dx: f32, available: f32) -> (f32, f32) {
        let room = shown.room(available);
        // The minimum of a column that may be hidden.
        let least = |width: f32| if width > 0.0 { MIN_COL_W } else { 0.0 };
        let left_of_trailing = MIN_REFS_W + MIN_GRAPH_W + MIN_DESCRIPTION_W;
        let taken = match self {
            Bar::Refs => {
                let max = (own + room + shown.slack()).max(MIN_REFS_W);
                return ((own + dx).clamp(MIN_REFS_W, max), 0.0);
            }
            Bar::Graph => {
                let max = (own + room + shown.slack()).max(MIN_GRAPH_W);
                return ((own + dx).clamp(MIN_GRAPH_W, max), 0.0);
            }
            Bar::Hash => left_of_trailing + shown.author + shown.date,
            Bar::Author => left_of_trailing + least(shown.hash) + shown.date,
            Bar::Date => left_of_trailing + least(shown.hash) + least(shown.author),
        };
        // Past the minimum by as much as the columns on the right can still give.
        let pushed = (MIN_COL_W - own).max(0.0);
        let lowest = MIN_COL_W - pushed - shown.slack_right_of(self);
        let width = (own - dx).clamp(lowest, (available - taken).max(MIN_COL_W));
        (width.max(MIN_COL_W), (MIN_COL_W - width).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVAILABLE: f32 = 830.0;

    fn preferred() -> Cols {
        Cols {
            refs: 330.0,
            graph: 100.0,
            hash: 0.0,
            author: 150.0,
            date: 0.0,
            push: 0.0,
        }
    }

    fn description(cols: &Cols, available: f32) -> f32 {
        available - cols.refs - cols.graph - cols.trailing()
    }

    /// Drags the bar in `steps` steps, as the UI does: one drag a frame. `last` is the bar
    /// dragged before.
    fn drag(mut prefs: Cols, last: Option<Bar>, bar: Bar, dx: f32, steps: usize) -> Cols {
        let mut last = last;
        for _ in 0..steps {
            prefs = bar.drag(&prefs, last, dx, AVAILABLE);
            last = Some(bar);
        }
        prefs
    }

    /// What is on screen after dragging `bar` from the preferred widths.
    fn shown_after(bar: Bar, dx: f32, steps: usize) -> Cols {
        drag(preferred(), None, bar, dx, steps).squeezed(AVAILABLE, Some(bar))
    }

    #[test]
    fn leaves_widths_alone_when_they_fit() {
        assert_eq!(preferred().squeezed(AVAILABLE, None), preferred());
    }

    #[test]
    fn description_gives_way_before_the_graph() {
        let shown = shown_after(Bar::Author, -10.0, 11);
        assert_eq!(shown.author, 260.0);
        assert_eq!(description(&shown, AVAILABLE), 140.0);
        assert_eq!(shown.graph, 100.0);
    }

    #[test]
    fn graph_gives_way_before_the_branch_column() {
        let shown = shown_after(Bar::Author, -10.0, 13);
        assert_eq!(shown.graph, 80.0);
        assert_eq!(shown.refs, 330.0);
        assert_eq!(description(&shown, AVAILABLE), 140.0);
    }

    #[test]
    fn author_bar_squeezes_every_column_to_its_minimum_and_stops() {
        let shown = shown_after(Bar::Author, -10.0, 100);
        assert_eq!(shown.refs, MIN_REFS_W);
        assert_eq!(shown.graph, MIN_GRAPH_W);
        assert_eq!(description(&shown, AVAILABLE), MIN_DESCRIPTION_W);
        assert_eq!(shown.author, 420.0);
    }

    #[test]
    fn dragging_back_restores_every_squeezed_column() {
        let squeezed = drag(preferred(), None, Bar::Author, -10.0, 100);
        // From 420 back to the 150 it started at.
        let restored = drag(squeezed, Some(Bar::Author), Bar::Author, 10.0, 27);
        assert_eq!(restored.squeezed(AVAILABLE, Some(Bar::Author)), preferred());
    }

    #[test]
    fn squeezed_columns_come_back_in_reverse_order() {
        let squeezed = drag(preferred(), None, Bar::Author, -10.0, 100);
        // At 280 the branch column has its width back; the graph is still 20 short.
        let shown = drag(squeezed, Some(Bar::Author), Bar::Author, 10.0, 14)
            .squeezed(AVAILABLE, Some(Bar::Author));
        assert_eq!(shown.author, 280.0);
        assert_eq!(shown.refs, 330.0);
        assert_eq!(shown.graph, 80.0);
    }

    #[test]
    fn date_bar_squeezes_the_columns_left_of_it_including_author() {
        let prefs = Cols {
            date: 100.0,
            ..preferred()
        };
        let shown = drag(prefs, None, Bar::Date, -10.0, 100).squeezed(AVAILABLE, Some(Bar::Date));
        assert_eq!(shown.author, MIN_COL_W);
        assert_eq!(shown.date, 370.0);
    }

    #[test]
    fn author_bar_does_not_squeeze_the_column_right_of_it() {
        let prefs = Cols {
            date: 100.0,
            ..preferred()
        };
        let shown =
            drag(prefs, None, Bar::Author, -10.0, 100).squeezed(AVAILABLE, Some(Bar::Author));
        assert_eq!(shown.date, 100.0);
    }

    #[test]
    fn a_column_stops_at_its_minimum_when_dragged_narrower() {
        assert_eq!(shown_after(Bar::Author, 10.0, 100).author, MIN_COL_W);
        assert_eq!(shown_after(Bar::Graph, -10.0, 100).graph, MIN_GRAPH_W);
        assert_eq!(shown_after(Bar::Refs, -10.0, 100).refs, MIN_REFS_W);
    }

    #[test]
    fn branch_and_graph_bars_push_the_columns_on_the_right_once_the_description_is_full() {
        // 830 - 100 - 50 - 140 leaves 540 for the branch column, the author at its minimum.
        let shown = shown_after(Bar::Refs, 10.0, 100);
        assert_eq!(shown.refs, 540.0);
        assert_eq!(shown.author, MIN_COL_W);
        assert_eq!(shown.graph, 100.0);
        // 830 - 330 - 50 - 140 leaves 310 for the graph.
        let shown = shown_after(Bar::Graph, 10.0, 100);
        assert_eq!(shown.graph, 310.0);
        assert_eq!(shown.refs, 330.0);
        assert_eq!(shown.author, MIN_COL_W);
    }

    #[test]
    fn dragging_the_branch_bar_back_restores_the_columns_on_the_right() {
        let pushed = drag(preferred(), None, Bar::Refs, 10.0, 100);
        // From 540 back to the 330 it started at.
        let restored = drag(pushed, Some(Bar::Refs), Bar::Refs, -10.0, 21);
        assert_eq!(restored.squeezed(AVAILABLE, Some(Bar::Refs)), preferred());
    }

    #[test]
    fn dragging_another_bar_starts_from_what_is_on_screen() {
        let pushed = drag(preferred(), None, Bar::Refs, 10.0, 100);
        // The author, squeezed to 50, grows by 10 from there, which the graph gives.
        let shown = drag(pushed, Some(Bar::Refs), Bar::Author, -10.0, 1)
            .squeezed(AVAILABLE, Some(Bar::Author));
        assert_eq!(shown.author, 60.0);
        assert_eq!(shown.refs, 540.0);
        assert_eq!(shown.graph, 90.0);
    }

    fn with_date() -> Cols {
        Cols {
            date: 110.0,
            ..preferred()
        }
    }

    #[test]
    fn a_right_hand_column_pushes_the_columns_right_of_it_past_its_minimum() {
        // The author shrinks to 50, then the bar keeps going and the date column gives 30.
        let prefs = drag(with_date(), None, Bar::Author, 10.0, 13);
        let shown = prefs.squeezed(AVAILABLE, Some(Bar::Author));
        assert_eq!(shown.author, MIN_COL_W);
        assert_eq!(shown.date, 80.0);
        // The date column keeps its own width, to come back to.
        assert_eq!(prefs.date, 110.0);
    }

    #[test]
    fn pushing_stops_when_the_columns_on_the_right_are_at_their_minimum() {
        let shown =
            drag(with_date(), None, Bar::Author, 10.0, 100).squeezed(AVAILABLE, Some(Bar::Author));
        assert_eq!(shown.author, MIN_COL_W);
        assert_eq!(shown.date, MIN_COL_W);
    }

    #[test]
    fn dragging_back_gives_pushed_columns_their_width_back() {
        let pushed = drag(with_date(), None, Bar::Author, 10.0, 100);
        // 100 to the minimum and 60 pushed: 160 back.
        let restored = drag(pushed, Some(Bar::Author), Bar::Author, -10.0, 16);
        assert_eq!(restored.squeezed(AVAILABLE, Some(Bar::Author)), with_date());
    }

    #[test]
    fn dragging_another_bar_keeps_what_was_pushed() {
        let pushed = drag(with_date(), None, Bar::Author, 10.0, 100);
        let shown = drag(pushed, Some(Bar::Author), Bar::Date, -10.0, 1)
            .squeezed(AVAILABLE, Some(Bar::Date));
        assert_eq!(shown.date, 60.0);
        assert_eq!(shown.author, MIN_COL_W);
    }

    #[test]
    fn hiding_the_pushing_column_gives_the_pushed_ones_their_width_back() {
        let pushed = drag(with_date(), None, Bar::Author, 10.0, 13);
        assert!(pushed.push > 0.0);
        let hidden = Cols {
            author: 0.0,
            ..pushed
        };
        assert_eq!(hidden.squeezed(AVAILABLE, Some(Bar::Author)).date, 110.0);
    }

    #[test]
    fn hidden_columns_stay_hidden() {
        let shown = shown_after(Bar::Author, -10.0, 100);
        assert_eq!(shown.hash, 0.0);
        assert_eq!(shown.date, 0.0);
    }

    #[test]
    fn a_narrow_window_squeezes_and_a_wider_one_restores() {
        assert_eq!(preferred().squeezed(680.0, None).graph, 60.0);
        assert_eq!(preferred().squeezed(680.0, None).refs, 330.0);
        assert_eq!(preferred().squeezed(AVAILABLE, None), preferred());
    }

    #[test]
    fn a_window_too_narrow_for_the_minimums_shows_the_minimums() {
        let shown = preferred().squeezed(100.0, None);
        assert_eq!(shown.refs, MIN_REFS_W);
        assert_eq!(shown.graph, MIN_GRAPH_W);
        assert_eq!(shown.author, MIN_COL_W);
    }
}
