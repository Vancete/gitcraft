//! Left pane: local branches, remotes and their branches, worktrees, stashes, pull requests,
//! tags.

use egui::{Align2, Rect, Response, Sense, Ui, pos2, vec2};

use super::widgets;
use super::{
    Action, Dialog, Refresh, RepoTab, Undo, compare_action, fast_forward_action, merge_action, ops,
    rebase_action,
};
use crate::git::{self, Branch};
use crate::icons;
use crate::theme::{self, Tokens};

/// The pane's rows: a little taller than the file lists', 2 px more above and below.
const ROW_H: f32 = widgets::ROW_H + 4.0;

impl RepoTab {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        widgets::panel_header(ui, "Branches", |_| {});
        // Sized like the commit search bar (Ctrl+F).
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            icons::inline(ui, "search", 15.0, t.text_faint);
            ui.add(
                egui::TextEdit::singleline(&mut self.branch_filter)
                    .hint_text("Filter branches and tags")
                    .desired_width(ui.available_width() - 10.0)
                    .margin(egui::Margin::symmetric(8, 4)),
            );
        });
        ui.add_space(8.0);

        let filter = self.branch_filter.to_lowercase();
        let matches = |b: &&Branch| filter.is_empty() || b.name.to_lowercase().contains(&filter);
        let idle = self.busy_op.is_none();

        egui::ScrollArea::vertical()
            .id_salt("branches")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;

                let local: Vec<&Branch> = self.refs.local.iter().filter(matches).collect();
                let (open, add) = widgets::section_with_button(
                    ui,
                    "local",
                    "laptop",
                    "LOCAL",
                    local.len(),
                    true,
                    Some(("plus", "New branch at HEAD…")),
                );
                if add && idle && !self.refs.head.is_empty() {
                    acts.push(Action::Dialog(Dialog::create_branch(
                        &self.refs.head,
                        "HEAD",
                    )));
                }
                if open {
                    for b in local {
                        self.local_branch_row(ui, acts, b, idle);
                    }
                }

                self.remotes_section(ui, acts, idle, &filter);
                self.worktree_section(ui, acts, idle);
                self.stash_section(ui, acts, idle);
                self.pr_section(ui, acts, idle);

                let tags: Vec<&Branch> = self.refs.tags.iter().filter(matches).collect();
                if !tags.is_empty()
                    && widgets::section(ui, "tags", "tag", "TAGS", tags.len(), false)
                {
                    for tag in tags {
                        let r = plain_row(ui, "tag", &tag.name, "");
                        if r.clicked() {
                            acts.push(Action::SelectHash(tag.target.clone()));
                        }
                        egui::Popup::context_menu(&r)
                            .style(crate::theme::menu_style)
                            .show(|ui| {
                                if ui.button("Copy name").clicked() {
                                    ui.ctx().copy_text(tag.name.clone());
                                }
                                if let Some(remote) = self.default_remote() {
                                    if ui
                                        .add_enabled(
                                            idle,
                                            egui::Button::new(format!("Push to {remote}")),
                                        )
                                        .clicked()
                                    {
                                        acts.push(Action::Ops {
                                            label: format!("Push tag {}", tag.name),
                                            ops: ops(&[&[
                                                "push",
                                                &remote,
                                                &format!("refs/tags/{}", tag.name),
                                            ]]),
                                            refresh: Refresh::All,
                                        });
                                    }
                                    if ui
                                        .add_enabled(
                                            idle,
                                            egui::Button::new(format!("Delete from {remote}…")),
                                        )
                                        .clicked()
                                    {
                                        acts.push(Action::Dialog(Dialog::confirm(
                                            "Delete remote tag",
                                            format!("Delete the tag “{}” from {remote}?", tag.name),
                                            "Delete",
                                            ops(&[&[
                                                "push",
                                                &remote,
                                                "--delete",
                                                &format!("refs/tags/{}", tag.name),
                                            ]]),
                                            None,
                                        )));
                                    }
                                }
                                ui.separator();
                                if ui
                                    .add_enabled(idle, egui::Button::new("Delete tag…"))
                                    .clicked()
                                {
                                    acts.push(Action::Dialog(
                                        Dialog::confirm(
                                            "Delete tag",
                                            format!("Delete the local tag “{}”?", tag.name),
                                            "Delete",
                                            ops(&[&["tag", "-d", &tag.name]]),
                                            None,
                                        )
                                        .undo(
                                            Undo::Custom {
                                                undo: ops(&[&["tag", &tag.name, &tag.target]]),
                                                redo: ops(&[&["tag", "-d", &tag.name]]),
                                            },
                                        ),
                                    ));
                                }
                            });
                    }
                }
            });
    }

    fn local_branch_row(&self, ui: &mut Ui, acts: &mut Vec<Action>, b: &Branch, idle: bool) {
        let (ahead, behind) = b.ahead_behind();
        // Checked out in another worktree: git won't check it out here too.
        let elsewhere = self
            .worktrees
            .iter()
            .find(|w| !w.current && w.branch.as_deref() == Some(b.name.as_str()));
        let has_pr = self.prs.iter().any(|p| p.branch == b.name);
        let r = branch_row(
            ui,
            &b.name,
            b.is_head,
            ahead,
            behind,
            b.upstream_gone(),
            elsewhere.is_some(),
            has_pr,
        );
        let tip = match (b.upstream.is_empty(), b.upstream_gone()) {
            (true, _) => "Not tracking a remote branch".to_owned(),
            (false, true) => format!("{} was deleted on the remote", b.upstream),
            (false, false) => format!("Tracks {}", b.upstream),
        };
        let r = match elsewhere {
            Some(wt) => r.on_hover_text(format!(
                "{tip}\nChecked out in worktree {}\nDouble-click to open it",
                wt.path.display()
            )),
            None => r.on_hover_text(format!("{tip}\nDouble-click to check out")),
        };
        if r.double_clicked() && elsewhere.is_some() {
            if let Some(wt) = elsewhere {
                acts.push(Action::OpenTab(wt.path.clone()));
            }
        } else if r.double_clicked() && !b.is_head && idle {
            acts.push(Action::Checkout(b.name.clone()));
        } else if r.clicked() {
            acts.push(Action::SelectHash(b.target.clone()));
        }
        egui::Popup::context_menu(&r)
            .style(crate::theme::menu_style)
            .show(|ui| self.branch_menu_items(ui, acts, b, idle));
    }

    /// The menu of a local branch: in the sidebar and in the graph's commit menu.
    pub(super) fn branch_menu_items(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        b: &Branch,
        idle: bool,
    ) {
        let elsewhere = self
            .worktrees
            .iter()
            .find(|w| !w.current && w.branch.as_deref() == Some(b.name.as_str()));
        if let Some(wt) = elsewhere {
            if ui.button("Open its worktree in a tab").clicked() {
                acts.push(Action::OpenTab(wt.path.clone()));
            }
        } else {
            if ui
                .add_enabled(!b.is_head && idle, egui::Button::new("Check out"))
                .clicked()
            {
                acts.push(Action::Checkout(b.name.clone()));
            }
            if ui
                .add_enabled(
                    !b.is_head && idle,
                    egui::Button::new("Check out in a new worktree…"),
                )
                .clicked()
            {
                acts.push(Action::Dialog(Dialog::add_worktree(
                    &self.path,
                    Some(&b.name),
                    &self.refs.head,
                )));
            }
        }
        if let Some(current) = self.refs.current.as_deref()
            && !b.is_head
        {
            ui.separator();
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Fast-forward {current} to {}", b.name)),
                )
                .clicked()
            {
                acts.push(fast_forward_action(&b.name, current));
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Merge {} into {current}", b.name)),
                )
                .clicked()
            {
                acts.push(merge_action(&b.name, current));
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Rebase {current} onto {}", b.name)),
                )
                .clicked()
            {
                acts.push(rebase_action(&b.name, current));
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Interactive rebase {current} onto {}…", b.name)),
                )
                .clicked()
            {
                acts.push(Action::InteractiveRebase {
                    onto: b.name.clone(),
                    label: b.name.clone(),
                });
            }
            ui.separator();
            super::graph_view::compare_items(ui, acts, &b.name, current);
        }
        // The checked-out branch against the one pull requests usually go to.
        if b.is_head
            && let Some(base) = self.base_ref()
            && base != b.name
        {
            ui.separator();
            if ui
                .button(format!("Changes {} adds to {base}", b.name))
                .on_hover_text("What it changed since the two diverged, as a pull request shows it")
                .clicked()
            {
                acts.push(compare_action(&base, &b.name, true));
            }
        }
        ui.separator();
        if let Some(remote) = self.default_remote()
            && ui
                .add_enabled(idle, egui::Button::new(format!("Push to {remote}")))
                .clicked()
        {
            acts.push(Action::Ops {
                label: format!("Push {}", b.name),
                ops: ops(&[&["push", "-u", &remote, &b.name]]),
                refresh: Refresh::All,
            });
        }
        if let Some(forge) = self.forge {
            let existing = self.prs.iter().find(|p| p.branch == b.name);
            match existing {
                Some(pr) => {
                    if ui
                        .button(format!("Open {} #{}", forge.noun(), pr.number))
                        .clicked()
                    {
                        acts.push(Action::OpenUrl(pr.url.clone()));
                    }
                }
                None => {
                    if ui
                        .add_enabled(idle, egui::Button::new(format!("Create {}…", forge.noun())))
                        .clicked()
                    {
                        let base = self.default_base();
                        let title = self
                            .history
                            .iter()
                            .find(|c| c.hash == b.target)
                            .map(|c| c.summary.clone())
                            .unwrap_or_default();
                        acts.push(Action::Dialog(Dialog::create_pr(
                            forge.noun(),
                            &b.name,
                            &base,
                            &title,
                        )));
                    }
                }
            }
        }
        ui.separator();
        if ui.button("Create branch here…").clicked() {
            acts.push(Action::Dialog(Dialog::create_branch(&b.target, &b.name)));
        }
        if ui.button("Create tag here…").clicked() {
            acts.push(Action::Dialog(Dialog::tag(
                &b.target,
                &b.name,
                self.default_remote(),
            )));
        }
        if ui.add_enabled(idle, egui::Button::new("Rename…")).clicked() {
            acts.push(Action::Dialog(Dialog::rename_branch(&b.name)));
        }
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(b.name.clone());
        }
        ui.separator();
        if ui
            .add_enabled(!b.is_head && idle, egui::Button::new("Delete…"))
            .on_disabled_hover_text("Can't delete the checked-out branch")
            .clicked()
        {
            acts.push(Action::Dialog(
                Dialog::confirm(
                    "Delete branch",
                    format!("Delete the local branch “{}”?", b.name),
                    "Delete",
                    ops(&[&["branch", "-d", &b.name]]),
                    Some((
                        "Force, even if it isn't merged (-D)".to_owned(),
                        ops(&[&["branch", "-D", &b.name]]),
                    )),
                )
                .undo(Undo::Custom {
                    undo: ops(&[&["branch", &b.name, &b.target]]),
                    redo: ops(&[&["branch", "-D", &b.name]]),
                }),
            ));
        }
    }

    /// The branch pull requests go into by default: the remote's HEAD, else main / master.
    /// The branch to compare the current one with: the local default base, or its remote one.
    fn base_ref(&self) -> Option<String> {
        let base = self.default_base();
        if self.refs.local.iter().any(|b| b.name == base) {
            return Some(base);
        }
        let remote = format!("{}/{base}", self.default_remote()?);
        self.refs
            .remotes
            .iter()
            .flat_map(|(_, list)| list)
            .any(|b| b.name == remote)
            .then_some(remote)
    }

    fn default_base(&self) -> String {
        let remote = self.default_remote().unwrap_or_else(|| "origin".to_owned());
        let names: Vec<&str> = self
            .refs
            .remotes
            .iter()
            .filter(|(r, _)| *r == remote)
            .flat_map(|(_, list)| list.iter().map(|b| b.name.as_str()))
            .collect();
        for candidate in ["main", "master", "develop"] {
            if names.contains(&format!("{remote}/{candidate}").as_str()) {
                return candidate.to_owned();
            }
        }
        "main".to_owned()
    }

    fn remotes_section(&self, ui: &mut Ui, acts: &mut Vec<Action>, idle: bool, filter: &str) {
        let t = Tokens::get(ui.ctx());
        let (open, add) = widgets::section_with_button(
            ui,
            "remotes",
            "cloud",
            "REMOTES",
            self.remotes.len(),
            true,
            Some(("plus", "Add a remote…")),
        );
        if add && idle {
            acts.push(Action::Dialog(Dialog::remote(None)));
        }
        if !open {
            return;
        }
        let current = self.refs.current.clone();
        for remote in &self.remotes {
            let branches: Vec<&Branch> = self
                .refs
                .remotes
                .iter()
                .filter(|(r, _)| *r == remote.name)
                .flat_map(|(_, list)| list.iter())
                .filter(|b| filter.is_empty() || b.name.to_lowercase().contains(filter))
                .collect();
            // A collapsible row per remote.
            let id = ui.id().with(("remote-open", &remote.name));
            let mut expanded = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            if r.clicked() {
                expanded = !expanded;
                ui.data_mut(|d| d.insert_temp(id, expanded));
            }
            let y = rect.center().y;
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 24.0, y), vec2(12.0, 12.0)),
                if expanded {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                11.0,
                t.text_faint,
            );
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 40.0, y), vec2(14.0, 14.0)),
                "cloud",
                13.0,
                t.icon,
            );
            let p = ui.painter_at(rect);
            p.text(
                pos2(rect.left() + 54.0, y),
                Align2::LEFT_CENTER,
                &remote.name,
                theme::medium(12.5),
                t.text,
            );
            p.text(
                pos2(rect.right() - 10.0, y),
                Align2::RIGHT_CENTER,
                branches.len().to_string(),
                theme::medium(11.0),
                t.text_faint,
            );
            let r = r.on_hover_text(&remote.url);
            egui::Popup::context_menu(&r).style(crate::theme::menu_style).show(|ui| {
                if ui.add_enabled(idle, egui::Button::new(format!("Fetch {}", remote.name))).clicked() {
                    acts.push(Action::Ops {
                        label: format!("Fetch {}", remote.name),
                        ops: ops(&[&["fetch", "--prune", &remote.name]]),
                        refresh: Refresh::All,
                    });
                }
                if ui.add_enabled(idle, egui::Button::new("Edit…")).clicked() {
                    acts.push(Action::Dialog(Dialog::remote(Some((remote.name.clone(), remote.url.clone())))));
                }
                if ui.button("Copy URL").clicked() {
                    ui.ctx().copy_text(remote.url.clone());
                }
                ui.separator();
                if ui.add_enabled(idle, egui::Button::new("Remove…")).clicked() {
                    acts.push(Action::Dialog(Dialog::confirm(
                        "Remove remote",
                        format!(
                            "Remove the remote “{}” ({})? Its branches disappear from this repository; nothing changes on the server.",
                            remote.name, remote.url
                        ),
                        "Remove",
                        ops(&[&["remote", "remove", &remote.name]]),
                        None,
                    )));
                }
            });
            if !expanded {
                continue;
            }
            for b in branches {
                let short = b
                    .name
                    .strip_prefix(&format!("{}/", remote.name))
                    .unwrap_or(&b.name);
                let has_pr = self.prs.iter().any(|p| p.branch == short);
                let r = branch_row_indented(ui, short, has_pr)
                    .on_hover_text(format!("{}\nDouble-click to check out", b.name));
                if r.double_clicked() && idle {
                    acts.push(Action::CheckoutRemote(b.name.clone()));
                } else if r.clicked() {
                    acts.push(Action::SelectHash(b.target.clone()));
                }
                egui::Popup::context_menu(&r)
                    .style(crate::theme::menu_style)
                    .show(|ui| {
                        if ui
                            .add_enabled(idle, egui::Button::new("Check out as local branch"))
                            .clicked()
                        {
                            acts.push(Action::CheckoutRemote(b.name.clone()));
                        }
                        if let Some(current) = current.as_deref() {
                            if ui
                                .add_enabled(
                                    idle,
                                    egui::Button::new(format!("Fast-forward {current} to it")),
                                )
                                .clicked()
                            {
                                acts.push(fast_forward_action(&b.name, current));
                            }
                            if ui
                                .add_enabled(
                                    idle,
                                    egui::Button::new(format!("Merge into {current}")),
                                )
                                .clicked()
                            {
                                acts.push(merge_action(&b.name, current));
                            }
                            if ui
                                .add_enabled(
                                    idle,
                                    egui::Button::new(format!("Rebase {current} onto it")),
                                )
                                .clicked()
                            {
                                acts.push(rebase_action(&b.name, current));
                            }
                            ui.separator();
                            super::graph_view::compare_items(ui, acts, &b.name, current);
                            ui.separator();
                        }
                        if ui.button("Create branch here…").clicked() {
                            acts.push(Action::Dialog(Dialog::create_branch(&b.target, &b.name)));
                        }
                        ui.separator();
                        if ui.button("Copy name").clicked() {
                            ui.ctx().copy_text(b.name.clone());
                        }
                        ui.separator();
                        if ui
                            .add_enabled(idle, egui::Button::new("Delete from remote…"))
                            .clicked()
                        {
                            acts.push(Action::Dialog(Dialog::confirm(
                            "Delete remote branch",
                            format!(
                                "Delete “{short}” from {}? This changes the remote for everyone.",
                                remote.name
                            ),
                            "Delete",
                            ops(&[&["push", &remote.name, "--delete", short]]),
                            None,
                        )));
                        }
                    });
            }
        }
    }

    fn stash_section(&self, ui: &mut Ui, acts: &mut Vec<Action>, idle: bool) {
        let t = Tokens::get(ui.ctx());
        let (open, add) = widgets::section_with_button(
            ui,
            "stashes",
            "archive",
            "STASHES",
            self.stashes.len(),
            true,
            Some(("plus", "Stash changes…")),
        );
        if add && idle && !self.work.is_clean() {
            acts.push(Action::Dialog(Dialog::stash()));
        }
        if !open {
            return;
        }
        let now = git::now();
        for (i, stash) in self.stashes.iter().enumerate() {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
            if self.stash_view == Some(i) {
                ui.painter().rect_filled(rect, 0.0, t.row_selected);
            } else if r.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let y = rect.center().y;
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
                "archive",
                13.0,
                t.text_faint,
            );
            let p = ui.painter_at(rect);
            p.text(
                pos2(rect.left() + 48.0, y - 7.0),
                Align2::LEFT_CENTER,
                &stash.message,
                egui::FontId::proportional(12.5),
                t.text,
            );
            p.text(
                pos2(rect.left() + 48.0, y + 8.0),
                Align2::LEFT_CENTER,
                format!("{} · {}", stash.name, git::relative_time(stash.time, now)),
                egui::FontId::proportional(11.0),
                t.text_faint,
            );
            if r.clicked() {
                acts.push(Action::SelectStash(i));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| stash_menu_items(ui, acts, &stash.name, idle));
        }
    }

    fn pr_section(&self, ui: &mut Ui, acts: &mut Vec<Action>, idle: bool) {
        let Some(forge) = self.forge else { return };
        let t = Tokens::get(ui.ctx());
        let title = match forge {
            crate::forge::Forge::GitHub => "PULL REQUESTS",
            crate::forge::Forge::GitLab => "MERGE REQUESTS",
        };
        if !widgets::section(ui, "prs", "git-pull-request", title, self.prs.len(), true) {
            return;
        }
        if let Some(err) = &self.pr_error {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
            ui.painter_at(rect).text(
                pos2(rect.left() + 34.0, rect.center().y),
                Align2::LEFT_CENTER,
                err.lines().next().unwrap_or_default(),
                egui::FontId::proportional(11.0),
                t.text_faint,
            );
            r.on_hover_text(err.as_str());
            return;
        }
        for pr in &self.prs {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let y = rect.center().y;
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
                "git-pull-request",
                13.0,
                if pr.draft { t.text_faint } else { t.success },
            );
            let p = ui.painter_at(rect);
            p.text(
                pos2(rect.left() + 48.0, y - 7.0),
                Align2::LEFT_CENTER,
                format!("#{} {}", pr.number, pr.title),
                egui::FontId::proportional(12.5),
                t.text,
            );
            let sub = format!(
                "{} · {}{}",
                pr.branch,
                pr.author,
                if pr.draft { " · draft" } else { "" }
            );
            p.text(
                pos2(rect.left() + 48.0, y + 8.0),
                Align2::LEFT_CENTER,
                sub,
                egui::FontId::proportional(11.0),
                t.text_faint,
            );
            let r = r.on_hover_text(format!("{}\nDouble-click to open in the browser", pr.url));
            // The branch's tip, local or on a remote.
            let target = self
                .refs
                .local
                .iter()
                .find(|b| b.name == pr.branch)
                .or_else(|| {
                    self.refs
                        .remotes
                        .iter()
                        .flat_map(|(_, l)| l.iter())
                        .find(|b| {
                            b.name.split_once('/').map(|(_, n)| n) == Some(pr.branch.as_str())
                        })
                })
                .map(|b| b.target.clone());
            if r.double_clicked() {
                acts.push(Action::OpenUrl(pr.url.clone()));
            } else if r.clicked()
                && let Some(target) = &target
            {
                acts.push(Action::SelectHash(target.clone()));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| {
                    if ui.button("Open in the browser").clicked() {
                        acts.push(Action::OpenUrl(pr.url.clone()));
                    }
                    if ui
                        .add_enabled(idle, egui::Button::new(format!("Check out {}", pr.branch)))
                        .clicked()
                    {
                        if self.refs.local.iter().any(|b| b.name == pr.branch) {
                            acts.push(Action::Checkout(pr.branch.clone()));
                        } else {
                            let remote =
                                self.default_remote().unwrap_or_else(|| "origin".to_owned());
                            acts.push(Action::CheckoutRemote(format!("{remote}/{}", pr.branch)));
                        }
                    }
                    if ui.button("Copy URL").clicked() {
                        ui.ctx().copy_text(pr.url.clone());
                    }
                });
        }
    }
}

