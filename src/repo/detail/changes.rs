//! The working-tree changes: conflicted, unstaged and staged files, staged and unstaged a file
//! or all at once.

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};

use crate::git::{Area, InProgress, StatusFile};
use crate::icons;
use crate::repo::diff::unstage_args;
use crate::repo::widgets::{self, RowExtra};
use crate::repo::{Action, Dialog, DiffKey, Refresh, RepoTab, abort_action, branch_tree, ops};
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(super) fn changes_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        widgets::panel_header(ui, "Changes", branch_tree::files_toggle);

        // The commit box stays at the bottom.
        egui::Panel::bottom("commit_box")
            .frame(
                egui::Frame::new()
                    .fill(t.card)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .stroke(egui::Stroke::new(1.0, t.separator)),
            )
            .show(ui, |ui| self.commit_box(ui, acts));

        egui::ScrollArea::vertical()
            .id_salt("changes_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if let Some(state) = self.work.in_progress {
                    self.in_progress_banner(ui, acts, state);
                }
                if !self.work.conflicted.is_empty() {
                    self.conflicted_list(ui, acts);
                }
                self.unstaged_list(ui, acts);
                self.staged_list(ui, acts);
            });
    }

    /// The merge, rebase or the like going on, with Abort and what is left to do.
    fn in_progress_banner(&self, ui: &mut Ui, acts: &mut Vec<Action>, state: InProgress) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        egui::Frame::new()
            .fill(t.warning.gamma_multiply(0.14))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    icons::inline(ui, "circle-alert", 16.0, t.warning);
                    ui.label(
                        RichText::new(format!("{} in progress", state.label()))
                            .color(t.warning)
                            .font(theme::medium(12.5)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(idle, egui::Button::new("Abort")).clicked() {
                            acts.push(abort_action(state));
                        }
                    });
                });
                let hint = if self.work.conflicted.is_empty() {
                    "All conflicts resolved: commit to finish."
                } else {
                    "Fix the conflicted files, mark them resolved, then commit."
                };
                ui.label(RichText::new(hint).color(t.text_dim).small());
            });
    }

    fn conflicted_list(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
        list_header(ui, "CONFLICTED", self.work.conflicted.len(), None);
        let buttons = [("check", "Mark resolved (stage)")];
        self.work_list(
            ui,
            acts,
            "conflicted",
            &self.work.conflicted,
            &buttons,
            |acts, f, click| match click {
                RowClick::Open => acts.push(Action::OpenDiff(DiffKey::Work(f.clone()))),
                RowClick::Button(_) => acts.push(stage_action(
                    "Mark resolved",
                    vec![vec!["add", "--", &f.path]],
                )),
                RowClick::None => {}
            },
        );
    }

    fn unstaged_list(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let idle = self.busy_op.is_none();
        let unstaged = &self.work.unstaged;
        if list_header(
            ui,
            "UNSTAGED",
            unstaged.len(),
            (!unstaged.is_empty() && idle).then_some("Stage all"),
        ) {
            acts.push(stage_action("Stage all", vec![vec!["add", "-A"]]));
        }
        let buttons = [("undo-2", "Discard changes…"), ("plus", "Stage")];
        self.work_list(
            ui,
            acts,
            "unstaged",
            unstaged,
            &buttons,
            |acts, f, click| match click {
                RowClick::Open => acts.push(Action::OpenDiff(DiffKey::Work(f.clone()))),
                RowClick::Button(0) => acts.push(Action::Dialog(discard_dialog(f))),
                RowClick::Button(_) => {
                    acts.push(stage_action("Stage", vec![vec!["add", "--", &f.path]]))
                }
                RowClick::None => {}
            },
        );
        if unstaged.is_empty() {
            empty_hint(ui, "Nothing to stage");
        }
    }

    fn staged_list(&self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let idle = self.busy_op.is_none();
        let head = &self.refs.head;
        let staged = &self.work.staged;
        if list_header(
            ui,
            "STAGED",
            staged.len(),
            (!staged.is_empty() && idle).then_some("Unstage all"),
        ) {
            let all: Vec<&str> = if head.is_empty() {
                vec!["rm", "--cached", "-r", "-q", "--", "."]
            } else {
                vec!["reset", "-q"]
            };
            acts.push(stage_action("Unstage all", vec![all]));
        }
        let buttons = [("minus", "Unstage")];
        self.work_list(
            ui,
            acts,
            "staged",
            staged,
            &buttons,
            |acts, f, click| match click {
                RowClick::Open => acts.push(Action::OpenDiff(DiffKey::Work(f.clone()))),
                RowClick::Button(_) => {
                    acts.push(stage_action("Unstage", vec![unstage_args(head, &f.path)]))
                }
                RowClick::None => {}
            },
        );
        if staged.is_empty() {
            empty_hint(ui, "Stage files to choose what goes in the commit");
        }
    }

    /// Working-tree files, each with `buttons`; `on` handles what was clicked on a row. `tree`:
    /// which list this is, for the folders opened and closed in it.
    fn work_list(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        tree: &str,
        files: &[StatusFile],
        buttons: &[(&str, &str)],
        mut on: impl FnMut(&mut Vec<Action>, &StatusFile, RowClick),
    ) {
        let id = egui::Id::new(("file-folder", self.repo_key(), tree));
        let selected = |f: &&StatusFile| self.open_key() == Some(&DiffKey::Work((*f).clone()));
        branch_tree::files(
            ui,
            (id, true),
            files,
            |f| &f.path,
            &selected,
            &mut |ui, indent, f| {
                let click = self.work_row(ui, acts, f, indent, buttons);
                on(acts, f, click);
            },
        );
    }

    /// One working-tree file with hover buttons (hidden while an operation runs).
    fn work_row(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        f: &StatusFile,
        indent: Option<f32>,
        buttons: &[(&str, &str)],
    ) -> RowClick {
        let key = DiffKey::Work(f.clone());
        let selected = self.open_key() == Some(&key);
        let buttons = if self.busy_op.is_none() { buttons } else { &[] };
        let r = widgets::file_row(
            ui,
            f.code,
            &f.path,
            f.old_path.as_deref(),
            indent,
            selected,
            RowExtra::Buttons(buttons),
        );
        if f.code != 'D' {
            widgets::file_menu(&r.row, &f.path, None, true, acts);
        }
        match r.button {
            Some(i) => RowClick::Button(i),
            None if r.row.clicked() => RowClick::Open,
            None => RowClick::None,
        }
    }
}

