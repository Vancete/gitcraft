//! The diff's lines, unified or side by side, in a scroll area with the change map beside it:
//! changed lines and hunks to pick, gutters to comment from, text to select by dragging.

use egui::{FontId, Galley, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::jump::{change_at, offset_of};
use super::map::change_map;
use super::paint::{filler, hunk_button, paint_line};
use super::select::{Hit, Side, TextSel, copy_selection, drag_selection};
use super::{DiffData, Kind, LINE_H, MAP_W, NUM_W, Pick, hunk_changes};
use crate::git::Area;
use crate::pull_request::DiffSide;
use crate::repo::pr_review::{
    DiffReview, LineTarget, PrAct, paint_gutter_mark, target_of, unified_spot,
};
use crate::repo::{Action, DiffKey, DiffView};
use crate::theme::{self, Tokens};

/// What painting the visible lines needs, and what it found on them this frame.
pub(super) struct Rows<'a> {
    data: &'a DiffData,
    view: &'a DiffView,
    /// The pull request's comments, when its file is shown.
    review: Option<&'a DiffReview>,
    partial: bool,
    split: bool,
    /// Which hunk actions the headers offer.
    hunk_picks: &'static [Pick],
    /// Only a selection made in the layout shown: one made side by side isn't on screen
    /// unified, and would be what Ctrl+C copies.
    text_sel: Option<TextSel>,
    /// Where the pointer was pressed, and where it is now.
    press: Option<Pos2>,
    pointer: Option<Pos2>,
    font: FontId,
    t: Tokens,
    /// A changed line clicked.
    pub(super) clicked: Option<usize>,
    /// How far the lines are scrolled down.
    pub(super) offset: f32,
    /// The change scrolled to this frame (by its place among the changes).
    pub(super) jumped: Option<usize>,
    /// Text selection by dragging: where the press and the pointer are in the lines drawn.
    press_hit: Option<Hit>,
    /// Each with whether the pointer is over that line's half, not only at its height.
    pointer_hits: Vec<(Hit, bool)>,
    text_hover: bool,
}

