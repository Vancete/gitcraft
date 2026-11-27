//! Right pane: the selected commit, or the working-tree changes with staging and the commit box.

use egui::{Align2, Key, Rect, RichText, Sense, Ui, pos2, vec2};

use super::widgets::{self, Check, RowExtra};
use super::{Action, Dialog, DiffKey, DiffView, Refresh, RepoTab, Reword, abort_action, ops};
use crate::git::{Area, ChangedFile, StatusFile};
use crate::icons;
use crate::settings::Settings;
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
        // "Edit message…" from the commit's menu, now that its details are here.
        let wants_edit = self.edit_message.as_deref() == Some(detail.hash.as_str());
        if wants_edit {
            self.edit_message = None;
        }
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
                                if (clicked || wants_edit) && editable {
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
        if Settings::get(ui.ctx()).files_as_tree {
            let tree = FileTree::build(files, |f| &f.path);
            return self.file_tree(ui, acts, &tree, "", 0, rev, &key);
        }
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

    /// One folder of [`FileTree`]: its subfolders (collapsible), then its files, `depth` in.
    #[allow(clippy::too_many_arguments)]
    fn file_tree(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        folder: &FileTree<'_, ChangedFile>,
        parent: &str,
        depth: usize,
        rev: Option<&str>,
        key: &dyn Fn(&str) -> DiffKey,
    ) {
        let indent = depth as f32 * TREE_INDENT;
        for (name, sub) in &folder.folders {
            let (name, sub) = sub.compact(name);
            let path = join_path(parent, &name);
            let id = ui.id().with(("files-folder", &path));
            let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
            let (r, _) = folder_row(ui, id, indent, &name, open, None);
            if r.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
            if open {
                self.file_tree(ui, acts, sub, &path, depth + 1, rev, key);
            }
        }
        for f in &folder.files {
            let k = key(&f.path);
            let selected = self.open_key() == Some(&k);
            let r = widgets::file_row_in(
                ui,
                f.status,
                &f.path,
                f.old_path.as_deref(),
                Some(indent + 16.0),
                None,
                selected,
                RowExtra::Stats(f.added, f.removed),
            );
            widgets::file_menu(&r.row, &f.path, rev, false, acts);
            if r.row.clicked() {
                acts.push(Action::OpenDiff(k));
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
        widgets::panel_header(ui, "Changes", |ui| {
            let mut settings = Settings::get(ui.ctx());
            let (icon, tip) = if settings.files_as_tree {
                ("list", "Show the files as a list of paths")
            } else {
                ("folder-tree", "Show the files in a tree of their folders")
            };
            if icons::small_button(ui, icon, tip).clicked() {
                settings.files_as_tree = !settings.files_as_tree;
                settings.set(ui.ctx());
            }
        });
        // Ticks on files that are gone (staged, committed, discarded) go too.
        self.picks_unstaged.keep(&self.work.unstaged);
        self.picks_staged.keep(&self.work.staged);

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
                    list_header(ui, "CONFLICTED", self.work.conflicted.len(), None, &[]);
                    for f in &self.work.conflicted {
                        // No double-click here: marking a file resolved should be meant.
                        let buttons: &[(&str, &str)] = if idle {
                            &[("check", "Mark resolved (stage)")]
                        } else {
                            &[]
                        };
                        let key = DiffKey::Work(f.clone());
                        let r = widgets::file_row(
                            ui,
                            f.code,
                            &f.path,
                            f.old_path.as_deref(),
                            self.open_key() == Some(&key),
                            RowExtra::Buttons(buttons),
                        );
                        if f.code != 'D' {
                            widgets::file_menu(&r.row, &f.path, None, true, acts);
                        }
                        if r.button.is_some() {
                            acts.push(stage_action(
                                "Mark resolved",
                                vec![vec!["add", "--", &f.path]],
                            ));
                        } else if r.row.clicked() {
                            acts.push(Action::OpenDiff(key));
                        }
                    }
                }

                let mut events = Vec::new();
                for list in [WorkList::Unstaged, WorkList::Staged] {
                    let files = match list {
                        WorkList::Unstaged => &self.work.unstaged,
                        WorkList::Staged => &self.work.staged,
                    };
                    let mut order = Vec::new();
                    self.work_list(ui, acts, list, files, &head, &mut events, &mut order);
                    let picks = match list {
                        WorkList::Unstaged => &mut self.picks_unstaged,
                        WorkList::Staged => &mut self.picks_staged,
                    };
                    for e in events.drain(..) {
                        picks.apply(e, &order, files);
                    }
                }
            });
    }

    /// The Unstaged or Staged list: a header with its buttons, then the files (as a list or a
    /// folder tree), each with a checkbox. Clicks come back as `events`; `order` gets the
    /// paths in the order shown, for Shift+click ranges.
    #[allow(clippy::too_many_arguments)]
    fn work_list(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        list: WorkList,
        files: &[StatusFile],
        head: &str,
        events: &mut Vec<PickEvent>,
        order: &mut Vec<String>,
    ) {
        let idle = self.busy_op.is_none();
        let picks = match list {
            WorkList::Unstaged => &self.picks_unstaged,
            WorkList::Staged => &self.picks_staged,
        };
        let picked: Vec<&str> = files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| picks.paths.contains(*p))
            .collect();
        let (title, empty) = match list {
            WorkList::Unstaged => ("UNSTAGED", "Nothing to stage"),
            WorkList::Staged => ("STAGED", "Stage files to choose what goes in the commit"),
        };
        let verb = list.verb();
        let picked_files: Vec<&StatusFile> = files
            .iter()
            .filter(|f| picks.paths.contains(&f.path))
            .collect();
        let selected_label = format!("{verb} selected ({})", picked.len());
        let all_label = format!("{verb} all");
        let discard_label = "Discard…";
        let mut buttons: Vec<&str> = Vec::new();
        if idle && !picked.is_empty() {
            if list == WorkList::Unstaged {
                buttons.push(discard_label);
            }
            buttons.push(&selected_label);
        }
        if idle && !files.is_empty() {
            buttons.push(&all_label);
        }
        let check = (!files.is_empty()).then(|| Check::of(picked.len(), files.len()));
        let (checked, button) = list_header(ui, title, files.len(), check, &buttons);
        if checked {
            events.push(PickEvent::All(picked.is_empty()));
        }
        match button.map(|i| buttons[i]) {
            Some(b) if b == discard_label => {
                acts.push(Action::Dialog(discard_files_dialog(&picked_files)));
            }
            Some(b) if b == selected_label => acts.push(list.action(head, &picked)),
            Some(_) => acts.push(list.all_action(head)),
            None => {}
        }
        if files.is_empty() {
            empty_hint(ui, empty);
            return;
        }
        let ctx = RowCtx {
            list,
            head,
            picked: &picked,
            picked_files: &picked_files,
            files: files.len(),
        };
        if Settings::get(ui.ctx()).files_as_tree {
            let tree = FileTree::build(files, |f| &f.path);
            self.work_tree(ui, acts, &ctx, &tree, "", 0, events, order);
        } else {
            for f in files {
                self.work_row(ui, acts, &ctx, f, None, events);
                order.push(f.path.clone());
            }
        }
    }

    /// One folder of the Changes tree: its subfolders (collapsible, with a checkbox for all
    /// their files), then its files.
    #[allow(clippy::too_many_arguments)]
    fn work_tree(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        ctx: &RowCtx<'_>,
        folder: &FileTree<'_, StatusFile>,
        parent: &str,
        depth: usize,
        events: &mut Vec<PickEvent>,
        order: &mut Vec<String>,
    ) {
        let indent = depth as f32 * TREE_INDENT;
        for (name, sub) in &folder.folders {
            let (name, sub) = sub.compact(name);
            let path = join_path(parent, &name);
            let id = ui.id().with(("work-folder", ctx.list, &path));
            let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
            let mut inside = Vec::new();
            sub.paths(&|f| &f.path, &mut inside);
            let n = inside.iter().filter(|p| ctx.picked.contains(p)).count();
            let check = Check::of(n, inside.len());
            let (r, checked) = folder_row(ui, id, indent, &name, open, Some(check));
            if checked {
                let all = inside.iter().map(|p| (*p).to_owned()).collect();
                events.push(PickEvent::Set(all, check != Check::On));
            } else if r.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
            if open {
                self.work_tree(ui, acts, ctx, sub, &path, depth + 1, events, order);
            }
        }
        for f in &folder.files {
            self.work_row(ui, acts, ctx, f, Some(indent + 16.0), events);
            order.push(f.path.clone());
        }
    }

    /// One working-tree file: checkbox, hover buttons (hidden while an operation runs) and
    /// its menu. Click opens the diff, Ctrl+click ticks it, Shift+click ticks the files from
    /// the last one clicked; a double-click stages / unstages it. Its Stage / Unstage button
    /// and menu item work on all the ticked files when it is one of them.
    fn work_row(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        ctx: &RowCtx<'_>,
        f: &StatusFile,
        indent: Option<f32>,
        events: &mut Vec<PickEvent>,
    ) {
        let idle = self.busy_op.is_none();
        let key = DiffKey::Work(f.clone());
        let picked = ctx.picked.contains(&f.path.as_str());
        // What its Stage / Unstage works on.
        let these: Vec<&str> = if picked && ctx.picked.len() > 1 {
            ctx.picked.to_vec()
        } else {
            vec![f.path.as_str()]
        };
        let verb = ctx.list.verb();
        let label = if these.len() > 1 {
            format!("{verb} {} selected", these.len())
        } else {
            verb.to_owned()
        };
        // What its Discard works on, likewise.
        let discard: Vec<&StatusFile> = if these.len() > 1 {
            ctx.picked_files.to_vec()
        } else {
            vec![f]
        };
        let discard_label = if discard.len() > 1 {
            format!("Discard {} selected…", discard.len())
        } else {
            "Discard changes…".to_owned()
        };
        let mut buttons: Vec<(&str, &str)> = Vec::new();
        if idle {
            if ctx.list == WorkList::Unstaged {
                buttons.push(("undo-2", &discard_label));
            }
            buttons.push((ctx.list.icon(), &label));
        }
        let r = widgets::file_row_in(
            ui,
            f.code,
            &f.path,
            f.old_path.as_deref(),
            indent,
            Some(if picked { Check::On } else { Check::Off }),
            picked || self.open_key() == Some(&key),
            RowExtra::Buttons(&buttons),
        );
        if f.code != 'D' {
            widgets::file_menu_with(&r.row, &f.path, None, true, acts, |ui, acts| {
                if !idle {
                    return false;
                }
                if ui.button(&label).clicked() {
                    acts.push(ctx.list.action(ctx.head, &these));
                }
                if ctx.files > these.len() && ui.button(format!("{verb} all")).clicked() {
                    acts.push(ctx.list.all_action(ctx.head));
                }
                if ctx.list == WorkList::Unstaged && ui.button(&discard_label).clicked() {
                    acts.push(Action::Dialog(discard_files_dialog(&discard)));
                }
                true
            });
        }
        if r.checked {
            events.push(PickEvent::Toggle(f.path.clone(), false));
        } else if let Some(i) = r.button {
            if i + 1 < buttons.len() {
                acts.push(Action::Dialog(discard_files_dialog(&discard)));
            } else {
                acts.push(ctx.list.action(ctx.head, &these));
            }
        } else if r.row.double_clicked() && idle {
            acts.push(ctx.list.action(ctx.head, &[f.path.as_str()]));
        } else if r.row.clicked() {
            let mods = ui.input(|i| i.modifiers);
            if mods.shift {
                events.push(PickEvent::Range(f.path.clone()));
            } else if mods.command {
                events.push(PickEvent::Toggle(f.path.clone(), true));
            } else {
                events.push(PickEvent::Click(f.path.clone()));
                acts.push(Action::OpenDiff(key));
            }
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

/// One of the Changes panel's two lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WorkList {
    Unstaged,
    Staged,
}

/// Paths per git command, so a long selection stays under the command-line limit.
const PATHS_PER_OP: usize = 200;

impl WorkList {
    fn verb(self) -> &'static str {
        match self {
            WorkList::Unstaged => "Stage",
            WorkList::Staged => "Unstage",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            WorkList::Unstaged => "plus",
            WorkList::Staged => "minus",
        }
    }

    /// Stages (or unstages) `paths`; `head` empty before the first commit.
    fn action(self, head: &str, paths: &[&str]) -> Action {
        let steps = paths
            .chunks(PATHS_PER_OP)
            .map(|chunk| {
                let mut args: Vec<&str> = match self {
                    WorkList::Unstaged => vec!["add", "--"],
                    WorkList::Staged if head.is_empty() => vec!["rm", "--cached", "-q", "--"],
                    WorkList::Staged => vec!["restore", "--staged", "--"],
                };
                args.extend_from_slice(chunk);
                args
            })
            .collect();
        let label = match paths.len() {
            1 => self.verb().to_owned(),
            n => format!("{} {n} files", self.verb()),
        };
        stage_action(&label, steps)
    }

    /// Stages (or unstages) everything.
    fn all_action(self, head: &str) -> Action {
        let label = format!("{} all", self.verb());
        match self {
            WorkList::Unstaged => stage_action(&label, vec![vec!["add", "-A"]]),
            WorkList::Staged => stage_action(&label, vec![unstage_all_args(head)]),
        }
    }
}

