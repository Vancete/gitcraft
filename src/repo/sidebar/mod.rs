//! Left pane: local branches, remotes and their branches, worktrees, stashes, pull requests,
//! tags.

mod branches;
mod pull_requests;
mod remotes;
mod stashes;
mod tags;
mod worktrees;

use egui::{Sense, Ui, vec2};

use super::pinned_sections::Sections;
use super::{Action, RepoTab, widgets};
use crate::git::Branch;
use crate::icons;
use crate::theme::Tokens;

pub(super) use remotes::{remote_of, remote_short};
pub(super) use stashes::stash_menu_items;

/// The pane's rows: a little taller than the file lists', 2 px more above and below.
pub(super) const ROW_H: f32 = widgets::ROW_H + 4.0;
/// How far in a remote's branches are, under their remote.
const REMOTE_INDENT: f32 = 18.0;

impl RepoTab {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        widgets::panel_header(ui, "Branches", |_| {});
        // Sized like the commit search bar (Ctrl+F).
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            icons::inline(ui, "search", 15.0, t.text_faint);
            let filter = ui.add(
                egui::TextEdit::singleline(&mut self.branch_filter)
                    .hint_text("Filter branches and tags")
                    .desired_width(ui.available_width() - 10.0)
                    .margin(egui::Margin::symmetric(8, 4)),
            );
            if std::mem::take(&mut self.focus_branch_filter) {
                filter.request_focus();
            }
        });
        ui.add_space(8.0);

        let filter = self.branch_filter.to_lowercase();
        let idle = self.busy_op.is_none();

        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("branches")
            .auto_shrink(false);
        if let Some(offset) = self.sidebar_jump.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        let mut sections = Sections::new(
            egui::Id::new(("sidebar-section", self.repo_key())),
            self.sidebar_reveal.take(),
        );
        let output = scroll.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.local_section(ui, acts, idle, &filter, &mut sections);
            self.remotes_section(ui, acts, idle, &filter, &mut sections);
            self.worktree_section(ui, acts, idle, &mut sections);
            self.stash_section(ui, acts, idle, &mut sections);
            self.pr_section(ui, acts, idle, &mut sections);
            self.tags_section(ui, acts, idle, &filter, &mut sections);
        });
        self.sidebar_jump = sections.show(ui, output.inner_rect, output.state.offset.y, acts);
    }

    /// The folded sidebar: a button for the filter and one for each section, about where they
    /// are when it is open. Each opens it there. Its outer edge belongs at `outer_edge`.
    pub(super) fn folded_sidebar(&self, ui: &mut Ui, outer_edge: f32, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let slid = widgets::slide_offset(ui, outer_edge, false);
        let mut col = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(ui.max_rect().translate(vec2(slid, 0.0)))
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        col.spacing_mut().item_spacing.y = 2.0;
        let (strip, _) = col.allocate_exact_size(
            vec2(col.available_width(), widgets::PANEL_HEADER_H),
            Sense::hover(),
        );
        col.painter().rect_filled(strip, 0.0, t.tab_strip);
        // Level with the filter, then with the first section header.
        col.add_space(5.0);
        if icons::tool_button(&mut col, "search", "", true)
            .on_hover_text("Filter branches and tags")
            .clicked()
        {
            acts.push(Action::OpenBranches(None));
        }
        col.add_space(5.0);
        let prs = self.forge.map(|forge| match forge {
            crate::forge::Forge::GitHub => "Pull requests",
            crate::forge::Forge::GitLab => "Merge requests",
        });
        let tags = (!self.refs.tags.is_empty()).then_some("Tags");
        let sections = [
            ("local", "laptop", Some("Local branches")),
            ("remotes", "cloud", Some("Remotes")),
            ("worktrees", "folder-tree", Some("Worktrees")),
            ("stashes", "archive", Some("Stashes")),
            ("prs", "git-pull-request", prs),
            ("tags", "tag", tags),
        ];
        for (key, icon, tooltip) in sections {
            let Some(tooltip) = tooltip else { continue };
            if icons::tool_button(&mut col, icon, "", true)
                .on_hover_text(tooltip)
                .clicked()
            {
                acts.push(Action::OpenBranches(Some(key)));
            }
        }
    }
}

/// Whether `b` passes the branch filter, `filter` already in lower case.
fn matches(filter: &str, b: &Branch) -> bool {
    filter.is_empty() || b.name.to_lowercase().contains(filter)
}
