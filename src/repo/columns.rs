//! Widths of the graph's columns.
//!
//! The settings hold each column's preferred width. What is shown comes from those and the
//! space there is: when they do not fit, the description gives way first, then the graph,
//! the branch column and the columns on the right. Dragging a bar only changes the preferred
//! width of its own column, so dragging back gives every squeezed column its width back.

pub(super) const MIN_REFS_W: f32 = 150.0;
const MAX_REFS_W: f32 = 800.0;
pub(super) const MIN_GRAPH_W: f32 = 60.0;
pub(super) const MIN_COL_W: f32 = 50.0;
const MIN_DESCRIPTION_W: f32 = 140.0;

/// Column widths; a hidden column is 0 wide. The description takes what is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cols {
    pub refs: f32,
    pub graph: f32,
    pub hash: f32,
    pub author: f32,
    pub date: f32,
}

/// A draggable bar, named after the column it resizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Bar {
    Refs,
    Graph,
    Hash,
    Author,
    Date,
}

impl Cols {
    /// Width of everything right of the description.
    pub fn trailing(&self) -> f32 {
        self.hash + self.author + self.date
    }

    /// The widths to show when these preferred ones get `available` in total.
    pub fn squeezed(self, available: f32) -> Cols {
        let mut overflow = self.refs + self.graph + self.trailing() + MIN_DESCRIPTION_W - available;
        let mut give_way = |width: f32, min: f32| {
            let taken = overflow.clamp(0.0, (width - min).max(0.0));
            overflow -= taken;
            width - taken
        };
        Cols {
            graph: give_way(self.graph, MIN_GRAPH_W),
            refs: give_way(self.refs, MIN_REFS_W),
            hash: give_way(self.hash, MIN_COL_W),
            author: give_way(self.author, MIN_COL_W),
            date: give_way(self.date, MIN_COL_W),
        }
    }
}

