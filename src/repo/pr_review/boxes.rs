//! The boxes a comment on the page or a review is written in.

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};

use crate::icons;
use crate::pr_post::{self, Verdict};
use crate::pr_threads::PrExtras;
use crate::repo::Action;
use crate::theme::{self, Tokens};

use super::{Place, PrAct, PrDraft, error_label, pr_acts};

/// The box on the pull request's page.
pub(in crate::repo) fn comment_box(ui: &mut Ui, draft: &mut PrDraft, acts: &mut Vec<Action>) {
    ui.add_enabled(
        !draft.posting,
        egui::TextEdit::multiline(&mut draft.comment)
            .hint_text("Leave a comment (Markdown works)")
            .desired_width(f32::INFINITY)
            .desired_rows(3),
    );
    ui.add_space(4.0);
    error_label(ui, draft.error_at(Place::Comment));
    let can_send = !draft.comment.trim().is_empty() && !draft.posting;
    if ui
        .add_enabled(can_send, egui::Button::new("Comment"))
        .clicked()
    {
        pr_acts(acts, PrAct::PostComment);
    }
}

/// The box under the files: the review being written.
pub(in crate::repo) fn review_box(
    ui: &mut Ui,
    draft: &mut PrDraft,
    extras: &PrExtras,
    own: bool,
    open: bool,
    acts: &mut Vec<Action>,
) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.label(
        RichText::new("REVIEW")
            .font(theme::semibold(10.5))
            .color(t.text_faint),
    );
    pending_list(ui, extras, acts);
    ui.add_enabled(
        !draft.posting,
        egui::TextEdit::multiline(&mut draft.review)
            .hint_text("What do you think of it as a whole?")
            .desired_width(f32::INFINITY)
            .desired_rows(2),
    );
    error_label(ui, draft.error_at(Place::Review));
    if own && open {
        ui.label(
            RichText::new("This one is yours: you can comment, but not approve it.")
                .color(t.text_faint)
                .small(),
        );
    }
    ui.horizontal_wrapped(|ui| verdict_buttons(ui, draft, extras, own, open, acts));
}

/// A button for each verdict the review can go out with, and one to discard the pending one.
fn verdict_buttons(
    ui: &mut Ui,
    draft: &PrDraft,
    extras: &PrExtras,
    own: bool,
    open: bool,
    acts: &mut Vec<Action>,
) {
    let count = extras.pending_comments().count();
    for (verdict, label) in [
        (Verdict::Comment, "Comment"),
        (Verdict::Approve, "Approve"),
        (Verdict::RequestChanges, "Request changes"),
    ] {
        // Whatever is pending can still go out, but a verdict comes too late.
        if !open && verdict != Verdict::Comment {
            continue;
        }
        let problem = pr_post::review_problem(verdict, &draft.review, count);
        let reviews_own = own && verdict != Verdict::Comment;
        let enabled = problem.is_none() && !reviews_own && !draft.posting;
        let button = ui.add_enabled(enabled, egui::Button::new(label));
        if button.clicked() {
            pr_acts(acts, PrAct::Submit(verdict));
        }
        if let Some(problem) = problem {
            button.on_disabled_hover_text(problem);
        }
    }
    if extras.pending_review.is_some()
        && ui
            .add_enabled(!draft.posting, egui::Button::new("Discard"))
            .on_hover_text("Throw away the pending review with its comments")
            .clicked()
    {
        pr_acts(acts, PrAct::DiscardPending);
    }
    if draft.posting {
        ui.spinner();
    }
}

fn pending_list(ui: &mut Ui, extras: &PrExtras, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let waiting: Vec<_> = extras.pending_comments().collect();
    if waiting.is_empty() {
        return;
    }
    ui.label(
        RichText::new(format!(
            "{} comment{} only you can see until you send this:",
            waiting.len(),
            if waiting.len() == 1 { "" } else { "s" }
        ))
        .color(t.accent)
        .small(),
    );
    for (thread, comment) in waiting {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let y = rect.center().y;
        let x = egui::Rect::from_center_size(pos2(rect.right() - 9.0, y), vec2(18.0, 18.0));
        let p = ui.painter_at(rect.with_max_x(x.left() - 4.0));
        let line = thread.line.map_or(String::new(), |n| format!(":{n}"));
        p.text(
            pos2(rect.left(), y),
            Align2::LEFT_CENTER,
            format!("{}{line}  {}", thread.path, first_line(&comment.body)),
            egui::FontId::proportional(11.5),
            t.text_dim,
        );
        let remove = ui
            .interact(
                x,
                ui.id().with(("pending-remove", &comment.id)),
                Sense::click(),
            )
            .on_hover_text("Remove from the review");
        if remove.hovered() {
            ui.painter().rect_filled(x, t.radius_sm, t.pressed);
        }
        icons::paint(ui, x, "x", 11.0, t.text_faint);
        if remove.clicked() {
            pr_acts(acts, PrAct::RemovePending(comment.id.clone()));
        }
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}
