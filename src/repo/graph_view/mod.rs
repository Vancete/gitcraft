//! Central pane: the commit graph of every branch, with branch pills on the left, search and
//! older history loaded as you scroll.

mod badge;
mod handles;
mod header;
mod lanes;
mod menu;
mod pills;
mod row;
mod search;

use egui::{Align2, FontId, Key, Response, Sense, Ui, vec2};

use super::actions::{BranchRef, compare_branches_action};
use super::columns::{Cols, MIN_COL_W, MIN_GRAPH_W, MIN_REFS_W};
use super::{Action, Dialog, Refresh, RepoTab, WIP_HASH, ops};
use crate::git;
use crate::settings::Settings;
use crate::theme::Tokens;
use badge::{BadgeKind, badges};
use row::Lanes;

const ROW_H: f32 = 28.0;
/// Space between rows, which the graph's lines cross.
const ROW_GAP: f32 = 2.0;
/// From one row to the next.
const ROW_PITCH: f32 = ROW_H + ROW_GAP;
/// From one lane to the next.
const LANE_W: f32 = 16.0;
/// From one lane to the next in a compact graph column.
const COMPACT_LANE_W: f32 = 10.0;
/// Radius of the corner where a line from a dot turns into another lane.
const CORNER_R: f32 = 7.0;
const MAX_LANES_SHOWN: usize = 24;
/// How much darker the pills that are not checked out are (0 = same colour, 1 = black).
const DIMMED: f32 = 0.4;
/// A translucent black, so the column handles read as dark on every theme's header.
const HANDLE_COLOR: egui::Color32 = egui::Color32::from_black_alpha(130);
const HANDLE_WIDTH: f32 = 2.0;
/// How far a handle sits left of the column it resizes.
const HANDLE_GAP: f32 = 5.0;
const AVATAR_D: f32 = 20.0;

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
            self.arrow_keys(ui, acts);
        }

        let settings = Settings::get(ui.ctx());
        let lanes = lane_layout(&settings, self.graph.width);
        let prefs = preferred_cols(&settings, lanes);
        let cols = prefs.squeezed(ui.available_width(), settings.last_resized);

        // Column headers.
        let (header, header_resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        header::column_headers(ui, &t, header, &cols);
        self.column_handles(ui, header, &prefs, &cols, &settings);
        header::header_menu(&header_resp, &settings);

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
                    loading_row(ui, &t);
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
                self.row_menu(&resp, i, idle, acts);
            }
        });
        ui.data_mut(|d| d.insert_temp(offset_id, out.state.offset.y));
    }

    /// Up and down select the row above or below the selected one.
    fn arrow_keys(&mut self, ui: &Ui, acts: &mut Vec<Action>) {
        let (up, down) = ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
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

    /// Right-click menu of row `i`: the changes row's, a stash's or a commit's.
    fn row_menu(&self, resp: &Response, i: usize, idle: bool, acts: &mut Vec<Action>) {
        let Some(commit) = self.commits.get(i) else {
            return;
        };
        if commit.hash == WIP_HASH {
            wip_menu(resp, idle, acts);
            return;
        }
        if let Some(stash) = self.stash_at(i).and_then(|s| self.stashes.get(s)) {
            egui::Popup::context_menu(resp)
                .style(crate::theme::menu_style)
                .show(|ui| super::sidebar::stash_menu(ui, acts, stash, idle));
            return;
        }
        self.commit_menu(resp, i, idle, acts);
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
}

/// The lanes to draw, from the settings and the graph's `width` in lanes.
fn lane_layout(settings: &Settings, width: usize) -> Lanes {
    Lanes {
        shown: width.clamp(1, MAX_LANES_SHOWN),
        width: if settings.compact_graph {
            COMPACT_LANE_W
        } else {
            LANE_W
        },
    }
}

/// The columns' preferred widths, from the settings; the graph fits its `lanes` unless its
/// width was dragged.
fn preferred_cols(settings: &Settings, lanes: Lanes) -> Cols {
    let shown_if = |shown: bool, width: f32| if shown { width.max(MIN_COL_W) } else { 0.0 };
    Cols {
        refs: settings.refs_width.max(MIN_REFS_W),
        graph: if settings.graph_width > 0.0 {
            settings.graph_width.max(MIN_GRAPH_W)
        } else {
            (lanes.shown as f32 * lanes.width + 12.0).max(MIN_GRAPH_W)
        },
        hash: shown_if(settings.show_hash, settings.hash_width),
        author: shown_if(settings.show_author, settings.author_width),
        when: shown_if(settings.show_when, settings.when_width),
        date: shown_if(settings.show_date, settings.date_width),
        push: settings.column_push,
    }
}

/// The last row while older history is still loading.
fn loading_row(ui: &mut Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_PITCH), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Loading older commits…",
        FontId::proportional(12.0),
        t.text_faint,
    );
}

/// Right-click menu of the changes row.
fn wip_menu(resp: &Response, idle: bool, acts: &mut Vec<Action>) {
    egui::Popup::context_menu(resp)
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
            ui.separator();
            if ui
                .add_enabled(idle, egui::Button::new("Apply patch…"))
                .on_hover_text(APPLY_PATCH_TIP)
                .clicked()
            {
                acts.push(Action::ApplyPatch);
            }
        });
}

/// What "Apply patch…" does.
const APPLY_PATCH_TIP: &str = "A patch from git format-patch becomes commits (git am), any other diff goes on the working \
     tree (git apply)";

/// Menu entries comparing `branch` with `current`: what it would bring in (since they
/// diverged, as a pull request shows it), and every difference.
pub(super) fn compare_items(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    branch: BranchRef<'_>,
    current: &str,
) {
    let name = branch.name;
    let current_ref = BranchRef::local(current);
    if ui
        .button(format!("Changes {name} adds to {current}"))
        .on_hover_text("What it changed since the two diverged, as a pull request shows it")
        .clicked()
    {
        acts.push(compare_branches_action(current_ref, branch, true));
    }
    if ui
        .button(format!("Compare {current} with {name}"))
        .on_hover_text("Every difference between the two")
        .clicked()
    {
        acts.push(compare_branches_action(current_ref, branch, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compact_graph_column_fits_its_lanes_closer_together() {
        let wide = Settings::default();
        let compact = Settings {
            compact_graph: true,
            ..Settings::default()
        };
        let graph = |s: &Settings| preferred_cols(s, lane_layout(s, 10)).graph;
        assert_eq!(graph(&wide), 172.0);
        assert_eq!(graph(&compact), 112.0);
        // A width dragged by hand stays.
        let dragged = Settings {
            graph_width: 150.0,
            ..compact
        };
        assert_eq!(graph(&dragged), 150.0);
    }
}
