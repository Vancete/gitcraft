//! The graph column of a row: the lanes crossing it, the lines to its dot, and the dot.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Stroke, Ui, pos2, vec2};

use super::CORNER_R;
use super::row::Row;
use crate::icons;
use crate::repo::{RepoTab, WIP_HASH};
use crate::theme::Tokens;

impl RepoTab {
    /// The lanes crossing the row, the lines joining its dot to other lanes, and the dot: a
    /// dashed circle for the changes row, a dashed box for a stash.
    pub(super) fn graph_cell(&self, ui: &Ui, painter: &Painter, row: &Row, lanes: usize) {
        let Some(graph_row) = self.graph.rows.get(row.i) else {
            return;
        };
        let t = Tokens::get(ui.ctx());
        let is_wip = row.commit.hash == WIP_HASH;
        let stash = self.stash_at(row.i).and_then(|s| self.stashes.get(s));
        let rect = row.rect;
        let (top, bottom) = (rect.top(), rect.bottom());
        let gp = painter.with_clip_rect(rect.intersect(Rect::from_min_size(
            pos2(row.gx, top),
            vec2(row.cols.graph, rect.height()),
        )));
        let x = |lane: usize| row.lane_x(lane);
        let node = pos2(x(graph_row.col), row.mid());
        let width = 2.0;

        for &lane in &graph_row.through {
            if lane < lanes {
                gp.line_segment(
                    [pos2(x(lane), top), pos2(x(lane), bottom)],
                    Stroke::new(width, t.lane(lane)),
                );
            }
        }
        for &lane in &graph_row.incoming {
            edge(&gp, node, pos2(x(lane), top), t.lane(lane), width);
        }
        for &lane in &graph_row.outgoing {
            let color = if is_wip || stash.is_some() {
                t.text_faint
            } else {
                t.lane(lane)
            };
            edge(&gp, node, pos2(x(lane), bottom), color, width);
        }

        if stash.is_some() {
            stash_box(ui, &gp, &t, node, row.gx + row.cols.graph);
        } else if is_wip {
            gp.circle_filled(node, 6.0, t.dock);
            gp.add(egui::Shape::dashed_line(
                &circle_points(node, 6.0, 16),
                Stroke::new(1.5, t.text_dim),
                2.5,
                2.0,
            ));
        } else if row.commit.parents.len() > 1 {
            gp.circle_filled(node, 3.5, row.color);
        } else {
            gp.circle_filled(node, 6.0, row.color);
            if row.commit.hash == self.refs.head {
                gp.circle_filled(node, 2.5, t.dock);
            }
        }
    }
}

/// A stash's dot: a dashed box with the stash icon, as the toolbar's Stash button shows it.
/// `graph_right` is where the graph column ends.
fn stash_box(ui: &Ui, gp: &Painter, t: &Tokens, node: Pos2, graph_right: f32) {
    let r = Rect::from_center_size(node, vec2(15.0, 15.0));
    gp.rect_filled(r, CornerRadius::same(3), t.dock);
    let (l, tp, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    gp.add(egui::Shape::dashed_line(
        &[
            pos2(l, tp),
            pos2(rt, tp),
            pos2(rt, b),
            pos2(l, b),
            pos2(l, tp),
        ],
        Stroke::new(1.5, t.text_dim),
        2.5,
        2.0,
    ));
    // Images ignore the clip; a squeezed graph column leaves the icon out.
    if r.right() <= graph_right {
        icons::paint(ui, r, "archive", 10.0, t.text_dim);
    }
}

fn circle_points(center: Pos2, radius: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect()
}

/// Joins a commit's dot to another lane at `end` (the row's top or bottom): straight across
/// the row from the dot, a small rounded corner, then straight along the lane.
fn edge(p: &Painter, node: Pos2, end: Pos2, color: Color32, width: f32) {
    let stroke = Stroke::new(width, color);
    let dx = end.x - node.x;
    if dx.abs() < 0.5 {
        p.line_segment([node, end], stroke);
        return;
    }
    let dy = end.y - node.y;
    let r = CORNER_R.min(dx.abs()).min(dy.abs());
    let (sx, sy) = (dx.signum(), dy.signum());
    // The corner's centre: inside the turn, `r` before the lane and `r` past the dot's row.
    let c = pos2(end.x - sx * r, node.y + sy * r);
    let mut points = vec![node];
    const STEPS: usize = 8;
    for k in 0..=STEPS {
        let a = k as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
        points.push(c + vec2(sx * r * a.sin(), -sy * r * a.cos()));
    }
    points.push(end);
    p.add(egui::Shape::line(points, stroke));
}
