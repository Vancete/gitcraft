//! The diff pane: its header (file, origin, actions, view options), what shows while there are
//! no lines, and the comment panel for a pull request's file.

use egui::{Sense, Stroke, Ui};

use super::images::image_view;
use super::rows::Rows;
use super::{DiffData, Kind, Pick, unstage_args};
use crate::git::{Area, StatusFile};
use crate::icons;
use crate::pull_request::DiffSide;
use crate::repo::pr_review::{self, PrDraft};
use crate::repo::pr_view::{self, PrView};
use crate::repo::{Action, DiffKey, DiffView, Refresh, RepoTab, file_text, ops, short, widgets};
use crate::settings::Settings;
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(in crate::repo) fn diff_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &self.diff else { return };
        let idle = self.busy_op.is_none();

        let (path, origin) = self.diff_origin(&view.key);
        let partial = matches!(&view.data, Some(Ok(d)) if d.partial) && idle;
        let settings = Settings::get(ui.ctx());
        let (split, ignore_ws, whole_file) = (
            settings.diff_split,
            self.diff_ignore_ws,
            settings.diff_whole_file,
        );
        let as_file = self.diff_as_file;
        let clear = self.diff_header(
            ui,
            view,
            (&path, &origin),
            partial,
            (split, ignore_ws, whole_file, as_file),
            acts,
        );

        if as_file {
            file_text::show(ui, ("diff_file", &path), view.file.as_ref());
            return;
        }
        let Some(data) = ready_data(ui, view, ignore_ws, &t) else {
            return;
        };

        // A file of the pull request shown: its lines take comments.
        let review = self.diff_review(&view.key);
        if review.as_ref().is_some_and(|r| r.target.is_some())
            && let Some(pr) = &mut self.pr_view
        {
            let draft = self.pr_drafts.entry(pr.number).or_default();
            line_comment_panel(ui, &t, pr, draft, data, acts);
        }

        let shift = ui.input(|i| i.modifiers.shift);
        let mut rows = Rows::new(ui, view, data, review.as_ref(), partial, split);
        let sel = rows.show(ui, &path, acts);
        let (clicked, offset, jumped) = (rows.clicked, rows.offset, rows.jumped);

        if let Some(view) = &mut self.diff {
            view.text_sel = sel;
            view.scroll = offset;
            view.jump = None;
            if let Some(i) = jumped {
                view.at_change = Some((i, offset));
            }
            if clear {
                view.chosen.clear();
                view.anchor = None;
            }
            if let Some(i) = clicked {
                view.pick_line(i, shift);
            }
        }
    }

    /// The file shown and where its diff comes from.
    fn diff_origin(&self, key: &DiffKey) -> (String, String) {
        match key {
            DiffKey::Commit { hash, path } => (
                path.clone(),
                format!("Commit {}", hash.get(..8).unwrap_or(hash)),
            ),
            DiffKey::Work(file) => (
                file.path.clone(),
                match file.area {
                    Area::Staged => "Staged changes".to_owned(),
                    Area::Unstaged => "Unstaged changes".to_owned(),
                    Area::Untracked => "New file".to_owned(),
                    Area::Conflicted => "Conflicted".to_owned(),
                },
            ),
            DiffKey::Range { from, to, path } => (path.clone(), self.range_label(from, to)),
        }
    }

    /// Header: file, where it comes from, options, actions, close. `opts`: (side by side,
    /// ignore whitespace, whole file, the file instead of the diff). True when the picked lines
    /// are to be cleared.
    fn diff_header(
        &self,
        ui: &mut Ui,
        view: &DiffView,
        (path, origin): (&str, &str),
        partial: bool,
        (split, ignore_ws, whole_file, as_file): (bool, bool, bool, bool),
        acts: &mut Vec<Action>,
    ) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut clear = false;
        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "file-diff", 16.0, t.icon);
                    let title = ui.add(
                        egui::Label::new(egui::RichText::new(path).font(theme::medium(13.0)))
                            .sense(Sense::click()),
                    );
                    let (rev, ignorable) = match &view.key {
                        DiffKey::Commit { hash, .. } => (Some(hash.as_str()), false),
                        DiffKey::Range { to: Some(to), .. } => (Some(to.as_str()), false),
                        DiffKey::Range { to: None, .. } => (None, false),
                        DiffKey::Work(_) => (None, true),
                    };
                    widgets::file_menu(&title, path, rev, ignorable, acts);
                    widgets::chip(ui, origin, t.text_dim);
                    if as_file && matches!(&view.file, Some(Ok(f)) if f.before) {
                        widgets::chip(ui, "before the change", t.warning);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::CloseDiff);
                        }
                        if let DiffKey::Work(file) = &view.key {
                            clear = self.file_actions(ui, view, file, partial && !as_file, acts);
                        }
                        ui.add_space(8.0);
                        if !as_file {
                            view_options(ui, &t, (split, ignore_ws, whole_file), acts);
                            ui.add_space(8.0);
                            change_steps(ui, acts);
                            ui.add_space(8.0);
                        }
                        let on = usize::from(as_file);
                        if let Some(i) = widgets::switch(ui, &["Diff", "File"], on) {
                            acts.push(Action::DiffAsFile(i == 1));
                        }
                    });
                });
            });
        clear
    }

    /// Stage, unstage or mark resolved for the whole file, and the buttons for the lines
    /// clicked in the diff. True when "Clear" was clicked.
    fn file_actions(
        &self,
        ui: &mut Ui,
        view: &DiffView,
        file: &StatusFile,
        partial: bool,
        acts: &mut Vec<Action>,
    ) -> bool {
        let idle = self.busy_op.is_none();
        let (label, args): (&str, Vec<&str>) = match file.area {
            Area::Staged => ("Unstage file", unstage_args(&self.refs.head, &file.path)),
            Area::Conflicted => ("Mark resolved", vec!["add", "--", &file.path]),
            _ => ("Stage file", vec!["add", "--", &file.path]),
        };
        if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
            acts.push(Action::Ops {
                label: label.to_owned(),
                ops: ops(&[&args]),
                refresh: Refresh::Status,
            });
        }
        // The lines clicked in the diff.
        let chosen = view.chosen.len();
        if !partial || chosen == 0 {
            return false;
        }
        ui.add_space(8.0);
        let lines = if chosen == 1 { "line" } else { "lines" };
        let picks: &[Pick] = if file.area == Area::Staged {
            &[Pick::Unstage]
        } else {
            &[Pick::Stage, Pick::Discard]
        };
        for &pick in picks {
            let b = egui::Button::new(format!("{} {chosen} {lines}", pick.verb()));
            if ui.add(b).clicked() {
                acts.push(Action::PickLines {
                    pick,
                    lines: view.chosen.clone(),
                });
            }
        }
        ui.button("Clear")
            .on_hover_text("Unselect the lines")
            .clicked()
    }

    /// "abc1234 → def5678" for a diff between two commits, with the names of the comparison.
    fn range_label(&self, from: &str, to: &Option<String>) -> String {
        let name = |rev: &str| {
            self.compare
                .as_ref()
                .and_then(|c| c.name_of(rev))
                .or_else(|| self.pr_name_of(rev))
                .unwrap_or_else(|| short(rev).to_owned())
        };
        match to {
            Some(to) => format!("{} → {}", name(from), name(to)),
            None => format!("{} → working tree", name(from)),
        }
    }
}