/// Apply / pop / drop for a stash (sidebar and detail pane).
pub(super) fn stash_menu_items(ui: &mut Ui, acts: &mut Vec<Action>, name: &str, idle: bool) {
    if ui
        .add_enabled(idle, egui::Button::new("Apply"))
        .on_hover_text("Bring the changes back and keep the stash")
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Apply {name}"),
            ops: ops(&[&["stash", "apply", "--index", name]]),
            refresh: Refresh::All,
        });
    }
    if ui
        .add_enabled(idle, egui::Button::new("Pop"))
        .on_hover_text("Bring the changes back and drop the stash")
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Pop {name}"),
            ops: ops(&[&["stash", "pop", "--index", name]]),
            refresh: Refresh::All,
        });
    }
    ui.separator();
    if ui.add_enabled(idle, egui::Button::new("Drop…")).clicked() {
        acts.push(Action::Dialog(Dialog::confirm(
            "Drop stash",
            format!("Delete {name}? Its changes are gone for good."),
            "Drop",
            ops(&[&["stash", "drop", name]]),
            None,
        )));
    }
}

impl RepoTab {
    fn worktree_section(&self, ui: &mut Ui, acts: &mut Vec<Action>, idle: bool) {
        let t = Tokens::get(ui.ctx());
        let (open, add) = widgets::section_with_button(
            ui,
            "worktrees",
            "folder-tree",
            "WORKTREES",
            self.worktrees.len(),
            true,
            Some(("plus", "Add a worktree…")),
        );
        if add && idle {
            acts.push(Action::Dialog(Dialog::add_worktree(
                &self.path,
                None,
                &self.refs.head,
            )));
        }
        if !open {
            return;
        }
        let any_prunable = self.worktrees.iter().any(|w| w.prunable.is_some());
        for wt in &self.worktrees {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
            let painter = ui.painter_at(rect);
            if wt.current {
                painter.rect_filled(rect, 0.0, t.accent.gamma_multiply(0.14));
            }
            if r.hovered() {
                painter.rect_filled(rect, 0.0, t.hover);
            }
            let y = rect.center().y;
            let icon = if wt.current { "check" } else { "folder-git-2" };
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
                icon,
                14.0,
                if wt.current { t.accent } else { t.text_faint },
            );
            let font = if wt.current {
                theme::semibold(12.5)
            } else {
                egui::FontId::proportional(12.5)
            };
            let mut right = rect.right() - 8.0;
            for (on, label, color) in [
                (wt.prunable.is_some(), "missing", t.danger),
                (wt.locked.is_some(), "locked", t.warning),
                (wt.main, "main", t.text_faint),
            ] {
                if on {
                    let r = painter.text(
                        pos2(right, y),
                        Align2::RIGHT_CENTER,
                        label,
                        theme::medium(10.5),
                        color,
                    );
                    right = r.left() - 6.0;
                }
            }
            let clip =
                painter.with_clip_rect(Rect::from_min_max(rect.min, pos2(right, rect.bottom())));
            clip.text(
                pos2(rect.left() + 48.0, y - 7.0),
                Align2::LEFT_CENTER,
                wt.name(),
                font,
                t.text,
            );
            let sub = match (&wt.branch, wt.bare) {
                (_, true) => "bare".to_owned(),
                (Some(branch), _) => branch.clone(),
                (None, _) => format!("detached at {}", wt.head.get(..8).unwrap_or(&wt.head)),
            };
            clip.text(
                pos2(rect.left() + 48.0, y + 8.0),
                Align2::LEFT_CENTER,
                sub,
                egui::FontId::proportional(11.0),
                t.text_faint,
            );

            let mut tip = wt.path.display().to_string();
            if let Some(reason) = wt.locked.as_deref().filter(|r| !r.is_empty()) {
                tip.push_str(&format!("\nLocked: {reason}"));
            }
            if let Some(reason) = &wt.prunable {
                tip.push_str(&format!("\nMissing: {reason}"));
            }
            if !wt.current {
                tip.push_str("\nDouble-click to open in a tab");
            }
            let r = r.on_hover_text(tip);
            if r.double_clicked() && !wt.current && wt.prunable.is_none() {
                acts.push(Action::OpenTab(wt.path.clone()));
            } else if r.clicked() && !wt.head.is_empty() {
                acts.push(Action::SelectHash(wt.head.clone()));
            }
            let path = wt.path.display().to_string();
            egui::Popup::context_menu(&r).style(crate::theme::menu_style).show(|ui| {
                if ui
                    .add_enabled(!wt.current && wt.prunable.is_none(), egui::Button::new("Open in a tab"))
                    .clicked()
                {
                    acts.push(Action::OpenTab(wt.path.clone()));
                }
                if ui.button("Open terminal here").clicked() {
                    acts.push(Action::NewTerminal { shell: None, cwd: Some(wt.path.clone()) });
                }
                if ui.button(super::reveal_label()).clicked() {
                    let _ = open::that_detached(&wt.path);
                }
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(path.clone());
                }
                if !wt.main {
                    ui.separator();
                    let (label, args) = if wt.locked.is_some() {
                        ("Unlock", vec!["worktree", "unlock", path.as_str()])
                    } else {
                        ("Lock", vec!["worktree", "lock", path.as_str()])
                    };
                    if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                        acts.push(Action::Ops {
                            label: format!("{label} worktree {}", wt.name()),
                            ops: ops(&[&args]),
                            refresh: Refresh::All,
                        });
                    }
                    if ui
                        .add_enabled(idle && !wt.current, egui::Button::new("Remove…"))
                        .on_disabled_hover_text("Open another worktree to remove this one")
                        .clicked()
                    {
                        acts.push(Action::Dialog(Dialog::confirm(
                            "Remove worktree",
                            format!(
                                "Remove the worktree at {path}? Its folder is deleted; the branch stays."
                            ),
                            "Remove",
                            ops(&[&["worktree", "remove", &path]]),
                            Some((
                                "Force, even with uncommitted changes or locked".to_owned(),
                                ops(&[&["worktree", "remove", "--force", "--force", &path]]),
                            )),
                        )));
                    }
                }
                if any_prunable {
                    ui.separator();
                    if ui.add_enabled(idle, egui::Button::new("Prune missing worktrees")).clicked() {
                        acts.push(Action::Ops {
                            label: "Prune worktrees".into(),
                            ops: ops(&[&["worktree", "prune"]]),
                            refresh: Refresh::All,
                        });
                    }
                }
            });
        }
    }
}

