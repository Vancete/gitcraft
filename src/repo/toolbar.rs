//! The tab's toolbar: undo / redo, remote and repository buttons, the current branch, and the
//! status of the last operation.

use std::path::PathBuf;
use std::time::Duration;

use egui::Ui;

use super::{Action, Dialog, RepoTab, abort_action, ci_status, widgets};
use crate::icons;
use crate::theme::Tokens;

impl RepoTab {
    pub(super) fn toolbar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let idle = self.busy_op.is_none();
            self.history_buttons(ui, idle, acts);
            widgets::toolbar_separator(ui);
            self.remote_buttons(ui, idle, acts);
            widgets::toolbar_separator(ui);
            self.repo_buttons(ui, idle, acts);
            widgets::toolbar_separator(ui);
            if icons::tool_button(ui, "refresh-cw", "", !self.loading)
                .on_hover_text("Reload (F5)")
                .clicked()
            {
                acts.push(Action::Reload);
            }
            widgets::toolbar_separator(ui);
            self.head_label(ui, idle, acts);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.op_status(ui);
            });
        });
    }

    /// Undo and redo, with what they'd undo or redo as tooltips.
    fn history_buttons(&self, ui: &mut Ui, idle: bool, acts: &mut Vec<Action>) {
        let undo_tip = match self.undo_stack.last() {
            Some(e) => format!("Undo {} (Ctrl+Z)", e.label),
            None => "Nothing to undo".to_owned(),
        };
        if icons::tool_button(ui, "undo-2", "", idle && !self.undo_stack.is_empty())
            .on_hover_text(undo_tip)
            .on_disabled_hover_text("Nothing to undo")
            .clicked()
        {
            acts.push(Action::Undo);
        }
        let redo_tip = match self.redo_stack.last() {
            Some(e) => format!("Redo {} (Ctrl+Y)", e.label),
            None => "Nothing to redo".to_owned(),
        };
        if icons::tool_button(ui, "redo-2", "", idle && !self.redo_stack.is_empty())
            .on_hover_text(redo_tip)
            .on_disabled_hover_text("Nothing to redo")
            .clicked()
        {
            acts.push(Action::Redo);
        }
    }

    /// Fetch, pull and push, with how far the current branch is behind and ahead.
    fn remote_buttons(&self, ui: &mut Ui, idle: bool, acts: &mut Vec<Action>) {
        let has_remote = !self.remotes.is_empty();
        let fetched = format!(
            "Fetch all remotes (git fetch --all --prune)\nLast fetched {} ago",
            short_duration(self.last_fetch.elapsed())
        );
        if icons::tool_button(ui, "cloud-download", "Fetch", idle && has_remote)
            .on_hover_text(fetched)
            .clicked()
        {
            acts.push(Action::Fetch);
        }
        let (ahead, behind) = self
            .refs
            .current
            .as_ref()
            .and_then(|c| self.refs.local.iter().find(|b| &b.name == c))
            .map(|b| b.ahead_behind())
            .unwrap_or((0, 0));
        if icons::tool_button_count(ui, "arrow-down-to-line", "Pull", behind, idle && has_remote)
            .on_hover_text("Fast-forward the current branch (git pull --ff-only)")
            .clicked()
        {
            acts.push(Action::Pull);
        }
        if icons::tool_button_count(ui, "arrow-up-from-line", "Push", ahead, idle && has_remote)
            .on_hover_text("Push the current branch (publishes it if it has no upstream yet)")
            .clicked()
        {
            acts.push(Action::Push);
        }
    }

    /// Branch, stash, terminal, search and the folder menu.
    fn repo_buttons(&self, ui: &mut Ui, idle: bool, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let head = self.refs.head.clone();
        if icons::tool_button(ui, "git-branch-plus", "Branch", idle && !head.is_empty())
            .on_hover_text("Create a branch at HEAD")
            .clicked()
        {
            acts.push(Action::Dialog(Dialog::create_branch(&head, "HEAD")));
        }
        if icons::tool_button(ui, "archive", "Stash", idle && !self.work.is_clean())
            .on_hover_text("Stash all changes, untracked files included")
            .on_disabled_hover_text("No changes to stash")
            .clicked()
        {
            acts.push(Action::Dialog(Dialog::stash()));
        }
        let terminal = icons::tool_button(ui, "square-terminal", "Terminal", true)
            .on_hover_text("Terminal in the repository folder");
        if self.terminal_open {
            widgets::active_underline(ui, terminal.rect, t.accent);
        }
        if terminal.clicked() {
            acts.push(Action::ToggleTerminal);
        }
        let search =
            icons::tool_button(ui, "search", "", true).on_hover_text("Search commits (Ctrl+F)");
        if self.search.open {
            widgets::active_underline(ui, search.rect, t.accent);
        }
        if search.clicked() {
            acts.push(Action::ToggleSearch);
        }
        let open = icons::tool_button(ui, "external-link", "", true)
            .on_hover_text("Open the folder or the repository's page…");
        let page = self.web_page();
        egui::Popup::menu(&open)
            .style(crate::theme::menu_style)
            .show(|ui| {
                let browser = ui
                    .add_enabled(page.is_some(), egui::Button::new("Open in browser"))
                    .on_disabled_hover_text("No remote with a web page");
                if let Some(url) = &page {
                    let browser = browser.on_hover_text(url);
                    if browser.clicked() {
                        acts.push(Action::OpenUrl(url.clone()));
                    }
                }
                if ui.button(reveal_label()).clicked() {
                    acts.push(Action::OpenPath(PathBuf::new()));
                }
                if ui.button("Open in editor").clicked() {
                    acts.push(Action::OpenInEditor(PathBuf::new()));
                }
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(self.path.display().to_string());
                }
            });
    }

    /// The checked-out branch (or detached HEAD), and a merge or rebase in progress.
    fn head_label(&self, ui: &mut Ui, idle: bool, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        match &self.refs.current {
            Some(branch) => {
                icons::inline(ui, "git-branch", 14.0, t.text_dim);
                let label = ui.label(egui::RichText::new(branch).font(crate::theme::medium(12.5)));
                if let Some(e) = &self.ci.error {
                    label.on_hover_text(format!("No build status: {e}"));
                }
            }
            None if self.refs.head.is_empty() => {
                ui.label(egui::RichText::new("No commits yet").color(t.text_dim));
            }
            None => {
                ui.label(
                    egui::RichText::new(format!(
                        "Detached HEAD at {}",
                        self.refs.head.get(..8).unwrap_or(&self.refs.head)
                    ))
                    .color(t.warning),
                );
            }
        }
        if let Some(status) = self.ci_status(&self.refs.head) {
            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            let id = ui.id().with("head-ci");
            acts.extend(ci_status::dot(
                ui,
                id,
                rect.center(),
                &self.refs.head,
                status,
            ));
        }

        if let Some(state) = self.work.in_progress {
            ui.add_space(10.0);
            widgets::chip(ui, &format!("{} in progress", state.label()), t.warning);
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!("Abort {}", state.label().to_lowercase())),
                )
                .clicked()
            {
                acts.push(abort_action(state));
            }
        }
    }

    /// The last operation's outcome, and a spinner while one runs (right to left).
    fn op_status(&self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        if let Some((msg, is_err)) = &self.status {
            let color = if *is_err { t.danger } else { t.text_dim };
            let first_line = headline(msg);
            ui.add(egui::Label::new(egui::RichText::new(first_line).color(color)).truncate())
                .on_hover_text(msg.as_str());
        }
        if let Some(op) = &self.busy_op {
            ui.label(egui::RichText::new(format!("{op}…")).color(t.text_dim));
            ui.spinner();
        } else if self.loading {
            ui.spinner();
        }
    }
}

pub(crate) fn reveal_label() -> &'static str {
    if cfg!(windows) {
        "Show in Explorer"
    } else if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Open folder"
    }
}

fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

/// The line of a git error worth showing in the toolbar: conflicts and `error:`/`fatal:` lines
/// beat progress chatter like "Auto-merging x".
pub(super) fn headline(msg: &str) -> String {
    let (label, body) = msg.split_once(": ").unwrap_or(("", msg));
    let conflicts = body.lines().filter(|l| l.starts_with("CONFLICT")).count();
    if conflicts > 0 {
        let files = if conflicts == 1 { "file" } else { "files" };
        return format!("{label}: conflicts in {conflicts} {files}, resolve them in Changes");
    }
    let line = body
        .lines()
        .find(|l| l.starts_with("error:") || l.starts_with("fatal:"))
        .or_else(|| body.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or_default();
    if label.is_empty() {
        line.to_owned()
    } else {
        format!("{label}: {line}")
    }
}
