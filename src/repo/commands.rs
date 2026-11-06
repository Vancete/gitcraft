//! The tab's everyday git commands: checkout, push, commit, stash, creating a pull request.

use std::time::Instant;

use super::actions::BranchRef;
use super::operations::{expect_ref, stash_push_args};
use super::{Action, Msg, OpKind, Refresh, RepoTab, Undo, ops, sidebar, spawn};
use crate::forge;
use crate::git::{self, Branch};

impl RepoTab {
    /// The checked-out branch, or HEAD's commit when detached (empty without commits).
    fn checked_out(&self) -> String {
        self.refs
            .current
            .clone()
            .unwrap_or_else(|| self.refs.head.clone())
    }

    pub(super) fn checkout(&mut self, target: &str, ctx: &egui::Context) {
        if refuse_option_like(target, &mut self.status) {
            return;
        }
        let previous = self.checked_out();
        let undo = if previous.is_empty() {
            Undo::None
        } else {
            Undo::Custom {
                undo: ops(&[&["checkout", &previous, "--"]]),
                redo: ops(&[&["checkout", target, "--"]]),
            }
        };
        // `--`: a name that is no branch (any more) must not be taken for a file to discard
        // the changes of.
        self.run_undoable(
            &format!("Checkout {target}"),
            ops(&[&["checkout", target, "--"]]),
            Refresh::All,
            undo,
            ctx,
        );
    }

    /// Checkout and `pull --ff-only`; undoing goes back and puts the branch where it was.
    pub(super) fn checkout_pull(&mut self, branch: &str, ctx: &egui::Context) {
        let previous = self.checked_out();
        let Some(was) = self
            .refs
            .local
            .iter()
            .find(|b| b.name == branch)
            .map(|b| b.target.clone())
        else {
            return self.checkout(branch, ctx);
        };
        if refuse_option_like(branch, &mut self.status) {
            return;
        }
        let run = ops(&[&["checkout", branch, "--"], &["pull", "--ff-only"]]);
        let undo = checkout_pull_undo(&previous, branch, &was, &run);
        self.run_undoable(
            &format!("Checkout and pull {branch}"),
            run,
            Refresh::All,
            undo,
            ctx,
        );
    }

    pub(super) fn checkout_remote(&mut self, remote_branch: &str, ctx: &egui::Context) {
        // `origin/feature` → local `feature` tracking it (or just switch if it exists); `feat`
        // for `my/fork/feat` of the remote `my/fork`, `main` for `remotes/origin/main`.
        let remotes: Vec<&str> = self.remotes.iter().map(|r| r.name.as_str()).collect();
        let local = local_name_of(remote_branch, &remotes);
        // The name comes from the remote (a pull request's branch…).
        if refuse_option_like(remote_branch, &mut self.status)
            || refuse_option_like(&local, &mut self.status)
        {
            return;
        }
        if self.refs.local.iter().any(|b| b.name == local) {
            self.checkout(&local, ctx);
            return;
        }
        let undo = checkout_remote_undo(&self.checked_out(), &local, remote_branch);
        // Named here, so that it is the branch undo deletes, whatever git would make of it.
        self.run_undoable(
            &format!("Checkout {local}"),
            ops(&[&[
                "checkout",
                "-b",
                &local,
                "--track",
                &remote_start(remote_branch),
                "--",
            ]]),
            Refresh::All,
            undo,
            ctx,
        );
    }

    pub(super) fn default_remote(&self) -> Option<String> {
        self.remotes
            .iter()
            .map(|r| r.name.as_str())
            .find(|r| *r == "origin")
            .or_else(|| self.remotes.first().map(|r| r.name.as_str()))
            .map(str::to_owned)
    }

