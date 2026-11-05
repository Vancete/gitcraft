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
            let delete = |flag: &str| {
                ops(&[
                    &["branch", flag, "--", &b.name],
                    &["push", &remote, "--delete", &remote_ref],
                ])
            };
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
