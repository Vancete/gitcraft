//! One commit row of the graph: its background, the description and the hash, author, when
//! and date columns.

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense, Stroke};
use egui::{Ui, pos2, vec2};

use super::{AVATAR_D, LANE_W, ROW_GAP, ROW_H, ROW_PITCH};
use crate::git::{self, Commit};
use crate::repo::columns::Cols;
use crate::repo::{Action, RepoTab, WIP_HASH, widgets};
use crate::theme::{self, Tokens};

/// The row being painted.
pub(super) struct Row<'a> {
    pub(super) i: usize,
    pub(super) commit: &'a Commit,
    pub(super) rect: Rect,
    pub(super) cols: Cols,
    /// Left edge of the graph column.
    pub(super) gx: f32,
    /// Where the commit's dot is, if the graph has the row.
    pub(super) node_x: Option<f32>,
    /// The colour of the commit's lane.
    pub(super) color: Color32,
}

impl Row<'_> {
    pub(super) fn mid(&self) -> f32 {
        self.rect.center().y
    }

    /// The centre of `lane` in the graph column.
    pub(super) fn lane_x(&self, lane: usize) -> f32 {
        lane_x(self.gx, lane)
    }
}

fn lane_x(gx: f32, lane: usize) -> f32 {
    gx + 6.0 + lane as f32 * LANE_W + LANE_W / 2.0
}

impl RepoTab {
    pub(super) fn commit_row(
        &self,
        ui: &mut Ui,
        i: usize,
        cols: Cols,
        lanes: usize,
        now: i64,
        acts: &mut Vec<Action>,
    ) -> Response {
        let t = Tokens::get(ui.ctx());
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_PITCH), Sense::click());
        let Some(commit) = self.commits.get(i) else {
            return resp;
        };
        let painter = ui.painter_at(rect);
        let is_wip = commit.hash == WIP_HASH;
        let is_match = self.search.matches.contains(&commit.hash);
        row_background(
            &painter,
            &t,
            rect,
            self.selected == Some(i),
            resp.hovered(),
            is_match,
            self.is_compared(commit, is_wip),
        );

        let gx = rect.left() + cols.refs;
        let graph_row = self.graph.rows.get(i);
        let row = Row {
            i,
            commit,
            rect,
            cols,
            gx,
            node_x: graph_row.map(|r| lane_x(gx, r.col)),
            color: graph_row.map(|r| t.lane(r.col)).unwrap_or(t.text_faint),
        };

        let resp = self.refs_cell(ui, &painter, &row, resp, acts);
        self.graph_cell(ui, &painter, &row, lanes);
        if is_wip {
            self.wip_description(&painter, &t, &row);
        } else {
            description(ui, &painter, &t, &row, is_match);
            trailing_columns(ui, &painter, &t, &row, now);
        }
        resp
    }

    /// Whether `commit` is an end of the comparison shown on the right (the changes row for
    /// the working tree).
    fn is_compared(&self, commit: &Commit, is_wip: bool) -> bool {
        self.compare
            .as_ref()
            .and_then(|c| c.data.as_ref()?.as_ref().ok())
            .is_some_and(|d| {
                d.from == commit.hash
                    || d.to.as_deref() == Some(commit.hash.as_str())
                    || (is_wip && d.to.is_none())
            })
    }

    /// The changes row's description: how many files are conflicted, staged and unstaged.
    fn wip_description(&self, painter: &Painter, t: &Tokens, row: &Row) {
        let mid = row.mid();
        let mut x = row.gx + row.cols.graph + 6.0;
        let font = theme::medium(12.5);
        let r = painter.text(
            pos2(x, mid),
            Align2::LEFT_CENTER,
            "Uncommitted changes",
            font,
            t.text,
        );
        x = r.right() + 10.0;
        let conflicts = if self.work.conflicted.len() == 1 {
            "conflict"
        } else {
            "conflicts"
        };
        for (n, label, color) in [
            (self.work.conflicted.len(), conflicts, t.danger),
            (self.work.staged.len(), "staged", t.success),
            (self.work.unstaged.len(), "unstaged", t.warning),
        ] {
            if n > 0 {
                x += pill(painter, pos2(x, mid), &format!("{n} {label}"), color) + 6.0;
            }
        }
    }
}

