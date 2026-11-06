//! The remotes and their branches.

use egui::{Align2, Rect, Response, Sense, Ui, pos2, vec2};

use super::{REMOTE_INDENT, ROW_H, matches, visibility};
use crate::git::{Branch, Remote};
use crate::icons;
use crate::repo::actions::BranchRef;
use crate::repo::branch_tree::{self, LeafRow, TreeView};
use crate::repo::branch_visibility::remote_ref;
use crate::repo::graph_view;
use crate::repo::pinned_sections::{Button, Header, Sections};
use crate::repo::{
    Action, Dialog, Refresh, RepoTab, fast_forward_action, merge_action, ops, rebase_action,
};
use crate::theme::{self, Tokens};

impl RepoTab {
    /// The remote-tracking branches of `remote`. Told apart by name, not by the first part of
    /// it: a remote can have a slash in its name (`my/fork`).
    pub(super) fn remote_branches<'a>(
        &'a self,
        remote: &'a str,
    ) -> impl Iterator<Item = &'a Branch> + 'a {
        let names: Vec<&str> = self.remotes.iter().map(|r| r.name.as_str()).collect();
        self.refs
            .remotes
            .iter()
            .flat_map(|(_, list)| list)
            .filter(move |b| remote_of(&b.name, &names) == Some(remote))
    }

    pub(super) fn remotes_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        filter: &str,
        sections: &mut Sections,
    ) {
        let open = sections.add(
            ui,
            Header {
                key: "remotes",
                icon: "cloud",
                title: "REMOTES",
                count: self.remotes.len(),
                default_open: true,
                button: Some(Button {
                    icon: "plus",
                    tooltip: "Add a remote…",
                    action: idle.then(|| Action::Dialog(Dialog::remote(None))),
                }),
            },
        );
        if !open {
            return;
        }
        for remote in &self.remotes {
            let branches: Vec<&Branch> = self
                .remote_branches(&remote.name)
                .filter(|b| matches(filter, b))
                .collect();
            // A collapsible row per remote, open or closed in this repository only.
            let id = egui::Id::new(("remote-open", self.repo_key(), &remote.name));
            let mut expanded = ui.data_mut(|d| d.get_persisted::<bool>(id)).unwrap_or(true);
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            if r.clicked() {
                expanded = !expanded;
                ui.data_mut(|d| d.insert_persisted(id, expanded));
            }
            paint_remote_header(ui, rect, &r, remote, branches.len(), expanded);
            let r = r.on_hover_text(&remote.url);
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| remote_menu_items(ui, acts, remote, idle));
            if !expanded {
                continue;
            }
            let tree = branch_tree::build(branches.iter().map(|b| {
                let short = remote_short(&b.name, &remote.name).unwrap_or(&b.name);
                (short, *b)
            }));
            let view = TreeView::branches(
                egui::Id::new(("branch-folder", self.repo_key(), "remote", &remote.name)),
                !filter.is_empty(),
                REMOTE_INDENT,
            );
            branch_tree::show(
                ui,
                &tree,
                &view,
                &|_: &&Branch| false,
                &mut |ui, row, b: &&Branch| {
                    self.remote_branch_row(ui, acts, &remote.name, b, row, idle);
                },
            );
        }
    }

    fn remote_branch_row(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        remote: &str,
        b: &Branch,
        row: LeafRow<'_>,
        idle: bool,
    ) {
        let short = remote_short(&b.name, remote).unwrap_or(&b.name);
        let has_pr = self.prs.iter().any(|p| p.branch == short);
        let full = remote_ref(&b.name);
        let shown = self.branch_shown(&full, false);
        let r = remote_row(ui, row, has_pr, !shown);
        acts.extend(visibility::eye(ui, &r, &full, shown));
        let r = r.on_hover_text(format!("{}\nDouble-click to check out", b.name));
        if r.double_clicked() && idle {
            acts.push(Action::CheckoutRemote(b.name.clone()));
        } else if r.clicked() {
            acts.push(Action::SelectHash(b.target.clone()));
        }
        egui::Popup::context_menu(&r)
            .style(crate::theme::menu_style)
            .show(|ui| self.remote_branch_menu_items(ui, acts, remote, short, b, idle));
    }

    fn remote_branch_menu_items(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        remote: &str,
        short: &str,
        b: &Branch,
        idle: bool,
    ) {
        if ui
            .add_enabled(idle, egui::Button::new("Check out as local branch"))
            .clicked()
        {
            acts.push(Action::CheckoutRemote(b.name.clone()));
        }
        if let Some(current) = self.refs.current.as_deref() {
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Fast-forward {current} to it")),
                )
                .clicked()
            {
                acts.push(fast_forward_action(BranchRef::remote(&b.name), current));
            }
            if ui
                .add_enabled(idle, egui::Button::new(format!("Merge into {current}")))
                .clicked()
            {
                acts.push(merge_action(BranchRef::remote(&b.name), current));
            }
            if ui
                .add_enabled(idle, egui::Button::new(format!("Rebase {current} onto it")))
                .clicked()
            {
                acts.push(rebase_action(BranchRef::remote(&b.name), current));
            }
            ui.separator();
            graph_view::compare_items(ui, acts, BranchRef::remote(&b.name), current);
            ui.separator();
        }
        if ui.button("Create branch here…").clicked() {
            acts.push(Action::Dialog(Dialog::create_branch(&b.target, &b.name)));
        }
        ui.separator();
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(b.name.clone());
        }
        ui.separator();
        self.visibility_items(ui, acts, &remote_ref(&b.name), false);
        ui.separator();
        if ui
            .add_enabled(idle, egui::Button::new("Delete from remote…"))
            .clicked()
        {
            acts.push(Action::Dialog(Dialog::confirm(
                "Delete remote branch",
                format!("Delete “{short}” from {remote}? This changes the remote for everyone.",),
                "Delete",
                // Only while it is where it was last fetched: commits pushed since are others'.
                ops(&[&[
                    "push",
                    &format!("--force-with-lease=refs/heads/{short}:{}", b.target),
                    remote,
                    "--delete",
                    &format!("refs/heads/{short}"),
                ]]),
                None,
            )));
        }
    }
}