/// Unstages everything; `head` empty before the first commit, when there is nothing to reset to.
pub(super) fn unstage_all_args(head: &str) -> Vec<&'static str> {
    if head.is_empty() {
        vec!["rm", "--cached", "-r", "-q", "--", "."]
    } else {
        vec!["reset", "-q"]
    }
}

/// What the rows of one list share while it is drawn.
struct RowCtx<'a> {
    list: WorkList,
    head: &'a str,
    /// The ticked paths, in the list's order.
    picked: &'a [&'a str],
    /// The same files.
    picked_files: &'a [&'a StatusFile],
    /// How many files the list has.
    files: usize,
}

/// The files ticked in one list of the Changes panel.
#[derive(Default)]
pub(super) struct Picks {
    paths: std::collections::BTreeSet<String>,
    /// The file last clicked, where a Shift+click range starts.
    anchor: Option<String>,
}

/// A click on a list's ticks, applied once the list is drawn.
#[derive(Debug)]
enum PickEvent {
    /// A plain click (the diff opens): where the next range starts.
    Click(String),
    /// Tick or untick; `true` from Ctrl+click, which also ticks the file open before.
    Toggle(String, bool),
    /// Shift+click: tick every file from the last one clicked to this one.
    Range(String),
    /// Tick (`true`) or untick these, as a folder's checkbox does.
    Set(Vec<String>, bool),
    /// The header's checkbox: tick all, or none.
    All(bool),
}

