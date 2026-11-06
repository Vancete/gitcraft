//! What Pull does: fast-forward when it can and merge otherwise, only fast-forward, or rebase.
//! Picked in the menu next to the toolbar's Pull and kept in the settings.

use egui::Ui;
use serde::{Deserialize, Serialize};

use super::{Action, Refresh, RepoTab, Undo, ops};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullMode {
    /// Fast-forward if possible, merge otherwise.
    Merge,
    #[default]
    FastForwardOnly,
    Rebase,
}

impl PullMode {
    pub const ALL: [PullMode; 3] = [PullMode::Merge, PullMode::FastForwardOnly, PullMode::Rebase];

    pub fn label(self) -> &'static str {
        match self {
            PullMode::Merge => "Pull (fast-forward if possible)",
            PullMode::FastForwardOnly => "Pull (fast-forward only)",
            PullMode::Rebase => "Pull (rebase)",
        }
    }

    /// The git command, explicit whatever `pull.rebase` / `pull.ff` the user's config sets. A rebase
    /// stashes changes and puts them back, as merging and fast-forwarding keep them anyway.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            PullMode::Merge => &["pull", "--no-rebase", "--ff", "--no-edit"],
            PullMode::FastForwardOnly => &["pull", "--ff-only"],
            PullMode::Rebase => &["pull", "--rebase", "--autostash"],
        }
    }

    /// What the toolbar's Pull says it does.
    pub fn tip(self) -> String {
        let what = match self {
            PullMode::Merge => "Fast-forward the current branch, or merge when it has diverged",
            PullMode::FastForwardOnly => "Fast-forward the current branch",
            PullMode::Rebase => "Rebase the current branch onto its upstream",
        };
        format!(
            "{what} (git {})\nRight-click or the arrow to choose",
            self.args().join(" ")
        )
    }
}

impl RepoTab {
    /// Pulls into the checked-out branch; undo puts it back where it was.
    pub(super) fn pull(&mut self, mode: PullMode, ctx: &egui::Context) {
        self.run_undoable(
            mode.label(),
            ops(&[mode.args()]),
            Refresh::All,
            Undo::HeadMove { mode: "--keep" },
            ctx,
        );
    }
}

/// The choice of what Pull does, and Fetch all.
pub(super) fn pull_menu(ui: &mut Ui, acts: &mut Vec<Action>) {
    let mut settings = Settings::get(ui.ctx());
    for mode in PullMode::ALL {
        if ui
            .radio(settings.pull_mode == mode, mode.label())
            .on_hover_text(format!("git {}", mode.args().join(" ")))
            .clicked()
            && settings.pull_mode != mode
        {
            settings.pull_mode = mode;
            settings.set(ui.ctx());
        }
    }
    ui.separator();
    if ui.button("Fetch all").clicked() {
        acts.push(Action::Fetch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pull_mode_says_how_to_reconcile() {
        for mode in PullMode::ALL {
            let args = mode.args();
            assert_eq!(args[0], "pull");
            assert!(
                args.iter()
                    .any(|a| ["--ff-only", "--rebase", "--no-rebase"].contains(a)),
                "{mode:?}"
            );
        }
    }
}
