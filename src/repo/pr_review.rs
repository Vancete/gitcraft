//! Saying something on the pull request shown: a comment on its page, a review from the box under
//! its files, threads on lines from the diff. What is only typed stays as a draft of the tab; a
//! comment added to the review is on GitHub already, in a pending review only its author sees.

use std::collections::HashMap;

use egui::{Align2, RichText, Sense, Ui, pos2, vec2};

use super::diff::Kind;
use super::pr_view::{self, PrView};
use super::{Action, DiffKey, Msg, RepoTab, spawn};
use crate::git;
use crate::icons;
use crate::pr_post::{self, LineComment, MergeMethod, Suggestion, Target, Verdict};
use crate::pr_threads::{PrExtras, Thread};
use crate::pull_request::DiffSide;
use crate::theme::{self, Tokens};

const THREAD_MAX_H: f32 = 260.0;

/// Something to send, on a thread of its own.
type PostJob = Box<dyn FnOnce(&Target) -> Result<(), String> + Send>;

/// A line a comment is being written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineTarget {
    pub path: String,
    pub line: u64,
    pub side: DiffSide,
}

/// Something done on the pull request shown.
#[derive(Clone, Debug)]
pub(crate) enum PrAct {
    PostComment,
    Submit(Verdict),
    OpenLine(LineTarget),
    CloseLine,
    AddPending,
    PostLineNow,
    /// A comment out of the pending review.
    RemovePending(String),
    DiscardPending,
    Reply(String),
    SetResolved(String, bool),
    ApplySuggestion {
        thread: String,
        suggestion: Suggestion,
    },
    StartDismiss(String),
    CancelDismiss,
    Dismiss,
    AskMerge,
    CancelMerge,
    Merge,
}

/// What was being sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Posted {
    Comment,
    Review(Verdict),
    LineNow,
    AddPending,
    RemovePending,
    Discard,
    Reply,
    Resolve(bool),
    Suggestion,
    Dismiss,
    Merge,
}

/// Where an error of something sent shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    Comment,
    Review,
    Line,
    Dismiss,
    Merge,
}

impl Posted {
    fn place(self) -> Place {
        match self {
            Posted::Comment => Place::Comment,
            Posted::Review(_) | Posted::RemovePending | Posted::Discard => Place::Review,
            Posted::LineNow
            | Posted::AddPending
            | Posted::Reply
            | Posted::Resolve(_)
            | Posted::Suggestion => Place::Line,
            Posted::Dismiss => Place::Dismiss,
            Posted::Merge => Place::Merge,
        }
    }

    /// What the notice says once it went out.
    /// Whether a failure can leave the pull request changed or the page behind it.
    fn catches_up_after_failure(self) -> bool {
        matches!(self, Posted::Merge | Posted::Suggestion)
    }

    fn done(self) -> &'static str {
        match self {
            Posted::Comment | Posted::LineNow => "Comment sent",
            Posted::Review(Verdict::Comment) => "Review sent",
            Posted::Review(Verdict::Approve) => "Pull request approved",
            Posted::Review(Verdict::RequestChanges) => "Changes requested",
            Posted::AddPending => "Added to your pending review",
            Posted::RemovePending => "Removed from your pending review",
            Posted::Discard => "Pending review discarded",
            Posted::Reply => "Reply sent",
            Posted::Resolve(true) => "Thread resolved",
            Posted::Resolve(false) => "Thread opened again",
            Posted::Suggestion => "Suggestion committed to the branch",
            Posted::Dismiss => "Review dismissed",
            Posted::Merge => "Pull request merged",
        }
    }
}

/// What has been typed on a pull request and not sent yet.
#[derive(Default)]
pub(crate) struct PrDraft {
    pub(super) comment: String,
    pub(super) review: String,
    pub(super) target: Option<LineTarget>,
    pub(super) line_text: String,
    pub(super) focus_line: bool,
    /// Something is being sent, or the page is being loaded again after it.
    pub(super) posting: bool,
    /// What's left of `posting` is the page being loaded again.
    pub(super) settling: bool,
    pub(super) error: Option<(Place, String)>,
    /// The review being dismissed, and why.
    pub(super) dismissing: Option<String>,
    pub(super) dismiss_text: String,
    pub(super) merge_method: Option<MergeMethod>,
    pub(super) delete_branch: Option<bool>,
    pub(super) confirm_merge: bool,
}

