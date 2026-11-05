//! The tab's drafts of pull requests: what the page asks for, sent on a thread of its own, and
//! what becomes of the draft once the answer comes.

use crate::pr_post::{LineComment, Target};
use crate::pull_request::PrDetails;
use crate::repo::{Action, DiffKey, Msg, RepoTab, spawn};

use super::lines::{DiffReview, line_marks};
use super::{Posted, PrAct, PrDraft};

/// Something to send, on a thread of its own.
type PostJob = Box<dyn FnOnce(&Target) -> Result<(), String> + Send>;

impl RepoTab {
    fn pr_draft(&mut self) -> Option<&mut PrDraft> {
        let number = self.pr_view.as_ref()?.number;
        Some(self.pr_drafts.entry(number).or_default())
    }

    /// Whether any pull request has something typed and not sent.
    pub(in crate::repo) fn has_pr_draft(&self) -> bool {
        self.pr_drafts.values().any(PrDraft::has_text)
    }

    /// The review of the diff `key`, if it belongs to the pull request shown.
    pub(in crate::repo) fn diff_review(&self, key: &DiffKey) -> Option<DiffReview> {
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
        // While new commits are being fetched, the file shown isn't the one comments would go on.
        if *from != files.from || *to != files.to || files.to != details.head_oid {
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
    pub(in crate::repo) fn line_comment_open(&self) -> bool {
        self.diff
            .as_ref()
            .and_then(|diff| self.diff_review(&diff.key))
            .is_some_and(|review| review.target.is_some())
    }

    /// The page of the pull request shown is being left.
    pub(in crate::repo) fn leave_pr(&mut self) {
        let number = self.pr_view.as_ref().map(|view| view.number);
        if let Some(draft) = number.and_then(|number| self.pr_drafts.get_mut(&number)) {
            draft.left();
        }
    }

    /// Leaves the page of the pull request shown, whatever is shown instead.
    pub(in crate::repo) fn close_pr(&mut self) {
        self.leave_pr();
        self.pr_view = None;
    }

    /// Does what the page, the review box or a thread asked for.
    pub(in crate::repo) fn pr_act(&mut self, act: PrAct, ctx: &egui::Context) {
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
                    draft.reveal_merge = true;
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
        let draft = self.pr_drafts.entry(number).or_default();
        let Some((what, job)) = post_job(act, draft, details, &target) else {
            return;
        };
        draft.posting = true;
        draft.error = None;
        spawn(&self.tx, ctx, move || Msg::PrPosted {
            number,
            what,
            result: job(&target),
        });
    }

    pub(in crate::repo) fn pr_posted(
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
        self.finished.push(outcome);
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

/// What `act` sends from `draft`, and how: `None` for what sends nothing.
fn post_job(
    act: PrAct,
    draft: &PrDraft,
    details: &PrDetails,
    target: &Target,
) -> Option<(Posted, PostJob)> {
    let line = draft.target.clone().map(|t| LineComment {
        path: t.path,
        line: t.line,
        side: t.side,
        body: draft.line_text.clone(),
    });
    let sent: (Posted, PostJob) = match act {
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
            let line = line?;
            (Posted::AddPending, Box::new(move |t| t.add_pending(&line)))
        }
        PrAct::PostLineNow => {
            let line = line?;
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
            let pending = details.extras.pending_review.is_some();
            (
                Posted::Reply { pending },
                Box::new(move |t| t.reply(&thread, &body)),
            )
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
            let review = draft.dismissing.clone()?;
            let message = draft.dismiss_text.clone();
            (
                Posted::Dismiss,
                Box::new(move |t| t.dismiss(&review, &message)),
            )
        }
        PrAct::Merge => {
            let method = draft.merge_method.unwrap_or_default();
            let delete = crate::repo::pr_merge::deletes_branch(
                details.extras.head_repo.as_deref() == Some(target.path()),
                details.extras.delete_branch_on_merge,
                draft.delete_branch,
            );
            (Posted::Merge, Box::new(move |t| t.merge(method, delete)))
        }
        _ => return None,
    };
    Some(sent)
}
