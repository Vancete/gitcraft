//! Editing a commit of the checked-out branch from its menu: its message, dropping it, moving it
//! up or down. Each is a rebase from below the commit with its todo list written here, without
//! the interactive rebase editor; undo puts the branch back.

use std::collections::HashMap;

use egui::Ui;

use super::rebase::{RebaseEditor, Step};
use super::{Action, Msg, Refresh, RepoTab, Undo, short, spawn};
use crate::git::{self, Commit, RebaseCommit};

/// A change to one commit made from its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Message,
    Drop,
    /// Towards the newer commits, as the graph has them on top.
    MoveUp,
    MoveDown,
}

impl Edit {
    const ALL: [Edit; 4] = [Edit::Message, Edit::Drop, Edit::MoveUp, Edit::MoveDown];

    fn label(self) -> &'static str {
        match self {
            Edit::Message => "Edit commit message",
            Edit::Drop => "Drop commit",
            Edit::MoveUp => "Move commit up",
            Edit::MoveDown => "Move commit down",
        }
    }

    fn explain(self) -> &'static str {
        match self {
            Edit::Message => "Edit the message in the commit's details",
            Edit::Drop => "Remove the commit and its changes from the branch",
            Edit::MoveUp => "Swap it with the commit after it",
            Edit::MoveDown => "Swap it with the commit before it",
        }
    }

    /// What the rebase replays the commits onto: below the commit, or below its parent too when
    /// it moves down past it.
    fn base(self, hash: &str) -> String {
        match self {
            Edit::MoveDown => format!("{hash}~2"),
            _ => format!("{hash}^"),
        }
    }
}

/// The checked-out branch from HEAD down its first parents, newest first, as far as `commits`
/// are loaded.
fn first_parents<'a>(
    commits: &'a [Commit],
    index: &HashMap<String, usize>,
    head: &str,
) -> Vec<&'a Commit> {
    let mut line = Vec::new();
    let mut next = Some(head);
    while let Some(c) = next
        .and_then(|h| index.get(h))
        .and_then(|&i| commits.get(i))
    {
        line.push(c);
        next = c.parents.first().map(String::as_str);
    }
    line
}

/// Why `edit` can't be done to `hash`, if it can't. `line`: the checked-out branch from HEAD
/// down its first parents.
fn refusal(edit: Edit, line: &[&Commit], hash: &str) -> Option<&'static str> {
    let Some(pos) = line.iter().position(|c| c.hash == hash) else {
        return Some("Only commits of the checked-out branch can be edited here");
    };
    let commit = line[pos];
    // The latest commit's message is amended, which works for any commit.
    if edit == Edit::Message && pos == 0 {
        return None;
    }
    if line[..pos].iter().any(|c| c.parents.len() > 1) {
        return Some("Merges came after this commit: rewriting it would flatten them");
    }
    if commit.parents.len() > 1 {
        return Some("Merge commits can't be rewritten here");
    }
    if commit.parents.is_empty() {
        return Some("The first commit has no parent to rebase from");
    }
    match edit {
        Edit::MoveUp if pos == 0 => Some("It is the latest commit already"),
        Edit::MoveDown => match line.get(pos + 1) {
            None => Some("The commit below it isn't loaded"),
            Some(below) if below.parents.len() > 1 => Some("The commit below it is a merge"),
            Some(below) if below.parents.is_empty() => {
                Some("The commit below it is the first one, with nothing to rebase onto")
            }
            Some(_) => None,
        },
        _ => None,
    }
}

