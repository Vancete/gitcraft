//! The tags.

use std::path::Path;

use egui::{Align2, Rect, Response, Ui, pos2, vec2};

use super::{ROW_H, matches};
use crate::git::{self, Branch};
use crate::icons;
use crate::repo::pinned_sections::{Header, Sections};
use crate::repo::{Action, Dialog, Refresh, RepoTab, Undo, branch_tree, ops, widgets};
use crate::theme::Tokens;

impl RepoTab {
    /// The tags passing `filter`; none at all hides the section.
    pub(super) fn tags_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        filter: &str,
        sections: &mut Sections,
    ) {
        let tags: Vec<&Branch> = self
            .refs
            .tags
            .iter()
            .filter(|b| matches(filter, b))
            .collect();
        if tags.is_empty()
            || !sections.add(
                ui,
                Header {
                    key: "tags",
                    icon: "tag",
                    title: "TAGS",
                    count: tags.len(),
                    default_open: false,
                    button: None,
                },
            )
        {
            return;
        }
        for tag in tags {
            // Known by the tag, not its place: a fetch adds tags under an open menu.
            let key = ("tag-row", tag.name.as_str());
            // Repositories can have thousands of tags: those out of sight are skipped over,
            // but for one whose menu is open.
            if !widgets::keyed_row_menu_open(ui, key) && branch_tree::skip_unseen(ui, ROW_H) {
                continue;
            }
            let r = plain_row(ui, key, "tag", &tag.name, "");
            if r.clicked() {
                acts.push(Action::SelectHash(tag.target.clone()));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| self.tag_menu_items(ui, acts, tag, idle));
        }
    }

    fn tag_menu_items(&self, ui: &mut Ui, acts: &mut Vec<Action>, tag: &Branch, idle: bool) {
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(tag.name.clone());
        }
        if let Some(remote) = self.default_remote() {
            if ui
                .add_enabled(idle, egui::Button::new(format!("Push to {remote}")))
                .clicked()
            {
                acts.push(Action::Ops {
                    label: format!("Push tag {}", tag.name),
                    ops: ops(&[&["push", &remote, &format!("refs/tags/{}", tag.name)]]),
                    refresh: Refresh::All,
                });
            }
            if ui
                .add_enabled(idle, egui::Button::new(format!("Delete from {remote}…")))
                .clicked()
            {
                acts.push(Action::Dialog(Dialog::confirm(
                    "Delete remote tag",
                    format!("Delete the tag “{}” from {remote}?", tag.name),
                    "Delete",
                    ops(&[&[
                        "push",
                        &remote,
                        "--delete",
                        &format!("refs/tags/{}", tag.name),
                    ]]),
                    None,
                )));
            }
        }
        ui.separator();
        if ui
            .add_enabled(idle, egui::Button::new("Delete tag…"))
            .clicked()
        {
            let object = tag_object(&self.path, &tag.name);
            acts.push(Action::Dialog(
                Dialog::confirm(
                    "Delete tag",
                    format!("Delete the local tag “{}”?", tag.name),
                    "Delete",
                    ops(&[&["tag", "-d", "--", &tag.name]]),
                    None,
                )
                .undo(delete_tag_undo(
                    &tag.name,
                    object.as_deref().unwrap_or(&tag.target),
                )),
            ));
        }
    }
}

/// What the tag `name` itself points at: for an annotated tag, its tag object (the message,
/// tagger and signature), not the commit a `Branch`'s `target` is.
fn tag_object(repo: &Path, name: &str) -> Option<String> {
    git::run(
        Some(repo),
        &["rev-parse", "--verify", "-q", &format!("refs/tags/{name}")],
    )
    .ok()
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty())
}

/// How deleting the tag `name` that points at `object` is undone: the tag made again pointing at
/// that same object. For an annotated tag, that is its tag object, still there once the tag is
/// gone: made at the commit instead, it would come back a lightweight tag, message lost.
fn delete_tag_undo(name: &str, object: &str) -> Undo {
    Undo::Custom {
        undo: ops(&[&["tag", "--", name, object]]),
        redo: ops(&[&["tag", "-d", "--", name]]),
    }
}

/// An icon, a name and optional faint text after it; known by `key` (see
/// [`widgets::keyed_row`]).
fn plain_row(ui: &mut Ui, key: impl egui::AsIdSalt, icon: &str, name: &str, sub: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = widgets::keyed_row(ui, ROW_H, key);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
        icon,
        13.0,
        t.text_faint,
    );
    let p = ui.painter_at(rect);
    let r = p.text(
        pos2(rect.left() + 48.0, y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        t.text,
    );
    if !sub.is_empty() {
        p.text(
            pos2(r.right() + 8.0, y),
            Align2::LEFT_CENTER,
            sub,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deleting an annotated tag and undoing that brings back the annotated tag, message and all,
    /// not a lightweight one at its commit.
    #[test]
    fn undoing_a_tag_delete_keeps_it_annotated() {
        let dir = std::env::temp_dir().join(format!("gitcraft-tag-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
            &["tag", "-a", "-m", "release notes", "v1"],
        ] {
            assert!(git(args).is_ok());
        }
        let object = tag_object(&dir, "v1");
        let undo = match delete_tag_undo("v1", object.as_deref().unwrap_or("")) {
            Undo::Custom { undo, .. } => undo,
            _ => Vec::new(),
        };
        assert!(git(&["tag", "-d", "--", "v1"]).is_ok());
        let restored = undo.iter().all(|op| {
            let args: Vec<&str> = op.iter().map(String::as_str).collect();
            git(&args).is_ok()
        });
        let kind = git(&["cat-file", "-t", "refs/tags/v1"]).unwrap_or_default();
        let message = git(&["tag", "-l", "-n1", "v1"]).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(restored);
        assert_eq!(kind.trim(), "tag");
        assert!(message.contains("release notes"), "{message}");
    }
}
