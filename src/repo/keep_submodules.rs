//! Updating the submodules, on request or, with "Keep submodules up to date", after each
//! checkout, pull, merge or rebase.

use super::{Action, OpKind, Refresh, RepoTab, ops};
use crate::settings::Settings;

/// `git submodule update --init --recursive`: each submodule cloned if it isn't and checked
/// out at the commit recorded for it, theirs too.
const UPDATE: [&str; 4] = ["submodule", "update", "--init", "--recursive"];

/// Updates all submodules.
pub(super) fn update_all() -> Action {
    Action::Ops {
        label: "Update submodules".into(),
        ops: ops(&[&UPDATE]),
        refresh: Refresh::All,
    }
}

/// Updates the submodule at `path`.
pub(super) fn update_one(path: &str) -> Action {
    let mut args = UPDATE.to_vec();
    args.extend(["--", path]);
    Action::Ops {
        label: format!("Update submodule {path}"),
        ops: ops(&[&args]),
        refresh: Refresh::All,
    }
}

impl RepoTab {
    /// Notes, as an operation starts, whether it moves the checkout and the setting asks for
    /// the submodules to follow.
    pub(super) fn note_checkout_move(&mut self, ops: &[Vec<String>], ctx: &egui::Context) {
        self.update_submodules = Settings::get(ctx).keep_submodules && moves_checkout(ops);
    }

    /// After an operation that moved the checkout, updates the submodules, when there are
    /// any. `ok` is whether the operation went through.
    pub(super) fn follow_checkout_move(&mut self, ok: bool, ctx: &egui::Context) {
        if std::mem::take(&mut self.update_submodules)
            && ok
            && self.path.join(".gitmodules").is_file()
        {
            self.run_ops(
                "Update submodules",
                ops(&[&UPDATE]),
                Refresh::All,
                OpKind::plain(),
                ctx,
            );
        }
    }
}

/// Whether one of `ops` checks out another commit: a checkout (not of files), switch, pull,
/// merge or rebase, also when continued or aborted.
pub(super) fn moves_checkout(ops: &[Vec<String>]) -> bool {
    ops.iter().any(|op| {
        let mut args = op.iter().map(String::as_str);
        // Past `-c key=value` before the command.
        let command = loop {
            match args.next() {
                Some("-c") => {
                    args.next();
                }
                other => break other,
            }
        };
        match command {
            Some("switch" | "pull" | "merge" | "rebase") => true,
            // `checkout <ref> --` moves; `checkout <side> -- <file>` only writes the file.
            Some("checkout") => args.skip_while(|a| *a != "--").nth(1).is_none(),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::ops;

    #[test]
    fn what_moves_the_checkout() {
        for moves in [
            ops(&[&["checkout", "main", "--"]]),
            ops(&[&["checkout", "-b", "x", "--track", "origin/x", "--"]]),
            ops(&[&["stash", "push"], &["checkout", "main"]]),
            ops(&[&["switch", "-c", "x", "HEAD"]]),
            ops(&[&["pull", "--ff-only"]]),
            ops(&[&["merge", "--no-edit", "feature"]]),
            ops(&[&["merge", "--continue"]]),
            ops(&[&["-c", "sequence.editor=cp todo", "rebase", "-i", "main"]]),
        ] {
            assert!(moves_checkout(&moves), "{moves:?}");
        }
        for stays in [
            ops(&[&["checkout", "--theirs", "--", "a.txt"]]),
            ops(&[&["fetch", "--all"]]),
            ops(&[&["commit", "-m", "merge"]]),
            ops(&[&["-c", "core.editor=true", "commit"]]),
            ops(&[&["submodule", "update", "--init", "--recursive"]]),
        ] {
            assert!(!moves_checkout(&stays), "{stays:?}");
        }
    }
}
