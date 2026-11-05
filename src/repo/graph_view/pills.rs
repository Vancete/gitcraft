//! The refs column of a row: its branch and tag pills and the `+N` stack of the others.

use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, pos2,
    vec2,
};

use super::DIMMED;
use super::badge::{Badge, BadgeKind, badges};
use super::menu::pill_menu_id;
use super::row::Row;
use crate::ci::CheckState;
use crate::icons;
use crate::repo::columns::Cols;
use crate::repo::{Action, RepoTab, ci_status};
use crate::theme::{self, Tokens};

impl RepoTab {
    /// Branch and tag pills, left of the graph, joined to the commit's dot. Returns `resp`
    /// with the column's tooltip.
    pub(super) fn refs_cell(
        &self,
        ui: &Ui,
        painter: &Painter,
        row: &Row,
        resp: Response,
        acts: &mut Vec<Action>,
    ) -> Response {
        let t = Tokens::get(ui.ctx());
        let badges = badges(&row.commit.refs, &self.refs);
        let (Some(first), Some(node_x)) = (badges.first(), row.node_x) else {
            return resp;
        };
        let (rect, mid, gx, cols) = (row.rect, row.mid(), row.gx, row.cols);
        let refs_clip = Rect::from_min_max(rect.min, pos2(gx - 4.0, rect.bottom()));
        let rp = painter.with_clip_rect(refs_clip);
        let more = badges.len() - 1;
        let more_w = if more > 0 { 28.0 } else { 0.0 };
        let max_w = cols.refs - 10.0 - 5.0 - more_w - 14.0;
        let pill = ref_pill(
            ui,
            &rp,
            pos2(rect.left() + 8.0, mid),
            first,
            row.color,
            max_w.max(40.0),
            self.marks(first, &row.commit.hash),
        );
        self.pill_clicks(ui, &resp, pill, first, &row.commit.hash, acts);
        let mut end = pill.right();
        let mut more_rect = None;
        if more > 0 {
            let r = Rect::from_min_size(pos2(end + 3.0, mid - 10.0), vec2(more_w - 3.0, 20.0));
            more_count(&rp, &t, r, more, row.color);
            end = r.right();
            more_rect = Some(r);
        }
        // Connector to the dot, drawn first so the lanes cross over it.
        painter.line_segment(
            [pos2(end, mid), pos2(node_x.min(gx + cols.graph), mid)],
            Stroke::new(1.5, row.color.gamma_multiply(0.8)),
        );
        let stack_open = more_rect.is_some_and(|more| {
            let at = pos2(rect.left() + 2.0, mid - 16.0);
            self.ref_stack(ui, &badges, row.i, more, at, cols, row.color, acts)
        });
        let refs_cell = Rect::from_min_max(rect.min, pos2(gx, rect.bottom()));
        if !stack_open && ui.rect_contains_pointer(refs_cell) {
            let lines: Vec<String> = badges.iter().map(Badge::describe).collect();
            return resp.on_hover_text(format!(
                "{}\nTip commit by {} · {}\nDouble-click a pill to check it out",
                lines.join("\n"),
                row.commit.author,
                row.commit.date
            ));
        }
        resp
    }

    /// What a pill of `badge` on commit `hash` carries besides its name: whether the branch has
    /// an open pull request, and the build status of its tip.
    fn marks(&self, badge: &Badge, hash: &str) -> Marks {
        let branch = badge.kind == BadgeKind::Branch;
        Marks {
            pr: branch && self.prs.iter().any(|p| p.branch == badge.label),
            ci: self
                .ci_status(hash)
                .filter(|_| branch)
                .map(|status| status.state),
        }
    }

    /// Clicks on a row's own pill (at `pill`): right-clicking a branch pill puts that branch's
    /// actions on top of the row's menu, double-clicking checks it out.
    fn pill_clicks(
        &self,
        ui: &Ui,
        resp: &Response,
        pill: Rect,
        badge: &Badge,
        hash: &str,
        acts: &mut Vec<Action>,
    ) {
        let on_pill = || {
            ui.ctx()
                .pointer_interact_pos()
                .is_some_and(|p| pill.contains(p))
        };
        if resp.secondary_clicked() {
            let branch = (on_pill() && badge.kind == BadgeKind::Branch).then(|| badge.git_ref());
            ui.data_mut(|d| d.insert_temp(pill_menu_id(hash), branch));
        }
        if resp.double_clicked()
            && on_pill()
            && let Some(act) = self.pill_checkout(badge)
        {
            acts.push(act);
        }
    }

