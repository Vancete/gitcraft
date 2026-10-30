//! Right pane: the selected commit, or the working-tree changes with staging and the commit box.

use egui::{Align2, Key, RichText, Sense, Ui, pos2, vec2};

use super::diff::unstage_args;
use super::widgets::{self, RowExtra};
use super::{Action, Dialog, DiffKey, DiffView, Refresh, RepoTab, Reword, abort_action, ops};
use crate::git::{Area, ChangedFile, StatusFile};
use crate::icons;
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(super) fn detail_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let wip = self.selected.is_some_and(|r| self.is_wip(r));
        if self.compare.is_some() {
            self.compare_panel(ui, acts);
        } else if wip {
            self.changes_panel(ui, acts);
        } else {
            self.commit_panel(ui, acts);
        }
    }

    fn open_key(&self) -> Option<&DiffKey> {
        self.diff.as_ref().map(|d: &DiffView| &d.key)
    }

    fn commit_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        let stash = self.stash_view.and_then(|i| self.stashes.get(i)).cloned();
        let title = if stash.is_some() { "Stash" } else { "Commit" };
        widgets::panel_header(ui, title, |ui| {
            if let Some(stash) = &stash {
                let more = icons::small_button(ui, "chevron-down", "Apply, pop or drop");
                egui::Popup::menu(&more)
                    .style(crate::theme::menu_style)
                    .show(|ui| {
                        super::sidebar::stash_menu_items(ui, acts, &stash.name, idle);
                    });
            }
        });
        if let Some(stash) = &stash {
            egui::Frame::new()
                .fill(t.accent.gamma_multiply(0.10))
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        icons::inline(ui, "archive", 15.0, t.accent);
                        ui.label(RichText::new(&stash.name).font(theme::medium(12.5)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (label, args) in [
                                ("Pop", ["stash", "pop", "--index", stash.name.as_str()]),
                                ("Apply", ["stash", "apply", "--index", stash.name.as_str()]),
                            ] {
                                if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                                    acts.push(Action::Ops {
                                        label: format!("{label} {}", stash.name),
                                        ops: super::ops(&[&args]),
                                        refresh: super::Refresh::All,
                                    });
                                }
                            }
                        });
                    });
                });
        }
        if let Some(err) = &self.detail_error {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.colored_label(t.danger, err);
            });
            return;
        }
        let Some(detail) = &self.detail else {
            ui.centered_and_justified(|ui| {
                if self.selected.is_some() {
                    ui.spinner();
                } else {
                    ui.label(RichText::new("Select a commit").color(t.text_dim));
                }
            });
            return;
        };

        // Taken out while the pane borrows `self`; put back after it.
        let mut reword = self.reword.take();
        let head = self.refs.head.clone();
        egui::ScrollArea::vertical()
            .id_salt("detail_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        let (subject, body) = detail
                            .message
                            .split_once('\n')
                            .map(|(s, b)| (s, b.trim()))
                            .unwrap_or((detail.message.as_str(), ""));
                        let mut close = false;
                        match reword.as_mut().filter(|r| r.hash == detail.hash) {
                            Some(r) => {
                                if let Some(save) = reword_editor(ui, r) {
                                    let message = r.message();
                                    if save && message != detail.message.trim() {
                                        acts.push(Action::Reword {
                                            hash: detail.hash.clone(),
                                            message,
                                        });
                                    }
                                    close = true;
                                }
                            }
                            None => {
                                // A commit of the checked-out branch: double-click its message to
                                // edit it (the root commit has no parent to rebase from).
                                let editable = idle
                                    && detail.in_head
                                    && (detail.hash == head || !detail.merges_after)
                                    && (detail.hash == head || !detail.parents.is_empty());
                                let sense = if editable {
                                    Sense::click()
                                } else {
                                    Sense::hover()
                                };
                                let summary = ui.add(
                                    egui::Label::new(
                                        RichText::new(subject).font(theme::semibold(15.0)),
                                    )
                                    .wrap()
                                    .sense(sense),
                                );
                                let mut clicked = if editable {
                                    summary.on_hover_text("Double-click to edit the message")
                                } else if detail.in_head && detail.merges_after {
                                    summary.on_hover_text(
                                        "Merges came after this commit: editing its message would flatten them",
                                    )
                                } else {
                                    summary
                                }
                                .double_clicked();
                                if !body.is_empty() {
                                    ui.add_space(2.0);
                                    clicked |= ui
                                        .add(
                                            egui::Label::new(RichText::new(body).color(t.text_dim))
                                                .wrap()
                                                .sense(sense),
                                        )
                                        .double_clicked();
                                }
                                if clicked && editable {
                                    reword = Some(Reword {
                                        hash: detail.hash.clone(),
                                        subject: subject.to_owned(),
                                        body: body.to_owned(),
                                        focus: true,
                                    });
                                }
                            }
                        }
                        if close {
                            reword = None;
                        }
                        ui.add_space(12.0);

                        // Author line with an initials avatar.
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                            widgets::avatar(ui, rect.center(), 16.0, &detail.author, &detail.email);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(&detail.author).font(theme::medium(12.5)))
                                    .on_hover_text(&detail.email);
                                ui.label(
                                    RichText::new(&detail.author_date)
                                        .color(t.text_faint)
                                        .font(theme::mono(11.0)),
                                );
                            });
                        });
                        if detail.committer != detail.author {
                            ui.label(
                                RichText::new(format!(
                                    "Committed by {} · {}",
                                    detail.committer, detail.commit_date
                                ))
                                .color(t.text_faint)
                                .small(),
                            );
                        }
                        ui.add_space(10.0);

                        egui::Grid::new("commit_meta")
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Commit").color(t.text_faint));
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(
                                            detail.hash.get(..12).unwrap_or(&detail.hash),
                                        )
                                        .font(theme::mono(12.0)),
                                    )
                                    .on_hover_text(&detail.hash);
                                    if icons::small_button(ui, "copy", "Copy full hash").clicked() {
                                        ui.ctx().copy_text(detail.hash.clone());
                                    }
                                });
                                ui.end_row();
                                if !detail.parents.is_empty() {
                                    ui.label(
                                        RichText::new(if detail.parents.len() > 1 {
                                            "Parents"
                                        } else {
                                            "Parent"
                                        })
                                        .color(t.text_faint),
                                    );
                                    ui.horizontal_wrapped(|ui| {
                                        for p in &detail.parents {
                                            if ui
                                                .link(
                                                    RichText::new(p.get(..8).unwrap_or(p))
                                                        .font(theme::mono(12.0)),
                                                )
                                                .clicked()
                                            {
                                                acts.push(Action::SelectHash(p.clone()));
                                            }
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    });

                let note = (detail.parents.len() > 1).then_some("vs. first parent");
                files_header(ui, &detail.files, note);
                ui.spacing_mut().item_spacing.y = 0.0;
                self.file_list(ui, acts, &detail.files, Some(&detail.hash), |path| {
                    DiffKey::Commit {
                        hash: detail.hash.clone(),
                        path: path.to_owned(),
                    }
                });
            });
        self.reword = reword;
    }

    /// Changed files, each opening its diff (`key`) when clicked; `rev`: where the files are,
    /// for their menu (`None`: the working tree).
    fn file_list(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        files: &[ChangedFile],
        rev: Option<&str>,
        key: impl Fn(&str) -> DiffKey,
    ) {
        for f in files {
            let key = key(&f.path);
            let selected = self.open_key() == Some(&key);
            let r = widgets::file_row(
                ui,
                f.status,
                &f.path,
                f.old_path.as_deref(),
                selected,
                RowExtra::Stats(f.added, f.removed),
            );
            widgets::file_menu(&r.row, &f.path, rev, false, acts);
            if r.row.clicked() {
                acts.push(Action::OpenDiff(key));
            }
        }
    }

    /// Right pane while comparing: the two ends, how far apart they are, and the files that
    /// differ.
    fn compare_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(c) = &self.compare else { return };
        widgets::panel_header(ui, "Compare", |ui| {
            if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                acts.push(Action::CloseCompare);
            }
        });
        let data = c.data.as_ref().and_then(|d| d.as_ref().ok());
        let target_name = if c.target.is_some() {
            c.target_label.as_str()
        } else {
            "the working tree"
        };
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                egui::Grid::new("compare_ends")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        // A name, and the commit it stands for (`note` before the hash).
                        let end = |ui: &mut Ui, label: &str, hash: Option<&str>, note: &str| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(label).font(theme::medium(12.5)));
                                // The hash, unless the label already is it.
                                if let Some(hash) = hash.filter(|h| super::short(h) != label) {
                                    ui.label(
                                        RichText::new(format!("{note}{}", super::short(hash)))
                                            .font(theme::mono(11.5))
                                            .color(t.text_faint),
                                    )
                                    .on_hover_text(hash);
                                }
                            });
                        };
                        let note = if c.since_fork { "merge base " } else { "" };
                        ui.label(RichText::new("From").color(t.text_faint));
                        end(ui, &c.base_label, data.map(|d| d.from.as_str()), note);
                        ui.end_row();
                        ui.label(RichText::new("To").color(t.text_faint));
                        end(ui, target_name, data.and_then(|d| d.to.as_deref()), "");
                        ui.end_row();
                    });
                if let Some(d) = data {
                    let (behind, ahead) = d.apart;
                    let commits = |n: u32| format!("{n} commit{}", if n == 1 { "" } else { "s" });
                    let other = if c.target.is_some() {
                        target_name
                    } else {
                        "HEAD"
                    };
                    let text = match (ahead, behind) {
                        (0, 0) => "Same history".to_owned(),
                        (a, 0) => format!("{other} is {} ahead", commits(a)),
                        (0, b) => format!("{other} is {} behind", commits(b)),
                        (a, b) => format!("{other} is {} ahead, {b} behind", commits(a)),
                    };
                    ui.label(RichText::new(text).color(t.text_dim).small());
                }
                ui.horizontal(|ui| {
                    let mut since = c.since_fork;
                    let label = format!("Only what {target_name} changed since they diverged");
                    if ui
                        .checkbox(&mut since, label)
                        .on_hover_text(
                            "Diff from the merge base, as a pull request shows it, instead of \
                             every difference between the two",
                        )
                        .changed()
                    {
                        acts.push(Action::CompareSinceFork(since));
                    }
                });
                if c.target.is_some() && ui.button("Swap").clicked() {
                    acts.push(Action::SwapCompare);
                }
            });

        match &c.data {
            None => {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.colored_label(t.danger, e);
                });
            }
            Some(Ok(d)) => {
                files_header(ui, &d.files, None);
                if d.files.is_empty() {
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No differences").color(t.text_dim));
                    });
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("compare_scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.file_list(ui, acts, &d.files, d.to.as_deref(), |path| {
                            DiffKey::Range {
                                from: d.from.clone(),
                                to: d.to.clone(),
                                path: path.to_owned(),
                            }
                        });
                    });
            }
        }
    }

    fn changes_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        widgets::panel_header(ui, "Changes", |_| {});

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
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(idle, egui::Button::new("Abort"))
                                            .clicked()
                                        {
                                            acts.push(abort_action(state));
                                        }
                                    },
                                );
                            });
                            let hint = if self.work.conflicted.is_empty() {
                                "All conflicts resolved: commit to finish."
                            } else {
                                "Fix the conflicted files, mark them resolved, then commit."
                            };
                            ui.label(RichText::new(hint).color(t.text_dim).small());
                        });
                }

                let head = self.refs.head.clone();
                if !self.work.conflicted.is_empty() {
                    list_header(ui, "CONFLICTED", self.work.conflicted.len(), None);
                    for f in &self.work.conflicted {
                        match self.work_row(ui, acts, f, &[("check", "Mark resolved (stage)")]) {
                            RowClick::Open => acts.push(Action::OpenDiff(DiffKey::Work(f.clone()))),
                            RowClick::Button(_) => acts.push(stage_action(
                                "Mark resolved",
                                vec![vec!["add", "--", &f.path]],
                            )),
                            RowClick::None => {}
                        }
                    }
                }

                let unstaged = &self.work.unstaged;
                if list_header(
                    ui,
                    "UNSTAGED",
                    unstaged.len(),
                    (!unstaged.is_empty() && idle).then_some("Stage all"),
                ) {
                    acts.push(stage_action("Stage all", vec![vec!["add", "-A"]]));
                }
                for f in unstaged {
                    match self.work_row(
                        ui,
                        acts,
                        f,
                        &[("undo-2", "Discard changes…"), ("plus", "Stage")],
                    ) {
                        RowClick::Open => acts.push(Action::OpenDiff(DiffKey::Work(f.clone()))),
                        RowClick::Button(0) => acts.push(Action::Dialog(discard_dialog(f))),
                        RowClick::Button(_) => {
                            acts.push(stage_action("Stage", vec![vec!["add", "--", &f.path]]))
                        }
                        RowClick::None => {}
                    }
                }
                if unstaged.is_empty() {
                    empty_hint(ui, "Nothing to stage");
                }

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
                for f in staged {
                    match self.work_row(ui, acts, f, &[("minus", "Unstage")]) {
                        RowClick::Open => acts.push(Action::OpenDiff(DiffKey::Work(f.clone()))),
                        RowClick::Button(_) => {
                            acts.push(stage_action("Unstage", vec![unstage_args(&head, &f.path)]))
                        }
                        RowClick::None => {}
                    }
                }
                if staged.is_empty() {
                    empty_hint(ui, "Stage files to choose what goes in the commit");
                }
            });
    }

    /// One working-tree file with hover buttons (hidden while an operation runs).
    fn work_row(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        f: &StatusFile,
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

    fn commit_box(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        let rebasing = self.work.in_progress == Some(crate::git::InProgress::Rebase);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if rebasing {
                    "CONTINUE REBASE"
                } else {
                    "COMMIT"
                })
                .font(theme::semibold(10.5))
                .color(t.text_faint),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let amend = ui
                    .add_enabled(
                        !rebasing && !self.refs.head.is_empty(),
                        egui::Checkbox::new(&mut self.amend, "Amend last commit"),
                    )
                    .on_hover_text(
                        "Rewrite the last commit with the staged changes and this message",
                    );
                if amend.changed() && self.amend && self.commit_summary.trim().is_empty() {
                    // Start from the message being amended.
                    if let Ok(msg) = crate::git::head_message(&self.path) {
                        let (summary, body) = msg.split_once('\n').unwrap_or((msg.as_str(), ""));
                        self.commit_summary = summary.trim().to_owned();
                        self.commit_body = body.trim().to_owned();
                    }
                }
            });
        });
        if self.amend {
            let pushed = self
                .refs
                .current
                .as_ref()
                .and_then(|c| self.refs.local.iter().find(|b| &b.name == c))
                .is_some_and(|b| !b.upstream.is_empty() && b.ahead_behind().0 == 0);
            if pushed {
                ui.label(
                    RichText::new(
                        "This commit is already pushed: amending rewrites shared history.",
                    )
                    .color(t.warning)
                    .small(),
                );
            }
        }
        ui.add_space(2.0);
        let summary = ui.add(
            egui::TextEdit::singleline(&mut self.commit_summary)
                .hint_text("Summary")
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        let body = ui.add(
            egui::TextEdit::multiline(&mut self.commit_body)
                .hint_text("Description (optional)")
                .desired_width(f32::INFINITY)
                .desired_rows(3)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        let len = self.commit_summary.chars().count();
        if len > 72 {
            ui.label(
                RichText::new(format!("Summary is {len} characters; 50–72 reads best"))
                    .color(t.warning)
                    .small(),
            );
        }
        ui.add_space(4.0);
        let staged = self.work.staged.len();
        let label = if rebasing {
            "Continue rebase".to_owned()
        } else if self.amend {
            "Amend last commit".to_owned()
        } else if staged > 0 {
            format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" })
        } else {
            "Stage all and commit".to_owned()
        };
        let can = idle && !self.commit_summary.trim().is_empty() && self.work.conflicted.is_empty();
        // Only from the message fields, so Ctrl+Enter in the terminal stays the shell's.
        let shortcut = (summary.has_focus() || body.has_focus())
            && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
        let resp = widgets::primary_button(ui, &label, can, ui.available_width());
        if (resp.clicked() || (shortcut && can)) && can {
            acts.push(Action::Commit);
        }
        resp.on_hover_text("Ctrl+Enter");

        // Who the commit will be by, and the saved identities to switch to.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let id = &self.identity;
            let who = if id.name.is_empty() {
                "No identity set (git config user.name)".to_owned()
            } else {
                format!(
                    "as {} <{}>{}",
                    id.name,
                    id.email,
                    if id.local { "" } else { " · global" }
                )
            };
            ui.add(egui::Label::new(RichText::new(who).color(t.text_faint).small()).truncate());
            let profiles = crate::settings::Settings::get(ui.ctx()).profiles;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = icons::small_button(ui, "user-round", "Commit as…");
                egui::Popup::menu(&b)
                    .style(crate::theme::menu_style)
                    .show(|ui| {
                        if profiles.is_empty() {
                            ui.label(
                                RichText::new("Add identities in Settings").color(t.text_faint),
                            );
                        }
                        for p in &profiles {
                            let label = format!("{} — {} <{}>", p.label, p.name, p.email);
                            if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                                acts.push(Action::SetIdentity(p.name.clone(), p.email.clone()));
                            }
                        }
                    });
            });
        });
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