/// The rebase that does `edit` to `hash`, from the commits after `edit.base(hash)` (oldest first,
/// as git lists them).
fn plan(edit: Edit, hash: &str, commits: Vec<RebaseCommit>) -> Result<RebaseEditor, String> {
    if commits.iter().any(|c| c.merge) {
        return Err("merges came after the commit, which this would flatten".to_owned());
    }
    let mut editor = RebaseEditor::new(edit.base(hash), short(hash).to_owned(), commits);
    let i = editor
        .entries
        .iter()
        .position(|e| e.commit.hash == hash)
        .ok_or("only commits of the checked-out branch can be edited")?;
    match edit {
        Edit::Message => return Err("the message is edited in the commit's details".to_owned()),
        Edit::Drop => editor.entries[i].step = Step::Drop,
        Edit::MoveUp if i + 1 < editor.entries.len() => editor.entries.swap(i, i + 1),
        Edit::MoveDown if i > 0 => editor.entries.swap(i - 1, i),
        Edit::MoveUp => return Err("it is the latest commit already".to_owned()),
        Edit::MoveDown => return Err("there is no commit below it to swap with".to_owned()),
    }
    Ok(editor)
}

impl RepoTab {
    /// Edit message, drop, move up and down for the commit `hash`, each disabled with the
    /// reason when it can't be done.
    pub(super) fn quick_edit_items(
        &self,
        ui: &mut Ui,
        hash: &str,
        idle: bool,
        acts: &mut Vec<Action>,
    ) {
        let line = first_parents(&self.commits, &self.index, &self.refs.head);
        let blocked = if self.refs.current.is_none() {
            Some("HEAD is detached: check out a branch first".to_owned())
        } else {
            self.work
                .in_progress
                .map(|s| format!("A {} is in progress", s.label().to_lowercase()))
        };
        for edit in Edit::ALL {
            let refused = blocked
                .clone()
                .or_else(|| refusal(edit, &line, hash).map(str::to_owned));
            let button = ui
                .add_enabled(idle && refused.is_none(), egui::Button::new(edit.label()))
                .on_hover_text(edit.explain());
            let button = match &refused {
                Some(why) => button.on_disabled_hover_text(why),
                None => button,
            };
            if button.clicked() {
                acts.push(match edit {
                    Edit::Message => Action::EditMessage(hash.to_owned()),
                    _ => Action::QuickEdit {
                        edit,
                        hash: hash.to_owned(),
                    },
                });
            }
        }
    }

    /// Opens the editor of the message of `hash` in the detail pane, once its detail is in.
    pub(super) fn edit_message(&mut self, hash: String, ctx: &egui::Context) {
        if let Some(&row) = self.index.get(&hash) {
            self.select(row, ctx);
        }
        let mut settings = crate::settings::Settings::get(ctx);
        if !settings.show_detail {
            settings.show_detail = true;
            settings.set(ctx);
        }
        self.reword = Some(super::Reword {
            hash,
            subject: String::new(),
            body: String::new(),
            focus: true,
            fill: true,
        });
        self.fill_reword();
    }

    /// Puts the message in an editor opened from the menu, when the detail it comes from is in.
    pub(super) fn fill_reword(&mut self) {
        let (Some(reword), Some(detail)) = (&mut self.reword, &self.detail) else {
            return;
        };
        if reword.fill && reword.hash == detail.hash {
            let (subject, body) = detail
                .message
                .split_once('\n')
                .map(|(s, b)| (s, b.trim()))
                .unwrap_or((detail.message.as_str(), ""));
            reword.subject = subject.to_owned();
            reword.body = body.to_owned();
            reword.fill = false;
        }
    }

