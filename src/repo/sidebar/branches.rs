//! The local branches: their section, rows and menu.

use egui::{Align2, Rect, Response, Sense, Ui, pos2, vec2};

use super::{ROW_H, matches, remote_short};
use crate::ci::Status;
use crate::git::Branch;
use crate::icons;
use crate::repo::branch_tree::{self, LeafRow, TreeView};
use crate::repo::graph_view;
use crate::repo::pinned_sections::{Button, Header, Sections};
use crate::repo::{
    Action, Dialog, RepoTab, Undo, ci_status, compare_action, fast_forward_action, merge_action,
    ops, rebase_action,
};
use crate::settings::Settings;
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(super) fn local_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        filter: &str,
        sections: &mut Sections,
    ) {
        let local: Vec<&Branch> = self
            .refs
            .local
            .iter()
            .filter(|b| matches(filter, b))
            .collect();
        let can_branch = idle && !self.refs.head.is_empty();
        let open = sections.add(
            ui,
            Header {
                key: "local",
                icon: "laptop",
                title: "LOCAL",
                count: local.len(),
                default_open: true,
                button: Some(Button {
                    icon: "plus",
                    tooltip: "New branch at HEAD…",
                    action: can_branch
                        .then(|| Action::Dialog(Dialog::create_branch(&self.refs.head, "HEAD"))),
                }),
            },
        );
        if !open {
            return;
        }
        let tree = branch_tree::build(local.iter().map(|b| (b.name.as_str(), *b)));
        let view = TreeView::branches(
            egui::Id::new(("branch-folder", self.repo_key(), "local")),
            !filter.is_empty(),
            0.0,
        );
        branch_tree::show(
            ui,
            &tree,
            &view,
            &|b: &&Branch| b.is_head,
            &mut |ui, row, b: &&Branch| self.local_branch_row(ui, acts, b, row, idle),
        );
    }

    fn local_branch_row(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        b: &Branch,
        row: LeafRow<'_>,
        idle: bool,
    ) {
        let (ahead, behind) = b.ahead_behind();
        // Checked out in another worktree: git won't check it out here too.
        let elsewhere = self
            .worktrees
            .iter()
            .find(|w| !w.current && w.branch.as_deref() == Some(b.name.as_str()));
        let has_pr = self.prs.iter().any(|p| p.branch == b.name);
        let ci = self.ci_status(&b.target);
        let marks = Marks {
            gone: b.upstream_gone(),
            in_worktree: elsewhere.is_some(),
            has_pr,
            ci,
        };
        let (r, ci_act) = branch_row(ui, row, b.is_head, ahead, behind, marks);
        acts.extend(ci_act);
        let mut tip = match (b.upstream.is_empty(), b.upstream_gone()) {
            (true, _) => "Not tracking a remote branch".to_owned(),
            (false, true) => format!("{} was deleted on the remote", b.upstream),
            (false, false) => format!("Tracks {}", b.upstream),
        };
        if let Some(status) = ci {
            tip.push_str(&format!("\nCI: {}", ci_status::state_word(status.state)));
        }
        let r = match elsewhere {
            Some(wt) => r.on_hover_text(format!(
                "{}\n{tip}\nChecked out in worktree {}\nDouble-click to open it",
                b.name,
                wt.path.display()
            )),
            None => r.on_hover_text(format!("{}\n{tip}\nDouble-click to check out", b.name)),
        };
        if r.double_clicked() && elsewhere.is_some() {
            if let Some(wt) = elsewhere {
                acts.push(Action::OpenWorktree(wt.path.clone()));
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
    pub(in crate::repo) fn branch_menu_items(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        b: &Branch,
        idle: bool,
    ) {
        self.checkout_items(ui, acts, b, idle);
        if let Some(current) = self.refs.current.as_deref()
            && !b.is_head
        {
            ui.separator();
            integrate_items(ui, acts, b, current, idle);
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
        self.publish_items(ui, acts, b, idle);
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
                    ops(&[&["branch", "-d", "--", &b.name]]),
                    Some((
                        "Force, even if it isn't merged (-D)".to_owned(),
                        ops(&[&["branch", "-D", "--", &b.name]]),
                    )),
                )
                .undo(Undo::Custom {
                    undo: ops(&[&["branch", "--", &b.name, &b.target]]),
                    redo: ops(&[&["branch", "-D", "--", &b.name]]),
                }),
            ));
        }
    }

    /// Check out `b`, here or in a new worktree; or open the worktree it is checked out in.
    fn checkout_items(&self, ui: &mut Ui, acts: &mut Vec<Action>, b: &Branch, idle: bool) {
        let elsewhere = self
            .worktrees
            .iter()
            .find(|w| !w.current && w.branch.as_deref() == Some(b.name.as_str()));
        if let Some(wt) = elsewhere {
            let label = if Settings::get(ui.ctx()).worktrees_in_same_tab {
                "Open its worktree"
            } else {
                "Open its worktree in a tab"
            };
            if ui.button(label).clicked() {
                acts.push(Action::OpenWorktree(wt.path.clone()));
            }
            return;
        }
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

    /// Push `b` (checked out or not), set what it tracks, and open or create its pull request.
    fn publish_items(&self, ui: &mut Ui, acts: &mut Vec<Action>, b: &Branch, idle: bool) {
        if let Some((to, push)) = self.push_branch(b)
            && ui
                .add_enabled(idle, egui::Button::new(format!("Push to {to}")))
                .clicked()
        {
            acts.push(push);
        }
        if let Some(remote) = self.default_remote()
            && ui
                .add_enabled(idle, egui::Button::new("Set upstream…"))
                .clicked()
        {
            let (remote, name) = self
                .split_upstream(&b.upstream)
                .map(|(r, n)| (r, n.to_owned()))
                .unwrap_or((remote, b.name.clone()));
            let undo = if b.upstream.is_empty() {
                ops(&[&["branch", "--unset-upstream", &b.name]])
            } else {
                self.track_ops(&b.name, &b.upstream)
            };
            acts.push(Action::Dialog(Dialog::set_upstream(
                &b.name, remote, &name, undo,
            )));
        }
        if !b.upstream.is_empty()
            && ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Stop tracking {}", b.upstream)),
                )
                .clicked()
        {
            acts.push(self.unset_upstream_action(b));
        }
        let Some(forge) = self.forge else { return };
        match self.prs.iter().find(|p| p.branch == b.name) {
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
            .remote_branches(&remote)
            .filter_map(|b| remote_short(&b.name, &remote))
            .collect();
        for candidate in ["main", "master", "develop"] {
            if names.contains(&candidate) {
                return candidate.to_owned();
            }
        }
        "main".to_owned()
    }
}

