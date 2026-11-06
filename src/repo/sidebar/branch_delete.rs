//! Deleting a local branch, alone or with the branch it tracks on the remote.

use egui::Ui;

use crate::git::Branch;
use crate::repo::operations::expect_ref;
use crate::repo::{Action, Dialog, RepoTab, Undo, ops};

impl RepoTab {
    /// Delete…, and Delete locally and on the remote… when `b` tracks a branch that is there.
    pub(super) fn delete_items(&self, ui: &mut Ui, acts: &mut Vec<Action>, b: &Branch, idle: bool) {
        let restore = ops(&[&["branch", "--", &b.name, &b.target]]);
        // git refuses to delete a branch checked out in another worktree: with the remote's
        // deleted first (-D), that would leave its commits on no ref and nothing to undo.
        let elsewhere = self
            .worktrees
            .iter()
            .any(|w| !w.current && w.branch.as_deref() == Some(b.name.as_str()));
        let why = if elsewhere {
            "Checked out in another worktree"
        } else {
            "Can't delete the checked-out branch"
        };
        let can = !b.is_head && !elsewhere && idle;
        if ui
            .add_enabled(can, egui::Button::new("Delete…"))
            .on_disabled_hover_text(why)
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
                    redo: redo_delete(&b.name, ops(&[&["branch", "-D", "--", &b.name]])),
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
                can,
                egui::Button::new(format!("Delete locally and on {remote}…")),
            )
            .on_hover_text(format!("Also delete {} from {remote}", b.upstream))
            .on_disabled_hover_text(why)
            .clicked()
        {
            let remote_ref = format!("refs/heads/{name}");
            let delete = |flag: &str| delete_both(flag, &b.name, &remote, name, &tracked.target);
            // Undo pushes the remote branch back where it was and tracks it again.
            let undo = delete_both_undo(
                restore,
                &remote,
                &format!("{}:{remote_ref}", tracked.target),
                self.track_ops(&b.name, &b.upstream),
            );
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
                    redo: redo_delete(&b.name, delete("-D")),
                }),
            ));
        }
    }
}

/// Redoing the delete of `branch` with `steps` (`-D`): only while the branch is where undoing put
/// it back. Commits it got since (in the terminal) are on no other branch, and `-D` would drop
/// them along with the branch's reflog.
fn redo_delete(branch: &str, steps: Vec<Vec<String>>) -> Vec<Vec<String>> {
    [expect_ref(&format!("refs/heads/{branch}"))]
        .into_iter()
        .chain(steps)
        .collect()
}

/// How deleting a branch here and on `remote` is undone: `refspec` (`<commit>:refs/heads/x`)
/// pushed back first, then the branch made again with `restore` and set to track it with
/// `track`. The push is what can fail (offline, signed out): first, it leaves nothing done, and
/// undoing again later works. After the branch here, it would leave that made, and undoing again
/// would stop at making it again, never pushing back the remote one.
fn delete_both_undo(
    restore: Vec<Vec<String>>,
    remote: &str,
    refspec: &str,
    track: Vec<Vec<String>>,
) -> Vec<Vec<String>> {
    [ops(&[&["push", remote, refspec]]), restore, track].concat()
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

    /// Undoing a delete here and on the remote that fails (the remote out of reach) can be done
    /// again later: it does not stop at making the branch here again.
    #[test]
    fn undoing_a_delete_on_the_remote_can_be_tried_again() {
        let base =
            std::env::temp_dir().join(format!("gitcraft-undo-delete-both-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (dir, bare, away) = (
            base.join("r"),
            base.join("remote.git"),
            base.join("away.git"),
        );
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
        let at = git::head(&dir);
        let undo = delete_both_undo(
            ops(&[&["branch", "--", "feat", &at]]),
            "origin",
            &format!("{at}:refs/heads/feat"),
            Vec::new(),
        );
        let deleted = run_all(&dir, &delete_both("-D", "feat", "origin", "feat", &at));
        // Out of reach: the undo fails…
        let moved = std::fs::rename(&bare, &away);
        let failed = !run_all(&dir, &undo);
        let back = std::fs::rename(&away, &bare);
        // …and goes through once the remote is back.
        let undone = run_all(&dir, &undo);
        let local = git::resolve(&dir, "refs/heads/feat").ok();
        let remote = git::run(Some(&bare), &["rev-parse", "refs/heads/feat"]).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&base);
        assert!(deleted && moved.is_ok() && failed && back.is_ok());
        assert!(undone);
        assert_eq!(local.as_deref(), Some(at.as_str()));
        assert_eq!(remote.trim(), at);
    }

    /// Redoing a branch's delete, once undone, leaves it alone after it got a commit since:
    /// `-D` would throw that away.
    #[test]
    fn redoing_a_delete_keeps_commits_made_on_the_branch_since() {
        use crate::repo::operations::run_steps;
        use crate::repo::{OpKind, UndoEntry};
        let dir = std::env::temp_dir().join(format!("gitcraft-redo-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
            &["branch", "feat"],
        ] {
            assert!(git(args).is_ok(), "{args:?}");
        }
        let at = git::head(&dir);
        let entry = UndoEntry {
            label: "Delete branch".to_owned(),
            undo: ops(&[&["branch", "--", "feat", &at]]),
            redo: redo_delete("feat", ops(&[&["branch", "-D", "--", "feat"]])),
            heads: None,
        };
        let deleted = run_all(&dir, &ops(&[&["branch", "-D", "--", "feat"]]));
        let (undone, kind) = run_steps(&dir, &entry.undo, OpKind::Undo(entry.clone()), None);
        // The redo, expecting the branch where undoing put it.
        let redo = match kind {
            OpKind::Undo(e) => e.redo,
            _ => Vec::new(),
        };
        // A commit on it, made elsewhere.
        let mine =
            git(&["commit-tree", "-p", "feat", "-m", "mine", "HEAD^{tree}"]).unwrap_or_default();
        let moved = git(&["update-ref", "refs/heads/feat", mine.trim()]);
        let (redone, _) = run_steps(&dir, &redo, OpKind::Redo(entry.clone()), None);
        let kept = git::resolve(&dir, "refs/heads/feat").ok();
        // Back where undo left it, redo deletes it.
        let back = git(&["update-ref", "refs/heads/feat", &at]);
        let (again, _) = run_steps(&dir, &redo, OpKind::Redo(entry), None);
        let gone = git::resolve(&dir, "refs/heads/feat").is_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(deleted && undone.is_ok() && moved.is_ok() && back.is_ok());
        assert!(!redo.is_empty());
        assert!(redone.is_err());
        assert_eq!(kept.as_deref(), Some(mine.trim()));
        assert!(again.is_ok() && gone);
    }
}
