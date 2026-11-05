//! Under a diff of the pull request: the threads on a line and the comment being written on it,
//! and the marks in the diff's gutter that open them.

use egui::{Align2, RichText, Ui, pos2};

use crate::git;
use crate::icons;
use crate::pr_post::Suggestion;
use crate::pr_threads::{Thread, ThreadComment};
use crate::pull_request::DiffSide;
use crate::repo::Action;
use crate::repo::pr_view::{self, PrView};
use crate::theme::{self, Tokens};

use super::lines::{suggestion_applies, suggestion_scope, thread_is_on};
use super::{LineTarget, Place, PrAct, PrDraft, error_label, pr_acts};

const THREAD_MAX_H: f32 = 260.0;

/// The panel under a diff: the threads on the line, and the comment being written on it.
/// `current` is the line as it is in the pull request, to suggest a change to.
pub(in crate::repo) fn line_panel(
    ui: &mut Ui,
    view: &mut PrView,
    draft: &mut PrDraft,
    current: Option<&str>,
    acts: &mut Vec<Action>,
) {
    let t = Tokens::get(ui.ctx());
    let Some(target) = draft.target.clone() else {
        return;
    };
    let which = match target.side {
        DiffSide::Left => " (as it was)",
        DiffSide::Right => "",
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{}:{}{which}", target.path, target.line))
                .font(theme::mono(11.5))
                .color(t.text_dim),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                pr_acts(acts, PrAct::CloseLine);
            }
        });
    });
    let threads = threads(ui, view, draft, &target, acts);
    ui.add_space(4.0);
    // A thread on the line takes the reply; on a line without one, a new one starts.
    let hint = if threads.reply_to.is_some() {
        "Reply to this thread"
    } else {
        "Comment on this line"
    };
    let field = ui.add_enabled(
        !draft.posting,
        egui::TextEdit::multiline(&mut draft.line_text)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .desired_rows(2),
    );
    if std::mem::take(&mut draft.focus_line) {
        field.request_focus();
    }
    error_label(ui, draft.error_at(Place::Line));
    ui.horizontal(|ui| line_buttons(ui, draft, &threads, current, acts));
}

/// What the comment being written on a line can do: go to its thread, or start one.
fn line_buttons(
    ui: &mut Ui,
    draft: &mut PrDraft,
    threads: &ThreadsShown,
    current: Option<&str>,
    acts: &mut Vec<Action>,
) {
    let can_send = !draft.line_text.trim().is_empty() && !draft.posting;
    match &threads.reply_to {
        Some(thread) => {
            if ui
                .add_enabled(can_send, egui::Button::new("Reply"))
                .clicked()
            {
                pr_acts(acts, PrAct::Reply(thread.clone()));
            }
        }
        None => {
            // A review started now couldn't be sent from a page without a review box.
            if (threads.open || threads.pending_exists)
                && ui
                    .add_enabled(can_send, egui::Button::new("Add to review"))
                    .on_hover_text("Keep it in a pending review, to send with the others")
                    .clicked()
            {
                pr_acts(acts, PrAct::AddPending);
            }
            let now = can_send && !threads.pending_exists;
            let comment_now = ui.add_enabled(now, egui::Button::new("Comment now"));
            if comment_now.clicked() {
                pr_acts(acts, PrAct::PostLineNow);
            }
            if threads.pending_exists {
                comment_now.on_disabled_hover_text("You have a pending review: add it to that one");
            }
        }
    }
    if let Some(line) = current.filter(|_| threads.open)
        && ui
            .button("Suggest a change")
            .on_hover_text("Start from the line as it is, for them to apply with a click")
            .clicked()
    {
        if !draft.line_text.is_empty() && !draft.line_text.ends_with('\n') {
            draft.line_text.push('\n');
        }
        draft
            .line_text
            .push_str(&format!("```suggestion\n{line}\n```\n"));
        draft.focus_line = true;
    }
    if draft.posting {
        ui.spinner();
    }
}

struct ThreadsShown {
    /// The thread a reply goes to.
    reply_to: Option<String>,
    pending_exists: bool,
    /// The pull request is still going.
    open: bool,
}

