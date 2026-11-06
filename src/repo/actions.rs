//! What the views ask the tab to do, the actions several views build, and the command palette.

use std::path::PathBuf;

use super::pull::PullMode;
use super::toolbar::reveal_label;
use super::{
    Dialog, DiffKey, Refresh, RepoTab, Undo, branch_visibility, ci_view, diff, file_view, ops,
    pr_review, quick_edit,
};
use crate::git::{self, InProgress};
use crate::terminal::ShellSpec;

/// Everything the views ask for; applied after drawing so views only borrow the state.
pub(crate) enum Action {
    Select(usize),
    SelectHash(String),
    SelectStash(usize),
    OpenDiff(DiffKey),
    /// Show a pull request in the big area.
    OpenPr(u64),
    ClosePr,
    ReloadPr,
    /// Something done on the pull request shown.
    Pr(pr_review::PrAct),
    /// Show the builds of a commit in the big area, a run of them first if it's given.
    OpenCi {
        sha: String,
        run: Option<u64>,
    },
    CloseCi,
    /// Something done in the pipeline view.
    Ci(ci_view::CiAct),
    CloseDiff,
    Ops {
        label: String,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
    },
    UndoableOps {
        label: String,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        undo: Undo,
    },
    Checkout(String),
    /// Checks out a local branch behind its upstream, then fast-forwards it.
    CheckoutPull(String),
    CheckoutRemote(String),
    Dialog(Dialog),
    Commit,
    Fetch,
    /// Pull the way the settings say.
    Pull,
    PullWith(PullMode),
    Push,
    Reload,
    LoadMore,
    Undo,
    Redo,
    StashPush(String),
    /// Open a folder (a worktree) in its own tab.
    OpenTab(PathBuf),
    /// Open a worktree the way the settings say: in this tab, or in its own.
    OpenWorktree(PathBuf),
    ToggleTerminal,
    /// Collapse or bring back the branches panel.
    ToggleBranches,
    /// Open the branches panel at a section (by its key), or in its filter when `None`.
    OpenBranches(Option<&'static str>),
    /// Hide, show or solo branches in the graph.
    BranchVisibility(branch_visibility::Change),
    /// Collapse or bring back the detail panel.
    ToggleDetail,
    NewTerminal {
        shell: Option<ShellSpec>,
        cwd: Option<PathBuf>,
    },
    CloseTerminal(usize),
    SelectTerminal(usize),
    ToggleSearch,
    SearchStep(i32),
    CreatePr {
        branch: String,
        base: String,
        title: String,
        body: String,
        draft: bool,
    },
    OpenUrl(String),
    /// Open a path (relative to the repository) with the system's default app.
    OpenPath(PathBuf),
    OpenInEditor(PathBuf),
    SetIdentity(String, String),
    /// How diffs show: side by side, without whitespace changes, the whole file.
    SetDiffOptions {
        split: bool,
        ignore_ws: bool,
        whole_file: bool,
    },
    /// Diffs show the whole file as it is after the change, or the changes.
    DiffAsFile(bool),
    /// Scroll the diff to the next change (`1`) or the previous one (`-1`).
    DiffStep(i32),
    /// The commit pane lists every file of the commit, or only those it changed.
    ViewAllFiles(bool),
    SaveConflict,
    /// Let Mergiraf solve what it can of the open conflict.
    SolveWithMergiraf,
    /// A file's history, blame or contents in the central pane (`rev`: the commit they are at).
    FileView {
        path: String,
        rev: Option<String>,
        tab: file_view::FileTab,
    },
    CloseFileView,
    /// Open the interactive rebase editor for the commits after `onto`.
    /// Gives a commit of the checked-out branch a new message: amending the latest, rebasing
    /// for an older one.
    Reword {
        hash: String,
        message: String,
    },
    /// Open the editor of a commit's message in the detail pane.
    EditMessage(String),
    /// Save a commit as a patch file, asking where.
    SavePatch {
        hash: String,
        summary: String,
    },
    /// Apply a patch file, asking which.
    ApplyPatch,
    /// Drop or move a commit of the checked-out branch.
    QuickEdit {
        edit: quick_edit::Edit,
        hash: String,
    },
    InteractiveRebase {
        onto: String,
        label: String,
    },
    RunRebase,
    CloseRebase,
    /// Remember these HTTPS credentials and run the operation that needed them again.
    SignIn(git::Credentials),
    /// Add `pattern` to `.gitignore`; stop tracking `path` if git tracks it.
    Ignore {
        pattern: String,
        path: String,
    },
    /// Stage, unstage or discard these changed lines of the open diff.
    PickLines {
        pick: diff::Pick,
        lines: std::collections::BTreeSet<usize>,
    },
    /// Compare `base` with `target` (the working tree when `None`) in the right pane.
    Compare {
        base: String,
        base_label: String,
        target: Option<String>,
        target_label: String,
        since_fork: bool,
    },
    /// Show only the target's changes since the two diverged, or every difference.
    CompareSinceFork(bool),
    SwapCompare,
    CloseCompare,
    /// Search commit messages, or what commits changed in files.
    SearchCode(bool),
}

/// Compares two branches, given to git by their full refs (a tag of the same name is not
/// what is meant), with their names as labels.
pub(crate) fn compare_branches_action(
    base: BranchRef<'_>,
    target: BranchRef<'_>,
    since_fork: bool,
) -> Action {
    Action::Compare {
        base: base.full(),
        base_label: base.name.to_owned(),
        target: Some(target.full()),
        target_label: target.name.to_owned(),
        since_fork,
    }
}

/// A branch an action brings in or goes onto: git is given its full ref, as a tag (or the
/// other kind of branch) of the same short name would be what `x` means to it; people see the
/// name.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BranchRef<'a> {
    /// `main`, or `origin/main` for a remote-tracking branch.
    pub name: &'a str,
    pub remote: bool,
}

