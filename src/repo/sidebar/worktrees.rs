//! The repository's worktrees.

use egui::{Align2, Rect, Response, Sense, Ui, pos2, vec2};

use crate::git::Worktree;
use crate::icons;
use crate::repo::pinned_sections::{Button, Header, Sections};
use crate::repo::{Action, Dialog, Refresh, RepoTab, ops, worktree_tab};
use crate::settings::Settings;
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(super) fn worktree_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        sections: &mut Sections,
    ) {
        let open = sections.add(
            ui,
            Header {
                key: "worktrees",
                icon: "folder-tree",
                title: "WORKTREES",
                count: self.worktrees.len(),
                default_open: true,
                button: Some(Button {
                    icon: "plus",
                    tooltip: "Add a worktree…",
                    action: idle.then(|| {
                        Action::Dialog(Dialog::add_worktree(&self.path, None, &self.refs.head))
                    }),
                }),
            },
        );
        if !open {
            return;
        }
        let same_tab = Settings::get(ui.ctx()).worktrees_in_same_tab;
        let any_prunable = self.worktrees.iter().any(|w| w.prunable.is_some());
        for wt in &self.worktrees {
            let r = worktree_row(ui, wt).on_hover_text(worktree_tip(wt, same_tab));
            let can_open = !wt.current && wt.prunable.is_none();
            if r.double_clicked() && can_open {
                acts.push(Action::OpenWorktree(wt.path.clone()));
            } else if r.clicked() && !wt.head.is_empty() {
                acts.push(Action::SelectHash(wt.head.clone()));
            }
            egui::Popup::context_menu(&r)
                .style(crate::theme::menu_style)
                .show(|ui| {
                    worktree_menu_items(ui, acts, wt, idle, same_tab, can_open, any_prunable);
                });
        }
    }
}

/// A worktree: its icon (a dot for uncommitted changes), name, branch and what is wrong with it.
fn worktree_row(ui: &mut Ui, wt: &Worktree) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    let painter = ui.painter_at(rect);
    if wt.current {
        painter.rect_filled(rect, 0.0, t.accent.gamma_multiply(0.14));
    }
    if r.hovered() {
        painter.rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    let icon_at = pos2(rect.left() + 34.0, y);
    if worktree_tab::changes_elsewhere(wt).is_some() {
        worktree_tab::paint_dirty(&painter, &t, icon_at);
    } else {
        let icon = if wt.current { "check" } else { "folder-git-2" };
        icons::paint(
            ui,
            Rect::from_center_size(icon_at, vec2(16.0, 16.0)),
            icon,
            14.0,
            if wt.current { t.accent } else { t.text_faint },
        );
    }
    let font = if wt.current {
        theme::semibold(12.5)
    } else {
        egui::FontId::proportional(12.5)
    };
    let mut right = rect.right() - 8.0;
    for (on, label, color) in [
        (wt.prunable.is_some(), "missing", t.danger),
        (wt.locked.is_some(), "locked", t.warning),
        (wt.main, "main", t.text_faint),
    ] {
        if on {
            let r = painter.text(
                pos2(right, y),
                Align2::RIGHT_CENTER,
                label,
                theme::medium(10.5),
                color,
            );
            right = r.left() - 6.0;
        }
    }
    let clip = painter.with_clip_rect(Rect::from_min_max(rect.min, pos2(right, rect.bottom())));
    clip.text(
        pos2(rect.left() + 48.0, y - 7.0),
        Align2::LEFT_CENTER,
        wt.name(),
        font,
        t.text,
    );
    let sub = match (&wt.branch, wt.bare) {
        (_, true) => "bare".to_owned(),
        (Some(branch), _) => branch.clone(),
        (None, _) => format!("detached at {}", wt.head.get(..8).unwrap_or(&wt.head)),
    };
    clip.text(
        pos2(rect.left() + 48.0, y + 8.0),
        Align2::LEFT_CENTER,
        sub,
        egui::FontId::proportional(11.0),
        t.text_faint,
    );
    r
}

/// The hover text of a worktree: where it is, what is going on in it, how to open it.
fn worktree_tip(wt: &Worktree, same_tab: bool) -> String {
    let mut tip = wt.path.display().to_string();
    if let Some(line) = worktree_tab::changes_line(wt) {
        tip.push_str(&format!("\n{line}"));
    }
    if let Some(reason) = wt.locked.as_deref().filter(|r| !r.is_empty()) {
        tip.push_str(&format!("\nLocked: {reason}"));
    }
    if let Some(reason) = &wt.prunable {
        tip.push_str(&format!("\nMissing: {reason}"));
    }
    if !wt.current {
        tip.push_str(if same_tab {
            "\nDouble-click to open"
        } else {
            "\nDouble-click to open in a tab"
        });
    }
    tip
}

fn worktree_menu_items(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    wt: &Worktree,
    idle: bool,
    same_tab: bool,
    can_open: bool,
    any_prunable: bool,
) {
    let path = wt.path.display().to_string();
    if same_tab
        && ui
            .add_enabled(can_open, egui::Button::new("Open"))
            .clicked()
    {
        acts.push(Action::OpenWorktree(wt.path.clone()));
    }
    let new_tab = if same_tab {
        "Open in a new tab"
    } else {
        "Open in a tab"
    };
    if ui
        .add_enabled(can_open, egui::Button::new(new_tab))
        .clicked()
    {
        acts.push(Action::OpenTab(wt.path.clone()));
    }
    if ui.button("Open terminal here").clicked() {
        acts.push(Action::NewTerminal {
            shell: None,
            cwd: Some(wt.path.clone()),
        });
    }
    if ui.button(crate::repo::reveal_label()).clicked() {
        let _ = open::that_detached(&wt.path);
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(path.clone());
    }
    if !wt.main {
        ui.separator();
        let (label, args) = if wt.locked.is_some() {
            ("Unlock", vec!["worktree", "unlock", path.as_str()])
        } else {
            ("Lock", vec!["worktree", "lock", path.as_str()])
        };
        if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
            acts.push(Action::Ops {
                label: format!("{label} worktree {}", wt.name()),
                ops: ops(&[&args]),
                refresh: Refresh::All,
            });
        }
        if ui
            .add_enabled(idle && !wt.current, egui::Button::new("Remove…"))
            .on_disabled_hover_text("Open another worktree to remove this one")
            .clicked()
        {
            acts.push(Action::Dialog(Dialog::confirm(
                "Remove worktree",
                format!("Remove the worktree at {path}? Its folder is deleted; the branch stays."),
                "Remove",
                ops(&[&["worktree", "remove", &path]]),
                Some((
                    "Force, even with uncommitted changes or locked".to_owned(),
                    ops(&[&["worktree", "remove", "--force", "--force", &path]]),
                )),
            )));
        }
    }
    if any_prunable {
        ui.separator();
        if ui
            .add_enabled(idle, egui::Button::new("Prune missing worktrees"))
            .clicked()
        {
            acts.push(Action::Ops {
                label: "Prune worktrees".into(),
                ops: ops(&[&["worktree", "prune"]]),
                refresh: Refresh::All,
            });
        }
    }
}