impl PrDraft {
    /// Clears what has been sent. Text not yet sent stays.
    fn sent(&mut self, what: Posted) {
        match what {
            Posted::Comment => self.comment.clear(),
            Posted::Review(_) => self.review.clear(),
            Posted::LineNow | Posted::AddPending => {
                self.line_text.clear();
                self.target = None;
            }
            Posted::Reply => self.line_text.clear(),
            Posted::Dismiss => {
                self.dismissing = None;
                self.dismiss_text.clear();
            }
            Posted::Discard
            | Posted::Merge
            | Posted::RemovePending
            | Posted::Resolve(_)
            | Posted::Suggestion => {}
        }
    }

    pub(super) fn error_at(&self, place: Place) -> Option<&str> {
        self.error
            .as_ref()
            .filter(|(at, _)| *at == place)
            .map(|(_, e)| e.as_str())
    }
}

/// The side and number of a diff line a comment can go on, if it can: GitHub takes them on the
/// lines of the diff, in `column` (the old or new half of the side-by-side view, `None` for the
/// unified one).
pub(super) fn target_of(
    kind: Kind,
    old: Option<u32>,
    new: Option<u32>,
    column: Option<DiffSide>,
) -> Option<(DiffSide, u64)> {
    let side = match (kind, column) {
        (Kind::Add, None | Some(DiffSide::Right)) | (Kind::Context, None) => DiffSide::Right,
        (Kind::Del, None | Some(DiffSide::Left)) => DiffSide::Left,
        (Kind::Context, Some(side)) => side,
        _ => return None,
    };
    let number = match side {
        DiffSide::Left => old,
        DiffSide::Right => new,
    };
    Some((side, u64::from(number?)))
}

/// Whether `thread` is on that line now: an outdated one isn't on any.
fn thread_is_on(thread: &Thread, path: &str, side: DiffSide, line: u64) -> bool {
    !thread.outdated && thread.path == path && thread.side == side && thread.line == Some(line)
}

/// How many comments each line of `path` has, the pending ones too.
pub(super) fn line_marks(threads: &[Thread], path: &str) -> HashMap<(DiffSide, u64), usize> {
    let mut marks = HashMap::new();
    for thread in threads
        .iter()
        .filter(|t| t.path == path && !t.outdated && !t.comments.is_empty())
    {
        if let Some(line) = thread.line {
            *marks.entry((thread.side, line)).or_insert(0) += thread.comments.len();
        }
    }
    marks
}

/// What a diff needs to offer comments: it shows a file of the pull request shown.
pub(super) struct DiffReview {
    pub marks: HashMap<(DiffSide, u64), usize>,
    pub target: Option<LineTarget>,
    pub path: String,
}

impl RepoTab {
    fn pr_draft(&mut self) -> Option<&mut PrDraft> {
        let number = self.pr_view.as_ref()?.number;
        Some(self.pr_drafts.entry(number).or_default())
    }

    /// The review of the diff `key`, if it belongs to the pull request shown.
    pub(super) fn diff_review(&self, key: &DiffKey) -> Option<DiffReview> {
        let view = self.pr_view.as_ref()?;
        let details = view.details.as_ref()?.as_ref().ok()?;
        let files = view.files.as_ref()?.as_ref().ok()?;
        let DiffKey::Range {
            from,
            to: Some(to),
            path,
        } = key
        else {
            return None;
        };
        if *from != files.from || *to != files.to {
            return None;
        }
        Some(DiffReview {
            marks: line_marks(&details.extras.threads, path),
            target: self
                .pr_drafts
                .get(&view.number)
                .and_then(|d| d.target.clone())
                .filter(|t| t.path == *path),
            path: path.clone(),
        })
    }

    /// Whether a thread is open under a diff, which `Esc` then closes first.
    pub(super) fn line_comment_open(&self) -> bool {
        let number = self.pr_view.as_ref().map(|v| v.number);
        number
            .and_then(|n| self.pr_drafts.get(&n))
            .is_some_and(|d| d.target.is_some())
    }