/// A branch: icon (a check for the checked-out one), name, ahead/behind counts.
#[allow(clippy::too_many_arguments)]
fn branch_row(
    ui: &mut Ui,
    name: &str,
    is_head: bool,
    ahead: u32,
    behind: u32,
    gone: bool,
    in_worktree: bool,
    has_pr: bool,
) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    if is_head {
        painter.rect_filled(rect, 0.0, t.accent.gamma_multiply(0.14));
    }
    if resp.hovered() {
        painter.rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0));
    if is_head {
        icons::paint(ui, icon_rect, "check", 14.0, t.accent);
    } else {
        icons::paint(ui, icon_rect, "git-branch", 13.0, t.text_faint);
    }

    let mut right = rect.right() - 8.0;
    let small = theme::mono(11.0);
    if has_pr {
        let ir = Rect::from_center_size(pos2(right - 6.0, y), vec2(12.0, 12.0));
        icons::paint(ui, ir, "git-pull-request", 12.0, t.success);
        right = ir.left() - 6.0;
    }
    if in_worktree {
        let ir = Rect::from_center_size(pos2(right - 6.0, y), vec2(12.0, 12.0));
        icons::paint(ui, ir, "folder-tree", 12.0, t.text_dim);
        right = ir.left() - 6.0;
    }
    if gone {
        let r = painter.text(
            pos2(right, y),
            Align2::RIGHT_CENTER,
            "gone",
            theme::medium(10.5),
            t.text_faint,
        );
        right = r.left() - 6.0;
    }
    for (n, icon) in [(behind, "arrow-down"), (ahead, "arrow-up")] {
        if n > 0 {
            let r = painter.text(
                pos2(right, y),
                Align2::RIGHT_CENTER,
                n.to_string(),
                small.clone(),
                t.text_dim,
            );
            let ir = Rect::from_center_size(pos2(r.left() - 7.0, y), vec2(12.0, 12.0));
            icons::paint(ui, ir, icon, 11.0, t.text_dim);
            right = ir.left() - 6.0;
        }
    }
    let font = if is_head {
        theme::semibold(12.5)
    } else {
        egui::FontId::proportional(12.5)
    };
    painter
        .with_clip_rect(Rect::from_min_max(
            pos2(rect.left(), rect.top()),
            pos2(right, rect.bottom()),
        ))
        .text(
            pos2(rect.left() + 48.0, y),
            Align2::LEFT_CENTER,
            name,
            font,
            t.text,
        );
    resp
}

/// A remote's branch, one level in under its remote.
fn branch_row_indented(ui: &mut Ui, name: &str, has_pr: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 52.0, y), vec2(16.0, 16.0)),
        "git-branch",
        13.0,
        t.text_faint,
    );
    let mut right = rect.right() - 8.0;
    if has_pr {
        let ir = Rect::from_center_size(pos2(right - 6.0, y), vec2(12.0, 12.0));
        icons::paint(ui, ir, "git-pull-request", 12.0, t.success);
        right = ir.left() - 6.0;
    }
    ui.painter_at(rect.with_max_x(right)).text(
        pos2(rect.left() + 66.0, y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        t.text,
    );
    resp
}

/// An icon, a name and optional faint text after it.
fn plain_row(ui: &mut Ui, icon: &str, name: &str, sub: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
        icon,
        13.0,
        t.text_faint,
    );
    let p = ui.painter_at(rect);
    let r = p.text(
        pos2(rect.left() + 48.0, y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        t.text,
    );
    if !sub.is_empty() {
        p.text(
            pos2(r.right() + 8.0, y),
            Align2::LEFT_CENTER,
            sub,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
    }
    resp
}
