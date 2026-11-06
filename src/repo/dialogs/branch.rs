//! Dialogs that create or rename a branch, create a tag, or stash the changes.

use egui::Ui;

use crate::repo::operations::expect_ref_at;
use crate::repo::{Action, Refresh, Undo};
use crate::theme::Tokens;

use super::{Outcome, buttons, dash_spaces, field, title, usable};

/// A new branch at `at`, checked out or not. `previous` is what its undo checks out again.
#[allow(clippy::too_many_arguments)]
pub(super) fn create_branch(
    ui: &mut Ui,
    enter: bool,
    previous: &str,
    at: &str,
    at_label: &str,
    name: &mut String,
    checkout: &mut bool,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    title(ui, "Create branch", &format!("Starting at {at_label}"), &t);
    field(ui, name, "feature/my-branch", focus);
    dash_spaces(name);
    ui.add_space(6.0);
    ui.checkbox(checkout, "Check it out");
    ui.add_space(12.0);
    let valid = usable(name);
    buttons(ui, "Create", valid, enter, || {
        let name = name.trim().to_owned();
        let args: Vec<String> = if *checkout {
            vec!["switch".into(), "-c".into(), name.clone(), at.to_owned()]
        } else {
            vec!["branch".into(), name.clone(), at.to_owned()]
        };
        Action::UndoableOps {
            label: format!("Create branch {name}"),
            ops: vec![args.clone()],
            refresh: Refresh::All,
            undo: Undo::Custom {
                undo: create_branch_undo(&name, at, *checkout, previous),
                redo: vec![args],
            },
        }
    })
}

/// How creating the branch `name` at the commit `at` (and checking it out, from `previous`) is
/// undone: back to `previous`, and the branch deleted while it is still at `at`. Commits made on
/// it since (in the terminal: undoing one made here comes first) are on no other branch, and
/// `branch -D` would throw them away.
fn create_branch_undo(name: &str, at: &str, checkout: bool, previous: &str) -> Vec<Vec<String>> {
    // Checked first, so that a refused undo hasn't switched the checkout already.
    let mut undo = vec![expect_ref_at(&format!("refs/heads/{name}"), at)];
    if checkout && !previous.is_empty() {
        // `--`: `previous` is a branch or a commit, never a file of that name to check out.
        undo.push(vec![
            "checkout".to_owned(),
            previous.to_owned(),
            "--".to_owned(),
        ]);
    }
    undo.push(vec![
        "update-ref".to_owned(),
        "-d".to_owned(),
        format!("refs/heads/{name}"),
        at.to_owned(),
    ]);
    undo
}

pub(super) fn stash(ui: &mut Ui, enter: bool, message: &mut String, focus: &mut bool) -> Outcome {
    let t = Tokens::get(ui.ctx());
    title(
        ui,
        "Stash changes",
        "Puts every change, untracked files included, aside and cleans the working tree.",
        &t,
    );
    field(ui, message, "Message (optional)", focus);
    buttons(ui, "Stash", true, enter, || {
        Action::StashPush(message.trim().to_owned())
    })
}

/// A tag at `at`, annotated when it has a message, and pushed to `remote` when asked.
#[allow(clippy::too_many_arguments)]
pub(super) fn tag(
    ui: &mut Ui,
    enter: bool,
    at: &str,
    at_label: &str,
    name: &mut String,
    message: &mut String,
    remote: &Option<String>,
    push: &mut bool,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    title(ui, "Create tag", &format!("At {at_label}"), &t);
    field(ui, name, "v1.2.0", focus);
    dash_spaces(name);
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::multiline(message)
            .hint_text("Message (optional: makes an annotated tag)")
            .desired_width(f32::INFINITY)
            .desired_rows(2)
            .margin(egui::Margin::symmetric(8, 6)),
    );
    if let Some(remote) = remote {
        ui.add_space(4.0);
        ui.checkbox(push, format!("Push it to {remote}"));
    }
    ui.add_space(8.0);
    let valid = usable(name);
    buttons(
        ui,
        "Create",
        valid,
        enter && !message.contains('\n'),
        || {
            let name = name.trim().to_owned();
            let mut create = vec!["tag".to_owned()];
            if !message.trim().is_empty() {
                create.extend(["-a".to_owned(), "-m".to_owned(), message.trim().to_owned()]);
            }
            create.extend([name.clone(), at.to_owned()]);
            let mut steps = vec![create.clone()];
            if *push && let Some(remote) = remote {
                steps.push(vec![
                    "push".to_owned(),
                    remote.clone(),
                    format!("refs/tags/{name}"),
                ]);
            }
            Action::UndoableOps {
                label: format!("Create tag {name}"),
                ops: steps,
                refresh: Refresh::All,
                undo: Undo::Custom {
                    undo: vec![vec!["tag".to_owned(), "-d".to_owned(), name.clone()]],
                    redo: vec![create],
                },
            }
        },
    )
}

pub(super) fn rename_branch(
    ui: &mut Ui,
    enter: bool,
    old: &str,
    new: &mut String,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    title(
        ui,
        "Rename branch",
        &format!("Renames {old} on this computer; its remote copy keeps its name."),
        &t,
    );
    field(ui, new, "new-name", focus);
    dash_spaces(new);
    ui.add_space(8.0);
    let valid = usable(new) && new.trim() != old;
    buttons(ui, "Rename", valid, enter, || {
        let new = new.trim().to_owned();
        let old = old.to_owned();
        Action::UndoableOps {
            label: format!("Rename {old} to {new}"),
            ops: vec![vec!["branch".into(), "-m".into(), old.clone(), new.clone()]],
            refresh: Refresh::All,
            undo: Undo::Custom {
                undo: vec![vec!["branch".into(), "-m".into(), new.clone(), old.clone()]],
                redo: vec![vec!["branch".into(), "-m".into(), old, new]],
            },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    /// Undoing a branch's creation deletes it while it is where it was made, and not once
    /// commits were made on it since.
    #[test]
    fn undoing_a_branch_keeps_commits_made_on_it() {
        let dir = std::env::temp_dir().join(format!("gitcraft-branch-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
        ] {
            assert!(git(args).is_ok());
        }
        let at = git::head(&dir);
        let undo_all = |undo: &[Vec<String>]| {
            crate::repo::operations::run_steps(&dir, undo, crate::repo::OpKind::plain(), None)
                .0
                .is_ok()
        };
        let exists = || git(&["rev-parse", "--verify", "-q", "refs/heads/feat"]).is_ok();
        // Untouched since: undone.
        assert!(git(&["switch", "-q", "-c", "feat", &at]).is_ok());
        let undone = undo_all(&create_branch_undo("feat", &at, true, "main"));
        let gone = !exists();
        // A commit on it since (in the terminal): it stays, with the commit.
        assert!(git(&["switch", "-q", "-c", "feat", &at]).is_ok());
        assert!(git(&["commit", "-q", "--allow-empty", "-m", "work"]).is_ok());
        let refused = !undo_all(&create_branch_undo("feat", &at, true, "main"));
        let kept = exists();
        // Refused before anything ran: still on the branch.
        let on_feat = git(&["symbolic-ref", "--short", "HEAD"]).map(|h| h.trim().to_owned());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(undone && gone);
        assert!(refused && kept);
        assert_eq!(on_feat.as_deref(), Ok("feat"));
    }
}