    /// Does what the page, the review box or a thread asked for.
    pub(super) fn pr_act(&mut self, act: PrAct, ctx: &egui::Context) {
        match act {
            PrAct::OpenLine(target) => {
                // The text being sent is only cleared once it has been, so it stays where it is.
                if let Some(draft) = self.pr_draft().filter(|draft| !draft.posting) {
                    draft.target = Some(target);
                    draft.focus_line = true;
                }
            }
            PrAct::CloseLine => {
                if let Some(draft) = self.pr_draft() {
                    draft.target = None;
                }
            }
            PrAct::StartDismiss(review) => {
                if let Some(draft) = self.pr_draft() {
                    draft.dismissing = Some(review);
                }
            }
            PrAct::CancelDismiss => {
                if let Some(draft) = self.pr_draft() {
                    draft.dismissing = None;
                    draft.dismiss_text.clear();
                }
            }
            PrAct::AskMerge => {
                if let Some(draft) = self.pr_draft() {
                    draft.confirm_merge = true;
                }
            }
            PrAct::CancelMerge => {
                if let Some(draft) = self.pr_draft() {
                    draft.confirm_merge = false;
                }
            }
            act => self.pr_send(act, ctx),
        }
    }

    fn pr_send(&mut self, act: PrAct, ctx: &egui::Context) {
        let Some(view) = &self.pr_view else {
            return;
        };
        let Some(Ok(details)) = &view.details else {
            return;
        };
        let Some(target) = Target::of(&self.path, details) else {
            return;
        };
        let number = view.number;
        let can_delete_branch = details.extras.head_repo.as_deref() == Some(target.path());
        let delete_default = details.extras.delete_branch_on_merge;
        let draft = self.pr_drafts.entry(number).or_default();
        let line = draft.target.clone().map(|t| LineComment {
            path: t.path,
            line: t.line,
            side: t.side,
            body: draft.line_text.clone(),
        });
        let (what, job): (Posted, PostJob) = match act {
            PrAct::PostComment => {
                let body = draft.comment.clone();
                (Posted::Comment, Box::new(move |t| t.comment(&body)))
            }
            PrAct::Submit(verdict) => {
                let body = draft.review.clone();
                (
                    Posted::Review(verdict),
                    Box::new(move |t| t.review(verdict, &body)),
                )
            }
            PrAct::AddPending => {
                let Some(line) = line else { return };
                (Posted::AddPending, Box::new(move |t| t.add_pending(&line)))
            }
            PrAct::PostLineNow => {
                let Some(line) = line else { return };
                (Posted::LineNow, Box::new(move |t| t.comment_now(&line)))
            }
            PrAct::RemovePending(comment) => {
                let last = details.extras.pending_comments().count() <= 1;
                (
                    Posted::RemovePending,
                    Box::new(move |t| t.remove_pending(&comment, last)),
                )
            }
            PrAct::DiscardPending => (Posted::Discard, Box::new(|t| t.discard_pending())),
            PrAct::Reply(thread) => {
                let body = draft.line_text.clone();
                (Posted::Reply, Box::new(move |t| t.reply(&thread, &body)))
            }
            PrAct::SetResolved(thread, resolved) => (
                Posted::Resolve(resolved),
                Box::new(move |t| t.set_resolved(&thread, resolved)),
            ),
            PrAct::ApplySuggestion { thread, suggestion } => (
                Posted::Suggestion,
                Box::new(move |t| {
                    t.apply_suggestion(&suggestion)?;
                    t.set_resolved(&thread, true)
                }),
            ),
            PrAct::Dismiss => {
                let Some(review) = draft.dismissing.clone() else {
                    return;
                };
                let message = draft.dismiss_text.clone();
                (
                    Posted::Dismiss,
                    Box::new(move |t| t.dismiss(&review, &message)),
                )
            }
            PrAct::Merge => {
                let method = draft.merge_method.unwrap_or_default();
                let delete = can_delete_branch && draft.delete_branch.unwrap_or(delete_default);
                (Posted::Merge, Box::new(move |t| t.merge(method, delete)))
            }
            _ => return,
        };
        draft.posting = true;
        draft.error = None;
        spawn(&self.tx, ctx, move || Msg::PrPosted {
            number,
            what,
            result: job(&target),
        });
    }