    /// The remote and the branch on it that the remote-tracking `upstream` (`origin/main`) is.
    pub(super) fn split_upstream<'a>(&self, upstream: &'a str) -> Option<(String, &'a str)> {
        let names: Vec<&str> = self.remotes.iter().map(|r| r.name.as_str()).collect();
        let remote = sidebar::remote_of(upstream, &names)?;
        Some((remote.to_owned(), sidebar::remote_short(upstream, remote)?))
    }

    /// Pushes `b`, checked out or not: to the branch it tracks, else publishes it on the
    /// default remote and tracks that. With where it goes: `origin/main`, or `origin`.
    pub(super) fn push_branch(&self, b: &Branch) -> Option<(String, Action)> {
        let tracked = if b.upstream_gone() {
            None
        } else {
            self.split_upstream(&b.upstream)
        };
        // Full refs: a tag of the same name must not be what is pushed.
        let (to, args) = match tracked {
            Some((remote, name)) => (
                b.upstream.clone(),
                vec![
                    "push".to_owned(),
                    remote,
                    format!("refs/heads/{}:refs/heads/{name}", b.name),
                ],
            ),
            None => {
                let remote = self.default_remote()?;
                (
                    remote.clone(),
                    vec![
                        "push".to_owned(),
                        "-u".to_owned(),
                        remote,
                        // No `:dst`, which would bypass `remote.<name>.push` (Gerrit's
                        // refs/for/*); the full source alone is not mistaken for a tag.
                        format!("refs/heads/{}", b.name),
                    ],
                )
            }
        };
        let action = Action::Ops {
            label: format!("Push {}", b.name),
            ops: vec![args],
            refresh: Refresh::All,
        };
        Some((to, action))
    }

    /// Makes `b` track nothing, undoably.
    pub(super) fn unset_upstream_action(&self, b: &Branch) -> Action {
        let unset = vec![
            "branch".to_owned(),
            "--unset-upstream".to_owned(),
            b.name.clone(),
        ];
        Action::UndoableOps {
            label: format!("Stop tracking {}", b.upstream),
            ops: vec![unset.clone()],
            refresh: Refresh::All,
            undo: Undo::Custom {
                undo: self.track_ops(&b.name, &b.upstream),
                redo: vec![unset],
            },
        }
    }

    /// Makes `branch` track `upstream` (`origin/main`) through its config, which works even
    /// when the remote branch is gone (`git branch -u` refuses then).
    pub(super) fn track_ops(&self, branch: &str, upstream: &str) -> Vec<Vec<String>> {
        let Some((remote, name)) = self.split_upstream(upstream) else {
            return Vec::new();
        };
        vec![
            vec![
                "config".to_owned(),
                format!("branch.{branch}.remote"),
                remote,
            ],
            vec![
                "config".to_owned(),
                format!("branch.{branch}.merge"),
                format!("refs/heads/{name}"),
            ],
        ]
    }

    /// The web page of the repository: the one of the remote the checked-out branch tracks
    /// (a fork's, not the repository it was forked from), else of the default remote.
    pub(super) fn web_page(&self) -> Option<String> {
        let tracked = self
            .refs
            .current
            .as_ref()
            .and_then(|c| self.refs.local.iter().find(|b| &b.name == c))
            .and_then(|b| self.split_upstream(&b.upstream))
            .map(|(remote, _)| remote);
        let remote = tracked.or_else(|| self.default_remote())?;
        let url = &self.remotes.iter().find(|r| r.name == remote)?.url;
        forge::web_url(url)
    }

    pub(super) fn push(&mut self, ctx: &egui::Context) {
        let Some(current) = self.refs.current.clone() else {
            self.status = Some(("Push needs a checked-out branch".to_owned(), true));
            return;
        };
        let has_upstream = self
            .refs
            .local
            .iter()
            .any(|b| b.name == current && !b.upstream.is_empty() && !b.upstream_gone());
        if has_upstream {
            self.run_ops(
                "Push",
                ops(&[&["push"]]),
                Refresh::All,
                OpKind::plain(),
                ctx,
            );
            return;
        }
        // First push of a branch: publish it and track the remote one.
        match self.default_remote() {
            Some(remote) => self.run_ops(
                &format!("Push {current} to {remote}"),
                // Full refs: a tag of the same name must not be what is pushed.
                ops(&[&[
                    "push",
                    "-u",
                    &remote,
                    // No `:dst`, which would bypass `remote.<name>.push`.
                    &format!("refs/heads/{current}"),
                ]]),
                Refresh::All,
                OpKind::plain(),
                ctx,
            ),
            None => {
                self.status = Some(("This repository has no remote to push to".to_owned(), true))
            }
        }
    }

    pub(super) fn commit(&mut self, ctx: &egui::Context) {
        if !self.work.conflicted.is_empty() {
            self.status = Some(("Resolve the conflicted files first".to_owned(), true));
            return;
        }
        // A rebase or patch stopped on conflicts finishes with `--continue`, not a new commit.
        if let Some(state) = self.work.in_progress.filter(|s| s.continues()) {
            let label = format!("Continue {}", state.label().to_lowercase());
            let steps = ops(&[&[state.command(), "--continue"]]);
            self.run_ops(&label, steps, Refresh::All, OpKind::plain(), ctx);
            return;
        }
        let summary = self.commit_summary.trim();
        if summary.is_empty() {
            self.status = Some(("Write a commit summary first".to_owned(), true));
            return;
        }
        let mut commit = vec!["commit".to_owned()];
        if self.amend {
            commit.push("--amend".to_owned());
        }
        commit.extend(["-m".to_owned(), summary.to_owned()]);
        let body = self.commit_body.trim();
        if !body.is_empty() {
            commit.extend(["-m".to_owned(), body.to_owned()]);
        }
        let mut steps = Vec::new();
        // Nothing staged: commit everything, like "Stage all and commit" (an amend with
        // nothing staged only rewrites the message).
        if self.work.staged.is_empty() && !self.amend {
            steps.push(vec!["add".to_owned(), "-A".to_owned()]);
        }
        steps.push(commit);
        let label = if self.amend { "Amend" } else { "Commit" };
        self.run_undoable(
            label,
            steps,
            Refresh::All,
            Undo::HeadMove { mode: "--soft" },
            ctx,
        );
    }

    pub(super) fn stash_push(&mut self, message: &str, ctx: &egui::Context) {
        let mut args: Vec<&str> = stash_push_args().to_vec();
        if !message.trim().is_empty() {
            args.extend(["-m", message.trim()]);
        }
        let redo = ops(&[&args]);
        self.run_undoable(
            "Stash",
            ops(&[&args]),
            Refresh::All,
            Undo::Custom {
                // This stash, not one pushed since.
                undo: [expect_ref("refs/stash")]
                    .into_iter()
                    .chain(ops(&[&["stash", "pop", "--index"]]))
                    .collect(),
                redo,
            },
            ctx,
        );
    }

    /// Publishes `branch` on the forge's remote, then opens a pull (or merge) request for it.
    pub(super) fn create_pr(
        &mut self,
        branch: String,
        base: String,
        title: String,
        body: String,
        draft: bool,
        ctx: &egui::Context,
    ) {
        let Some(forge) = self.forge else { return };
        if self.busy_op.is_some() {
            return;
        }
        // The remote the forge was found by.
        let Some(remote) = self.default_remote() else {
            return;
        };
        let label = format!("Create {}", forge.noun());
        self.busy_op = Some(label.clone());
        self.op_started = Some(Instant::now());
        // Signing in after this fails must not run some earlier operation again.
        self.last_op = None;
        let path = self.path.clone();
        let creds = self.credentials.clone();
        spawn(&self.tx, ctx, move || {
            // Publish the branch first, so the forge can see it.
            // No `:dst`, which would bypass `remote.<name>.push`.
            let refspec = format!("refs/heads/{branch}");
            let pushed = git::run_with(
                Some(&path),
                &["push", "-u", &remote, &refspec],
                creds.as_ref(),
            );
            let result = pushed
                .and_then(|_| forge::create(&path, forge, &branch, &base, &title, &body, draft));
            Msg::OpDone {
                label,
                result,
                refresh: Refresh::All,
                kind: OpKind::plain(),
                quiet: false,
            }
        });
    }
}

