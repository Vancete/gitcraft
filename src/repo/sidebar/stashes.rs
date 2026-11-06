//! The stashes, and the menu a stash has here and in the detail pane.

use egui::{Align2, Rect, Sense, Ui, pos2, vec2};

use crate::git::{self, Stash};
use crate::icons;
use crate::repo::pinned_sections::{Button, Header, Sections};
use crate::repo::{Action, Dialog, Refresh, RepoTab, ops};
use crate::theme::Tokens;

impl RepoTab {
    pub(super) fn stash_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        sections: &mut Sections,
    ) {
        let t = Tokens::get(ui.ctx());
        let can_stash = idle && !self.work.is_clean();
        let open = sections.add(
            ui,
            Header {
                key: "stashes",
                icon: "archive",
                title: "STASHES",
                count: self.stashes.len(),
                default_open: true,
                button: Some(Button {
                    icon: "plus",
                    tooltip: "Stash changes…",
                    action: can_stash.then(|| Action::Dialog(Dialog::stash())),
                }),
            },
        );
        if !open {
            return;
        }
        let now = git::now();
        for (i, stash) in self.stashes.iter().enumerate() {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
            if self.stash_view == Some(i) {
                ui.painter().rect_filled(rect, 0.0, t.row_selected);
            } else if r.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let y = rect.center().y;
            icons::paint(
                ui,
                Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
                "archive",
                13.0,
                t.text_faint,
            );
            let p = ui.painter_at(rect);
            p.text(
                pos2(rect.left() + 48.0, y - 7.0),
                Align2::LEFT_CENTER,
                &stash.message,
                egui::FontId::proportional(12.5),
                t.text,
            );
            p.text(
                pos2(rect.left() + 48.0, y + 8.0),
                Align2::LEFT_CENTER,
                format!("{} · {}", stash.name, git::relative_time(stash.time, now)),
                egui::FontId::proportional(11.0),
                t.text_faint,
            );
            if r.clicked() {
                acts.push(Action::SelectStash(i));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| stash_menu(ui, acts, stash, idle));
        }
    }
}

/// What the stash menu runs on a stash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::repo) enum StashOp {
    Apply,
    Pop,
    Drop,
}

/// The commands that apply, pop or drop `stash`, the one that was seen. A stash's number moves
/// as stashes are pushed and dropped (in the terminal, in another worktree, while a confirmation
/// is open): applying goes by its commit, and popping or dropping, which git only does by number,
/// first checks that the number is still that commit's, so that it never drops another stash.
pub(in crate::repo) fn stash_ops(stash: &Stash, op: StashOp) -> Vec<Vec<String>> {
    let (name, hash) = (stash.name.as_str(), stash.hash.as_str());
    if op == StashOp::Apply || name.is_empty() || hash.is_empty() {
        let target = if hash.is_empty() { name } else { hash };
        return match op {
            StashOp::Apply => ops(&[&["stash", "apply", "--index", target]]),
            StashOp::Pop => ops(&[&["stash", "pop", "--index", name]]),
            StashOp::Drop => ops(&[&["stash", "drop", name]]),
        };
    }
    // Each an ancestor of the other: the same commit.
    let mut steps = ops(&[
        &["merge-base", "--is-ancestor", name, hash],
        &["merge-base", "--is-ancestor", hash, name],
    ]);
    steps.extend(match op {
        StashOp::Pop => ops(&[&["stash", "pop", "--index", name]]),
        _ => ops(&[&["stash", "drop", name]]),
    });
    steps
}

/// Apply / pop / drop for `stash`, by what it is rather than where it was in the list.
pub(in crate::repo) fn stash_menu(ui: &mut Ui, acts: &mut Vec<Action>, stash: &Stash, idle: bool) {
    menu(ui, acts, &stash.name, idle, |op| stash_ops(stash, op));
}