    /// What double-clicking a pill does: check out that branch (a tracking branch for one only on
    /// a remote, and pulling a local one behind its upstream) or tag; pull when it is the
    /// checked-out branch's remote, further on. Nothing for the checked-out one or while an
    /// operation runs.
    fn pill_checkout(&self, badge: &Badge) -> Option<Action> {
        if badge.head || self.busy_op.is_some() {
            return None;
        }
        // The checked-out branch's remote pill, further on than the branch: pull.
        let current = self.refs.current.as_deref() == Some(badge.label.as_str());
        if badge.kind == BadgeKind::Branch && !badge.local && current {
            let behind = self
                .refs
                .local
                .iter()
                .find(|b| b.name == badge.label)
                .is_some_and(|b| b.ahead_behind().1 > 0);
            return behind.then_some(Action::Pull);
        }
        match badge.kind {
            BadgeKind::Branch if badge.local => {
                let behind = self
                    .refs
                    .local
                    .iter()
                    .find(|b| b.name == badge.label)
                    .is_some_and(|b| matches!(b.ahead_behind(), (0, behind) if behind > 0));
                Some(if behind {
                    Action::CheckoutPull(badge.label.clone())
                } else {
                    Action::Checkout(badge.label.clone())
                })
            }
            BadgeKind::Branch => badge
                .remotes
                .first()
                .map(|remote| Action::CheckoutRemote(format!("{remote}/{}", badge.label))),
            BadgeKind::Tag => Some(Action::Checkout(format!("refs/tags/{}", badge.label))),
            BadgeKind::DetachedHead => None,
        }
    }

    /// While the pointer is on a row's `+N` (or on the list itself), every pill of the commit
    /// stacked over the row, to pick one to check out. Returns whether it is open.
    #[allow(clippy::too_many_arguments)]
    fn ref_stack(
        &self,
        ui: &Ui,
        badges: &[Badge],
        i: usize,
        more: Rect,
        at: Pos2,
        cols: Cols,
        lane: Color32,
        acts: &mut Vec<Action>,
    ) -> bool {
        let t = Tokens::get(ui.ctx());
        let id = ui.id().with(("ref_stack", i));
        let last: Option<Rect> = ui.data(|d| d.get_temp(id));
        let slot_id = |n: usize| id.with(("slot", n));
        // A pill's menu keeps the list up while the pointer is on the menu.
        let menu_open =
            (0..badges.len()).any(|n| egui::Popup::is_id_open(ui.ctx(), slot_id(n).with("popup")));
        let open = menu_open
            || ui.ctx().pointer_hover_pos().is_some_and(|p| {
                more.contains(p) || last.is_some_and(|r| r.expand(2.0).contains(p))
            });
        if !open {
            ui.data_mut(|d| d.remove::<Rect>(id));
            return false;
        }
        let slot_w = (cols.refs - 16.0).max(160.0);
        let hash = self.commits.get(i).map_or("", |c| c.hash.as_str());
        let shown = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for (n, b) in badges.iter().enumerate() {
                            let (slot, _) =
                                ui.allocate_exact_size(vec2(slot_w, 20.0), Sense::hover());
                            let resp = ui.interact(slot, slot_id(n), Sense::click());
                            if resp.hovered()
                                || menu_open
                                    && egui::Popup::is_id_open(ui.ctx(), slot_id(n).with("popup"))
                            {
                                ui.painter().rect_filled(
                                    slot.expand(2.0),
                                    CornerRadius::same(t.radius as u8),
                                    t.hover,
                                );
                            }
                            let marks = self.marks(b, hash);
                            ref_pill(ui, ui.painter(), slot.left_center(), b, lane, slot_w, marks);
                            self.stack_slot_clicks(resp, b, i, acts);
                        }
                    });
            });
        ui.data_mut(|d| d.insert_temp(id, shown.response.rect));
        true
    }

    /// Clicks on one pill of a row's stack, which act like those on the row's own pill.
    fn stack_slot_clicks(&self, resp: Response, b: &Badge, i: usize, acts: &mut Vec<Action>) {
        let hint = match self.pill_checkout(b) {
            _ if b.head => "checked out",
            Some(Action::Pull) => "double-click to pull",
            _ => "double-click to check out",
        };
        let resp = resp.on_hover_text(format!("{} ({hint})", b.describe()));
        if resp.double_clicked() {
            if let Some(act) = self.pill_checkout(b) {
                acts.push(act);
            }
        } else if resp.clicked() {
            acts.push(Action::Select(i));
        }
        // The same menu as right-clicking the row's own pill.
        let branch = (b.kind == BadgeKind::Branch).then(|| b.git_ref());
        egui::Popup::context_menu(&resp)
            .style(crate::theme::menu_style)
            .show(|ui| self.commit_menu_items(ui, i, branch, self.busy_op.is_none(), acts));
    }
}