    /// Reads the commits the rebase for `edit` replays.
    pub(super) fn quick_edit(&mut self, edit: Edit, hash: String, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let result = git::rebase_commits(&path, &edit.base(&hash));
            Msg::QuickEditCommits { edit, hash, result }
        });
    }

    /// Runs the rebase for `edit` on the commits `quick_edit` read.
    pub(super) fn run_quick_edit(
        &mut self,
        edit: Edit,
        hash: String,
        result: Result<Vec<RebaseCommit>, String>,
        ctx: &egui::Context,
    ) {
        let label = format!("{} {}", edit.label(), short(&hash));
        let command = result
            .and_then(|commits| plan(edit, &hash, commits))
            .and_then(|editor| editor.command());
        match command {
            // Started while something else ran: say so rather than drop it.
            Ok(_) if self.busy_op.is_some() => {
                self.status = Some((
                    format!(
                        "{label}: {} is still running, try again when it's done",
                        self.busy_op.as_deref().unwrap_or_default()
                    ),
                    true,
                ));
            }
            Ok(command) => self.run_undoable(
                &label,
                vec![command],
                Refresh::All,
                Undo::HeadMove { mode: "--keep" },
                ctx,
            ),
            Err(e) => self.status = Some((format!("{label}: {e}"), true)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.to_owned(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            ..Commit::default()
        }
    }

    fn rebase_commit(hash: &str) -> RebaseCommit {
        RebaseCommit {
            hash: hash.to_owned(),
            ..RebaseCommit::default()
        }
    }

    type Planned = Result<(String, Vec<(String, Step)>), String>;

    /// What the plan for `edit` rebases onto, and its hashes and steps, oldest first.
    fn planned(edit: Edit, hash: &str, hashes: &[&str]) -> Planned {
        let commits = hashes.iter().map(|h| rebase_commit(h)).collect();
        plan(edit, hash, commits).map(|editor| {
            let steps = editor
                .entries
                .iter()
                .map(|e| (e.commit.hash.clone(), e.step))
                .collect();
            (editor.onto, steps)
        })
    }

    fn picks(hashes: &[&str]) -> Vec<(String, Step)> {
        hashes
            .iter()
            .map(|h| ((*h).to_owned(), Step::Pick))
            .collect()
    }

    #[test]
    fn dropping_marks_the_commit() {
        let mut steps = picks(&["b", "c", "d"]);
        steps[1].1 = Step::Drop;
        assert_eq!(
            planned(Edit::Drop, "c", &["b", "c", "d"]),
            Ok(("c^".to_owned(), steps))
        );
    }

    #[test]
    fn moving_swaps_with_a_neighbour() {
        assert_eq!(
            planned(Edit::MoveUp, "c", &["b", "c", "d"]),
            Ok(("c^".to_owned(), picks(&["b", "d", "c"])))
        );
        assert_eq!(
            planned(Edit::MoveDown, "c", &["b", "c", "d"]),
            Ok(("c~2".to_owned(), picks(&["c", "b", "d"])))
        );
    }

    #[test]
    fn plans_that_cant_be() {
        let commits = ["c", "d"].map(rebase_commit).to_vec();
        assert!(plan(Edit::MoveUp, "d", commits.clone()).is_err());
        assert!(plan(Edit::MoveDown, "c", commits.clone()).is_err());
        assert!(plan(Edit::Drop, "x", commits.clone()).is_err());
        let mut merged = commits;
        merged[1].merge = true;
        assert!(plan(Edit::Drop, "c", merged).is_err());
    }

    #[test]
    fn what_the_menu_allows() {
        // HEAD e, a merge d of x into c, then c, b and the root a.
        let commits = vec![
            commit("e", &["d"]),
            commit("d", &["c", "x"]),
            commit("x", &["b"]),
            commit("c", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ];
        let index = commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.hash.clone(), i))
            .collect();
        let line = first_parents(&commits, &index, "e");
        let ok = |edit, hash| refusal(edit, &line, hash).is_none();
        assert!(ok(Edit::Message, "e"));
        assert!(ok(Edit::Drop, "e"));
        assert!(!ok(Edit::MoveUp, "e"));
        assert!(!ok(Edit::MoveDown, "e"), "the commit below is a merge");
        assert!(!ok(Edit::Drop, "d"), "a merge");
        assert!(!ok(Edit::Message, "c"), "merges after it");
        assert!(!ok(Edit::Drop, "x"), "not on the first-parent line");

        let line = first_parents(&commits, &index, "c");
        let ok = |edit, hash| refusal(edit, &line, hash).is_none();
        assert!(ok(Edit::MoveUp, "b"));
        assert!(!ok(Edit::MoveDown, "b"), "the root is below it");
        assert!(ok(Edit::MoveDown, "c"));
        assert!(!ok(Edit::Drop, "a"), "the root");
        assert!(ok(Edit::Message, "b"));
    }
}