/// The local branch to check the remote-tracking branch `name` out as: its name on the remote
/// (`remotes`: the configured ones), or what follows the first `/` for a remote not configured.
pub(super) fn local_name_of(name: &str, remotes: &[&str]) -> String {
    sidebar::remote_of(name, remotes)
        .and_then(|r| sidebar::remote_short(name, r))
        .or_else(|| name.split_once('/').map(|(_, b)| b))
        .unwrap_or(name)
        .to_owned()
}

/// How checking out `remote_branch` as the new branch `local` from `previous` is undone: back to
/// `previous`, and the branch deleted. With `-d`, which keeps it if commits were made on it since
/// (elsewhere: undoing a commit made here comes first), as they are on no other branch.
pub(super) fn checkout_remote_undo(previous: &str, local: &str, remote_branch: &str) -> Undo {
    if previous.is_empty() {
        return Undo::None;
    }
    Undo::Custom {
        undo: ops(&[&["checkout", previous, "--"], &["branch", "-d", local]]),
        redo: ops(&[&[
            "checkout",
            "-b",
            local,
            "--track",
            &remote_start(remote_branch),
            "--",
        ]]),
    }
}

/// The full ref of the remote-tracking branch `name` (`origin/x`) to start a branch from: a
/// tag named `origin/x` would be what the short name means to git (or make it ambiguous).
pub(super) fn remote_start(name: &str) -> String {
    if name.starts_with("refs/") {
        name.to_owned()
    } else if name.starts_with("remotes/") {
        format!("refs/{name}")
    } else {
        BranchRef::remote(name).full()
    }
}

