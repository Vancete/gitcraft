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

/// How far out of view a comment card is still drawn.
const CARD_MARGIN: f32 = 200.0;

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
    // With a screen reader on, every card is drawn: one stepped over would be missing from
    // what it reads. (Asking for the accessibility node of this `Ui`, which exists already,
    // answers whether accessibility is on without adding anything.)
    let reader = ui.ctx().accesskit_node_builder(ui.id(), |_| ()).is_some();
    for (i, (entry, body)) in d.timeline.iter().zip(bodies).enumerate() {
        // A long discussion is hundreds of cards of formatted text: one out of sight is only
        // stepped over, by its height when last drawn (at this width, with this text, with or
        // without the changed files' button, with or without the box to dismiss it). Each card has ids of its own, so stepping over
        // one changes nothing in the others. A margin around the view keeps a card that has
        // grown a little since from being stepped over while it shows.
        let height_id = ui.id().with(("pr-card-height", i));
        let dismissing = entry.review_id.is_some() && entry.review_id == draft.dismissing;
        let text = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            entry.body.hash(&mut hasher);
            hasher.finish()
        };
        let fits = (width, text, files.is_some(), dismissing);
        let known = ui
            .data(|data| data.get_temp::<((f32, u64, bool, bool), f32)>(height_id))
            .filter(|(measured, _)| *measured == fits)
            .map(|(_, h)| h);
        let unseen = known.filter(|&h| {
            if reader {
                return false;
            }
            let card = egui::Rect::from_min_size(ui.cursor().min, vec2(width, h));
            !ui.clip_rect().expand(CARD_MARGIN).intersects(card)
        });
        if let Some(h) = unseen {
            ui.push_id(("pr-card", i), |ui| ui.allocate_space(vec2(width, h)));
        } else {
            let card = ui.push_id(("pr-card", i), |ui| {
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
            });
            let h = card.response.rect.height();
            ui.data_mut(|data| data.insert_temp(height_id, (fits, h)));
        }
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