/// The row's band: selected, hovered or a search match, outlined when `compared`.
fn row_background(
    painter: &Painter,
    t: &Tokens,
    rect: Rect,
    selected: bool,
    hovered: bool,
    is_match: bool,
    compared: bool,
) {
    // The row's own band, without the gap.
    let band = rect.shrink2(vec2(0.0, ROW_GAP / 2.0));
    if selected {
        painter.rect_filled(band, 0.0, t.row_selected);
    } else if hovered {
        painter.rect_filled(band, 0.0, t.hover);
    } else if is_match {
        painter.rect_filled(band, 0.0, t.warning.gamma_multiply(0.10));
    }
    if is_match {
        painter.rect_filled(
            Rect::from_min_size(band.min, vec2(3.0, ROW_H)),
            0.0,
            t.warning,
        );
    }
    if compared {
        painter.rect_stroke(
            band.shrink(1.0),
            0.0,
            Stroke::new(1.0, t.accent),
            egui::StrokeKind::Inside,
        );
    }
}

/// The summary, then the rest of the message faded, cut with an ellipsis.
fn description(ui: &Ui, painter: &Painter, t: &Tokens, row: &Row, is_match: bool) {
    let (rect, mid) = (row.rect, row.mid());
    let x = row.gx + row.cols.graph + 6.0;
    let text_right = rect.right() - row.cols.trailing() - 10.0;
    let font = if is_match {
        theme::medium(12.5)
    } else {
        FontId::proportional(12.5)
    };
    let text_clip = painter.with_clip_rect(Rect::from_min_max(
        pos2(x, rect.top()),
        pos2(text_right, rect.bottom()),
    ));
    let summary = text_clip.text(
        pos2(x + 2.0, mid),
        Align2::LEFT_CENTER,
        &row.commit.summary,
        font.clone(),
        t.text,
    );
    let room = text_right - summary.right() - 12.0;
    if !row.commit.body.is_empty() && room > 24.0 {
        let mut job = egui::text::LayoutJob::single_section(
            row.commit.body.clone(),
            egui::TextFormat::simple(FontId::proportional(12.5), t.text.gamma_multiply(0.5)),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        text_clip.galley(
            pos2(summary.right() + 10.0, mid - galley.size().y / 2.0),
            galley,
            t.text,
        );
    }
}

/// The hash, author, when and date columns, those the settings show.
fn trailing_columns(ui: &Ui, painter: &Painter, t: &Tokens, row: &Row, now: i64) {
    let (rect, mid, cols, commit) = (row.rect, row.mid(), row.cols, row.commit);
    let small = FontId::proportional(11.5);
    if cols.hash > 0.0 {
        painter.text(
            pos2(rect.right() - cols.trailing(), mid),
            Align2::LEFT_CENTER,
            commit.hash.get(..7).unwrap_or(&commit.hash),
            theme::mono(11.0),
            t.text_faint,
        );
    }
    if cols.author > 0.0 {
        let author_clip = Rect::from_min_max(
            pos2(
                rect.right() - cols.author - cols.when - cols.date,
                rect.top(),
            ),
            pos2(rect.right() - cols.when - cols.date - 8.0, rect.bottom()),
        );
        let avatar = pos2(author_clip.left() + AVATAR_D / 2.0, mid);
        widgets::avatar(ui, avatar, AVATAR_D / 2.0, &commit.author, &commit.email);
        painter.with_clip_rect(author_clip).text(
            pos2(author_clip.left() + AVATAR_D + 6.0, mid),
            Align2::LEFT_CENTER,
            &commit.author,
            small.clone(),
            t.text_dim,
        );
    }
    if cols.when > 0.0 {
        let when_clip = Rect::from_min_max(
            pos2(rect.right() - cols.when - cols.date, rect.top()),
            pos2(rect.right() - cols.date - 8.0, rect.bottom()),
        );
        painter.with_clip_rect(when_clip).text(
            pos2(when_clip.left(), mid),
            Align2::LEFT_CENTER,
            git::relative_time(commit.time, now),
            small.clone(),
            t.text_dim,
        );
    }
    if cols.date > 0.0 {
        painter.text(
            pos2(rect.right() - cols.date, mid),
            Align2::LEFT_CENTER,
            &commit.date,
            theme::mono(11.0),
            t.text_faint,
        );
    }
}

/// A small rounded count label; returns its width.
fn pill(painter: &Painter, left_center: Pos2, text: &str, color: Color32) -> f32 {
    let galley = painter.layout_no_wrap(text.to_owned(), theme::medium(11.0), color);
    let w = galley.size().x + 12.0;
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - 9.0), vec2(w, 18.0));
    painter.rect_filled(r, CornerRadius::same(9), color.gamma_multiply(0.18));
    painter.galley(
        pos2(r.left() + 6.0, r.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    w
}
