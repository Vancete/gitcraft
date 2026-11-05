//! Saying something on a pull request, through `gh api`: comments and reviews, with their
//! comments on lines. The text goes over stdin, so its length and contents don't matter to the
//! command line.

mod suggestion;
mod target;

use serde_json::{Value, json};

use crate::pull_request::DiffSide;

pub use suggestion::{Suggestion, shown_suggestion};
pub use target::Target;

/// What a review says about the pull request as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Comment,
    Approve,
    RequestChanges,
}

/// A comment on a line, written and not sent yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineComment {
    pub path: String,
    pub line: u64,
    pub side: DiffSide,
    pub body: String,
}

/// Why this review can't be sent, if it can't. GitHub would refuse the same, less clearly.
pub fn review_problem(verdict: Verdict, body: &str, pending: usize) -> Option<&'static str> {
    let says_something = !body.trim().is_empty() || pending > 0;
    match verdict {
        Verdict::Approve => None,
        _ if says_something => None,
        Verdict::Comment => Some("Write something to send"),
        Verdict::RequestChanges => Some("Say what should change"),
    }
}

/// What `POST …/pulls/N/reviews` is sent.
pub(crate) fn review_payload(
    head_oid: &str,
    verdict: Verdict,
    body: &str,
    pending: &[LineComment],
) -> Value {
    let event = event_name(verdict);
    let mut payload = json!({ "commit_id": head_oid, "event": event, "body": body });
    if !pending.is_empty() {
        let comments: Vec<Value> = pending
            .iter()
            .map(|comment| {
                json!({
                    "path": comment.path,
                    "line": comment.line,
                    "side": side_name(comment.side),
                    "body": comment.body,
                })
            })
            .collect();
        payload["comments"] = Value::Array(comments);
    }
    payload
}

/// Whether `viewer` is the author of a pull request: GitHub lets nobody review their own.
pub fn is_own(author: &str, viewer: Option<&str>) -> bool {
    viewer.is_some_and(|viewer| viewer.eq_ignore_ascii_case(author))
}

/// How a pull request gets merged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

/// What `PUT …/pulls/N/merge` is sent. The commit is named, so that nothing pushed since the page
/// was read gets merged unseen.
pub(crate) fn merge_payload(head_oid: &str, method: MergeMethod) -> Value {
    let method = match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    };
    json!({ "sha": head_oid, "merge_method": method })
}

/// Why this pull request can't be merged, if it can't.
pub fn merge_blocker(
    draft: bool,
    mergeable: &str,
    merge_state: &str,
    can_write: bool,
    can_bypass_rules: bool,
) -> Option<&'static str> {
    if !can_write {
        Some("You don't have the rights to merge here.")
    } else if draft {
        Some("It's still a draft.")
    } else if mergeable == "CONFLICTING" {
        Some("It has conflicts to resolve first.")
    } else if mergeable != "MERGEABLE" {
        Some("GitHub is still checking whether it can be merged. Load it again in a moment.")
    } else if merge_state == "BLOCKED" && !can_bypass_rules {
        Some("The repository's rules don't allow it yet: required reviews or checks are missing.")
    } else {
        None
    }
}

/// What GitHub calls a review's verdict.
fn event_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Comment => "COMMENT",
        Verdict::Approve => "APPROVE",
        Verdict::RequestChanges => "REQUEST_CHANGES",
    }
}