impl Picks {
    /// Forgets paths no longer in the list.
    fn keep(&mut self, files: &[StatusFile]) {
        if self.paths.is_empty() && self.anchor.is_none() {
            return;
        }
        let present: std::collections::HashSet<&str> =
            files.iter().map(|f| f.path.as_str()).collect();
        self.paths.retain(|p| present.contains(p.as_str()));
        if self.anchor.as_deref().is_some_and(|a| !present.contains(a)) {
            self.anchor = None;
        }
    }

    /// `order`: the paths as shown (folded folders left out), for ranges.
    fn apply(&mut self, event: PickEvent, order: &[String], files: &[StatusFile]) {
        match event {
            PickEvent::Click(path) => self.anchor = Some(path),
            PickEvent::Toggle(path, ctrl) => {
                if ctrl && self.paths.is_empty() {
                    // Ctrl+click after a plain click: both files.
                    if let Some(a) = self.anchor.take().filter(|a| order.contains(a)) {
                        self.paths.insert(a);
                    }
                }
                if !self.paths.remove(&path) {
                    self.paths.insert(path.clone());
                }
                self.anchor = Some(path);
            }
            PickEvent::Range(path) => {
                let at = |p: &str| order.iter().position(|o| o == p);
                let to = at(&path);
                let from = self.anchor.as_deref().and_then(at).or(to);
                if let (Some(a), Some(b)) = (from, to) {
                    let (a, b) = (a.min(b), a.max(b));
                    self.paths.extend(order[a..=b].iter().cloned());
                }
                if self.anchor.is_none() {
                    self.anchor = Some(path);
                }
            }
            PickEvent::Set(paths, on) => {
                for p in paths {
                    if on {
                        self.paths.insert(p);
                    } else {
                        self.paths.remove(&p);
                    }
                }
            }
            PickEvent::All(on) => {
                self.paths.clear();
                if on {
                    self.paths.extend(files.iter().map(|f| f.path.clone()));
                }
            }
        }
    }
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

/// Asks before throwing away the unstaged changes to `files` (deleting the untracked ones).
fn discard_files_dialog(files: &[&StatusFile]) -> Dialog {
    if let [f] = files {
        return discard_dialog(f);
    }
    let (new, tracked): (Vec<&StatusFile>, Vec<&StatusFile>) =
        files.iter().partition(|f| f.area == Area::Untracked);
    let mut steps = Vec::new();
    for (args, group) in [
        (&["restore", "--"][..], &tracked),
        (&["clean", "-f", "-q", "--"][..], &new),
    ] {
        for chunk in group.chunks(PATHS_PER_OP) {
            let mut step: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            step.extend(chunk.iter().map(|f| f.path.clone()));
            steps.push(step);
        }
    }
    let n = files.len();
    let body = match new.len() {
        0 => format!("Discard the unstaged changes to {n} files? This can't be undone."),
        k if k == n => {
            format!("Delete {n} new files? They aren't tracked by git, so this can't be undone.")
        }
        k => format!(
            "Discard the unstaged changes to {n} files? {k} of them are new files git doesn't              track, and are deleted. This can't be undone."
        ),
    };
    let button = if new.len() == n {
        "Delete files"
    } else {
        "Discard"
    };
    Dialog::confirm("Discard changes", body, button, steps, None).refresh(Refresh::Status)
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

/// How much further in each level of a file tree is.
const TREE_INDENT: f32 = 14.0;

/// Files grouped by folder, for the tree views.
struct FileTree<'a, T> {
    folders: std::collections::BTreeMap<String, FileTree<'a, T>>,
    files: Vec<&'a T>,
}

impl<T> Default for FileTree<'_, T> {
    fn default() -> Self {
        Self {
            folders: Default::default(),
            files: Vec::new(),
        }
    }
}

