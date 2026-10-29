//! Central pane: the commit graph of every branch, with branch pills on the left, search and
//! older history loaded as you scroll.

use egui::{
    Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2,
};

use super::widgets;
use super::{
    Action, Dialog, Refresh, RepoTab, Undo, WIP_HASH, compare_action, fast_forward_action,
    merge_action, ops, rebase_action,
};
use crate::git::{self, Refs};
use crate::icons;
use crate::settings::Settings;
use crate::theme::{self, Tokens};

const ROW_H: f32 = 28.0;
/// Space between rows, which the graph's lines cross.
const ROW_GAP: f32 = 2.0;
/// From one row to the next.
const ROW_PITCH: f32 = ROW_H + ROW_GAP;
const LANE_W: f32 = 16.0;
/// Radius of the corner where a line from a dot turns into another lane.
const CORNER_R: f32 = 7.0;
const MAX_LANES_SHOWN: usize = 24;
/// How much darker the pills that are not checked out are (0 = same colour, 1 = black).
const DIMMED: f32 = 0.4;
const AUTHOR_W: f32 = 150.0;
const DATE_W: f32 = 118.0;
const HASH_W: f32 = 70.0;
const AVATAR_D: f32 = 20.0;

/// Column widths for this frame (from the settings).
#[derive(Clone, Copy)]
struct Cols {
    refs: f32,
    graph: f32,
    hash: f32,
    author: f32,
    date: f32,
}

impl Cols {
    /// Width of everything right of the description.
    fn trailing(&self) -> f32 {
        self.hash + self.author + self.date
    }
}