/// The name of `remote`'s branch `name` on the remote: `main` for `origin/main`. git writes
/// `remotes/origin/main` when a local branch is called `origin/main` too.
pub(in crate::repo) fn remote_short<'a>(name: &'a str, remote: &str) -> Option<&'a str> {
    let under = |n: &'a str| n.strip_prefix(remote)?.strip_prefix('/');
    under(name).or_else(|| under(name.strip_prefix("remotes/")?))
}

/// Which of `remotes` the remote-tracking branch `name` is of: the longest that fits, so that
/// `my/fork/main` is `my/fork`'s even when there is a remote `my`.
pub(in crate::repo) fn remote_of<'r>(name: &str, remotes: &[&'r str]) -> Option<&'r str> {
    remotes
        .iter()
        .copied()
        .filter(|r| remote_short(name, r).is_some())
        .max_by_key(|r| r.len())
}

/// The collapsible row of a remote at `rect`: chevron, icon, name and how many branches it
/// shows.
fn paint_remote_header(
    ui: &Ui,
    rect: Rect,
    r: &Response,
    remote: &Remote,
    count: usize,
    expanded: bool,
) {
    let t = Tokens::get(ui.ctx());
    if r.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 24.0, y), vec2(12.0, 12.0)),
        if expanded {
            "chevron-down"
        } else {
            "chevron-right"
        },
        11.0,
        t.text_faint,
    );
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 40.0, y), vec2(14.0, 14.0)),
        "cloud",
        13.0,
        t.icon,
    );
    let p = ui.painter_at(rect);
    p.text(
        pos2(rect.left() + 54.0, y),
        Align2::LEFT_CENTER,
        &remote.name,
        theme::medium(12.5),
        t.text,
    );
    p.text(
        pos2(rect.right() - 10.0, y),
        Align2::RIGHT_CENTER,
        count.to_string(),
        theme::medium(11.0),
        t.text_faint,
    );
}

fn remote_menu_items(ui: &mut Ui, acts: &mut Vec<Action>, remote: &Remote, idle: bool) {
    if ui
        .add_enabled(idle, egui::Button::new(format!("Fetch {}", remote.name)))
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Fetch {}", remote.name),
            ops: ops(&[&["fetch", "--prune", &remote.name]]),
            refresh: Refresh::All,
        });
    }
    if ui.add_enabled(idle, egui::Button::new("Edit…")).clicked() {
        acts.push(Action::Dialog(Dialog::remote(Some((
            remote.name.clone(),
            remote.url.clone(),
        )))));
    }
    if ui.button("Copy URL").clicked() {
        ui.ctx().copy_text(remote.url.clone());
    }
    ui.separator();
    if ui.add_enabled(idle, egui::Button::new("Remove…")).clicked() {
        acts.push(Action::Dialog(Dialog::confirm(
            "Remove remote",
            format!(
                "Remove the remote “{}” ({})? Its branches disappear from this repository; nothing changes on the server.",
                remote.name, remote.url
            ),
            "Remove",
            ops(&[&["remote", "remove", &remote.name]]),
            None,
        )));
    }
}

/// A remote's branch: only the icon and the name, in from its remote; the name dimmed while
/// the graph leaves the branch out.
fn remote_row(ui: &mut Ui, row: LeafRow<'_>, has_pr: bool, hidden: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 34.0 + row.indent, y), vec2(16.0, 16.0)),
        "git-branch",
        13.0,
        t.text_faint,
    );
    let mut right = rect.right() - 8.0;
    if has_pr {
        let ir = Rect::from_center_size(pos2(right - 6.0, y), vec2(12.0, 12.0));
        icons::paint(ui, ir, "git-pull-request", 12.0, t.success);
        right = ir.left() - 6.0;
    }
    ui.painter_at(rect.with_max_x(right)).text(
        pos2(rect.left() + 48.0 + row.indent, y),
        Align2::LEFT_CENTER,
        row.name,
        egui::FontId::proportional(12.5),
        if hidden { t.text_faint } else { t.text },
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_branch_is_named_without_its_remote() {
        assert_eq!(remote_short("origin/main", "origin"), Some("main"));
        assert_eq!(remote_short("origin/feat/x", "origin"), Some("feat/x"));
        assert_eq!(remote_short("my/fork/main", "my/fork"), Some("main"));
        assert_eq!(remote_short("origin/main", "orig"), None);
        assert_eq!(remote_short("upstream/main", "origin"), None);
    }

    #[test]
    fn an_ambiguous_remote_branch_is_still_found() {
        // `refname:short` when a local branch is called `origin/main` too.
        assert_eq!(remote_short("remotes/origin/main", "origin"), Some("main"));
    }

    #[test]
    fn a_remote_with_a_slash_owns_its_branches() {
        let remotes = ["origin", "my/fork", "my"];
        assert_eq!(remote_of("my/fork/main", &remotes), Some("my/fork"));
        assert_eq!(remote_of("my/other", &remotes), Some("my"));
        assert_eq!(remote_of("origin/my/fork/x", &remotes), Some("origin"));
        assert_eq!(remote_of("gone/main", &remotes), None);
    }
}
