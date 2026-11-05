//! The right-click menu of a commit row, and of a branch pill on it.

use egui::{Response, Ui};

use super::badge::{BadgeKind, badges};
use super::compare_items;
use crate::git;
use crate::repo::{
    Action, Dialog, Refresh, RepoTab, Undo, WIP_HASH, fast_forward_action, merge_action, ops,
    rebase_action,
};

/// Where a row remembers which branch pill was right-clicked, for its menu.
pub(super) fn pill_menu_id(hash: &str) -> egui::Id {
    egui::Id::new(("pill_menu", hash))
}

impl RepoTab {
    /// Right-click menu of a commit row.
    pub(super) fn commit_menu(
        &self,
        resp: &Response,
        i: usize,
        idle: bool,
        acts: &mut Vec<Action>,
    ) {
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
    pub(super) fn commit_menu_items(
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
            self.pill_items(ui, branch, idle, acts);
            ui.separator();
        }
        self.branch_here_items(ui, &hash, &short, idle, is_head, acts);
        ui.separator();
        self.compare_with_items(ui, i, &hash, &short, acts);
        ui.separator();
        self.quick_edit_items(ui, &hash, idle, acts);
        ui.separator();
        rewrite_items(ui, &hash, &short, &summary, &target, idle, is_head, acts);
        ui.separator();
        let merge = commit.parents.len() > 1;
        if ui
            .add_enabled(!merge, egui::Button::new("Create patch from commit…"))
            .on_hover_text("Save it as a file for git am (git format-patch)")
            .on_disabled_hover_text("A merge commit makes no patch")
            .clicked()
        {
            acts.push(Action::SavePatch {
                hash: hash.clone(),
                summary: summary.clone(),
            });
        }
        if is_head
            && ui
                .add_enabled(idle, egui::Button::new("Apply patch…"))
                .on_hover_text(super::APPLY_PATCH_TIP)
                .clicked()
        {
            acts.push(Action::ApplyPatch);
        }
        ui.separator();
        copy_items(ui, &hash, &summary, branch_name.as_deref());
    }

    /// Actions on the right-clicked `branch`: check it out, or bring it into the checked-out
    /// branch.
    fn pill_items(&self, ui: &mut Ui, branch: &str, idle: bool, acts: &mut Vec<Action>) {
        if ui
            .add_enabled(idle, egui::Button::new(format!("Check out {branch}")))
            .clicked()
        {
            let local = self.refs.local.iter().any(|b| b.name == branch);
            acts.push(if local {
                Action::Checkout(branch.to_owned())
            } else {
                Action::CheckoutRemote(branch.to_owned())
            });
        }
        let Some(current) = self.refs.current.as_deref() else {
            return;
        };
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
                onto: branch.to_owned(),
                label: branch.to_owned(),
            });
        }
        ui.separator();
        compare_items(ui, acts, branch, current);
    }

    /// A submenu for each local branch pointing at the commit, creating a branch or tag there,
    /// and checking it out detached.
    fn branch_here_items(
        &self,
        ui: &mut Ui,
        hash: &str,
        short: &str,
        idle: bool,
        is_head: bool,
        acts: &mut Vec<Action>,
    ) {
        // Local branches pointing here get their own submenu.
        let here: Vec<&git::Branch> = self
            .refs
            .local
            .iter()
            .filter(|b| b.target == hash)
            .collect();
        for b in &here {
            ui.menu_button(format!("Branch {}", b.name), |ui| {
                self.branch_menu_items(ui, acts, b, idle);
            });
        }
        if !here.is_empty() {
            ui.separator();
        }
        if ui.button("Create branch here…").clicked() {
            acts.push(Action::Dialog(Dialog::create_branch(hash, short)));
        }
        if ui.button("Create tag here…").clicked() {
            acts.push(Action::Dialog(Dialog::tag(
                hash,
                short,
                self.default_remote(),
            )));
        }
        if ui
            .button("Create worktree from this commit…")
            .on_hover_text("A second working folder on a new branch starting here")
            .clicked()
        {
            acts.push(Action::Dialog(Dialog::worktree_at(&self.path, hash)));
        }
        if ui
            .add_enabled(
                idle && !is_head,
                egui::Button::new("Check out this commit (detached)"),
            )
            .clicked()
        {
            acts.push(Action::Checkout(hash.to_owned()));
        }
    }

    /// Comparing row `i` with the selected row, or with the working tree.
    fn compare_with_items(
        &self,
        ui: &mut Ui,
        i: usize,
        hash: &str,
        short: &str,
        acts: &mut Vec<Action>,
    ) {
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
                        crate::repo::short(&c.hash).to_owned()
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
                base: hash.to_owned(),
                base_label: short.to_owned(),
                target: None,
                target_label: "working tree".to_owned(),
                since_fork: false,
            });
        }
    }
}