impl DiffView {
    /// Click on a changed line: toggles it, or with Shift selects every changed line from the
    /// last one clicked.
    fn pick_line(&mut self, i: usize, extend: bool) {
        let Some(Ok(data)) = &self.data else { return };
        match self.anchor {
            Some(a) if extend => {
                let (lo, hi) = (a.min(i), a.max(i));
                self.chosen.extend(
                    (lo..=hi).filter(|&k| matches!(data.lines[k].kind, Kind::Add | Kind::Del)),
                );
            }
            _ => {
                if !self.chosen.remove(&i) {
                    self.chosen.insert(i);
                }
                self.anchor = Some(i);
            }
        }
    }
}

/// Whitespace, whole file, side by side and unified toggles, laid out right to left.
fn view_options(
    ui: &mut Ui,
    t: &Tokens,
    (split, ignore_ws, whole_file): (bool, bool, bool),
    acts: &mut Vec<Action>,
) {
    let ws = icons::small_button(
        ui,
        "pilcrow",
        if ignore_ws {
            "Show whitespace changes"
        } else {
            "Ignore whitespace changes"
        },
    );
    if ignore_ws {
        widgets::active_underline(ui, ws.rect, t.accent);
    }
    if ws.clicked() {
        acts.push(Action::SetDiffOptions {
            split,
            ignore_ws: !ignore_ws,
            whole_file,
        });
    }
    let whole = icons::small_button(
        ui,
        "unfold-vertical",
        if whole_file {
            "Show only the changes"
        } else {
            "Show the whole file"
        },
    );
    if whole_file {
        widgets::active_underline(ui, whole.rect, t.accent);
    }
    if whole.clicked() {
        acts.push(Action::SetDiffOptions {
            split,
            ignore_ws,
            whole_file: !whole_file,
        });
    }
    let s = icons::small_button(ui, "columns-2", "Side by side");
    if split {
        widgets::active_underline(ui, s.rect, t.accent);
    }
    if s.clicked() {
        acts.push(Action::SetDiffOptions {
            split: true,
            ignore_ws,
            whole_file,
        });
    }
    let u = icons::small_button(ui, "rows-2", "Unified");
    if !split {
        widgets::active_underline(ui, u.rect, t.accent);
    }
    if u.clicked() {
        acts.push(Action::SetDiffOptions {
            split: false,
            ignore_ws,
            whole_file,
        });
    }
}