    pub(super) fn pr_posted(
        &mut self,
        number: u64,
        what: Posted,
        result: Result<(), String>,
        ctx: &egui::Context,
    ) {
        let draft = self.pr_drafts.entry(number).or_default();
        draft.posting = false;
        let outcome = match result {
            Ok(()) => {
                draft.sent(what);
                (what.done().to_owned(), false)
            }
            Err(e) => {
                draft.error = Some((what.place(), e.clone()));
                if !what.catches_up_after_failure() {
                    return;
                }
                // Said here too, as the box with the error goes if it was merged after all.
                (e, true)
            }
        };
        self.finished = Some(outcome);
        // Until the page shows the new state, anything sent would go by the old one.
        if self.pr_view.as_ref().is_some_and(|v| v.number == number) {
            draft.posting = true;
            draft.settling = true;
            self.load_pr(ctx);
        }
        if what == Posted::Merge {
            if let Some(draft) = self.pr_drafts.get_mut(&number) {
                draft.confirm_merge = false;
            }
            // The list of open pull requests has changed, and so has the base branch.
            self.load_prs(ctx);
            self.apply(Action::Fetch, ctx);
        }
    }
}

fn error_label(ui: &mut Ui, error: Option<&str>) {
    if let Some(error) = error {
        let t = Tokens::get(ui.ctx());
        ui.colored_label(t.danger, error);
    }
}

fn pr_acts(acts: &mut Vec<Action>, act: PrAct) {
    acts.push(Action::Pr(act));
}

/// The box on the pull request's page.
pub(super) fn comment_box(ui: &mut Ui, draft: &mut PrDraft, acts: &mut Vec<Action>) {
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
pub(super) fn review_box(
    ui: &mut Ui,
    draft: &mut PrDraft,
    extras: &PrExtras,
    own: bool,
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
    if own {
        ui.label(
            RichText::new("This one is yours: you can comment, but not approve it.")
                .color(t.text_faint)
                .small(),
        );
    }
    let count = extras.pending_comments().count();
    ui.horizontal_wrapped(|ui| {
        for (verdict, label) in [
            (Verdict::Comment, "Comment"),
            (Verdict::Approve, "Approve"),
            (Verdict::RequestChanges, "Request changes"),
        ] {
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
    });
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

/// The panel under a diff: the threads on the line, and the comment being written on it.
/// `current` is the line as it is in the pull request, to suggest a change to.
pub(super) fn line_panel(
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
    let reply_to = threads.reply_to;
    let hint = if reply_to.is_some() {
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
    let can_send = !draft.line_text.trim().is_empty() && !draft.posting;
    ui.horizontal(|ui| {
        match &reply_to {
            Some(thread) => {
                if ui
                    .add_enabled(can_send, egui::Button::new("Reply"))
                    .clicked()
                {
                    pr_acts(acts, PrAct::Reply(thread.clone()));
                }
            }
            None => {
                if ui
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
                    comment_now
                        .on_disabled_hover_text("You have a pending review: add it to that one");
                }
            }
        }
        if let Some(line) = current
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
    });
}

struct ThreadsShown {
    /// The thread a reply goes to.
    reply_to: Option<String>,
    pending_exists: bool,
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
    };
    let Some(Ok(details)) = &view.details else {
        return shown;
    };
    let extras = &details.extras;
    shown.pending_exists = extras.pending_review.is_some();
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
                    if let Some(body) = bodies.get(&comment.id) {
                        pr_view::blocks(ui, markdown, &comment.id, body, width);
                    }
                    let suggestion = suggestions
                        .get(&comment.id)
                        .filter(|_| !comment.pending && thread.side == DiffSide::Right);
                    if let Some(text) = suggestion
                        && extras.can_write()
                        && let Some(line) = thread.line
                        && ui
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
                                    start: thread.start_line.unwrap_or(line),
                                    end: line,
                                    text: text.clone(),
                                    author: comment.author.clone(),
                                },
                            },
                        );
                    }
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
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
                });
                ui.add_space(6.0);
            }
        });
    shown
}