impl RepoTab {
    pub(super) fn graph_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        if self.search.open {
            self.search_bar(ui, acts);
        }
        if let Some(err) = &self.load_error {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(t.danger, err);
            });
            return;
        }
        if self.commits.is_empty() {
            ui.centered_and_justified(|ui| {
                if self.loading {
                    ui.spinner();
                } else {
                    ui.label(
                        egui::RichText::new("This repository has no commits yet.")
                            .color(t.text_dim),
                    );
                }
            });
            return;
        }

        // Keyboard navigation (only when no text field has focus).
        if !ui.ctx().egui_wants_keyboard_input() && self.dialog.is_none() {
            let (up, down) =
                ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
            let current = self.selected.unwrap_or(0);
            let next = if up {
                Some(current.saturating_sub(1))
            } else if down {
                Some((current + 1).min(self.commits.len().saturating_sub(1)))
            } else {
                None
            };
            if let Some(next) = next {
                acts.push(Action::Select(next));
                self.scroll_to = Some(next);
            }
        }

        let settings = Settings::get(ui.ctx());
        let lanes = self.graph.width.clamp(1, MAX_LANES_SHOWN);
        let cols = Cols {
            refs: settings.refs_width.clamp(150.0, 600.0),
            graph: (lanes as f32 * LANE_W + 12.0).max(60.0),
            hash: if settings.show_hash { HASH_W } else { 0.0 },
            author: if settings.show_author { AUTHOR_W } else { 0.0 },
            date: if settings.show_date { DATE_W } else { 0.0 },
        };

        // Column headers.
        let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        {
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
                labels.push((right - cols.author - cols.date, "AUTHOR"));
            }
            if cols.date > 0.0 {
                labels.push((right - cols.date, "DATE"));
            }
            for (x, label) in labels {
                p.text(
                    pos2(x, y),
                    Align2::LEFT_CENTER,
                    label,
                    font.clone(),
                    t.text_faint,
                );
            }
        }

        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("graph_scroll")
            .auto_shrink(false);
        let offset_id = ui.id().with("graph_offset");
        if let Some(row) = self.scroll_to.take() {
            let view_h = ui.available_height();
            let top = row as f32 * ROW_PITCH;
            // Only scroll if the row is outside the visible part.
            let offset = ui.data(|d| d.get_temp::<f32>(offset_id)).unwrap_or(0.0);
            if top < offset || top + ROW_PITCH > offset + view_h {
                scroll = scroll.vertical_scroll_offset((top - view_h / 2.0).max(0.0));
            }
        }

        // `show_rows` sizes rows as ROW_PITCH + item spacing, so the spacing must be zero here.
        ui.spacing_mut().item_spacing.y = 0.0;
        let idle = self.busy_op.is_none();
        let now = git::now();
        let total = self.commits.len() + usize::from(self.has_more);
        let out = scroll.show_rows(ui, ROW_PITCH, total, |ui, range| {
            // Near the end: fetch the next page of history.
            if range.end + 40 >= self.commits.len() && self.has_more && !self.loading_more {
                acts.push(Action::LoadMore);
            }
            for i in range {
                if i >= self.commits.len() {
                    let (rect, _) = ui
                        .allocate_exact_size(vec2(ui.available_width(), ROW_PITCH), Sense::hover());
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "Loading older commits…",
                        FontId::proportional(12.0),
                        t.text_faint,
                    );
                    continue;
                }
                let resp = self.commit_row(ui, i, cols, lanes, now, acts);
                if resp.clicked() {
                    // Ctrl+click: compare the selected commit with this one.
                    let compare = ui
                        .input(|inp| inp.modifiers.command)
                        .then(|| self.compare_rows(self.selected?, i))
                        .flatten();
                    acts.push(compare.unwrap_or(Action::Select(i)));
                }
                let Some(commit) = self.commits.get(i) else {
                    continue;
                };
                if commit.hash == WIP_HASH {
                    egui::Popup::context_menu(&resp)
                        .style(crate::theme::menu_style)
                        .show(|ui| {
                            if ui
                                .add_enabled(idle, egui::Button::new("Stage all changes"))
                                .clicked()
                            {
                                acts.push(Action::Ops {
                                    label: "Stage all".into(),
                                    ops: ops(&[&["add", "-A"]]),
                                    refresh: Refresh::Status,
                                });
                            }
                            if ui
                                .add_enabled(idle, egui::Button::new("Stash changes…"))
                                .clicked()
                            {
                                acts.push(Action::Dialog(Dialog::stash()));
                            }
                        });
                    continue;
                }
                self.commit_menu(&resp, i, idle, acts);
            }
        });
        ui.data_mut(|d| d.insert_temp(offset_id, out.state.offset.y));
    }

    /// Compares two rows of the graph, the older one first; the changes row stands for the
    /// working tree. `None` for the same row twice.
    fn compare_rows(&self, a: usize, b: usize) -> Option<Action> {
        if a == b {
            return None;
        }
        // Rows go newest first.
        let (old, new) = (a.max(b), a.min(b));
        let end = |row: usize| {
            let c = self.commits.get(row)?;
            let label = badges(&c.refs, &self.refs)
                .into_iter()
                .find(|b| b.kind != BadgeKind::DetachedHead)
                .map(|b| b.label)
                .unwrap_or_else(|| super::short(&c.hash).to_owned());
            Some((c.hash.clone(), label))
        };
        let (base, base_label) = end(old)?;
        let (target, target_label) = if self.is_wip(new) {
            (None, "working tree".to_owned())
        } else {
            let (hash, label) = end(new)?;
            (Some(hash), label)
        };
        Some(Action::Compare {
            base,
            base_label,
            target,
            target_label,
            since_fork: false,
        })
    }

    /// Right-click menu of a commit row.
    fn commit_menu(&self, resp: &Response, i: usize, idle: bool, acts: &mut Vec<Action>) {
        let Some(commit) = self.commits.get(i) else {
            return;
        };
        // The branch whose pill was right-clicked, if any.
        let pill: Option<String> = resp
            .ctx
            .data(|d| d.get_temp::<Option<String>>(pill_menu_id(&commit.hash)))
            .flatten();
        egui::Popup::context_menu(resp)
            .style(crate::theme::menu_style)
            .show(|ui| self.commit_menu_items(ui, i, pill, idle, acts));
    }

    /// The entries of a commit's menu; `pill` is the branch whose pill was right-clicked, whose
    /// actions go on top.
    fn commit_menu_items(
        &self,
        ui: &mut Ui,
        i: usize,
        pill: Option<String>,
        idle: bool,
        acts: &mut Vec<Action>,
    ) {
        let Some(commit) = self.commits.get(i) else {
            return;
        };
        let hash = commit.hash.clone();
        let short = hash.get(..8).unwrap_or(&hash).to_owned();
        let summary = commit.summary.clone();
        let is_head = hash == self.refs.head;
        // Local branches pointing here get their own submenu.
        let here: Vec<&git::Branch> = self
            .refs
            .local
            .iter()
            .filter(|b| b.target == hash)
            .collect();
        // The right-clicked branch, else the commit's first branch, for copying.
        let branch_name = pill.clone().or_else(|| {
            badges(&commit.refs, &self.refs)
                .into_iter()
                .find(|b| b.kind == BadgeKind::Branch)
                .map(|b| b.git_ref())
        });
        let target = self
            .refs
            .current
            .clone()
            .unwrap_or_else(|| "HEAD".to_owned());
        if let Some(branch) = &pill
            && self.refs.current.as_deref() != Some(branch.as_str())
        {
            if ui
                .add_enabled(idle, egui::Button::new(format!("Check out {branch}")))
                .clicked()
            {
                let local = self.refs.local.iter().any(|b| &b.name == branch);
                acts.push(if local {
                    Action::Checkout(branch.clone())
                } else {
                    Action::CheckoutRemote(branch.clone())
                });
            }
            if let Some(current) = self.refs.current.as_deref() {
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Fast-forward {current} to {branch}")),
                    )
                    .clicked()
                {
                    acts.push(fast_forward_action(branch, current));
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Merge {branch} into {current}")),
                    )
                    .clicked()
                {
                    acts.push(merge_action(branch, current));
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Rebase {current} onto {branch}")),
                    )
                    .clicked()
                {
                    acts.push(rebase_action(branch, current));
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Interactive rebase {current} onto {branch}…")),
                    )
                    .clicked()
                {
                    acts.push(Action::InteractiveRebase {
                        onto: branch.clone(),
                        label: branch.clone(),
                    });
                }
                ui.separator();
                compare_items(ui, acts, branch, current);
            }
            ui.separator();
        }
        for b in &here {
            ui.menu_button(format!("Branch {}", b.name), |ui| {
                self.branch_menu_items(ui, acts, b, idle);
            });
        }
        if !here.is_empty() {
            ui.separator();
        }
        if ui.button("Create branch here…").clicked() {
            acts.push(Action::Dialog(Dialog::create_branch(&hash, &short)));
        }
        if ui.button("Create tag here…").clicked() {
            acts.push(Action::Dialog(Dialog::tag(
                &hash,
                &short,
                self.default_remote(),
            )));
        }
        if ui
            .add_enabled(
                idle && !is_head,
                egui::Button::new("Check out this commit (detached)"),
            )
            .clicked()
        {
            acts.push(Action::Checkout(hash.clone()));
        }
        ui.separator();
        if let Some(compare) = self
            .selected
            .filter(|&s| s != i)
            .and_then(|s| self.compare_rows(s, i))
        {
            let other = self
                .selected
                .and_then(|s| self.commits.get(s))
                .map(|c| {
                    if c.hash == WIP_HASH {
                        "the working tree".to_owned()
                    } else {
                        super::short(&c.hash).to_owned()
                    }
                })
                .unwrap_or_default();
            if ui
                .button(format!("Compare with {other}"))
                .on_hover_text("Also Ctrl+click on a commit")
                .clicked()
            {
                acts.push(compare);
            }
        }
        if ui.button("Compare with the working tree").clicked() {
            acts.push(Action::Compare {
                base: hash.clone(),
                base_label: short.clone(),
                target: None,
                target_label: "working tree".to_owned(),
                since_fork: false,
            });
        }
        ui.separator();
        if ui
            .add_enabled(
                idle && !is_head,
                egui::Button::new(format!("Interactive rebase {target} onto this commit…")),
            )
            .on_hover_text("Reorder, reword, squash or drop the commits after this one")
            .clicked()
        {
            acts.push(Action::InteractiveRebase {
                onto: hash.clone(),
                label: short.clone(),
            });
        }
        if ui
            .add_enabled(
                idle && !is_head,
                egui::Button::new("Cherry-pick onto current branch"),
            )
            .clicked()
        {
            acts.push(Action::UndoableOps {
                label: format!("Cherry-pick {short}"),
                ops: ops(&[&["cherry-pick", &hash]]),
                refresh: Refresh::All,
                undo: Undo::HeadMove { mode: "--keep" },
            });
        }
        if ui
            .add_enabled(idle, egui::Button::new("Revert commit"))
            .clicked()
        {
            acts.push(Action::UndoableOps {
                label: format!("Revert {short}"),
                ops: ops(&[&["revert", "--no-edit", &hash]]),
                refresh: Refresh::All,
                undo: Undo::HeadMove { mode: "--keep" },
            });
        }
        ui.add_enabled_ui(idle && !is_head, |ui| {
                ui.menu_button(format!("Reset {target} to here"), |ui| {
                    for (mode, undo_mode, label, explain) in [
                        ("--soft", "--soft", "Soft", "Keeps every change, staged."),
                        ("--mixed", "--mixed", "Mixed", "Keeps every change, unstaged."),
                        ("--hard", "--keep", "Hard", "Throws away uncommitted changes and the commits after this one. Uncommitted work can't be recovered."),
                    ] {
                        if ui.button(format!("{label} — {}", explain.split('.').next().unwrap_or(explain))).clicked() {
                            acts.push(Action::Dialog(
                                Dialog::confirm(
                                    &format!("{label} reset"),
                                    format!("Move {target} to {short} ({summary})? {explain}"),
                                    &format!("Reset {}", label.to_lowercase()),
                                    ops(&[&["reset", mode, &hash]]),
                                    None,
                                )
                                .undo(Undo::HeadMove { mode: undo_mode }),
                            ));
                        }
                    }
                });
            });
        ui.separator();
        if ui.button("Copy commit hash").clicked() {
            ui.ctx().copy_text(hash.clone());
        }
        if ui.button("Copy message").clicked() {
            ui.ctx().copy_text(summary.clone());
        }
        if let Some(name) = &branch_name
            && ui.button(format!("Copy branch name ({name})")).clicked()
        {
            ui.ctx().copy_text(name.clone());
        }
    }

    /// What double-clicking a pill does: check out that branch (a tracking branch for one only on
    /// a remote, and pulling a local one behind its upstream) or tag. Nothing for the checked-out
    /// one or while an operation runs.
    fn pill_checkout(&self, badge: &Badge) -> Option<Action> {
        if badge.head || self.busy_op.is_some() {
            return None;
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
                            let pr = b.kind == BadgeKind::Branch
                                && self.prs.iter().any(|p| p.branch == b.label);
                            ref_pill(ui, ui.painter(), slot.left_center(), b, lane, slot_w, pr);
                            let hint = if b.head {
                                "checked out"
                            } else {
                                "double-click to check out"
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
                                .show(|ui| {
                                    self.commit_menu_items(
                                        ui,
                                        i,
                                        branch,
                                        self.busy_op.is_none(),
                                        acts,
                                    )
                                });
                        }
                    });
            });
        ui.data_mut(|d| d.insert_temp(id, shown.response.rect));
        true
    }

    fn search_bar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "search", 15.0, t.text_faint);
                    let code = self.search.code;
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.search.query)
                            .hint_text(if code {
                                "Search code added or removed by a commit, in all history"
                            } else {
                                "Search messages, authors and hashes in all history"
                            })
                            .desired_width(ui.available_width() - 290.0)
                            .margin(egui::Margin::symmetric(8, 4)),
                    );
                    // Where to look: commit messages or the code they changed.
                    for (label, on, tip) in [
                        (
                            "Messages",
                            false,
                            "Search commit messages, authors and hashes",
                        ),
                        (
                            "Code",
                            true,
                            "Search the commits that added or removed this text in a file \
                             (git log -S)",
                        ),
                    ] {
                        if ui
                            .selectable_label(code == on, label)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            acts.push(Action::SearchCode(on));
                        }
                    }
                    if std::mem::take(&mut self.search.focus) {
                        edit.request_focus();
                    }
                    if edit.changed() {
                        self.search.edited = Some(std::time::Instant::now());
                    }
                    let (enter, shift, esc) = ui.input(|i| {
                        (
                            i.key_pressed(Key::Enter),
                            i.modifiers.shift,
                            i.key_pressed(Key::Escape),
                        )
                    });
                    if edit.lost_focus() && enter {
                        acts.push(Action::SearchStep(if shift { -1 } else { 1 }));
                        edit.request_focus();
                    }
                    if edit.has_focus() && esc {
                        acts.push(Action::ToggleSearch);
                    }
                    let n = self.search.results.len();
                    let summary = if self.search.running {
                        "searching…".to_owned()
                    } else if self.search.searched.is_empty() {
                        String::new()
                    } else if n == 0 {
                        "no results".to_owned()
                    } else {
                        format!("{} of {n}", self.search.current + 1)
                    };
                    ui.label(egui::RichText::new(summary).color(t.text_dim).small());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::ToggleSearch);
                        }
                        if icons::small_button(ui, "chevron-down", "Next (Enter)").clicked() {
                            acts.push(Action::SearchStep(1));
                        }
                        if icons::small_button(ui, "chevron-up", "Previous (Shift+Enter)").clicked()
                        {
                            acts.push(Action::SearchStep(-1));
                        }
                    });
                });
            });
    }

    fn commit_row(
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
        // The row's own band, without the gap.
        let band = rect.shrink2(vec2(0.0, ROW_GAP / 2.0));

        if self.selected == Some(i) {
            painter.rect_filled(band, 0.0, t.row_selected);
        } else if resp.hovered() {
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
        // An end of the comparison shown on the right (the changes row for the working tree).
        let compared = self
            .compare
            .as_ref()
            .and_then(|c| c.data.as_ref()?.as_ref().ok())
            .is_some_and(|d| {
                d.from == commit.hash
                    || d.to.as_deref() == Some(commit.hash.as_str())
                    || (is_wip && d.to.is_none())
            });
        if compared {
            painter.rect_stroke(
                band.shrink(1.0),
                0.0,
                Stroke::new(1.0, t.accent),
                egui::StrokeKind::Inside,
            );
        }

        let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
        let gx = rect.left() + cols.refs;
        let x = |lane: usize| gx + 6.0 + lane as f32 * LANE_W + LANE_W / 2.0;
        let row_color = self
            .graph
            .rows
            .get(i)
            .map(|r| t.lane(r.col))
            .unwrap_or(t.text_faint);
        let node_x = self.graph.rows.get(i).map(|r| x(r.col));

        // Branch and tag pills, left of the graph, joined to the commit's dot, with the tip's
        // author and how long ago it was made.
        let badges = badges(&commit.refs, &self.refs);
        let mut resp = resp;
        if let (Some(first), Some(node_x)) = (badges.first(), node_x) {
            let refs_clip = Rect::from_min_max(rect.min, pos2(gx - 4.0, bottom));
            let rp = painter.with_clip_rect(refs_clip);
            let when = git::relative_time(commit.time, now);
            let when_galley = rp.layout_no_wrap(when, theme::medium(10.5), t.text_dim);
            let when_w = when_galley.size().x + 12.0;
            let more = badges.len() - 1;
            let more_w = if more > 0 { 28.0 } else { 0.0 };
            let max_w = cols.refs - 10.0 - AVATAR_D - 8.0 - more_w - when_w - 14.0;
            let has_pr =
                first.kind == BadgeKind::Branch && self.prs.iter().any(|p| p.branch == first.label);
            let pill = ref_pill(
                ui,
                &rp,
                pos2(rect.left() + 8.0, mid),
                first,
                row_color,
                max_w.max(40.0),
                has_pr,
            );
            // Right-clicking a branch pill puts that branch's actions on top of the menu.
            if resp.secondary_clicked() {
                let on_pill = ui
                    .ctx()
                    .pointer_interact_pos()
                    .is_some_and(|p| pill.contains(p));
                let branch = (on_pill && first.kind == BadgeKind::Branch).then(|| first.git_ref());
                ui.data_mut(|d| d.insert_temp(pill_menu_id(&commit.hash), branch));
            }
            if resp.double_clicked()
                && ui
                    .ctx()
                    .pointer_interact_pos()
                    .is_some_and(|p| pill.contains(p))
                && let Some(act) = self.pill_checkout(first)
            {
                acts.push(act);
            }
            let mut end = pill.right();
            let mut more_rect = None;
            if more > 0 {
                let r = Rect::from_min_size(pos2(end + 3.0, mid - 10.0), vec2(more_w - 3.0, 20.0));
                rp.rect_filled(
                    r,
                    CornerRadius::same(t.radius as u8),
                    row_color
                        .lerp_to_gamma(Color32::BLACK, 0.35)
                        .lerp_to_gamma(Color32::BLACK, DIMMED),
                );
                rp.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    format!("+{more}"),
                    theme::medium(10.5),
                    Color32::WHITE,
                );
                end = r.right();
                more_rect = Some(r);
            }
            let avatar = pos2(end + 5.0 + AVATAR_D / 2.0, mid);
            let when_rect = Rect::from_min_size(
                pos2(avatar.x + AVATAR_D / 2.0 + 5.0, mid - 9.0),
                vec2(when_w, 18.0),
            );
            // Connector to the dot, drawn first so the lanes cross over it.
            painter.line_segment(
                [pos2(when_rect.right(), mid), pos2(node_x, mid)],
                Stroke::new(1.5, row_color.gamma_multiply(0.8)),
            );
            widgets::avatar(ui, avatar, AVATAR_D / 2.0, &commit.author, &commit.email);
            rp.rect_filled(when_rect, CornerRadius::same(9), t.field);
            rp.rect_stroke(
                when_rect,
                CornerRadius::same(9),
                Stroke::new(1.0, row_color.gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
            rp.galley(
                pos2(when_rect.left() + 6.0, mid - when_galley.size().y / 2.0),
                when_galley,
                t.text_dim,
            );
            let stack_open = more_rect.is_some_and(|more| {
                let at = pos2(rect.left() + 2.0, mid - 16.0);
                self.ref_stack(ui, &badges, i, more, at, cols, row_color, acts)
            });
            let refs_cell = Rect::from_min_max(rect.min, pos2(gx, bottom));
            if !stack_open && ui.rect_contains_pointer(refs_cell) {
                let lines: Vec<String> = badges.iter().map(Badge::describe).collect();
                resp = resp.on_hover_text(format!(
                    "{}\nTip commit by {} · {}\nDouble-click a pill to check it out",
                    lines.join("\n"),
                    commit.author,
                    commit.date
                ));
            }
        }

        // Graph.
        if let Some(row) = self.graph.rows.get(i) {
            let gp = painter.with_clip_rect(rect.intersect(Rect::from_min_size(
                pos2(gx, top),
                vec2(cols.graph, rect.height()),
            )));
            let node = pos2(x(row.col), mid);
            let width = 2.0;

            for &lane in &row.through {
                if lane < lanes {
                    gp.line_segment(
                        [pos2(x(lane), top), pos2(x(lane), bottom)],
                        Stroke::new(width, t.lane(lane)),
                    );
                }
            }
            for &lane in &row.incoming {
                edge(&gp, node, pos2(x(lane), top), t.lane(lane), width);
            }
            for &lane in &row.outgoing {
                let color = if is_wip { t.text_faint } else { t.lane(lane) };
                edge(&gp, node, pos2(x(lane), bottom), color, width);
            }

            if is_wip {
                gp.circle_filled(node, 6.0, t.dock);
                gp.add(egui::Shape::dashed_line(
                    &circle_points(node, 6.0, 16),
                    Stroke::new(1.5, t.text_dim),
                    2.5,
                    2.0,
                ));
            } else if commit.parents.len() > 1 {
                gp.circle_filled(node, 3.5, row_color);
            } else {
                gp.circle_filled(node, 6.0, row_color);
                if commit.hash == self.refs.head {
                    gp.circle_filled(node, 2.5, t.dock);
                }
            }
        }

        let mut x = gx + cols.graph + 6.0;
        let text_right = rect.right() - cols.trailing() - 10.0;

        if is_wip {
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
                    x += pill(&painter, pos2(x, mid), &format!("{n} {label}"), color) + 6.0;
                }
            }
            return resp;
        }

        let font = if is_match {
            theme::medium(12.5)
        } else {
            FontId::proportional(12.5)
        };
        // The summary, then the rest of the message faded, cut with an ellipsis.
        let text_clip = painter.with_clip_rect(Rect::from_min_max(
            pos2(x, rect.top()),
            pos2(text_right, rect.bottom()),
        ));
        let summary = text_clip.text(
            pos2(x + 2.0, mid),
            Align2::LEFT_CENTER,
            &commit.summary,
            font.clone(),
            t.text,
        );
        let room = text_right - summary.right() - 12.0;
        if !commit.body.is_empty() && room > 24.0 {
            let mut job = egui::text::LayoutJob::single_section(
                commit.body.clone(),
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
                pos2(rect.right() - cols.author - cols.date, rect.top()),
                pos2(rect.right() - cols.date - 8.0, rect.bottom()),
            );
            painter.with_clip_rect(author_clip).text(
                pos2(author_clip.left(), mid),
                Align2::LEFT_CENTER,
                &commit.author,
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
        resp
    }
}

/// Menu entries comparing `branch` with `current`: what it would bring in (since they
/// diverged, as a pull request shows it), and every difference.
pub(super) fn compare_items(ui: &mut Ui, acts: &mut Vec<Action>, branch: &str, current: &str) {
    if ui
        .button(format!("Changes {branch} adds to {current}"))
        .on_hover_text("What it changed since the two diverged, as a pull request shows it")
        .clicked()
    {
        acts.push(compare_action(current, branch, true));
    }
    if ui
        .button(format!("Compare {current} with {branch}"))
        .on_hover_text("Every difference between the two")
        .clicked()
    {
        acts.push(compare_action(current, branch, false));
    }
}

/// One pill in the refs column: a branch (local, remote or both), a tag or a detached HEAD.
#[derive(Debug)]
struct Badge {
    label: String,
    kind: BadgeKind,
    /// The branch exists locally.
    local: bool,
    /// Remotes that have the branch at this commit.
    remotes: Vec<String>,
    /// The checked-out branch (or a detached HEAD).
    head: bool,
}

#[derive(Debug, PartialEq)]
enum BadgeKind {
    Branch,
    Tag,
    DetachedHead,
}

/// Where a row remembers which branch pill was right-clicked, for its menu.
fn pill_menu_id(hash: &str) -> egui::Id {
    egui::Id::new(("pill_menu", hash))
}

impl Badge {
    /// The name git knows a branch pill by: the local branch, else `remote/branch`.
    fn git_ref(&self) -> String {
        match self.remotes.first() {
            Some(remote) if !self.local => format!("{remote}/{}", self.label),
            _ => self.label.clone(),
        }
    }

    fn describe(&self) -> String {
        match self.kind {
            BadgeKind::Tag => format!("tag {}", self.label),
            BadgeKind::DetachedHead => "HEAD (detached)".to_owned(),
            BadgeKind::Branch => {
                let mut places = Vec::new();
                if self.local {
                    places.push(if self.head {
                        "local, checked out".to_owned()
                    } else {
                        "local".to_owned()
                    });
                }
                places.extend(self.remotes.iter().cloned());
                format!("{} — {}", self.label, places.join(", "))
            }
        }
    }
}

/// Turns `git log --format=%D` decorations into pills: `x` and `origin/x` at the same commit
/// become one pill marked local and remote.
fn badges(decorations: &[String], refs: &Refs) -> Vec<Badge> {
    let remote_of = |d: &str| {
        refs.remotes.iter().find_map(|(remote, _)| {
            d.strip_prefix(remote.as_str())
                .and_then(|r| r.strip_prefix('/'))
                .map(|branch| (remote.clone(), branch.to_owned()))
        })
    };
    fn branch(out: &mut Vec<Badge>, name: &str) -> usize {
        match out
            .iter()
            .position(|b| b.kind == BadgeKind::Branch && b.label == name)
        {
            Some(i) => i,
            None => {
                out.push(Badge {
                    label: name.to_owned(),
                    kind: BadgeKind::Branch,
                    local: false,
                    remotes: Vec::new(),
                    head: false,
                });
                out.len() - 1
            }
        }
    }
    let mut out: Vec<Badge> = Vec::new();
    for d in decorations {
        if let Some(name) = d.strip_prefix("HEAD -> ") {
            let i = branch(&mut out, name);
            if let Some(b) = out.get_mut(i) {
                b.local = true;
                b.head = true;
            }
        } else if d == "HEAD" {
            out.push(Badge {
                label: "HEAD".to_owned(),
                kind: BadgeKind::DetachedHead,
                local: false,
                remotes: Vec::new(),
                head: true,
            });
        } else if let Some(tag) = d.strip_prefix("tag: ") {
            out.push(Badge {
                label: tag.to_owned(),
                kind: BadgeKind::Tag,
                local: false,
                remotes: Vec::new(),
                head: false,
            });
        } else if let Some((remote, name)) = remote_of(d) {
            // `origin/HEAD` only says which branch the remote defaults to.
            if name == "HEAD" {
                continue;
            }
            let i = branch(&mut out, &name);
            if let Some(b) = out.get_mut(i) {
                b.remotes.push(remote);
            }
        } else {
            let i = branch(&mut out, d);
            if let Some(b) = out.get_mut(i) {
                b.local = true;
            }
        }
    }
    // Checked out first, then local branches, remote-only ones, tags.
    out.sort_by_key(|b| match b.kind {
        _ if b.head => 0,
        BadgeKind::Branch if b.local => 1,
        BadgeKind::Branch => 2,
        BadgeKind::Tag => 3,
        BadgeKind::DetachedHead => 4,
    });
    out
}

/// Paints a pill with white text and icons, at most `max_w` wide; returns its rect.
fn ref_pill(
    ui: &Ui,
    painter: &egui::Painter,
    left_center: Pos2,
    badge: &Badge,
    lane: Color32,
    max_w: f32,
    pr: bool,
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
    let font = if badge.head {
        theme::semibold(11.0)
    } else {
        theme::medium(11.0)
    };
    let icons_w = (lead.len() + trail.len()) as f32 * ICON;
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
    r
}

/// `text`, cut with an ellipsis to fit `width`.
fn truncate(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> String {
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

/// A small rounded count label; returns its width.
fn pill(painter: &egui::Painter, left_center: Pos2, text: &str, color: Color32) -> f32 {
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
fn edge(p: &egui::Painter, node: Pos2, end: Pos2, color: Color32, width: f32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn refs_with_origin() -> Refs {
        Refs {
            remotes: vec![("origin".to_owned(), Vec::new())],
            ..Refs::default()
        }
    }

    #[test]
    fn remote_twin_folds_into_local_badge() {
        let decos = [
            "origin/main",
            "HEAD -> main",
            "origin/HEAD",
            "origin/dev",
            "tag: v1",
        ]
        .map(String::from);
        let b = badges(&decos, &refs_with_origin());
        let labels: Vec<&str> = b.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, vec!["main", "dev", "v1"]);
        assert!(b[0].head && b[0].local && b[0].remotes == ["origin"]);
        assert!(!b[1].local && b[1].remotes == ["origin"]);
        assert_eq!(b[2].kind, BadgeKind::Tag);
    }

    #[test]
    fn local_branch_with_slash_is_not_remote() {
        let decos = ["feature/x".to_owned()];
        let b = badges(&decos, &refs_with_origin());
        assert!(b[0].kind == BadgeKind::Branch && b[0].local && b[0].remotes.is_empty());
    }
}
