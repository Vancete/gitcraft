//! The pull (or merge) requests of the repository's forge.

use egui::{Align2, Rect, Sense, Ui, pos2, vec2};

use super::remote_short;
use crate::forge::{Forge, PullRequest};
use crate::icons;
use crate::repo::pinned_sections::{Header, Sections};
use crate::repo::{Action, RepoTab, ci_status};
use crate::theme::Tokens;

impl RepoTab {
    pub(super) fn pr_section(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        idle: bool,
        sections: &mut Sections,
    ) {
        let Some(forge) = self.forge else { return };
        let t = Tokens::get(ui.ctx());
        let title = match forge {
            Forge::GitHub => "PULL REQUESTS",
            Forge::GitLab => "MERGE REQUESTS",
        };
        let open = sections.add(
            ui,
            Header {
                key: "prs",
                icon: "git-pull-request",
                title,
                count: self.prs.len(),
                default_open: true,
                button: None,
            },
        );
        if !open {
            return;
        }
        if let Some(err) = &self.pr_error {
            let (rect, r) =
                ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
            ui.painter_at(rect).text(
                pos2(rect.left() + 34.0, rect.center().y),
                Align2::LEFT_CENTER,
                err.lines().next().unwrap_or_default(),
                egui::FontId::proportional(11.0),
                t.text_faint,
            );
            r.on_hover_text(err.as_str());
            return;
        }
        for pr in &self.prs {
            self.pr_row(ui, acts, forge, pr, idle);
        }
    }

    fn pr_row(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        forge: Forge,
        pr: &PullRequest,
        idle: bool,
    ) {
        let t = Tokens::get(ui.ctx());
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        let shown = self.pr_view.as_ref().is_some_and(|v| v.number == pr.number);
        if shown {
            ui.painter().rect_filled(rect, 0.0, t.row_selected);
        } else if r.hovered() {
            ui.painter().rect_filled(rect, 0.0, t.hover);
        }
        let y = rect.center().y;
        icons::paint(
            ui,
            Rect::from_center_size(pos2(rect.left() + 34.0, y), vec2(16.0, 16.0)),
            "git-pull-request",
            13.0,
            if pr.draft { t.text_faint } else { t.success },
        );
        let ci = self.ci_status(&pr.sha);
        // The build status on the right, and the texts up to it.
        let text_right = if ci.is_some() {
            rect.right() - 24.0
        } else {
            rect.right()
        };
        let p = ui.painter_at(rect).with_clip_rect(Rect::from_min_max(
            rect.min,
            pos2(text_right, rect.bottom()),
        ));
        p.text(
            pos2(rect.left() + 48.0, y - 7.0),
            Align2::LEFT_CENTER,
            format!("#{} {}", pr.number, pr.title),
            egui::FontId::proportional(12.5),
            t.text,
        );
        let sub = format!(
            "{} · {}{}",
            pr.branch,
            pr.author,
            if pr.draft { " · draft" } else { "" }
        );
        p.text(
            pos2(rect.left() + 48.0, y + 8.0),
            Align2::LEFT_CENTER,
            sub,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
        if let Some(status) = ci {
            let center = pos2(rect.right() - 14.0, y);
            acts.extend(ci_status::dot(ui, r.id.with("ci"), center, status));
        }
        let has_view = forge == Forge::GitHub;
        let r = r.on_hover_text(if has_view {
            format!("{}\nClick to open, double-click for the browser", pr.url)
        } else {
            format!("{}\nDouble-click to open in the browser", pr.url)
        });
        if r.double_clicked() {
            acts.push(Action::OpenUrl(pr.url.clone()));
        } else if r.clicked() && has_view {
            acts.push(Action::OpenPr(pr.number));
        } else if r.clicked()
            && let Some(target) = self.pr_target(pr)
        {
            acts.push(Action::SelectHash(target));
        }
        egui::Popup::context_menu(&r)
            .style(crate::theme::menu_style)
            .show(|ui| {
                if ui.button("Open in the browser").clicked() {
                    acts.push(Action::OpenUrl(pr.url.clone()));
                }
                if ui
                    .add_enabled(idle, egui::Button::new(format!("Check out {}", pr.branch)))
                    .clicked()
                {
                    if self.refs.local.iter().any(|b| b.name == pr.branch) {
                        acts.push(Action::Checkout(pr.branch.clone()));
                    } else {
                        let remote = self.default_remote().unwrap_or_else(|| "origin".to_owned());
                        acts.push(Action::CheckoutRemote(format!("{remote}/{}", pr.branch)));
                    }
                }
                if ui.button("Copy URL").clicked() {
                    ui.ctx().copy_text(pr.url.clone());
                }
            });
    }

    /// The tip of `pr`'s branch, local or on a remote.
    fn pr_target(&self, pr: &PullRequest) -> Option<String> {
        self.refs
            .local
            .iter()
            .find(|b| b.name == pr.branch)
            .or_else(|| {
                self.refs
                    .remotes
                    .iter()
                    .flat_map(|(_, l)| l.iter())
                    .find(|b| {
                        self.remotes
                            .iter()
                            .filter_map(|r| remote_short(&b.name, &r.name))
                            .any(|n| n == pr.branch)
                    })
            })
            .map(|b| b.target.clone())
    }
}