impl<'a, T> FileTree<'a, T> {
    fn build(files: &'a [T], path: impl Fn(&'a T) -> &'a str) -> Self {
        let mut root = FileTree::default();
        for f in files {
            let mut node = &mut root;
            let mut parts: Vec<&str> = path(f).split('/').collect();
            parts.pop();
            for part in parts {
                node = node.folders.entry(part.to_owned()).or_default();
            }
            node.files.push(f);
        }
        root
    }

    /// A folder holding only one folder reads as one row: `src/repo`. This folder (`name`)
    /// and the ones it holds that way.
    fn compact(&self, name: &str) -> (String, &Self) {
        let (mut name, mut sub) = (name.to_owned(), self);
        while sub.files.is_empty() && sub.folders.len() == 1 {
            let Some((next, inner)) = sub.folders.iter().next() else {
                break;
            };
            name = format!("{name}/{next}");
            sub = inner;
        }
        (name, sub)
    }

    /// The paths of every file in here, subfolders too.
    fn paths(&self, path: &dyn Fn(&'a T) -> &'a str, out: &mut Vec<&'a str>) {
        for sub in self.folders.values() {
            sub.paths(path, out);
        }
        out.extend(self.files.iter().map(|f| path(f)));
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// A folder in a file tree, `indent` in: chevron, an optional checkbox, folder icon and name.
/// Returns the row and whether the checkbox was clicked.
fn folder_row(
    ui: &mut Ui,
    id: egui::Id,
    indent: f32,
    name: &str,
    open: bool,
    check: Option<Check>,
) -> (egui::Response, bool) {
    let t = Tokens::get(ui.ctx());
    let (rect, r) =
        ui.allocate_exact_size(vec2(ui.available_width(), widgets::ROW_H), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    let x = rect.left() + indent;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(x + 12.0, y), vec2(12.0, 12.0)),
        if open {
            "chevron-down"
        } else {
            "chevron-right"
        },
        11.0,
        t.text_faint,
    );
    let checked = check.is_some_and(|state| {
        let center = pos2(x + 16.0 + widgets::CHECK_W - 2.0, y);
        widgets::check_box(ui, center, id.with("check"), state).clicked()
    });
    let x = x + if check.is_some() {
        widgets::CHECK_W + 6.0
    } else {
        0.0
    };
    icons::paint(
        ui,
        Rect::from_center_size(pos2(x + 28.0, y), vec2(14.0, 14.0)),
        "folder-open",
        13.0,
        t.text_dim,
    );
    ui.painter_at(rect).text(
        pos2(x + 42.0, y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        t.text_dim,
    );
    (r, checked)
}

/// "N files changed +a −d", an optional note, and the Path / Tree switch on the right.
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
    // Path / Tree, the one in use underlined.
    let mut settings = Settings::get(ui.ctx());
    let mut right = rect.right() - 6.0;
    for (tree, icon, tip) in [
        (true, "folder-tree", "Files in a tree of their folders"),
        (false, "list", "Files as a list of paths"),
    ] {
        let r = Rect::from_center_size(pos2(right - 10.0, y), vec2(20.0, 20.0));
        let b = ui
            .interact(r, ui.id().with(("files-view", tree)), Sense::click())
            .on_hover_text(tip);
        let on = settings.files_as_tree == tree;
        if b.hovered() {
            ui.painter().rect_filled(r, t.radius_sm, t.pressed);
        }
        icons::paint(
            ui,
            r,
            icon,
            13.0,
            if on || b.hovered() {
                t.text
            } else {
                t.text_faint
            },
        );
        if on {
            widgets::active_underline(ui, r, t.accent);
        }
        if b.clicked() && !on {
            settings.files_as_tree = tree;
            settings.set(ui.ctx());
        }
        right = r.left() - 2.0;
    }
    if let Some(note) = note {
        ui.painter().text(
            pos2(right - 6.0, y),
            Align2::RIGHT_CENTER,
            note,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
    }
}

/// A list title with a count, an optional checkbox before it, and text buttons on the right
/// (in order, the last at the edge). Returns whether the checkbox was clicked, and which button.
fn list_header(
    ui: &mut Ui,
    title: &str,
    count: usize,
    check: Option<Check>,
    buttons: &[&str],
) -> (bool, Option<usize>) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t.tab_strip);
    let y = rect.center().y;
    // Lined up with the rows' checkboxes.
    let checked = check.is_some_and(|state| {
        let center = pos2(rect.left() + 18.0, y);
        widgets::check_box(ui, center, ui.id().with(("list_check", title)), state)
            .on_hover_text(if state == Check::Off {
                "Select all"
            } else {
                "Select none"
            })
            .clicked()
    });
    let left = rect.left() + if check.is_some() { 32.0 } else { 12.0 };
    let p = ui.painter();
    let r = p.text(
        pos2(left, y),
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
    let mut clicked = None;
    let mut right = rect.right() - 6.0;
    for (i, label) in buttons.iter().enumerate().rev() {
        let galley =
            ui.painter()
                .layout_no_wrap((*label).to_owned(), theme::medium(11.5), t.accent);
        let b = egui::Rect::from_min_max(
            pos2(right - galley.size().x - 12.0, y - 11.0),
            pos2(right, y + 11.0),
        );
        let resp = ui.interact(b, ui.id().with(("list_btn", title, i)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(b, t.radius_sm, t.hover);
        }
        ui.painter().galley(
            pos2(b.left() + 6.0, y - galley.size().y / 2.0),
            galley,
            t.accent,
        );
        if resp.clicked() {
            clicked = Some(i);
        }
        right = b.left() - 2.0;
    }
    (checked, clicked)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<StatusFile> {
        paths
            .iter()
            .map(|p| StatusFile {
                path: (*p).to_owned(),
                old_path: None,
                code: 'M',
                area: Area::Unstaged,
            })
            .collect()
    }

    fn ticked(p: &Picks) -> Vec<&str> {
        p.paths.iter().map(String::as_str).collect()
    }

    #[test]
    fn ctrl_click_ticks_the_open_file_too() {
        let fs = files(&["a", "b", "c"]);
        let order: Vec<String> = fs.iter().map(|f| f.path.clone()).collect();
        let mut p = Picks::default();
        p.apply(PickEvent::Click("a".into()), &order, &fs);
        assert!(p.paths.is_empty());
        p.apply(PickEvent::Toggle("c".into(), true), &order, &fs);
        assert_eq!(ticked(&p), ["a", "c"]);
        p.apply(PickEvent::Toggle("a".into(), true), &order, &fs);
        assert_eq!(ticked(&p), ["c"]);
    }

    #[test]
    fn shift_click_ticks_the_range_shown() {
        let fs = files(&["x/1", "x/2", "y", "z"]);
        let order: Vec<String> = fs.iter().map(|f| f.path.clone()).collect();
        let mut p = Picks::default();
        p.apply(PickEvent::Click("z".into()), &order, &fs);
        p.apply(PickEvent::Range("x/2".into()), &order, &fs);
        assert_eq!(ticked(&p), ["x/2", "y", "z"]);
        p.apply(PickEvent::All(false), &order, &fs);
        assert!(p.paths.is_empty());
    }

    #[test]
    fn ticks_go_with_the_files() {
        let mut p = Picks::default();
        let fs = files(&["a", "b"]);
        p.apply(PickEvent::All(true), &[], &fs);
        p.keep(&files(&["b"]));
        assert_eq!(ticked(&p), ["b"]);
    }

    fn steps(a: Action) -> Option<(String, Vec<Vec<String>>)> {
        match a {
            Action::Ops { label, ops, .. } => Some((label, ops)),
            _ => None,
        }
    }

    #[test]
    fn long_selections_split_into_several_commands() {
        let paths: Vec<String> = (0..450).map(|i| format!("f{i}")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let (label, ops) = steps(WorkList::Unstaged.action("abc", &refs)).unwrap_or_default();
        assert_eq!(label, "Stage 450 files");
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0][..2], ["add", "--"]);
        let (_, ops) = steps(WorkList::Staged.action("", &refs[..1])).unwrap_or_default();
        assert_eq!(ops, [["rm", "--cached", "-q", "--", "f0"]]);
    }
}
