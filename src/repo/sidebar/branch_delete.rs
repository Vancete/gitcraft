//! Deleting a local branch, alone or with the branch it tracks on the remote.

use egui::Ui;

use crate::git::Branch;
use crate::repo::{Action, Dialog, RepoTab, Undo, ops};

impl RepoTab {
    /// Delete…, and Delete locally and on the remote… when `b` tracks a branch that is there.
    pub(super) fn delete_items(&self, ui: &mut Ui, acts: &mut Vec<Action>, b: &Branch, idle: bool) {
        let restore = ops(&[&["branch", "--", &b.name, &b.target]]);
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
                    undo: restore.clone(),
                    redo: ops(&[&["branch", "-D", "--", &b.name]]),
                }),
            ));
        }
        // Where the upstream is, on the remote and here.
        let upstream = (!b.upstream_gone())
            .then(|| self.split_upstream(&b.upstream))
            .flatten()
            .zip(
                self.refs
                    .remotes
                    .iter()
                    .flat_map(|(_, bs)| bs)
                    .find(|r| r.name == b.upstream),
            );
        let Some(((remote, name), tracked)) = upstream else {
            return;
        };
        if ui
            .add_enabled(
                !b.is_head && idle,
                egui::Button::new(format!("Delete locally and on {remote}…")),
            )
            .on_hover_text(format!("Also delete {} from {remote}", b.upstream))
            .on_disabled_hover_text("Can't delete the checked-out branch")
            .clicked()
        {
            let remote_ref = format!("refs/heads/{name}");
            let delete = |flag: &str| delete_both(flag, &b.name, &remote, name, &tracked.target);
            // Undo pushes the remote branch back where it was and tracks it again.
            let mut undo = restore;
            undo.extend(ops(&[&[
                "push",
                &remote,
                &format!("{}:{remote_ref}", tracked.target),
            ]]));
            undo.extend(self.track_ops(&b.name, &b.upstream));
            acts.push(Action::Dialog(
                Dialog::confirm(
                    "Delete branch locally and on the remote",
                    format!(
                        "Delete the local branch “{}” and “{name}” on {remote}? Deleting it \
                         on the remote changes it for everyone.",
                        b.name
                    ),
                    "Delete both",
                    delete("-d"),
                    Some((
                        "Force, even if it isn't merged (-D)".to_owned(),
                        delete("-D"),
                    )),
                )
                .undo(Undo::Custom {
                    undo,
                    redo: delete("-D"),
                }),
            ));
        }
    }
}

/// Deleting the branch `branch` (with `flag`, `-d` or `-D`) and `name` on `remote`, which was at
/// `expected` when last fetched.
fn delete_both(
    flag: &str,
    branch: &str,
    remote: &str,
    name: &str,
    expected: &str,
) -> Vec<Vec<String>> {
    let remote_ref = format!("refs/heads/{name}");
    // Only while the remote branch is where it was last fetched: commits pushed to it since (by
    // someone else) are not this branch's to throw away.
    let lease = format!("--force-with-lease={remote_ref}:{expected}");
    let here = ops(&[&["branch", flag, "--", branch]]);
    let there = ops(&[&["push", &lease, remote, "--delete", &remote_ref]]);
    // `-d` refuses before anything is gone, so it goes first. `-D` doesn't: the push, which can
    // be refused, goes first, or a refusal would leave the branch's unmerged commits deleted
    // here, with nothing to undo (a failed operation has no undo).
    if flag == "-d" {
        [here, there].concat()
    } else {
        [there, here].concat()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::git;

    fn run_all(dir: &Path, steps: &[Vec<String>]) -> bool {
        steps.iter().all(|op| {
            let args: Vec<&str> = op.iter().map(String::as_str).collect();
            git::run(Some(dir), &args).is_ok()
        })
    }

    /// Deleting a branch here and on the remote leaves both alone once someone pushed to the
    /// remote one since the last fetch: their commits are not ours to delete.
    #[test]
    fn remote_branch_pushed_to_since_is_not_deleted() {
        let base =
            std::env::temp_dir().join(format!("gitcraft-delete-both-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (dir, bare) = (base.join("r"), base.join("remote.git"));
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        assert!(git::run(None, &["init", "-q", "--bare", &bare.to_string_lossy()]).is_ok());
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
            &["remote", "add", "origin", &bare.to_string_lossy()],
            &["branch", "feat"],
            &["push", "-q", "origin", "feat"],
        ] {
            assert!(git(args).is_ok(), "{args:?}");
        }
        let seen = git(&["rev-parse", "refs/remotes/origin/feat"]).unwrap_or_default();
        // Someone else's commit on the remote branch, not fetched here.
        let theirs =
            git(&["commit-tree", "-p", "HEAD", "-m", "theirs", "HEAD^{tree}"]).unwrap_or_default();
        let to = format!("{}:refs/heads/feat", theirs.trim());
        assert!(git(&["push", "-q", &bare.to_string_lossy(), &to]).is_ok());
        let local = || git(&["rev-parse", "--verify", "-q", "refs/heads/feat"]).is_ok();
        // Forced: the push goes first, and is refused, so the branch here stays too.
        let forced = run_all(
            &dir,
            &delete_both("-D", "feat", "origin", "feat", seen.trim()),
        );
        let kept_forced = local();
        // Not forced: the branch here, merged, goes; the remote one stays.
        let plain = run_all(
            &dir,
            &delete_both("-d", "feat", "origin", "feat", seen.trim()),
        );
        let kept_plain = local();
        let remote = git::run(Some(&bare), &["rev-parse", "refs/heads/feat"]).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&base);
        assert!(!forced && kept_forced);
        assert!(!plain && !kept_plain);
        assert_eq!(remote.trim(), theirs.trim());
    }
}
