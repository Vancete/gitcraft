//! A pull request in the big area: its description, build status, comments and commits. The
//! right pane lists the files it changes, which open in the normal diff.

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::widgets;
use super::{Action, DiffKey, Msg, RepoTab, short, spawn};
use crate::git;
use crate::icons;
use crate::pull_request::{self as pr, Block, CheckState, Entry, EntryKind, PrDetails, PrFiles};
use crate::theme::{self, Tokens};

const CONTENT_W: f32 = 900.0;
const MARGIN_X: f32 = 24.0;
const TABLE_MARGIN: f32 = 8.0;

pub(crate) struct PrView {
    pub number: u64,
    request: u64,
    loading: bool,
    details: Option<Result<PrDetails, String>>,
    /// The description and what the timeline says, made safe to show.
    body: Vec<Block>,
    entries: Vec<Vec<Block>>,
    files: Option<Result<PrFiles, String>>,
    markdown: CommonMarkCache,
}

impl RepoTab {
    /// Shows the pull request numbered `number` in the big area.
    pub(super) fn open_pr(&mut self, number: u64, ctx: &egui::Context) {
        self.diff = None;
        self.file_view = None;
        self.compare = None;
        self.pr_view = Some(PrView {
            number,
            request: 0,
            loading: true,
            details: None,
            body: Vec::new(),
            entries: Vec::new(),
            files: None,
            markdown: CommonMarkCache::default(),
        });
        self.load_pr(ctx);
    }

    /// Reads the pull request shown (again). What is shown stays until the answer comes.
    pub(super) fn load_pr(&mut self, ctx: &egui::Context) {
        self.pr_requests += 1;
        let request = self.pr_requests;
        let Some(view) = &mut self.pr_view else {
            return;
        };
        view.request = request;
        view.loading = true;
        let (path, number) = (self.path.clone(), view.number);
        spawn(&self.tx, ctx, move || {
            Msg::PrDetails(request, Box::new(pr::details(&path, number)))
        });
    }

    pub(super) fn pr_details_arrived(
        &mut self,
        request: u64,
        result: Result<PrDetails, String>,
        ctx: &egui::Context,
    ) {
        let Some(view) = self.pr_view.as_mut().filter(|v| v.request == request) else {
            return;
        };
        view.loading = false;
        if let Ok(details) = &result {
            let host = pr::repo_of_url(&details.url).map_or(String::new(), |(host, _)| host);
            let prepare = |text: &str| pr::split_image_tables(&pr::safe_markdown(text, &host));
            view.body = prepare(&details.body);
            view.entries = details
                .timeline
                .iter()
                .map(|entry| prepare(&entry.body))
                .collect();
            let details = details.clone();
            let (path, remotes, credentials) = (
                self.path.clone(),
                self.remotes.clone(),
                self.credentials.clone(),
            );
            spawn(&self.tx, ctx, move || {
                let files = pr::files(&path, &details, &remotes, credentials.as_ref());
                Msg::PrFiles(request, files)
            });
        }
        view.details = Some(result);
    }

    pub(super) fn pr_files_arrived(&mut self, request: u64, result: Result<PrFiles, String>) {
        if let Some(view) = self.pr_view.as_mut().filter(|v| v.request == request) {
            view.files = Some(result);
        }
    }

    /// What a pull request calls a commit of a diff opened from it.
    pub(super) fn pr_name_of(&self, rev: &str) -> Option<String> {
        let view = self.pr_view.as_ref()?;
        let details = view.details.as_ref()?.as_ref().ok()?;
        let files = view.files.as_ref()?.as_ref().ok()?;
        if files.to == rev {
            Some(details.head_branch.clone())
        } else if files.from == rev {
            Some(format!("{} (merge base)", details.base_branch))
        } else {
            None
        }
    }