/// "N files changed +a −d", and an optional note on the right.
fn files_header(ui: &mut Ui, files: &[ChangedFile], note: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| {
        (a + f.added.unwrap_or(0), d + f.removed.unwrap_or(0))
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.tab_strip);
    let y = rect.center().y;
    let r = p.text(
        pos2(rect.left() + 14.0, y),
        Align2::LEFT_CENTER,
        format!(
            "{} file{} changed",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ),
        theme::semibold(11.5),
        t.text_dim,
    );
    let r = p.text(
        pos2(r.right() + 10.0, y),
        Align2::LEFT_CENTER,
        format!("+{adds}"),
        theme::mono(11.0),
        t.success,
    );
    p.text(
        pos2(r.right() + 6.0, y),
        Align2::LEFT_CENTER,
        format!("−{dels}"),
        theme::mono(11.0),
        t.danger,
    );
    if let Some(note) = note {
        p.text(
            pos2(rect.right() - 10.0, y),
            Align2::RIGHT_CENTER,
            note,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
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

impl Reword {
    /// The summary, then the description after a blank line.
    fn message(&self) -> String {
        let (subject, body) = (self.subject.trim(), self.body.trim());
        if body.is_empty() {
            subject.to_owned()
        } else {
            format!("{subject}\n\n{body}")
        }
    }
}

/// The summary and description being edited, with Save and Cancel: `Some(true)` to save,
/// `Some(false)` to cancel.
fn reword_editor(ui: &mut Ui, reword: &mut Reword) -> Option<bool> {
    let t = Tokens::get(ui.ctx());
    let summary = ui.add(
        egui::TextEdit::singleline(&mut reword.subject)
            .font(theme::semibold(14.0))
            .hint_text("Summary")
            .desired_width(f32::INFINITY),
    );
    if reword.focus {
        summary.request_focus();
        reword.focus = false;
    }
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::multiline(&mut reword.body)
            .hint_text("Description (optional)")
            .desired_rows(4)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);
    let can_save = !reword.subject.trim().is_empty();
    let (save_keys, cancel_keys) = ui.input(|i| {
        (
            i.modifiers.command && i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
        )
    });
    let mut out = None;
    ui.horizontal(|ui| {
        if widgets::primary_button(ui, "Save message", can_save, 110.0).clicked() {
            out = Some(true);
        }
        if ui.button("Cancel").clicked() {
            out = Some(false);
        }
        ui.label(
            RichText::new("Ctrl+Enter saves · Esc cancels")
                .small()
                .color(t.text_faint),
        );
    });
    if save_keys && can_save {
        out = Some(true);
    } else if cancel_keys {
        out = Some(false);
    }
    out
}