/// Entries that rewrite `target` (the checked-out branch) from the commit: interactive rebase,
/// cherry-pick, revert and reset.
#[allow(clippy::too_many_arguments)]
fn rewrite_items(
    ui: &mut Ui,
    hash: &str,
    short: &str,
    summary: &str,
    target: &str,
    idle: bool,
    is_head: bool,
    acts: &mut Vec<Action>,
) {
    if ui
        .add_enabled(
            idle && !is_head,
            egui::Button::new(format!("Interactive rebase {target} onto this commit…")),
        )
        .on_hover_text("Reorder, reword, squash or drop the commits after this one")
        .clicked()
    {
        acts.push(Action::InteractiveRebase {
            onto: hash.to_owned(),
            label: short.to_owned(),
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
            ops: ops(&[&["cherry-pick", hash]]),
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
            ops: ops(&[&["revert", "--no-edit", hash]]),
            refresh: Refresh::All,
            undo: Undo::HeadMove { mode: "--keep" },
        });
    }
    ui.add_enabled_ui(idle && !is_head, |ui| {
        ui.menu_button(format!("Reset {target} to here"), |ui| {
            reset_items(ui, hash, short, summary, target, acts);
        });
    });
}

/// The reset modes, each asking before it moves `target` to the commit.
fn reset_items(
    ui: &mut Ui,
    hash: &str,
    short: &str,
    summary: &str,
    target: &str,
    acts: &mut Vec<Action>,
) {
    for (mode, undo_mode, label, explain) in [
        ("--soft", "--soft", "Soft", "Keeps every change, staged."),
        (
            "--mixed",
            "--mixed",
            "Mixed",
            "Keeps every change, unstaged.",
        ),
        (
            "--hard",
            "--keep",
            "Hard",
            "Throws away uncommitted changes and the commits after this one. Uncommitted \
             work can't be recovered.",
        ),
    ] {
        let first = explain.split('.').next().unwrap_or(explain);
        if ui.button(format!("{label} — {first}")).clicked() {
            acts.push(Action::Dialog(
                Dialog::confirm(
                    &format!("{label} reset"),
                    format!("Move {target} to {short} ({summary})? {explain}"),
                    &format!("Reset {}", label.to_lowercase()),
                    ops(&[&["reset", mode, hash]]),
                    None,
                )
                .undo(Undo::HeadMove { mode: undo_mode }),
            ));
        }
    }
}

/// Copying the commit's hash, message, and `branch` when there is one.
fn copy_items(ui: &mut Ui, hash: &str, summary: &str, branch: Option<&str>) {
    if ui.button("Copy commit hash").clicked() {
        ui.ctx().copy_text(hash.to_owned());
    }
    if ui.button("Copy message").clicked() {
        ui.ctx().copy_text(summary.to_owned());
    }
    if let Some(name) = branch
        && ui.button(format!("Copy branch name ({name})")).clicked()
    {
        ui.ctx().copy_text(name.to_owned());
    }
}