impl Bar {
    /// The new preferred width of this bar's column after a drag by `dx`, from the widths
    /// on screen. The columns on the right grow to the left, as far as the columns left of
    /// them can give way; the branch and graph columns grow into the description only.
    pub fn dragged(self, shown: &Cols, dx: f32, available: f32) -> f32 {
        let room =
            (available - shown.refs - shown.graph - shown.trailing() - MIN_DESCRIPTION_W).max(0.0);
        // The minimum of a column that may be hidden.
        let least = |width: f32| if width > 0.0 { MIN_COL_W } else { 0.0 };
        let left_of_trailing = MIN_REFS_W + MIN_GRAPH_W + MIN_DESCRIPTION_W;
        let (width, taken) = match self {
            Bar::Refs => {
                let max = (shown.refs + room).clamp(MIN_REFS_W, MAX_REFS_W);
                return (shown.refs + dx).clamp(MIN_REFS_W, max);
            }
            Bar::Graph => {
                let max = (shown.graph + room).max(MIN_GRAPH_W);
                return (shown.graph + dx).clamp(MIN_GRAPH_W, max);
            }
            Bar::Hash => (shown.hash, left_of_trailing + shown.author + shown.date),
            Bar::Author => (
                shown.author,
                left_of_trailing + least(shown.hash) + shown.date,
            ),
            Bar::Date => (
                shown.date,
                left_of_trailing + least(shown.hash) + least(shown.author),
            ),
        };
        (width - dx).clamp(MIN_COL_W, (available - taken).max(MIN_COL_W))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVAILABLE: f32 = 700.0;

    fn preferred() -> Cols {
        Cols {
            refs: 200.0,
            graph: 100.0,
            hash: 0.0,
            author: 150.0,
            date: 0.0,
        }
    }

    fn description(cols: &Cols, available: f32) -> f32 {
        available - cols.refs - cols.graph - cols.trailing()
    }

    /// Drags the bar in `steps` steps, as the UI does: from the widths on screen each frame.
    fn drag(mut prefs: Cols, bar: Bar, dx: f32, steps: usize, available: f32) -> Cols {
        for _ in 0..steps {
            let shown = prefs.squeezed(available);
            let width = bar.dragged(&shown, dx, available);
            match bar {
                Bar::Refs => prefs.refs = width,
                Bar::Graph => prefs.graph = width,
                Bar::Hash => prefs.hash = width,
                Bar::Author => prefs.author = width,
                Bar::Date => prefs.date = width,
            }
        }
        prefs
    }

    #[test]
    fn leaves_widths_alone_when_they_fit() {
        assert_eq!(preferred().squeezed(AVAILABLE), preferred());
    }

    #[test]
    fn description_gives_way_before_the_graph() {
        let prefs = drag(preferred(), Bar::Author, -10.0, 11, AVAILABLE);
        let shown = prefs.squeezed(AVAILABLE);
        assert_eq!(shown.author, 260.0);
        assert_eq!(description(&shown, AVAILABLE), 140.0);
        assert_eq!(shown.graph, 100.0);
    }

    #[test]
    fn graph_gives_way_before_the_branch_column() {
        let prefs = drag(preferred(), Bar::Author, -10.0, 13, AVAILABLE);
        let shown = prefs.squeezed(AVAILABLE);
        assert_eq!(shown.graph, 80.0);
        assert_eq!(shown.refs, 200.0);
        assert_eq!(description(&shown, AVAILABLE), 140.0);
    }

    #[test]
    fn author_bar_squeezes_every_column_to_its_minimum_and_stops() {
        let prefs = drag(preferred(), Bar::Author, -10.0, 100, AVAILABLE);
        let shown = prefs.squeezed(AVAILABLE);
        assert_eq!(shown.refs, MIN_REFS_W);
        assert_eq!(shown.graph, MIN_GRAPH_W);
        assert_eq!(description(&shown, AVAILABLE), MIN_DESCRIPTION_W);
        assert_eq!(shown.author, 350.0);
    }

    #[test]
    fn dragging_back_restores_every_squeezed_column() {
        let squeezed = drag(preferred(), Bar::Author, -10.0, 100, AVAILABLE);
        // From 350 back to the 150 it started at.
        let restored = drag(squeezed, Bar::Author, 10.0, 20, AVAILABLE);
        assert_eq!(restored.squeezed(AVAILABLE), preferred());
    }

    #[test]
    fn squeezed_columns_come_back_in_reverse_order() {
        let squeezed = drag(preferred(), Bar::Author, -10.0, 100, AVAILABLE);
        // At 280 the branch column has its width back; the graph is still 20 short.
        let shown = drag(squeezed, Bar::Author, 10.0, 7, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.author, 280.0);
        assert_eq!(shown.refs, 200.0);
        assert_eq!(shown.graph, 80.0);
    }

    #[test]
    fn date_bar_squeezes_the_columns_left_of_it_including_author() {
        let prefs = Cols {
            date: 100.0,
            ..preferred()
        };
        let shown = drag(prefs, Bar::Date, -10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.author, MIN_COL_W);
        assert_eq!(shown.date, 300.0);
    }

    #[test]
    fn author_bar_does_not_squeeze_the_column_right_of_it() {
        let prefs = Cols {
            date: 100.0,
            ..preferred()
        };
        let shown = drag(prefs, Bar::Author, -10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.date, 100.0);
    }

    #[test]
    fn a_column_stops_at_its_minimum_when_dragged_narrower() {
        let shown = drag(preferred(), Bar::Author, 10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.author, MIN_COL_W);
        let shown = drag(preferred(), Bar::Graph, -10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.graph, MIN_GRAPH_W);
        let shown = drag(preferred(), Bar::Refs, -10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.refs, MIN_REFS_W);
    }

    #[test]
    fn branch_and_graph_bars_stop_when_the_description_is_at_its_minimum() {
        // 700 - 100 - 150 - 140 leaves 310 for the branch column.
        let shown = drag(preferred(), Bar::Refs, 10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.refs, 310.0);
        assert_eq!(shown.author, 150.0);
        // 700 - 200 - 150 - 140 leaves 210 for the graph.
        let shown = drag(preferred(), Bar::Graph, 10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.graph, 210.0);
        assert_eq!(shown.refs, 200.0);
    }

    #[test]
    fn hidden_columns_stay_hidden() {
        let shown = drag(preferred(), Bar::Author, -10.0, 100, AVAILABLE).squeezed(AVAILABLE);
        assert_eq!(shown.hash, 0.0);
        assert_eq!(shown.date, 0.0);
    }

    #[test]
    fn a_narrow_window_squeezes_and_a_wider_one_restores() {
        assert_eq!(preferred().squeezed(550.0).graph, 60.0);
        assert_eq!(preferred().squeezed(550.0).refs, 200.0);
        assert_eq!(preferred().squeezed(AVAILABLE), preferred());
    }

    #[test]
    fn a_window_too_narrow_for_the_minimums_shows_the_minimums() {
        let shown = preferred().squeezed(100.0);
        assert_eq!(shown.refs, MIN_REFS_W);
        assert_eq!(shown.graph, MIN_GRAPH_W);
        assert_eq!(shown.author, MIN_COL_W);
    }
}
