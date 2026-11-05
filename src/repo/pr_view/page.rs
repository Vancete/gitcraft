//! The page of the pull request in the big area, and the files it changes in the right pane.

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};

use crate::git;
use crate::icons;
use crate::pull_request::{self as pr, CheckState, PrDetails};
use crate::repo::pr_review::{self, Place, PrDraft};
use crate::repo::{Action, DiffKey, RepoTab, short, widgets};
use crate::theme::{self, Tokens};

use super::timeline::timeline;
use super::{PrView, blocks, only_web_links};

const CONTENT_W: f32 = 900.0;
const MARGIN_X: f32 = 24.0;

impl RepoTab {
    /// The big area.
    pub(in crate::repo) fn pr_view_ui(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &mut self.pr_view else {
            return;
        };
        page_header(ui, view, acts);
        match &view.details {
            None => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.colored_label(t.danger, e));
            }
            Some(Ok(_)) => {
                // Text wraps to this; only what can't (a table of images) scrolls sideways.
                let width = (ui.available_width() - 2.0 * MARGIN_X).min(CONTENT_W);
                let drafts = &mut self.pr_drafts;
                egui::ScrollArea::both()
                    .id_salt(("pr-view", view.number))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(MARGIN_X as i8, 18))
                            .show(ui, |ui| {
                                ui.set_max_width(width);
                                let draft = drafts.entry(view.number).or_default();
                                page(ui, view, draft, width, acts);
                            });
                    });
            }
        }
        only_web_links(ui.ctx());
    }

    /// The right pane: the files the pull request changes.
    pub(in crate::repo) fn pr_files_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        widgets::panel_header(ui, "Changed files", crate::repo::branch_tree::files_toggle);
        let Some(view) = &self.pr_view else {
            return;
        };
        if let Some(Ok(details)) = view
            .details
            .as_ref()
            // What's pending stays to send or discard, whatever has become of the pull request.
            .filter(|d| {
                d.as_ref().is_ok_and(|d| {
                    !d.extras.pr_id.is_empty()
                        && (d.state == "OPEN" || d.extras.pending_review.is_some())
                })
            })
        {
            let own = crate::pr_post::is_own(&details.author, details.extras.viewer.as_deref());
            let draft = self.pr_drafts.entry(view.number).or_default();
            egui::Panel::bottom("review_box")
                .frame(
                    egui::Frame::new()
                        .fill(t.card)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .stroke(egui::Stroke::new(1.0, t.separator)),
                )
                .show(ui, |ui| {
                    let open = details.state == "OPEN";
                    pr_review::review_box(ui, draft, &details.extras, own, open, acts);
                });
        }
        match &view.files {
            None => {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.colored_label(t.danger, format!("Couldn't read the changes: {e}"));
                    });
            }
            Some(Ok(files)) => {
                crate::repo::detail::files_header(ui, &files.files, None);
                egui::ScrollArea::vertical()
                    .id_salt("pr-files")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.file_list(ui, acts, "pr", &files.files, Some(&files.to), |path| {
                            DiffKey::Range {
                                from: files.from.clone(),
                                to: Some(files.to.clone()),
                                path: path.to_owned(),
                            }
                        });
                    });
            }
        }
    }
}

/// The title bar of the page: its number, and the buttons to leave, open or load it again.
fn page_header(ui: &mut Ui, view: &PrView, acts: &mut Vec<Action>) {
    let url = match &view.details {
        Some(Ok(details)) => Some(details.url.clone()),
        _ => None,
    };
    let title = format!("Pull request #{}", view.number);
    let loading = view.loading;
    widgets::panel_header(ui, &title, |ui| {
        if icons::small_button(ui, "x", "Back to the graph (Esc)").clicked() {
            acts.push(Action::ClosePr);
        }
        if let Some(url) = url
            && icons::small_button(ui, "external-link", "Open in the browser").clicked()
        {
            acts.push(Action::OpenUrl(url));
        }
        if loading {
            ui.spinner();
        } else if icons::small_button(ui, "refresh-cw", "Load again").clicked() {
            acts.push(Action::ReloadPr);
        }
    });
}