impl<'a> BranchRef<'a> {
    pub(crate) fn local(name: &'a str) -> Self {
        Self {
            name,
            remote: false,
        }
    }

    pub(crate) fn remote(name: &'a str) -> Self {
        Self { name, remote: true }
    }

    /// `refs/heads/main`, `refs/remotes/origin/main`.
    pub(crate) fn full(&self) -> String {
        if self.remote {
            branch_visibility::remote_ref(self.name)
        } else {
            branch_visibility::local_ref(self.name)
        }
    }
}

/// The message git gives merging `branch` into `current` by its short name; given the full
/// ref, it would say `Merge branch 'refs/heads/x'`. Like git, without `into` for `main` and
/// `master`.
pub(crate) fn merge_message(branch: BranchRef<'_>, current: &str) -> String {
    let kind = if branch.remote {
        "remote-tracking branch"
    } else {
        "branch"
    };
    let mut message = format!("Merge {kind} '{}'", branch.name);
    if !matches!(current, "main" | "master") {
        message.push_str(&format!(" into {current}"));
    }
    message
}

pub(crate) fn merge_action(branch: BranchRef<'_>, current: &str) -> Action {
    let message = merge_message(branch, current);
    Action::UndoableOps {
        label: format!("Merge {} into {current}", branch.name),
        ops: ops(&[&["merge", "--no-edit", "-m", &message, &branch.full()]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

/// Moves the current branch forward to `branch`; fails (and says so) if they have diverged.
pub(crate) fn fast_forward_action(branch: BranchRef<'_>, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Fast-forward {current} to {}", branch.name),
        ops: ops(&[&["merge", "--ff-only", &branch.full()]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

pub(crate) fn rebase_action(onto: BranchRef<'_>, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Rebase {current} onto {}", onto.name),
        ops: ops(&[&["rebase", &onto.full()]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

/// Opens the interactive rebase editor for the checked-out branch onto `onto`.
pub(crate) fn interactive_rebase_action(onto: BranchRef<'_>) -> Action {
    Action::InteractiveRebase {
        onto: onto.full(),
        label: onto.name.to_owned(),
    }
}

/// Asks before aborting: the conflicts resolved so far (in the index only) are thrown away, with
/// nothing to undo it.
pub(super) fn abort_action(state: InProgress) -> Action {
    let what = state.label().to_lowercase();
    Action::Dialog(Dialog::confirm(
        &format!("Abort {what}"),
        format!(
            "Stop the {what} and go back to where it started? Conflicts resolved so far are \
             lost, and can't be brought back."
        ),
        "Abort",
        ops(&[&[state.command(), "--abort"]]),
        None,
    ))
}

impl RepoTab {
    /// Changes whenever what [`Self::palette_items`] is made from (refs, remotes, submodules…)
    /// is read again: the reload, and the submodules read since.
    pub(crate) fn palette_generation(&self) -> (u64, u64) {
        (self.history_gen, self.submodules_read)
    }

    /// What the command palette offers for this repository.
    pub(crate) fn palette_items(&self) -> Vec<(String, String, Action)> {
        let mut items: Vec<(String, String, Action)> = vec![
            ("Fetch".into(), "remote".into(), Action::Fetch),
            ("Pull".into(), "remote".into(), Action::Pull),
            ("Push".into(), "remote".into(), Action::Push),
        ];
        items.extend(
            PullMode::ALL.map(|m| (m.label().into(), "remote".into(), Action::PullWith(m))),
        );
        items.extend([
            ("Apply patch…".into(), "patch".into(), Action::ApplyPatch),
            ("Undo".into(), "Ctrl+Z".into(), Action::Undo),
            ("Redo".into(), "Ctrl+Y".into(), Action::Redo),
            (
                "Stash changes".into(),
                "stash".into(),
                Action::Dialog(Dialog::stash()),
            ),
            (
                "Search commits".into(),
                "Ctrl+F".into(),
                Action::ToggleSearch,
            ),
            (
                "Toggle terminal".into(),
                "terminal".into(),
                Action::ToggleTerminal,
            ),
            (
                "New terminal".into(),
                "terminal".into(),
                Action::NewTerminal {
                    shell: None,
                    cwd: None,
                },
            ),
            ("Reload".into(), "F5".into(), Action::Reload),
            (
                reveal_label().into(),
                "folder".into(),
                Action::OpenPath(PathBuf::new()),
            ),
            (
                "Open in editor".into(),
                "folder".into(),
                Action::OpenInEditor(PathBuf::new()),
            ),
        ]);
        if let Some(url) = self.web_page() {
            items.push((
                "Open in browser".into(),
                "remote".into(),
                Action::OpenUrl(url),
            ));
        }
        if !self.submodules.is_empty() {
            items.push((
                "Update submodules".into(),
                "submodule".into(),
                super::keep_submodules::update_all(),
            ));
        }
        if self.lfs {
            for (label, args) in [
                ("LFS: Fetch", ["lfs", "fetch"]),
                ("LFS: Pull", ["lfs", "pull"]),
            ] {
                items.push((
                    label.into(),
                    "remote".into(),
                    Action::Ops {
                        label: label.into(),
                        ops: ops(&[&args]),
                        refresh: Refresh::All,
                    },
                ));
            }
        }
        if !self.refs.head.is_empty() && self.ci_project().is_some() {
            items.push((
                "Show CI for HEAD".into(),
                "builds".into(),
                Action::OpenCi {
                    sha: self.refs.head.clone(),
                    run: None,
                },
            ));
        }
        if !self.refs.head.is_empty() {
            items.push((
                "Create branch".into(),
                "branch".into(),
                Action::Dialog(Dialog::create_branch(&self.refs.head, "HEAD")),
            ));
            items.push((
                "Create tag".into(),
                "tag".into(),
                Action::Dialog(Dialog::tag(&self.refs.head, "HEAD", self.default_remote())),
            ));
        }
        for b in &self.refs.local {
            if !b.is_head {
                items.push((
                    format!("Check out {}", b.name),
                    "branch".into(),
                    Action::Checkout(b.name.clone()),
                ));
                if let Some((to, push)) = self.push_branch(b) {
                    items.push((format!("Push {} to {to}", b.name), "remote".into(), push));
                }
            }
        }
        if let Some(current) = &self.refs.current {
            for b in &self.refs.local {
                if !b.is_head {
                    items.push((
                        format!("Changes {} adds to {current}", b.name),
                        "compare".into(),
                        compare_branches_action(
                            BranchRef::local(current),
                            BranchRef::local(&b.name),
                            true,
                        ),
                    ));
                    items.push((
                        format!("Compare {current} with {}", b.name),
                        "compare".into(),
                        compare_branches_action(
                            BranchRef::local(current),
                            BranchRef::local(&b.name),
                            false,
                        ),
                    ));
                    items.push((
                        format!("Merge {} into {current}", b.name),
                        "merge".into(),
                        merge_action(BranchRef::local(&b.name), current),
                    ));
                    items.push((
                        format!("Rebase {current} onto {}", b.name),
                        "rebase".into(),
                        rebase_action(BranchRef::local(&b.name), current),
                    ));
                }
            }
        }
        for (remote, branches) in &self.refs.remotes {
            for b in branches {
                items.push((
                    format!("Check out {}", b.name),
                    remote.clone(),
                    Action::CheckoutRemote(b.name.clone()),
                ));
            }
        }
        items
    }
}
