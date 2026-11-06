//! Rewriting history: editing a commit's message, and the interactive rebase.

use super::operations::expect_ref_at;
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
            // HEAD as the last read saw it: a commit made since (in the terminal) is not the
            // one whose message this is, and must not get it.
            let guarded = [expect_ref_at("HEAD", &hash)]
                .into_iter()
                .chain(ops(&[&amend]))
                .collect();
            self.run_undoable(
                &format!("Edit the message of {}", short(&hash)),
                guarded,
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
            Ok((editor.command()?, editor.head))
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
            // The todo list replaces the commits read: not one made since.
            Ok((command, head)) => self.run_undoable(
                &format!("Edit the message of {}", short(&hash)),
                vec![expect_ref_at("HEAD", &head), command],
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
        // Committed or pulled while the editor was open: its todo list would drop that. Checked
        // here against the last read, and again on disk right before it runs (that read can
        // lag a commit just made in the terminal by as long as a reload takes).
        if let Some(why) = editor.moved(&self.refs.head) {
            self.status = Some((why, true));
            return;
        }
        match editor.command() {
            Ok(command) => {
                let label = format!("Interactive rebase onto {}", editor.onto_label);
                let guard = expect_ref_at("HEAD", &editor.head);
                self.rebase = None;
                self.run_undoable(
                    &label,
                    vec![guard, command],
                    Refresh::All,
                    Undo::HeadMove { mode: "--keep" },
                    ctx,
                );
            }
            Err(e) => self.status = Some((e, true)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::super::{Msg, RepoTab, rebase};
    use crate::git;

    fn repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gitcraft-rewrite-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "root"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
            &["commit", "-q", "--allow-empty", "-m", "read"],
        ] {
            assert!(git::run(Some(&dir), args).is_ok());
        }
        dir
    }

    /// Waits for the operation the tab started, and says how it ended.
    fn op_result(tab: &RepoTab) -> Option<Result<String, String>> {
        // Past the watcher, which comes on the same channel.
        loop {
            match tab.rx.recv_timeout(Duration::from_secs(30)) {
                Ok(Msg::OpDone { result, .. }) => return Some(result),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }

    fn subject(dir: &Path) -> String {
        git::head_message(dir)
            .map(|m| m.trim().to_owned())
            .unwrap_or_default()
    }

    /// A commit made in the terminal after the editor read the branch, and before a reload
    /// told the tab: the rebase refuses rather than drop it.
    #[test]
    fn a_rebase_does_not_drop_a_commit_the_tab_has_not_seen_yet() {
        let dir = repo("rebase-unseen");
        let ctx = egui::Context::default();
        let mut tab = RepoTab::new_deferred(dir.clone(), &ctx);
        let read = git::head(&dir);
        tab.refs.head = read.clone();
        let commits = git::rebase_commits(&dir, "HEAD~1").unwrap_or_default();
        tab.rebase = Some(rebase::RebaseEditor::new(
            "HEAD~1".to_owned(),
            "HEAD~1".to_owned(),
            commits,
        ));
        let made = git::run(
            Some(&dir),
            &["commit", "-q", "--allow-empty", "-m", "unseen"],
        );
        let unseen = git::head(&dir);
        tab.run_rebase(&ctx);
        let result = op_result(&tab);
        let head = git::head(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(made.is_ok());
        assert!(matches!(result, Some(Err(_))), "{result:?}");
        assert_eq!(head, unseen);
    }

    /// Editing the message of what the tab thinks is the latest commit, after another was made
    /// on top: that one keeps its own message.
    #[test]
    fn rewording_the_last_commit_seen_leaves_a_newer_one_alone() {
        let dir = repo("reword-unseen");
        let ctx = egui::Context::default();
        let mut tab = RepoTab::new_deferred(dir.clone(), &ctx);
        let read = git::head(&dir);
        tab.refs.head = read.clone();
        let made = git::run(
            Some(&dir),
            &["commit", "-q", "--allow-empty", "-m", "unseen"],
        );
        tab.reword_commit(read, "reworded".to_owned(), &ctx);
        let result = op_result(&tab);
        let now = subject(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(made.is_ok());
        assert!(matches!(result, Some(Err(_))), "{result:?}");
        assert_eq!(now, "unseen");
    }

    /// An older commit's message: the commits are read right before, and the rebase replays
    /// them only if HEAD is still where that read saw it.
    #[test]
    fn rewording_an_older_commit_refuses_once_head_moved() {
        let dir = repo("reword-older");
        let ctx = egui::Context::default();
        let mut tab = RepoTab::new_deferred(dir.clone(), &ctx);
        let base = git::resolve(&dir, "HEAD~1").unwrap_or_default();
        let commits = git::rebase_commits(&dir, &format!("{base}^"));
        let made = git::run(
            Some(&dir),
            &["commit", "-q", "--allow-empty", "-m", "unseen"],
        );
        let unseen = git::head(&dir);
        tab.reword_older(base, "reworded".to_owned(), commits, &ctx);
        let result = op_result(&tab);
        let head = git::head(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(made.is_ok());
        assert!(matches!(result, Some(Err(_))), "{result:?}");
        assert_eq!(head, unseen);
    }
}