/// What the page shows of a pull request that was read, top to bottom.
fn page(ui: &mut Ui, view: &mut PrView, draft: &mut PrDraft, width: f32, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let Some(Ok(details)) = &view.details else {
        return;
    };
    header(ui, details);
    section(ui, "DESCRIPTION");
    if view.body.is_empty() {
        ui.label(
            RichText::new("No description.")
                .color(t.text_faint)
                .italics(),
        );
    }
    blocks(ui, &mut view.markdown, "body", &view.body, width);
    section(ui, "BUILD STATUS");
    checks(ui, details, acts);
    if details.state == "OPEN" {
        section(ui, "MERGE");
        crate::repo::pr_merge::merge_box(ui, draft, details, acts);
    } else if let Some(error) = draft.error_at(Place::Merge) {
        // The box is gone, but what went wrong with it still counts.
        ui.add_space(8.0);
        ui.colored_label(t.warning, error);
    }
    section(ui, &format!("COMMENTS ({})", details.timeline.len()));
    // Lines are shown in the changes of the commit the page names, not in those of an earlier
    // one still listed until the new ones come.
    let files = view
        .files
        .as_ref()
        .and_then(|f| f.as_ref().ok())
        .filter(|f| f.to == details.head_oid);
    timeline(
        ui,
        details,
        files,
        &view.entries,
        &mut view.markdown,
        draft,
        width,
        acts,
    );
    section(ui, "ADD A COMMENT");
    pr_review::comment_box(ui, draft, acts);
    section(ui, &format!("COMMITS ({})", details.commits.len()));
    commits(ui, details);
}

fn section(ui: &mut Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(22.0);
    ui.label(
        RichText::new(title)
            .font(theme::semibold(10.5))
            .color(t.text_faint),
    );
    ui.add_space(6.0);
}

/// Title, state, who wants what merged where, and how things stand.
fn header(ui: &mut Ui, d: &PrDetails) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&d.title).font(theme::semibold(19.0)));
        ui.label(
            RichText::new(format!("#{}", d.number))
                .font(egui::FontId::proportional(19.0))
                .color(t.text_faint),
        );
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let (state, color) = match (d.state.as_str(), d.draft) {
            ("MERGED", _) => ("Merged", t.accent),
            ("CLOSED", _) => ("Closed", t.danger),
            (_, true) => ("Draft", t.text_dim),
            _ => ("Open", t.success),
        };
        widgets::chip(ui, state, color);
        ui.label(RichText::new(&d.author).font(theme::medium(12.5)));
        let verb = match d.state.as_str() {
            // Whoever opened it isn't necessarily who merged it.
            "MERGED" | "CLOSED" => "proposed merging",
            _ => "wants to merge",
        };
        ui.label(RichText::new(verb).color(t.text_dim));
        ui.label(RichText::new(&d.head_branch).font(theme::mono(11.5)));
        ui.label(RichText::new("into").color(t.text_dim));
        ui.label(RichText::new(&d.base_branch).font(theme::mono(11.5)));
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| stats(ui, d));
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| chips(ui, d));
}

/// When it was opened and updated, and how much it changes.
fn stats(ui: &mut Ui, d: &PrDetails) {
    let t = Tokens::get(ui.ctx());
    let now = git::now();
    ui.label(
        RichText::new(format!(
            "Opened {} · updated {} ·",
            git::relative_time(d.created_at, now),
            git::relative_time(d.updated_at, now)
        ))
        .color(t.text_faint)
        .small(),
    );
    ui.label(
        RichText::new(format!("+{}", d.additions))
            .color(t.success)
            .small(),
    );
    ui.label(
        RichText::new(format!("−{}", d.deletions))
            .color(t.danger)
            .small(),
    );
    ui.label(
        RichText::new(format!(
            "· {} file{}",
            d.changed_files,
            if d.changed_files == 1 { "" } else { "s" }
        ))
        .color(t.text_faint)
        .small(),
    );
}