    /// The big area.
    pub(super) fn pr_view_ui(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &mut self.pr_view else {
            return;
        };
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
        match &view.details {
            None => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.colored_label(t.danger, e));
            }
            Some(Ok(details)) => {
                // Text wraps to this; only what can't (a table of images) scrolls sideways.
                let width = (ui.available_width() - 2.0 * MARGIN_X).min(CONTENT_W);
                egui::ScrollArea::both()
                    .id_salt(("pr-view", view.number))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(MARGIN_X as i8, 18))
                            .show(ui, |ui| {
                                ui.set_max_width(width);
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
                                section(ui, &format!("COMMENTS ({})", details.timeline.len()));
                                timeline(
                                    ui,
                                    details,
                                    &view.entries,
                                    &mut view.markdown,
                                    width,
                                    acts,
                                );
                                section(ui, &format!("COMMITS ({})", details.commits.len()));
                                commits(ui, details);
                            });
                    });
            }
        }
        // Text from strangers may link anywhere: a click only ever opens a web page.
        ui.ctx().output_mut(|o| {
            o.commands.retain(
                |c| !matches!(c, egui::OutputCommand::OpenUrl(open) if !pr::is_web_url(&open.url)),
            );
        });
    }

    /// The right pane: the files the pull request changes.
    pub(super) fn pr_files_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        widgets::panel_header(ui, "Changed files", |_| {});
        let Some(view) = &self.pr_view else {
            return;
        };
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
                super::detail::files_header(ui, &files.files, None);
                egui::ScrollArea::vertical()
                    .id_salt("pr-files")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.file_list(ui, acts, &files.files, Some(&files.to), |path| {
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

/// Text of the pull request, wrapped to `width`.
fn blocks(ui: &mut Ui, cache: &mut CommonMarkCache, id: &str, blocks: &[Block], width: f32) {
    ui.push_id(id, |ui| {
        for (i, block) in blocks.iter().enumerate() {
            ui.push_id(i, |ui| match block {
                Block::Markdown(text) => markdown(ui, cache, text, width),
                Block::ImageTable { header, rows } => image_table(ui, cache, header, rows, width),
            });
        }
    });
}

fn markdown(ui: &mut Ui, cache: &mut CommonMarkCache, text: &str, width: f32) {
    ui.set_max_width(width);
    // The viewer wraps text to at least this, so it can't be more than there is room for.
    CommonMarkViewer::new()
        .max_image_width(Some(width as usize))
        .show(ui, cache, text);
}

/// A table with images, its columns sharing `width` evenly.
fn image_table(
    ui: &mut Ui,
    cache: &mut CommonMarkCache,
    header: &[String],
    rows: &[Vec<String>],
    width: f32,
) {
    let t = Tokens::get(ui.ctx());
    let columns = rows
        .iter()
        .map(Vec::len)
        .fold(header.len(), usize::max)
        .max(1);
    let inner = width - 2.0 * TABLE_MARGIN;
    let gap = ui.spacing().item_spacing.x;
    let column_width = (inner - gap * (columns - 1) as f32) / columns as f32;
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, t.separator))
        .corner_radius(t.radius)
        .inner_margin(TABLE_MARGIN)
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.columns(columns, |cells| {
                for (cell, title) in cells.iter_mut().zip(header) {
                    cell.label(RichText::new(title).font(theme::semibold(12.0)));
                }
            });
            for (r, row) in rows.iter().enumerate() {
                ui.add_space(6.0);
                ui.columns(columns, |cells| {
                    for (c, (cell, text)) in cells.iter_mut().zip(row).enumerate() {
                        cell.push_id((r, c), |ui| markdown(ui, cache, text, column_width));
                    }
                });
            }
        });
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
        ui.label(RichText::new("wants to merge").color(t.text_dim));
        ui.label(RichText::new(&d.head_branch).font(theme::mono(11.5)));
        ui.label(RichText::new("into").color(t.text_dim));
        ui.label(RichText::new(&d.base_branch).font(theme::mono(11.5)));
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
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
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        match d.review_decision.as_str() {
            "APPROVED" => widgets::chip(ui, "Approved", t.success),
            "CHANGES_REQUESTED" => widgets::chip(ui, "Changes requested", t.danger),
            "REVIEW_REQUIRED" => widgets::chip(ui, "Review required", t.warning),
            _ => widgets::chip(ui, "No reviews yet", t.text_dim),
        };
        match d.mergeable.as_str() {
            "MERGEABLE" => widgets::chip(ui, "No conflicts", t.success),
            "CONFLICTING" => widgets::chip(ui, "Has conflicts", t.danger),
            _ => widgets::chip(ui, "Checking for conflicts", t.text_dim),
        };
        if d.merge_state == "BEHIND" {
            widgets::chip(ui, "Behind its base", t.warning);
        }
        for label in &d.labels {
            widgets::chip(ui, label, t.accent);
        }
    });
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

fn timeline(
    ui: &mut Ui,
    d: &PrDetails,
    bodies: &[Vec<Block>],
    cache: &mut CommonMarkCache,
    width: f32,
    acts: &mut Vec<Action>,
) {
    let t = Tokens::get(ui.ctx());
    if let Some(e) = &d.inline_error {
        ui.colored_label(
            t.warning,
            format!("Comments on lines couldn't be read: {e}"),
        );
    }
    if d.timeline.is_empty() {
        ui.label(
            RichText::new("No comments yet.")
                .color(t.text_faint)
                .italics(),
        );
        return;
    }
    for (i, (entry, body)) in d.timeline.iter().zip(bodies).enumerate() {
        egui::Frame::new()
            .fill(t.card)
            .corner_radius(t.radius)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                let inner = width - 24.0;
                ui.set_width(inner);
                entry_header(ui, entry, acts);
                if !body.is_empty() {
                    ui.add_space(6.0);
                    blocks(ui, cache, &format!("entry-{i}"), body, inner);
                }
            });
        ui.add_space(8.0);
    }
}

fn entry_header(ui: &mut Ui, entry: &Entry, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        widgets::avatar(ui, rect.center(), 10.0, &entry.author, "");
        ui.label(RichText::new(&entry.author).font(theme::medium(12.5)));
        let verb = match &entry.kind {
            EntryKind::Comment | EntryKind::Inline { .. } => "commented",
            EntryKind::Review(verdict) => verdict,
        };
        ui.label(RichText::new(verb).color(t.text_dim));
        if let EntryKind::Inline { path, line } = &entry.kind {
            let place = match line {
                Some(line) => format!("on {path}:{line}"),
                None => format!("on {path}"),
            };
            ui.label(
                RichText::new(place)
                    .font(theme::mono(11.0))
                    .color(t.text_dim),
            );
        }
        ui.label(
            RichText::new(git::relative_time(entry.at, git::now()))
                .color(t.text_faint)
                .small(),
        );
        if pr::is_web_url(&entry.url)
            && icons::small_button(ui, "external-link", "Open in the browser").clicked()
        {
            acts.push(Action::OpenUrl(entry.url.clone()));
        }
    });
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