/// The threads on `target`.
fn threads(
    ui: &mut Ui,
    view: &mut PrView,
    draft: &PrDraft,
    target: &LineTarget,
    acts: &mut Vec<Action>,
) -> ThreadsShown {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width() - 16.0;
    let mut shown = ThreadsShown {
        reply_to: None,
        pending_exists: false,
        open: false,
    };
    let Some(Ok(details)) = &view.details else {
        return shown;
    };
    let extras = &details.extras;
    shown.pending_exists = extras.pending_review.is_some();
    shown.open = details.state == "OPEN";
    let open = shown.open;
    let markdown = &mut view.markdown;
    let bodies = &view.thread_bodies;
    let suggestions = &view.suggestions;
    egui::ScrollArea::vertical()
        .id_salt("pr-line-thread")
        .max_height(THREAD_MAX_H)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for thread in extras
                .threads
                .iter()
                .filter(|th| thread_is_on(th, &target.path, target.side, target.line))
            {
                if thread.can_reply && shown.reply_to.is_none() {
                    shown.reply_to = Some(thread.id.clone());
                }
                for comment in &thread.comments {
                    comment_header(ui, comment);
                    if let Some(body) = bodies.get(&comment.id) {
                        pr_view::blocks(ui, markdown, &comment.id, body, width);
                    }
                    let suggestion = suggestions
                        .get(&comment.id)
                        .filter(|_| suggestion_applies(thread, comment.pending, open));
                    if let Some(text) = suggestion
                        && extras.can_write()
                        && let Some(line) = thread.line
                    {
                        apply_suggestion(ui, thread, comment, text, line, draft, acts);
                    }
                    ui.add_space(6.0);
                }
                if thread.more_comments {
                    ui.label(
                        RichText::new("This thread goes on: the rest is on GitHub.")
                            .color(t.text_faint)
                            .small(),
                    );
                }
                ui.horizontal(|ui| resolve_buttons(ui, thread, draft, acts));
                ui.add_space(6.0);
            }
        });
    shown
}

/// Who wrote a comment of a thread, and when.
fn comment_header(ui: &mut Ui, comment: &ThreadComment) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(RichText::new(&comment.author).font(theme::medium(12.0)));
        ui.label(
            RichText::new(git::relative_time(comment.at, git::now()))
                .color(t.text_faint)
                .small(),
        );
        if comment.pending {
            ui.label(RichText::new("pending").color(t.accent).small());
        }
    });
}

/// The button that commits the suggestion `text` of `comment`, which ends at `line`.
fn apply_suggestion(
    ui: &mut Ui,
    thread: &Thread,
    comment: &ThreadComment,
    text: &str,
    line: u64,
    draft: &PrDraft,
    acts: &mut Vec<Action>,
) {
    let t = Tokens::get(ui.ctx());
    let start = thread.start_line.unwrap_or(line);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!draft.posting, egui::Button::new("Apply suggestion"))
            .on_hover_text("Commit this change to the pull request's branch")
            .clicked()
        {
            pr_acts(
                acts,
                PrAct::ApplySuggestion {
                    thread: thread.id.clone(),
                    suggestion: Suggestion {
                        path: thread.path.clone(),
                        start,
                        end: line,
                        text: text.to_owned(),
                        author: comment.author.clone(),
                    },
                },
            );
        }
        ui.label(
            RichText::new(suggestion_scope(start, line))
                .color(t.text_faint)
                .small(),
        );
    });
}

/// Whether the thread is resolved, and the buttons that change it.
fn resolve_buttons(ui: &mut Ui, thread: &Thread, draft: &PrDraft, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    if thread.resolved {
        ui.label(RichText::new("Resolved").color(t.success).small());
    }
    if thread.can_resolve
        && ui
            .add_enabled(!draft.posting, egui::Button::new("Resolve"))
            .clicked()
    {
        pr_acts(acts, PrAct::SetResolved(thread.id.clone(), true));
    }
    if thread.can_unresolve
        && ui
            .add_enabled(!draft.posting, egui::Button::new("Open again"))
            .clicked()
    {
        pr_acts(acts, PrAct::SetResolved(thread.id.clone(), false));
    }
}

/// A diff line's gutter: how many comments it has, or a `+` while the pointer is on it.
pub(in crate::repo) fn paint_gutter_mark(
    ui: &Ui,
    rect: egui::Rect,
    count: Option<usize>,
    hovered: bool,
    active: bool,
) {
    let t = Tokens::get(ui.ctx());
    let center = pos2(rect.left() + 9.0, rect.center().y);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if let Some(count) = count {
        painter.circle_filled(center, 7.0, t.accent);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            count.to_string(),
            theme::medium(9.5),
            t.accent_text,
        );
    } else if hovered || active {
        painter.circle_filled(center, 7.0, t.accent);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            "+",
            theme::semibold(12.0),
            t.accent_text,
        );
    }
}
