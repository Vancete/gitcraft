//! Saying something on the pull request shown: a comment on its page, a review from the box under
//! its files, threads on lines from the diff. What is only typed stays as a draft of the tab; a
//! comment added to the review is on GitHub already, in a pending review only its author sees.

mod boxes;
mod line_panel;
mod lines;
mod send;

use egui::Ui;

use super::Action;
use crate::pr_post::{MergeMethod, Suggestion, Verdict};
use crate::pull_request::DiffSide;
use crate::theme::Tokens;

pub(super) use boxes::{comment_box, review_box};
pub(super) use line_panel::{line_panel, paint_gutter_mark};
pub(super) use lines::{DiffReview, target_of, unified_spot};

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
    /// Into the pending review, when there is one.
    Reply {
        pending: bool,
    },
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
            | Posted::Reply { .. }
            | Posted::Resolve(_)
            | Posted::Suggestion => Place::Line,
            Posted::Dismiss => Place::Dismiss,
            Posted::Merge => Place::Merge,
        }
    }

    /// Whether a failure can leave the pull request changed or the page behind it.
    fn catches_up_after_failure(self) -> bool {
        matches!(
            self,
            Posted::Merge | Posted::Suggestion | Posted::AddPending
        )
    }

    /// What the notice says once it went out.
    fn done(self) -> &'static str {
        match self {
            Posted::Comment | Posted::LineNow => "Comment sent",
            Posted::Review(Verdict::Comment) => "Review sent",
            Posted::Review(Verdict::Approve) => "Pull request approved",
            Posted::Review(Verdict::RequestChanges) => "Changes requested",
            Posted::AddPending => "Added to your pending review",
            Posted::RemovePending => "Removed from your pending review",
            Posted::Discard => "Pending review discarded",
            Posted::Reply { pending: false } => "Reply sent",
            // Nobody else sees it until the review is sent.
            Posted::Reply { pending: true } => "Reply added to your pending review",
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
    /// The buttons of the merge box have just come up, and should be in view.
    pub(super) reveal_merge: bool,
}

impl PrDraft {
    /// The page is being left: only what's typed stays for next time.
    pub(super) fn left(&mut self) {
        self.target = None;
        self.confirm_merge = false;
        self.reveal_merge = false;
    }

    /// New commits have come: a yes given for the old ones doesn't count for them.
    pub(super) fn head_moved(&mut self) {
        self.confirm_merge = false;
        self.reveal_merge = false;
    }

    /// Clears what has been sent. Text not yet sent stays.
    fn sent(&mut self, what: Posted) {
        match what {
            Posted::Comment => self.comment.clear(),
            Posted::Review(_) => self.review.clear(),
            Posted::LineNow | Posted::AddPending => {
                self.line_text.clear();
                self.target = None;
            }
            Posted::Reply { .. } => self.line_text.clear(),
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

    /// Whether anything typed would be lost.
    pub fn has_text(&self) -> bool {
        [
            &self.comment,
            &self.review,
            &self.line_text,
            &self.dismiss_text,
        ]
        .iter()
        .any(|text| !text.trim().is_empty())
    }

    pub(super) fn error_at(&self, place: Place) -> Option<&str> {
        self.error
            .as_ref()
            .filter(|(at, _)| *at == place)
            .map(|(_, e)| e.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_with_nothing_typed_is_nothing_to_lose() {
        let mut draft = PrDraft {
            target: Some(LineTarget {
                path: "a.rs".to_owned(),
                line: 1,
                side: DiffSide::Right,
            }),
            ..PrDraft::default()
        };
        assert!(!draft.has_text());
        draft.comment = "  ".to_owned();
        assert!(!draft.has_text());
        draft.line_text = "Typo".to_owned();
        assert!(draft.has_text());
    }

    #[test]
    fn errors_show_where_they_belong() {
        let draft = PrDraft {
            error: Some((Posted::Comment.place(), "No".to_owned())),
            ..PrDraft::default()
        };
        assert_eq!(draft.error_at(Place::Comment), Some("No"));
        assert_eq!(draft.error_at(Place::Line), None);
        assert_eq!(Posted::Reply { pending: true }.place(), Place::Line);
        assert_eq!(Posted::Discard.place(), Place::Review);
        assert_eq!(Posted::Merge.place(), Place::Merge);
    }

    #[test]
    fn leaving_the_page_keeps_only_what_is_typed() {
        let mut draft = PrDraft {
            review: "Summary".to_owned(),
            confirm_merge: true,
            target: Some(LineTarget {
                path: "a.rs".to_owned(),
                line: 1,
                side: DiffSide::Right,
            }),
            ..PrDraft::default()
        };
        draft.left();
        assert!(!draft.confirm_merge);
        assert!(draft.target.is_none());
        assert_eq!(draft.review, "Summary");
    }

    #[test]
    fn new_commits_take_back_the_yes_to_merging() {
        let mut draft = PrDraft {
            confirm_merge: true,
            ..PrDraft::default()
        };
        draft.head_moved();
        assert!(!draft.confirm_merge);
    }

    #[test]
    fn the_repository_deleting_branches_leaves_nothing_to_delete() {
        use super::super::pr_merge::deletes_branch;
        assert!(!deletes_branch(true, true, None));
        assert!(!deletes_branch(true, true, Some(true)));
        assert!(!deletes_branch(true, false, None));
        assert!(deletes_branch(true, false, Some(true)));
        assert!(!deletes_branch(false, false, Some(true)), "a fork's branch");
    }

    #[test]
    fn a_reply_into_the_pending_review_is_not_said_to_be_sent() {
        assert_eq!(Posted::Reply { pending: false }.done(), "Reply sent");
        assert!(!Posted::Reply { pending: true }.done().contains("sent"));
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
        assert!(Posted::AddPending.catches_up_after_failure());
        assert!(!Posted::Reply { pending: false }.catches_up_after_failure());
    }
}