/// The previous and next change buttons, laid out right to left.
fn change_steps(ui: &mut Ui, acts: &mut Vec<Action>) {
    if icons::small_button(ui, "arrow-down", "Next change (F7, Alt+↓)").clicked() {
        acts.push(Action::DiffStep(1));
    }
    if icons::small_button(ui, "arrow-up", "Previous change (Shift+F7, Alt+↑)").clicked() {
        acts.push(Action::DiffStep(-1));
    }
}

/// The diff when it has lines to show; otherwise shows a spinner, the error, the images or a
/// "no changes" note in its place.
fn ready_data<'a>(
    ui: &mut Ui,
    view: &'a DiffView,
    ignore_ws: bool,
    t: &Tokens,
) -> Option<&'a DiffData> {
    let data = match &view.data {
        None => {
            ui.centered_and_justified(|ui| ui.spinner());
            return None;
        }
        Some(Err(e)) => {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(t.danger, e);
            });
            return None;
        }
        Some(Ok(data)) => data,
    };
    if let Some(images) = &data.image {
        image_view(ui, images);
        return None;
    }
    if data.lines.is_empty() {
        ui.centered_and_justified(|ui| {
            let msg = if ignore_ws {
                "No changes besides whitespace"
            } else {
                "No textual changes"
            };
            ui.label(egui::RichText::new(msg).color(t.text_dim));
        });
        return None;
    }
    Some(data)
}

/// The panel below the diff to write a comment on the line picked in a pull request's file.
fn line_comment_panel(
    ui: &mut Ui,
    t: &Tokens,
    pr: &mut PrView,
    draft: &mut PrDraft,
    data: &DiffData,
    acts: &mut Vec<Action>,
) {
    egui::Panel::bottom("pr_line_comment")
        .frame(
            egui::Frame::new()
                .fill(t.card)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .stroke(Stroke::new(1.0, t.separator)),
        )
        .show(ui, |ui| {
            // The line as it is in the pull request, to suggest a change to.
            let current = draft
                .target
                .as_ref()
                .filter(|t| t.side == DiffSide::Right)
                .and_then(|t| {
                    data.lines
                        .iter()
                        .find(|l| l.kind != Kind::Del && l.new.map(u64::from) == Some(t.line))
                        .and_then(|l| l.raw.get(1..))
                });
            pr_review::line_panel(ui, pr, draft, current, acts);
            pr_view::only_web_links(ui.ctx());
        });
}