impl<'a> Rows<'a> {
    pub(super) fn new(
        ui: &Ui,
        view: &'a DiffView,
        data: &'a DiffData,
        review: Option<&'a DiffReview>,
        partial: bool,
        split: bool,
    ) -> Self {
        let hunk_picks: &[Pick] = match &view.key {
            DiffKey::Work(f) if partial && f.area == Area::Staged => &[Pick::Unstage],
            DiffKey::Work(_) if partial => &[Pick::Stage, Pick::Discard],
            _ => &[],
        };
        let (press, pointer) = ui.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos()));
        Self {
            data,
            view,
            review,
            partial,
            split,
            hunk_picks,
            text_sel: view.text_sel.filter(|s| s.made_in(split)),
            press,
            pointer,
            font: theme::mono(12.0),
            t: Tokens::get(ui.ctx()),
            clicked: None,
            offset: 0.0,
            jumped: None,
            press_hit: None,
            pointer_hits: Vec::new(),
            text_hover: false,
        }
    }

    /// Shows the lines and the change map; returns the text selection after this frame's drag.
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        path: &str,
        acts: &mut Vec<Action>,
    ) -> Option<TextSel> {
        let data = self.data;
        let split = self.split;
        let char_w = ui
            .painter()
            .layout_no_wrap("M".to_owned(), self.font.clone(), self.t.text)
            .size()
            .x;
        let longest = data.longest as f32;

        ui.spacing_mut().item_spacing.y = 0.0;
        let count = if split {
            data.split.len()
        } else {
            data.lines.len()
        };
        // Every change of the file in a strip beside the scrollbar, to see where to scroll to
        // (above all when the whole file is shown). Clicking a change's mark jumps to the
        // change, clicking elsewhere or dragging scrolls there.
        let (runs, changes) = data.runs(split);
        let area = ui.available_rect_before_wrap();
        let strip = Rect::from_min_max(pos2(area.right() - MAP_W, area.top()), area.max);
        let body = Rect::from_min_max(area.min, pos2(strip.left() - 1.0, area.bottom()));
        let total_h = count as f32 * LINE_H;
        let map = ui
            .interact(
                strip,
                ui.id().with(("diff_map", path)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        // Under the lines, so dragging over them selects text (a click still picks a line).
        let text_drag = ui.interact(
            body,
            ui.id().with(("diff_text", path)),
            Sense::click_and_drag(),
        );
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("diff_scroll", path, split))
            .auto_shrink(false);
        if let Some(i) = self.view.jump.filter(|&i| i < changes.len()) {
            self.jumped = Some(i);
        } else if (map.clicked() || map.dragged())
            && let Some(p) = map.interact_pointer_pos()
        {
            let frac = ((p.y - strip.top()) / strip.height()).clamp(0.0, 1.0);
            // A mark is at least 2 points tall; a little either side of it still hits it.
            let slack = 3.0 / strip.height() * count as f32;
            self.jumped = map
                .clicked()
                .then(|| change_at(changes, frac * count as f32, slack))
                .flatten();
            if self.jumped.is_none() {
                let at = frac * total_h;
                scroll = scroll.vertical_scroll_offset((at - body.height() / 2.0).max(0.0));
            }
        }
        if let Some(change) = self.jumped.and_then(|i| changes.get(i)) {
            scroll = scroll.vertical_scroll_offset(offset_of(change.start));
        }
        let out = ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            scroll.show_rows(ui, LINE_H, count, |ui, range| {
                if split {
                    // Each half is half of what's visible, however long the lines: scrolling
                    // sideways moves the text in both, the line numbers stay.
                    let view = ui.clip_rect();
                    let half = (view.width() / 2.0).floor();
                    let overflow = (NUM_W + 24.0 + longest * char_w + 16.0 - half).max(0.0);
                    for row in range {
                        self.split_row(ui, row, view, half, overflow, acts);
                    }
                } else {
                    let width =
                        (NUM_W * 2.0 + 24.0 + longest * char_w + 24.0).max(ui.available_width());
                    for i in range {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, LINE_H), Sense::hover());
                        self.line(ui, rect, i, Side::Unified, 0.0, acts);
                    }
                }
            })
        });
        let shown = out.inner;
        self.offset = shown.state.offset.y;
        change_map(
            ui,
            strip,
            runs,
            split,
            (shown.state.offset.y, shown.inner_rect.height(), total_h),
            &self.t,
        );

        // Select from where the drag started to the pointer, on the side it started on.
        let sel = drag_selection(
            self.text_sel,
            &text_drag,
            self.press_hit,
            &self.pointer_hits,
        );
        if self.text_hover || text_drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        copy_selection(ui, &text_drag, sel, &data.lines, &data.uncut);
        sel
    }

    /// One side-by-side row: the old line on the left half, the new one on the right, each
    /// `half` wide within the visible `view` and scrolled sideways by up to `overflow`.
    fn split_row(
        &mut self,
        ui: &mut Ui,
        row: usize,
        view: Rect,
        half: f32,
        overflow: f32,
        acts: &mut Vec<Action>,
    ) {
        let Some(&(l, r)) = self.data.split.get(row) else {
            return;
        };
        let (rect, _) = ui.allocate_exact_size(vec2(half * 2.0 + overflow, LINE_H), Sense::hover());
        let scroll_x = (view.left() - rect.left()).clamp(0.0, overflow);
        let left =
            Rect::from_min_size(pos2(rect.left() + scroll_x, rect.top()), vec2(half, LINE_H));
        let right = Rect::from_min_size(pos2(left.right(), rect.top()), vec2(half, LINE_H));
        // Hunk headers and notes span both halves.
        if l == r
            && let Some(i) = l
            && matches!(
                self.data.lines.get(i).map(|x| x.kind),
                Some(Kind::Hunk | Kind::Meta)
            )
        {
            self.line(ui, left.union(right), i, Side::Both, 0.0, acts);
            return;
        }
        match l {
            Some(i) => self.line(ui, left, i, Side::Old, scroll_x, acts),
            None => filler(ui, left, &self.t),
        }
        match r {
            Some(i) => self.line(ui, right, i, Side::New, scroll_x, acts),
            None => filler(ui, right, &self.t),
        }
        ui.painter().line_segment(
            [right.left_top(), right.left_bottom()],
            Stroke::new(1.0, self.t.separator),
        );
    }

    /// Line `i` in `rect`, where it shows on `side`, with what can be done on it.
    fn line(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        i: usize,
        side: Side,
        scroll_x: f32,
        acts: &mut Vec<Action>,
    ) {
        let data = self.data;
        let kind = data.lines.get(i).map(|l| l.kind);
        let pickable = self.partial && matches!(kind, Some(Kind::Add | Kind::Del));
        let spot = self.comment_spot(i, side);
        let active = spot.is_some_and(|spot| self.is_target(spot));
        let picked = self.view.chosen.contains(&i) || active;
        // Hunk headers aren't copied, so they don't look selected either.
        let sel_chars = self
            .text_sel
            .and_then(|s| s.chars_of(i, side))
            .filter(|_| kind.is_some_and(|k| side.shows(k)));
        let placed = paint_line(
            ui, rect, data, i, side, scroll_x, &self.font, &self.t, picked, sel_chars,
        );
        // Where the drag started and where the pointer is, in this line's text.
        if let Some((origin, galley)) = placed.filter(|_| kind.is_some_and(|k| side.shows(k))) {
            self.hit_test(rect, (i, side), origin, &galley, pickable);
        }
        if let Some(spot) = spot {
            self.comment_gutter(ui, rect, i, side, spot, acts);
        }
        if pickable {
            let resp = ui
                .interact(rect, ui.id().with(("diff_line", i)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                self.clicked = Some(i);
            }
        }
        if kind == Some(Kind::Hunk) && !self.hunk_picks.is_empty() {
            self.hunk_buttons(ui, rect, i, acts);
        }
    }

    /// The side and number line `i` takes a comment on where it shows on `side`, when a pull
    /// request's file is shown.
    fn comment_spot(&self, i: usize, side: Side) -> Option<(DiffSide, u64)> {
        let review = self.review?;
        let column = column_of(side);
        self.data
            .lines
            .get(i)
            .filter(|_| side != Side::Both)
            .and_then(|l| match column {
                None => unified_spot(l.kind, l.old, l.new, &review.marks),
                _ => target_of(l.kind, l.old, l.new, column),
            })
    }

    /// Whether the comment being written is on `spot`.
    fn is_target(&self, (side, line): (DiffSide, u64)) -> bool {
        self.review
            .and_then(|r| r.target.as_ref())
            .is_some_and(|t| t.side == side && t.line == line)
    }

    /// Notes where the press and the pointer are in the text of line `i` on `side`, laid out
    /// in `galley` from `origin`.
    fn hit_test(
        &mut self,
        rect: Rect,
        (i, side): (usize, Side),
        origin: Pos2,
        galley: &Galley,
        pickable: bool,
    ) {
        let hit = |p: Pos2| (i, side, galley.cursor_from_pos(p - origin).index.0);
        if let Some(p) = self.press.filter(|p| rect.contains(*p)) {
            self.press_hit = Some(hit(p));
        }
        if let Some(p) = self.pointer.filter(|p| rect.y_range().contains(p.y)) {
            let over = rect.x_range().contains(p.x);
            self.pointer_hits.push((hit(p), over));
            self.text_hover |= over && !pickable;
        }
    }

    /// The line's gutter in a pull request's file: its comments, and a click to write one.
    fn comment_gutter(
        &self,
        ui: &mut Ui,
        rect: Rect,
        i: usize,
        side: Side,
        (spot_side, number): (DiffSide, u64),
        acts: &mut Vec<Action>,
    ) {
        let Some(review) = self.review else { return };
        let gutter = if side == Side::Unified {
            NUM_W * 2.0
        } else {
            NUM_W
        };
        let area = Rect::from_min_size(rect.min, vec2(gutter, LINE_H));
        let resp = ui
            .interact(
                area,
                ui.id().with(("diff_gutter", i, column_of(side))),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Comment on this line");
        let count = review.marks.get(&(spot_side, number)).copied();
        let active = self.is_target((spot_side, number));
        paint_gutter_mark(ui, area, count, resp.hovered(), active);
        if resp.clicked() {
            acts.push(Action::Pr(PrAct::OpenLine(LineTarget {
                path: review.path.clone(),
                line: number,
                side: spot_side,
            })));
        }
    }

    /// The hunk actions in the header at line `i`, at the visible right end, however wide the
    /// line is.
    fn hunk_buttons(&self, ui: &mut Ui, rect: Rect, i: usize, acts: &mut Vec<Action>) {
        let mut right = rect.right().min(ui.clip_rect().right()) - 6.0;
        for &pick in self.hunk_picks.iter().rev() {
            let (resp, left) = hunk_button(ui, right, rect.center().y, pick, i);
            if resp.clicked() {
                acts.push(Action::PickLines {
                    pick,
                    lines: hunk_changes(&self.data.lines, i),
                });
            }
            right = left - 4.0;
        }
    }
}

/// The half of the side-by-side view a side is, for comments (`None`: unified).
fn column_of(side: Side) -> Option<DiffSide> {
    match side {
        Side::Old => Some(DiffSide::Left),
        Side::New => Some(DiffSide::Right),
        Side::Unified | Side::Both => None,
    }
}