/// Apply / pop / drop for the stash `name` (`stash@{n}`), whichever stash has that number when
/// it runs: `stash_menu` is the one that can't hit another stash.
pub(in crate::repo) fn stash_menu_items(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    name: &str,
    idle: bool,
) {
    let by_name = Stash {
        name: name.to_owned(),
        hash: String::new(),
        base: String::new(),
        message: String::new(),
        author: String::new(),
        email: String::new(),
        time: 0,
        date: String::new(),
    };
    menu(ui, acts, name, idle, |op| stash_ops(&by_name, op));
}

fn menu(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    name: &str,
    idle: bool,
    steps: impl Fn(StashOp) -> Vec<Vec<String>>,
) {
    if ui
        .add_enabled(idle, egui::Button::new("Apply"))
        .on_hover_text("Bring the changes back and keep the stash")
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Apply {name}"),
            ops: steps(StashOp::Apply),
            refresh: Refresh::All,
        });
    }
    if ui
        .add_enabled(idle, egui::Button::new("Pop"))
        .on_hover_text("Bring the changes back and drop the stash")
        .clicked()
    {
        acts.push(Action::Ops {
            label: format!("Pop {name}"),
            ops: steps(StashOp::Pop),
            refresh: Refresh::All,
        });
    }
    ui.separator();
    if ui.add_enabled(idle, egui::Button::new("Drop…")).clicked() {
        acts.push(Action::Dialog(Dialog::confirm(
            "Drop stash",
            format!("Delete {name}? Its changes are gone for good."),
            "Drop",
            steps(StashOp::Drop),
            None,
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stash dropped or popped from the menu is the one seen, even once the list has moved
    /// (a stash popped in the terminal since): never the one that took its number.
    #[test]
    fn stash_ops_hit_the_stash_seen() {
        let dir = std::env::temp_dir().join(format!("gitcraft-stash-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
        ] {
            assert!(git(args).is_ok());
        }
        let file = dir.join("f.txt");
        let _ = std::fs::write(&file, "base\n");
        assert!(git(&["add", "."]).is_ok());
        assert!(git(&["commit", "-q", "-m", "base"]).is_ok());
        for text in ["c", "b", "a"] {
            let _ = std::fs::write(&file, format!("{text}\n"));
            assert!(git(&["stash", "push", "-q", "-m", text]).is_ok());
        }
        let run = |steps: Vec<Vec<String>>| {
            steps.iter().all(|op| {
                let args: Vec<&str> = op.iter().map(String::as_str).collect();
                git(&args).is_ok()
            })
        };
        let messages = || -> Vec<String> {
            git::stashes(&dir)
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.message)
                .collect()
        };
        // `b`, seen as stash@{1}; then `a` is popped elsewhere, and `c` is stash@{1} now.
        let seen = git::stashes(&dir)
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.message == "b");
        let seen_name = seen.as_ref().map(|s| s.name.clone());
        assert!(git(&["stash", "pop", "-q"]).is_ok());
        assert!(git(&["checkout", "-q", "--", "."]).is_ok());
        assert!(seen.is_some());
        let Some(seen) = seen else { return };
        let dropped = run(stash_ops(&seen, StashOp::Drop));
        let popped = run(stash_ops(&seen, StashOp::Pop));
        let after_refusals = messages();
        // Applying goes by the commit: it brings back `b`.
        let applied = run(stash_ops(&seen, StashOp::Apply));
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(git(&["checkout", "-q", "--", "."]).is_ok());
        // Where it is now, it goes.
        let now = git::stashes(&dir)
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.hash == seen.hash);
        let dropped_now = now.is_some_and(|now| run(stash_ops(&now, StashOp::Drop)));
        let at_end = messages();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen_name.as_deref(), Some("stash@{1}"));
        assert!(!dropped && !popped);
        assert_eq!(after_refusals, ["b", "c"]);
        assert!(applied);
        assert_eq!(text, "b\n");
        assert!(dropped_now);
        assert_eq!(at_end, ["c"]);
    }
}