/// How checking out and pulling `branch` (which was at `was`) from `previous` is undone: back to
/// `previous`, and the branch put back, but not over commits made on it since the pull.
pub(super) fn checkout_pull_undo(
    previous: &str,
    branch: &str,
    was: &str,
    run: &[Vec<String>],
) -> Undo {
    if previous.is_empty() {
        return Undo::None;
    }
    Undo::Custom {
        undo: [expect_ref(&format!("refs/heads/{branch}"))]
            .into_iter()
            .chain(ops(&[
                &["checkout", previous, "--"],
                &["branch", "-f", branch, was],
            ]))
            .collect(),
        redo: run.to_vec(),
    }
}

/// Refuses (saying so in `status`) a branch or commit name git would read as an option: one
/// from a remote or a pull request can be anything a ref name may be.
pub(super) fn refuse_option_like(name: &str, status: &mut Option<(String, bool)>) -> bool {
    let bad = name.is_empty() || name.starts_with('-');
    if bad {
        *status = Some((
            format!("Can't check out \"{name}\": not a usable name"),
            true,
        ));
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::{Undo, checkout_remote_undo, remote_start};
    use crate::git;
    use crate::repo::OpKind;
    use crate::repo::operations::run_steps;

    #[test]
    fn remote_branches_start_from_their_full_ref() {
        assert_eq!(remote_start("origin/x"), "refs/remotes/origin/x");
        assert_eq!(remote_start("remotes/origin/x"), "refs/remotes/origin/x");
        assert_eq!(remote_start("refs/remotes/o/x"), "refs/remotes/o/x");
    }

    /// A tag named like the remote-tracking branch is no obstacle to checking it out.
    #[test]
    fn a_tag_named_like_the_remote_branch_does_not_shadow_it() {
        let base = std::env::temp_dir().join(format!("gitcraft-cmd-tag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::create_dir_all(&base);
        let (up, clone) = (base.join("up"), base.join("clone"));
        let up_s = up.to_string_lossy().into_owned();
        let clone_s = clone.to_string_lossy().into_owned();
        let setup: [(Option<&std::path::Path>, Vec<&str>); 7] = [
            (None, vec!["init", "-q", "-b", "main", &up_s]),
            (
                Some(&up),
                vec![
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "a",
                ],
            ),
            (Some(&up), vec!["branch", "feat"]),
            (None, vec!["clone", "-q", &up_s, &clone_s]),
            (Some(&clone), vec!["config", "user.name", "t"]),
            (Some(&clone), vec!["config", "user.email", "t@t"]),
            (Some(&clone), vec!["tag", "origin/feat", "HEAD"]),
        ];
        let set_up = setup.iter().all(|(dir, args)| git::run(*dir, args).is_ok());
        let redo = match checkout_remote_undo("main", "feat", "origin/feat") {
            Undo::Custom { redo, .. } => redo,
            _ => Vec::new(),
        };
        let (result, _) = run_steps(&clone, &redo, OpKind::plain(), None);
        let merge = git::run(Some(&clone), &["config", "branch.feat.merge"]);
        let _ = std::fs::remove_dir_all(&base);
        assert!(set_up);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            merge.map(|m| m.trim().to_owned()),
            Ok("refs/heads/feat".to_owned())
        );
    }
}