enum RowClick {
    None,
    Open,
    Button(usize),
}

fn stage_action(label: &str, steps: Vec<Vec<&str>>) -> Action {
    Action::Ops {
        label: label.to_owned(),
        ops: steps
            .into_iter()
            .map(|s| s.into_iter().map(str::to_owned).collect())
            .collect(),
        refresh: Refresh::Status,
    }
}

fn discard_dialog(f: &StatusFile) -> Dialog {
    if f.area == Area::Untracked {
        Dialog::confirm(
            "Delete new file",
            format!(
                "Delete “{}”? It isn't tracked by git, so this can't be undone.",
                f.path
            ),
            "Delete file",
            ops(&[&["clean", "-f", "-q", "--", &f.path]]),
            None,
        )
        .refresh(Refresh::Status)
    } else {
        Dialog::confirm(
            "Discard changes",
            format!(
                "Discard the unstaged changes to “{}”? This can't be undone.",
                f.path
            ),
            "Discard",
            ops(&[&["restore", "--", &f.path]]),
            None,
        )
        .refresh(Refresh::Status)
    }
}

/// A list title with a count and an optional text button on the right. Returns the click.
fn list_header(ui: &mut Ui, title: &str, count: usize, button: Option<&str>) -> bool {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.tab_strip);
    let y = rect.center().y;
    let r = p.text(
        pos2(rect.left() + 12.0, y),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(11.0),
        t.text_dim,
    );
    p.text(
        pos2(r.right() + 8.0, y),
        Align2::LEFT_CENTER,
        count.to_string(),
        theme::medium(11.0),
        t.text_faint,
    );
    let Some(label) = button else { return false };
    let galley = p.layout_no_wrap(label.to_owned(), theme::medium(11.5), t.accent);
    let b = egui::Rect::from_center_size(
        pos2(rect.right() - 12.0 - galley.size().x / 2.0 - 6.0, y),
        vec2(galley.size().x + 12.0, 22.0),
    );
    let resp = ui.interact(b, ui.id().with(("list_btn", title)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(b, t.radius_sm, t.hover);
    }
    ui.painter().galley(
        pos2(b.left() + 6.0, y - galley.size().y / 2.0),
        galley,
        t.accent,
    );
    resp.clicked()
}

fn empty_hint(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(
        pos2(rect.left() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(11.5),
        t.text_faint,
    );
}