/// A diff line's gutter: how many comments it has, or a `+` while the pointer is on it.
pub(super) fn paint_gutter_mark(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_threads::ThreadComment;

    #[test]
    fn in_the_unified_diff_a_line_has_one_place_to_comment() {
        assert_eq!(
            target_of(Kind::Add, None, Some(7), None),
            Some((DiffSide::Right, 7))
        );
        assert_eq!(
            target_of(Kind::Del, Some(5), None, None),
            Some((DiffSide::Left, 5))
        );
        assert_eq!(
            target_of(Kind::Context, Some(5), Some(7), None),
            Some((DiffSide::Right, 7))
        );
    }

    #[test]
    fn side_by_side_a_line_that_stayed_can_be_commented_on_either_side() {
        assert_eq!(
            target_of(Kind::Context, Some(5), Some(7), Some(DiffSide::Left)),
            Some((DiffSide::Left, 5))
        );
        assert_eq!(
            target_of(Kind::Context, Some(5), Some(7), Some(DiffSide::Right)),
            Some((DiffSide::Right, 7))
        );
    }

    #[test]
    fn headers_and_notes_take_no_comments() {
        assert_eq!(target_of(Kind::Hunk, None, None, None), None);
        assert_eq!(target_of(Kind::Meta, Some(1), Some(1), None), None);
    }

    fn thread(path: &str, line: u64, side: DiffSide, outdated: bool, comments: usize) -> Thread {
        Thread {
            id: format!("{path}:{line}"),
            path: path.to_owned(),
            line: Some(line),
            start_line: None,
            side,
            outdated,
            resolved: false,
            can_resolve: true,
            can_unresolve: false,
            can_reply: true,
            comments: vec![ThreadComment::default(); comments],
        }
    }

    #[test]
    fn marks_count_the_comments_of_this_file() {
        let threads = [
            thread("a.rs", 3, DiffSide::Right, false, 2),
            thread("a.rs", 3, DiffSide::Left, false, 1),
            thread("b.rs", 3, DiffSide::Right, false, 5),
        ];
        let marks = line_marks(&threads, "a.rs");
        assert_eq!(marks.get(&(DiffSide::Right, 3)), Some(&2));
        assert_eq!(marks.get(&(DiffSide::Left, 3)), Some(&1));
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn outdated_threads_mark_no_line() {
        let threads = [thread("a.rs", 3, DiffSide::Right, true, 1)];
        assert!(line_marks(&threads, "a.rs").is_empty());
        assert!(!thread_is_on(&threads[0], "a.rs", DiffSide::Right, 3));
    }

    #[test]
    fn a_thread_is_on_its_own_line_and_side_only() {
        let on = thread("a.rs", 3, DiffSide::Right, false, 1);
        assert!(thread_is_on(&on, "a.rs", DiffSide::Right, 3));
        assert!(!thread_is_on(&on, "a.rs", DiffSide::Left, 3));
        assert!(!thread_is_on(&on, "a.rs", DiffSide::Right, 4));
        assert!(!thread_is_on(&on, "b.rs", DiffSide::Right, 3));
    }

    #[test]
    fn errors_show_where_they_belong() {
        let draft = PrDraft {
            error: Some((Posted::Comment.place(), "No".to_owned())),
            ..PrDraft::default()
        };
        assert_eq!(draft.error_at(Place::Comment), Some("No"));
        assert_eq!(draft.error_at(Place::Line), None);
        assert_eq!(Posted::Reply.place(), Place::Line);
        assert_eq!(Posted::Discard.place(), Place::Review);
        assert_eq!(Posted::Merge.place(), Place::Merge);
    }

    #[test]
    fn discarding_the_pending_comments_keeps_the_text_of_the_review() {
        let mut draft = PrDraft {
            review: "Summary".to_owned(),
            ..PrDraft::default()
        };
        draft.sent(Posted::Discard);
        assert_eq!(draft.review, "Summary");
        draft.sent(Posted::Review(Verdict::Comment));
        assert_eq!(draft.review, "");
    }

    #[test]
    fn what_can_stop_halfway_is_loaded_again_after_a_failure() {
        assert!(Posted::Merge.catches_up_after_failure());
        assert!(Posted::Suggestion.catches_up_after_failure());
        assert!(!Posted::Comment.catches_up_after_failure());
        assert!(!Posted::AddPending.catches_up_after_failure());
    }
}