/// How the reviews and the merge stand, and the labels.
fn chips(ui: &mut Ui, d: &PrDetails) {
    let t = Tokens::get(ui.ctx());
    let label = pr::reviews_label(&d.review_decision, d.state == "OPEN", &d.timeline);
    let color = match label {
        "Approved" => t.success,
        "Changes requested" => t.danger,
        "Review required" => t.warning,
        _ => t.text_dim,
    };
    widgets::chip(ui, label, color);
    // Whether it can be merged only means something while it can still be.
    if d.state == "OPEN" {
        match d.mergeable.as_str() {
            "MERGEABLE" => widgets::chip(ui, "No conflicts", t.success),
            "CONFLICTING" => widgets::chip(ui, "Has conflicts", t.danger),
            _ => widgets::chip(ui, "Checking for conflicts", t.text_dim),
        };
        if d.merge_state == "BEHIND" {
            widgets::chip(ui, "Behind its base", t.warning);
        }
    }
    for label in &d.labels {
        widgets::chip(ui, label, t.accent);
    }
}

fn checks(ui: &mut Ui, d: &PrDetails, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    if d.checks.is_empty() {
        ui.label(
            RichText::new("No checks ran on this pull request.")
                .color(t.text_faint)
                .italics(),
        );
        return;
    }
    let (passed, failed, running, skipped) = pr::tally(&d.checks);
    let summary: Vec<String> = [
        (failed, "failed"),
        (running, "running"),
        (passed, "passed"),
        (skipped, "skipped"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, word)| format!("{n} {word}"))
    .collect();
    ui.label(RichText::new(summary.join(" · ")).color(t.text_dim));
    ui.add_space(4.0);
    // What needs attention first.
    let rank = |state: CheckState| match state {
        CheckState::Failed => 0,
        CheckState::Running => 1,
        CheckState::Passed => 2,
        CheckState::Skipped => 3,
    };
    let mut sorted: Vec<_> = d.checks.iter().collect();
    sorted.sort_by_key(|c| rank(c.state));
    for check in sorted {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        let opens = pr::is_web_url(&check.url);
        if r.hovered() && opens {
            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
        }
        let y = rect.center().y;
        let icon = egui::Rect::from_center_size(pos2(rect.left() + 12.0, y), vec2(14.0, 14.0));
        match check.state {
            CheckState::Passed => icons::paint(ui, icon, "check", 13.0, t.success),
            CheckState::Failed => icons::paint(ui, icon, "x", 13.0, t.danger),
            CheckState::Skipped => icons::paint(ui, icon, "minus", 13.0, t.text_faint),
            CheckState::Running => {
                egui::Spinner::new().size(11.0).paint_at(ui, icon);
            }
        }
        let painter = ui.painter_at(rect);
        let name = painter.text(
            pos2(rect.left() + 26.0, y),
            Align2::LEFT_CENTER,
            &check.name,
            egui::FontId::proportional(12.5),
            t.text,
        );
        painter.text(
            pos2(name.right() + 8.0, y),
            Align2::LEFT_CENTER,
            &check.detail,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
        if opens && r.on_hover_text(&check.url).clicked() {
            acts.push(Action::OpenUrl(check.url.clone()));
        }
    }
}

fn commits(ui: &mut Ui, d: &PrDetails) {
    let t = Tokens::get(ui.ctx());
    for commit in &d.commits {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(short(&commit.oid))
                    .font(theme::mono(11.0))
                    .color(t.text_faint),
            );
            ui.label(&commit.headline);
            ui.label(RichText::new(&commit.author).color(t.text_faint).small());
        });
    }
}