fn side_name(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(path: &str, line: u64, side: DiffSide, body: &str) -> LineComment {
        LineComment {
            path: path.to_owned(),
            line,
            side,
            body: body.to_owned(),
        }
    }

    #[test]
    fn an_approval_needs_no_words() {
        assert_eq!(review_problem(Verdict::Approve, "", 0), None);
    }

    #[test]
    fn requesting_changes_has_to_say_which() {
        assert!(review_problem(Verdict::RequestChanges, "  \n", 0).is_some());
        assert_eq!(review_problem(Verdict::RequestChanges, "Fix it", 0), None);
    }

    #[test]
    fn requesting_changes_can_say_so_on_the_lines_alone() {
        assert_eq!(review_problem(Verdict::RequestChanges, "", 1), None);
    }

    #[test]
    fn a_comment_review_has_to_say_something_somewhere() {
        assert!(review_problem(Verdict::Comment, "", 0).is_some());
        assert_eq!(review_problem(Verdict::Comment, "Nice", 0), None);
        assert_eq!(review_problem(Verdict::Comment, "", 2), None);
    }

    #[test]
    fn a_review_names_its_verdict_and_commit() {
        for (verdict, event) in [
            (Verdict::Comment, "COMMENT"),
            (Verdict::Approve, "APPROVE"),
            (Verdict::RequestChanges, "REQUEST_CHANGES"),
        ] {
            let payload = review_payload("abc", verdict, "Text", &[]);
            assert_eq!(payload["event"], event);
            assert_eq!(payload["commit_id"], "abc");
            assert_eq!(payload["body"], "Text");
        }
    }

    #[test]
    fn a_review_without_comments_on_lines_sends_none() {
        let payload = review_payload("abc", Verdict::Approve, "", &[]);
        assert!(payload.get("comments").is_none());
    }

    #[test]
    fn comments_on_lines_go_along_with_their_side() {
        let pending = [
            line("src/a.rs", 12, DiffSide::Right, "New line"),
            line("src/b.rs", 3, DiffSide::Left, "Removed line"),
        ];
        let payload = review_payload("abc", Verdict::Comment, "", &pending);
        assert_eq!(
            payload["comments"],
            json!([
                {"path": "src/a.rs", "line": 12, "side": "RIGHT", "body": "New line"},
                {"path": "src/b.rs", "line": 3, "side": "LEFT", "body": "Removed line"},
            ])
        );
    }

    #[test]
    fn text_is_sent_as_it_is_whatever_it_contains() {
        let body = "\"quotes\" \\ and\nlines --flag $(x)";
        let payload = review_payload("abc", Verdict::Comment, body, &[]);
        let sent: Value = serde_json::from_str(&payload.to_string()).unwrap_or_default();
        assert_eq!(sent["body"], body);
    }

    #[test]
    fn a_merge_names_its_method_and_commit() {
        for (method, name) in [
            (MergeMethod::Merge, "merge"),
            (MergeMethod::Squash, "squash"),
            (MergeMethod::Rebase, "rebase"),
        ] {
            let payload = merge_payload("abc", method);
            assert_eq!(payload["merge_method"], name);
            assert_eq!(payload["sha"], "abc");
        }
    }

    #[test]
    fn a_ready_pull_request_can_be_merged() {
        assert_eq!(
            merge_blocker(false, "MERGEABLE", "CLEAN", true, false),
            None
        );
        // Failing checks that aren't required don't stop a merge on GitHub either.
        assert_eq!(
            merge_blocker(false, "MERGEABLE", "UNSTABLE", true, false),
            None
        );
        assert_eq!(
            merge_blocker(false, "MERGEABLE", "BEHIND", true, false),
            None
        );
    }

    #[test]
    fn what_keeps_a_pull_request_from_being_merged() {
        assert!(merge_blocker(true, "MERGEABLE", "DRAFT", true, false).is_some());
        assert!(merge_blocker(false, "CONFLICTING", "DIRTY", true, false).is_some());
        assert!(merge_blocker(false, "UNKNOWN", "UNKNOWN", true, false).is_some());
        assert!(merge_blocker(false, "MERGEABLE", "BLOCKED", true, false).is_some());
        assert!(merge_blocker(false, "MERGEABLE", "CLEAN", false, false).is_some());
    }

    #[test]
    fn rules_are_no_blocker_for_who_may_bypass_them() {
        assert_eq!(
            merge_blocker(false, "MERGEABLE", "BLOCKED", true, true),
            None
        );
        assert!(merge_blocker(false, "CONFLICTING", "DIRTY", true, true).is_some());
        assert!(merge_blocker(true, "MERGEABLE", "DRAFT", true, true).is_some());
    }

    #[test]
    fn nobody_reviews_their_own_pull_request() {
        assert!(is_own("ana", Some("ana")));
        assert!(is_own("Ana", Some("aNA")));
        assert!(!is_own("ana", Some("bo")));
        // Not knowing who is logged in, it's up to GitHub to say.
        assert!(!is_own("ana", None));
    }
}
