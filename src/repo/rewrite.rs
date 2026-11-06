//! Rewriting history: editing a commit's message, and the interactive rebase.

use super::{Msg, Refresh, RepoTab, Undo, ops, rebase, short, spawn};
use crate::git;

impl RepoTab {
    /// The latest commit is amended (its message only, whatever is staged); an older one is
    /// reworded by an interactive rebase from its parent.
    pub(super) fn reword_commit(&mut self, hash: String, message: String, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        self.reword = None;
        if hash == self.refs.head {
            let (subject, body) = message.split_once("\n\n").unwrap_or((&message, ""));
            let mut amend = vec![
                "commit",
                "--amend",
                "--only",
                "--allow-empty",
                "-m",
                subject,
            ];
            if !body.is_empty() {
                amend.extend(["-m", body]);
            }
            self.run_undoable(
                &format!("Edit the message of {}", short(&hash)),
                ops(&[&amend]),
                Refresh::All,
                Undo::HeadMove { mode: "--keep" },
                ctx,
            );
            return;
        }
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let result = git::rebase_commits(&path, &format!("{hash}^"));
            Msg::RewordCommits {
                hash,
                message,
                result,
            }
        });
    }

    /// Rewords the older commit `hash` with a rebase of `result`, the commits after its parent.
    pub(super) fn reword_older(
        &mut self,
        hash: String,
        message: String,
        result: Result<Vec<git::RebaseCommit>, String>,
        ctx: &egui::Context,
    ) {
        let result = result.and_then(|commits| {
            let onto = format!("{hash}^");
            let mut editor = rebase::RebaseEditor::new(onto, short(&hash).to_owned(), commits);
            let entry = editor
                .entries
                .iter_mut()
                .find(|e| e.commit.hash == hash)
                .ok_or("only commits of the checked-out branch can be edited")?;
            entry.step = rebase::Step::Reword;
            entry.message = message;
            if let Some(problem) = editor.problem() {
                return Err(problem);
            }
            editor.command()
        });
        match result {
            // Started while something else ran: say so rather than drop it.
            Ok(_) if self.busy_op.is_some() => {
                self.status = Some((
                    format!(
                        "Edit message: {} is still running, try again when it's done",
                        self.busy_op.as_deref().unwrap_or_default()
                    ),
                    true,
                ));
            }
            Ok(command) => self.run_undoable(
                &format!("Edit the message of {}", short(&hash)),
                vec![command],
                Refresh::All,
                Undo::HeadMove { mode: "--keep" },
                ctx,
            ),
            Err(e) => self.status = Some((format!("Edit message: {e}"), true)),
        }
    }

    /// Reads the commits after `onto`, for the interactive rebase editor.
    pub(super) fn interactive_rebase(&mut self, onto: String, label: String, ctx: &egui::Context) {
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let result = git::rebase_commits(&path, &onto);
            Msg::RebaseCommits {
                onto,
                label,
                result,
            }
        });
    }

    /// Opens the interactive rebase editor on the commits `interactive_rebase` read.
    pub(super) fn open_rebase(
        &mut self,
        onto: String,
        label: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    ) {
        match result {
            Ok(commits) if commits.is_empty() => {
                self.status = Some((
                    format!(
                        "Nothing to rebase: {label} already contains every commit of this branch"
                    ),
                    true,
                ));
            }
            Ok(commits) => {
                self.diff = None;
                self.conflict = None;
                self.file_view = None;
                self.rebase = Some(rebase::RebaseEditor::new(onto, label, commits));
            }
            Err(e) => self.status = Some((format!("Interactive rebase: {e}"), true)),
        }
    }

    /// Runs the rebase the editor describes.
    pub(super) fn run_rebase(&mut self, ctx: &egui::Context) {
        let Some(editor) = &self.rebase else { return };
        if editor.problem().is_some() || self.busy_op.is_some() {
            return;
        }
        // Committed or pulled while the editor was open: its todo list would drop that.
        if let Some(why) = editor.moved(&self.refs.head) {
            self.status = Some((why, true));
            return;
        }
        match editor.command() {
            Ok(command) => {
                let label = format!("Interactive rebase onto {}", editor.onto_label);
                self.rebase = None;
                self.run_undoable(
                    &label,
                    vec![command],
                    Refresh::All,
                    Undo::HeadMove { mode: "--keep" },
                    ctx,
                );
            }
            Err(e) => self.status = Some((e, true)),
        }
    }
}