/// The `+N` after a row's pill, counting the commit's other pills.
fn more_count(painter: &Painter, t: &Tokens, r: Rect, more: usize, lane: Color32) {
    painter.rect_filled(
        r,
        CornerRadius::same(t.radius as u8),
        lane.lerp_to_gamma(Color32::BLACK, 0.35)
            .lerp_to_gamma(Color32::BLACK, DIMMED),
    );
    painter.text(
        r.center(),
        Align2::CENTER_CENTER,
        format!("+{more}"),
        theme::medium(10.5),
        Color32::WHITE,
    );
}

/// What a pill carries besides its name and where the branch lives.
#[derive(Clone, Copy, Default)]
struct Marks {
    /// The branch has an open pull request.
    pr: bool,
    /// The build status of its tip.
    ci: Option<CheckState>,
}

/// Paints a pill with white text and icons, at most `max_w` wide; returns its rect.
fn ref_pill(
    ui: &Ui,
    painter: &Painter,
    left_center: Pos2,
    badge: &Badge,
    lane: Color32,
    max_w: f32,
    marks: Marks,
) -> Rect {
    const ICON: f32 = 15.0;
    let t = Tokens::get(ui.ctx());
    let white = Color32::WHITE;
    let fill = match badge.kind {
        BadgeKind::Branch if badge.local => lane,
        // Only on a remote: a darker shade of the lane.
        BadgeKind::Branch => lane.lerp_to_gamma(Color32::BLACK, 0.35),
        BadgeKind::Tag => Color32::from_rgb(104, 104, 112),
        BadgeKind::DetachedHead => t.warning,
    };
    // Every pill but the checked-out one is a darker shade of its colour.
    let fill = if badge.head {
        fill
    } else {
        fill.lerp_to_gamma(Color32::BLACK, DIMMED)
    };
    let (lead, trail) = pill_icons(badge, marks.pr);
    let font = if badge.head {
        theme::semibold(11.0)
    } else {
        theme::medium(11.0)
    };
    let dot_w = if marks.ci.is_some() { 12.0 } else { 0.0 };
    let icons_w = (lead.len() + trail.len()) as f32 * ICON + dot_w;
    let text_room = (max_w - 16.0 - icons_w).max(16.0);
    let label = truncate(painter, &badge.label, &font, text_room);
    let galley = painter.layout_no_wrap(label, font, white);
    let w = (galley.size().x + 16.0 + icons_w).min(max_w);
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - 10.0), vec2(w, 20.0));
    painter.rect_filled(r, CornerRadius::same(t.radius as u8), fill);
    let mut x = r.left() + 7.0;
    for icon in lead {
        icons::paint(
            ui,
            Rect::from_center_size(pos2(x + 6.0, r.center().y), vec2(12.0, 12.0)),
            icon,
            11.0,
            white,
        );
        x += ICON;
    }
    let gw = galley.size().x;
    painter.galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, white);
    x += gw + 1.0;
    for icon in trail {
        icons::paint(
            ui,
            Rect::from_center_size(pos2(x + 8.0, r.center().y), vec2(12.0, 12.0)),
            icon,
            11.0,
            white,
        );
        x += ICON;
    }
    if let Some(state) = marks.ci {
        // On a ring of the pill's colour, so it reads on any lane.
        let center = pos2((x + 7.0).min(r.right() - 7.0), r.center().y);
        painter.circle_filled(center, ci_status::DOT_R + 1.5, Color32::WHITE);
        ci_status::paint_dot(painter, &t, center, ci_status::DOT_R, state);
    }
    r
}

/// The icons before and after a pill's label; `pr`: the branch has a pull request.
fn pill_icons(badge: &Badge, pr: bool) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut lead = Vec::new();
    let mut trail = Vec::new();
    match badge.kind {
        BadgeKind::Tag => lead.push("tag"),
        BadgeKind::DetachedHead => lead.push("check"),
        BadgeKind::Branch => {
            if badge.head {
                lead.push("check");
            }
            // Where the branch lives: this computer, a remote, or both.
            if badge.local {
                trail.push("laptop");
            }
            if !badge.remotes.is_empty() {
                trail.push("cloud");
            }
            if pr {
                trail.push("git-pull-request");
            }
        }
    }
    (lead, trail)
}

/// `text`, cut with an ellipsis to fit `width`.
fn truncate(painter: &Painter, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
            <= width
    };
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let s: String = chars.iter().take(n).collect::<String>() + "…";
        if fits(&s) {
            return s;
        }
    }
    "…".to_owned()
}