/// Fast-forward, merge, rebase and compare `current` with `b`.
fn integrate_items(ui: &mut Ui, acts: &mut Vec<Action>, b: &Branch, current: &str, idle: bool) {
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
    graph_view::compare_items(ui, acts, &b.name, current);
}

/// What a branch's row shows on the right besides how far ahead and behind it is.
struct Marks<'a> {
    /// Its upstream was deleted on the remote.
    gone: bool,
    /// Checked out in another worktree.
    in_worktree: bool,
    has_pr: bool,
    /// The build status of its tip.
    ci: Option<&'a Status>,
}

/// A branch: icon (a check for the checked-out one), name, ahead/behind counts; and what
/// clicking its build status asked for.
fn branch_row(
    ui: &mut Ui,
    row: LeafRow<'_>,
    is_head: bool,
    ahead: u32,
    behind: u32,
    marks: Marks,
) -> (Response, Option<Action>) {
    let Marks {
        gone,
        in_worktree,
        has_pr,
        ci,
    } = marks;
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
    let icon_rect =
        Rect::from_center_size(pos2(rect.left() + 34.0 + row.indent, y), vec2(16.0, 16.0));
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
    let mut ci_act = None;
    if let Some(status) = ci {
        let center = pos2(right - ci_status::DOT_R, y);
        ci_act = ci_status::dot(ui, resp.id.with("ci"), center, status);
        right = center.x - ci_status::DOT_R - 6.0;
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
            pos2(rect.left() + 48.0 + row.indent, y),
            Align2::LEFT_CENTER,
            row.name,
            font,
            t.text,
        );
    (resp, ci_act)
}
