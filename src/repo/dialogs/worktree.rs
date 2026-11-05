//! The dialog that adds a worktree, on a branch of its own in a folder of its own.

use egui::{RichText, Ui};

use crate::git::{Branch, Worktree};
use crate::repo::{Action, Refresh};
use crate::theme::Tokens;

use super::{Dialog, Outcome, buttons, dash_spaces, field, title, usable};

impl Dialog {
    /// A folder beside the repository named after the branch: `app` + `feature/x` → `app-feature-x`.
    pub(super) fn suggest_path(&mut self) {
        if let Dialog::AddWorktree {
            repo,
            path,
            path_edited: false,
            new_branch,
            branch,
            new_name,
            ..
        } = self
        {
            let name = if *new_branch {
                new_name.as_str()
            } else {
                branch.as_str()
            };
            let repo_name = repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "worktree".to_owned());
            let folder = if name.trim().is_empty() {
                format!("{repo_name}-worktree")
            } else {
                format!("{repo_name}-{}", name.trim().replace(['/', '\\', ' '], "-"))
            };
            let parent = repo.parent().unwrap_or(repo.as_path());
            *path = parent.join(folder).display().to_string();
        }
    }
}

/// Branches that aren't checked out in any worktree yet.
pub(super) fn free_branches<'a>(local: &'a [Branch], worktrees: &[Worktree]) -> Vec<&'a str> {
    let busy: Vec<&str> = worktrees
        .iter()
        .filter_map(|w| w.branch.as_deref())
        .collect();
    local
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| !busy.contains(n))
        .collect()
}

/// A worktree on an existing branch out of `free`, or on a new one from `base`.
#[allow(clippy::too_many_arguments)]
pub(super) fn add_worktree(
    ui: &mut Ui,
    enter: bool,
    free: &[&str],
    path: &mut String,
    path_edited: &mut bool,
    new_branch: &mut bool,
    branch: &mut String,
    new_name: &mut String,
    base: &str,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    title(
        ui,
        "Add worktree",
        "A second working folder of this repository, on its own branch.",
        &t,
    );
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .radio_value(new_branch, false, "Existing branch")
            .changed();
        changed |= ui.radio_value(new_branch, true, "New branch").changed();
    });
    ui.add_space(4.0);
    if *new_branch {
        changed |= new_branch_field(ui, new_name, base, focus);
    } else {
        changed |= branch_choice(ui, branch, free);
    }
    ui.add_space(8.0);
    folder_field(ui, path, path_edited);
    ui.add_space(12.0);
    let name_ok = if *new_branch {
        usable(new_name)
    } else {
        usable(branch)
    };
    let valid = name_ok && usable(path);
    let outcome = buttons(ui, "Add", valid, enter, || {
        let path = path.trim().to_owned();
        let args: Vec<String> = if *new_branch {
            vec![
                "worktree".into(),
                "add".into(),
                "-b".into(),
                new_name.trim().to_owned(),
                path.clone(),
                base.to_owned(),
            ]
        } else {
            vec![
                "worktree".into(),
                "add".into(),
                path.clone(),
                branch.clone(),
            ]
        };
        Action::Ops {
            label: "Add worktree".to_owned(),
            ops: vec![args],
            refresh: Refresh::All,
        }
    });
    match outcome {
        Outcome::Open if changed => Outcome::Suggest,
        other => other,
    }
}

/// The name of the new branch, and where it starts. Whether the name changed.
fn new_branch_field(ui: &mut Ui, new_name: &mut String, base: &str, focus: &mut bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let edit = field(ui, new_name, "feature/my-branch", focus);
    dash_spaces(new_name);
    ui.label(
        RichText::new(format!(
            "Starting at HEAD ({})",
            base.get(..8).unwrap_or(base)
        ))
        .color(t.text_faint)
        .small(),
    );
    edit.changed()
}

/// The branch to check out, out of `free`. Whether another one was chosen.
fn branch_choice(ui: &mut Ui, branch: &mut String, free: &[&str]) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut changed = false;
    egui::ComboBox::from_id_salt("worktree-branch")
        .width(ui.available_width())
        .selected_text(if branch.is_empty() {
            "Choose a branch"
        } else {
            branch.as_str()
        })
        .show_ui(ui, |ui| {
            for name in free {
                if ui.selectable_label(branch == name, *name).clicked() {
                    *branch = (*name).to_owned();
                    changed = true;
                }
            }
            if free.is_empty() {
                ui.label(
                    RichText::new("Every branch is checked out somewhere").color(t.text_faint),
                );
            }
        });
    changed
}

/// Where the worktree goes, typed or picked.
fn folder_field(ui: &mut Ui, path: &mut String, path_edited: &mut bool) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new("Folder").color(t.text_dim).small());
    ui.horizontal(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(path)
                .desired_width(ui.available_width() - 90.0)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        if edit.changed() {
            *path_edited = true;
        }
        if ui
            .add(egui::Button::new("Browse…").min_size(egui::vec2(80.0, 28.0)))
            .clicked()
            && let Some(dir) = rfd::FileDialog::new()
                .set_title("Worktree folder")
                .pick_folder()
        {
            *path = dir.display().to_string();
            *path_edited = true;
        }
    });
}
