//! What the views ask the tab to do, the actions several views build, and the command palette.

use std::path::PathBuf;

use super::pull::PullMode;
use super::toolbar::reveal_label;
use super::{Dialog, DiffKey, Refresh, RepoTab, Undo, ci_view, diff, file_view, ops, pr_review};
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
    SaveConflict,
    /// Let Mergiraf solve what it can of the open conflict.
    SolveWithMergiraf,
    /// A file's history or blame in the central pane (`rev`: the commit to blame at).
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

/// Compares branch (or commit) `base` with `target`; `since_fork`: only what `target` changed
/// since they diverged.
pub(crate) fn compare_action(base: &str, target: &str, since_fork: bool) -> Action {
    Action::Compare {
        base: base.to_owned(),
        base_label: base.to_owned(),
        target: Some(target.to_owned()),
        target_label: target.to_owned(),
        since_fork,
    }
}

pub(crate) fn merge_action(branch: &str, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Merge {branch} into {current}"),
        ops: ops(&[&["merge", "--no-edit", branch]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

/// Moves the current branch forward to `branch`; fails (and says so) if they have diverged.
pub(crate) fn fast_forward_action(branch: &str, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Fast-forward {current} to {branch}"),
        ops: ops(&[&["merge", "--ff-only", branch]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

pub(crate) fn rebase_action(onto: &str, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Rebase {current} onto {onto}"),
        ops: ops(&[&["rebase", onto]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

pub(super) fn abort_action(state: InProgress) -> Action {
    Action::Ops {
        label: format!("Abort {}", state.label().to_lowercase()),
        ops: ops(&[&[state.command(), "--abort"]]),
        refresh: Refresh::All,
    }
}

impl RepoTab {
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
                        compare_action(current, &b.name, true),
                    ));
                    items.push((
                        format!("Compare {current} with {}", b.name),
                        "compare".into(),
                        compare_action(current, &b.name, false),
                    ));
                    items.push((
                        format!("Merge {} into {current}", b.name),
                        "merge".into(),
                        merge_action(&b.name, current),
                    ));
                    items.push((
                        format!("Rebase {current} onto {}", b.name),
                        "rebase".into(),
                        rebase_action(&b.name, current),
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
