//! The comments and reviews of the pull request, one card each.

use egui::{RichText, Sense, Ui, vec2};
use egui_commonmark::CommonMarkCache;

use crate::git;
use crate::icons;
use crate::pull_request::{self as pr, Block, DiffSide, Entry, EntryKind, PrDetails, PrFiles};
use crate::repo::pr_review::{LineTarget, Place, PrAct, PrDraft};
use crate::repo::{Action, DiffKey, widgets};
use crate::theme::{self, Tokens};

use super::blocks;

#[allow(clippy::too_many_arguments)]
pub(super) fn timeline(
    ui: &mut Ui,
    d: &PrDetails,
    files: Option<&PrFiles>,
    bodies: &[Vec<Block>],
    cache: &mut CommonMarkCache,
    draft: &mut PrDraft,
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
                entry_header(ui, entry, d, files, acts);
                if !body.is_empty() {
                    ui.add_space(6.0);
                    blocks(ui, cache, &format!("entry-{i}"), body, inner);
                }
                dismiss_box(ui, entry, draft, acts);
            });
        ui.add_space(8.0);
    }
}

/// Asks why a review gets dismissed, for the review it is being done to.
fn dismiss_box(ui: &mut Ui, entry: &Entry, draft: &mut PrDraft, acts: &mut Vec<Action>) {
    if entry.review_id.is_none() || entry.review_id != draft.dismissing {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    ui.add_enabled(
        !draft.posting,
        egui::TextEdit::singleline(&mut draft.dismiss_text)
            .hint_text("Why is this review dismissed?")
            .desired_width(f32::INFINITY),
    );
    if let Some(error) = draft.error_at(Place::Dismiss) {
        ui.colored_label(t.danger, error);
    }
    ui.horizontal(|ui| {
        let can_send = !draft.dismiss_text.trim().is_empty() && !draft.posting;
        if ui
            .add_enabled(can_send, egui::Button::new("Dismiss review"))
            .clicked()
        {
            acts.push(Action::Pr(PrAct::Dismiss));
        }
        if ui.button("Cancel").clicked() {
            acts.push(Action::Pr(PrAct::CancelDismiss));
        }
    });
}

fn entry_header(
    ui: &mut Ui,
    entry: &Entry,
    d: &PrDetails,
    files: Option<&PrFiles>,
    acts: &mut Vec<Action>,
) {
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
        if let EntryKind::Inline {
            path,
            line,
            outdated,
            resolved,
            side,
        } = &entry.kind
        {
            if let (false, Some(number), Some(files)) = (*outdated, line, files)
                && icons::small_button(ui, "file-diff", "Show it in the diff").clicked()
            {
                show_in_diff(files, path, *number, *side, acts);
            }
            ui.label(
                RichText::new(inline_place(path, *line, *outdated, *resolved))
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
        if let Some(review) = &entry.review_id
            && d.state == "OPEN"
            && d.extras.can_write()
            && ui.small_button("Dismiss").clicked()
        {
            acts.push(Action::Pr(PrAct::StartDismiss(review.clone())));
        }
    });
}

/// Where a comment on a line is, and what has become of its thread.
fn inline_place(path: &str, line: Option<u64>, outdated: bool, resolved: bool) -> String {
    let place = match line {
        Some(line) => format!("on {path}:{line}"),
        None => format!("on {path}"),
    };
    match (outdated, resolved) {
        (true, true) => format!("{place} (outdated, resolved)"),
        (true, false) => format!("{place} (outdated)"),
        (false, true) => format!("{place} (resolved)"),
        (false, false) => place,
    }
}

/// Opens the file of a comment in the diff, with the thread of its line.
fn show_in_diff(files: &PrFiles, path: &str, line: u64, side: DiffSide, acts: &mut Vec<Action>) {
    acts.push(Action::OpenDiff(DiffKey::Range {
        from: files.from.clone(),
        to: Some(files.to.clone()),
        path: path.to_owned(),
    }));
    acts.push(Action::Pr(PrAct::OpenLine(LineTarget {
        path: path.to_owned(),
        line,
        side,
    })));
}
