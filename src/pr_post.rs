//! Saying something on a pull request, through `gh api`: comments and reviews, with their
//! comments on lines. The text goes over stdin, so its length and contents don't matter to the
//! command line.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use crate::forge::{self, Forge};
use crate::pull_request::{DiffSide, PrDetails, repo_of_url};

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
    let event = match verdict {
        Verdict::Comment => "COMMENT",
        Verdict::Approve => "APPROVE",
        Verdict::RequestChanges => "REQUEST_CHANGES",
    };
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
) -> Option<&'static str> {
    if !can_write {
        Some("You don't have the rights to merge here.")
    } else if draft {
        Some("It's still a draft.")
    } else if mergeable == "CONFLICTING" {
        Some("It has conflicts to resolve first.")
    } else if mergeable != "MERGEABLE" {
        Some("GitHub is still checking whether it can be merged. Load it again in a moment.")
    } else if merge_state == "BLOCKED" {
        Some("The repository's rules don't allow it yet: required reviews or checks are missing.")
    } else {
        None
    }
}

/// The suggestion to offer, only if what would be committed is what the comment shows. Text can
/// hide in a comment's source, for one in an HTML comment.
pub fn shown_suggestion(body: &str, host: &str) -> Option<String> {
    let raw = parse_suggestion(body)?;
    let shown = parse_suggestion(&crate::pull_request::safe_markdown(body, host))?;
    (raw == shown).then_some(raw)
}

/// The first suggested change in a comment's text, each line ended, so that no text at all (remove
/// the line) differs from one blank line.
pub fn parse_suggestion(body: &str) -> Option<String> {
    let mut lines = body.lines();
    lines.find(|line| line.trim_end() == "```suggestion")?;
    let mut suggestion = String::new();
    for line in lines {
        if line.trim_end() == "```" {
            return Some(suggestion);
        }
        suggestion.push_str(line);
        suggestion.push('\n');
    }
    None
}

/// `content` with lines `start` to `end` (counted from 1) replaced by `replacement`. Line ends stay
/// what the file uses, and a last line without one stays without.
pub(crate) fn apply_lines(
    content: &str,
    start: u64,
    end: u64,
    replacement: &str,
) -> Result<String, String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if start == 0 || start > end || end as usize > lines.len() {
        return Err(format!(
            "lines {start} to {end} aren't in the file, which has {}",
            lines.len()
        ));
    }
    let line_end = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let (from, to) = (start as usize - 1, end as usize);
    let keeps_no_end = to == lines.len() && !lines[to - 1].ends_with('\n');
    let mut out = lines[..from].concat();
    if !replacement.is_empty() {
        out.push_str(&replacement.lines().collect::<Vec<_>>().join(line_end));
        if !keeps_no_end {
            out.push_str(line_end);
        }
    }
    out.push_str(&lines[to..].concat());
    Ok(out)
}

/// A file's path for a URL: every part encoded, the slashes between them not.
pub(crate) fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ if byte.is_ascii_alphanumeric() => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A suggested change to apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub path: String,
    pub start: u64,
    pub end: u64,
    pub text: String,
    pub author: String,
}

/// A pull request to say something on.
pub struct Target {
    repo: PathBuf,
    host: String,
    /// `owner/name`.
    path: String,
    number: u64,
    head_oid: String,
    head_branch: String,
    head_repo: Option<String>,
    pr_id: String,
    pending_review: Option<String>,
}

impl Target {
    /// The pull request `details` of the repository at `repo`.
    pub fn of(repo: &Path, details: &PrDetails) -> Option<Target> {
        let (host, path) = repo_of_url(&details.url)?;
        Some(Target {
            repo: repo.to_owned(),
            host,
            path,
            number: details.number,
            head_oid: details.head_oid.clone(),
            head_branch: details.head_branch.clone(),
            head_repo: details.extras.head_repo.clone(),
            pr_id: details.extras.pr_id.clone(),
            pending_review: details.extras.pending_review.clone(),
        })
    }

    /// `owner/name` of the repository.
    pub fn path(&self) -> &str {
        &self.path
    }

    fn api(&self, method: &str, endpoint: &str, payload: Option<&Value>) -> Result<String, String> {
        let mut args = vec!["api", "-X", method, endpoint];
        if payload.is_some() {
            args.extend(["--input", "-"]);
        }
        if self.host != "github.com" {
            args.extend(["--hostname", &self.host]);
        }
        let input = payload.map(Value::to_string);
        forge::tool_with_input(Forge::GitHub, &self.repo, &args, input.as_deref())
    }

    /// Runs a mutation taking one `input`, and returns what GitHub answers.
    fn mutate(&self, name: &str, input: Value, answer: &str) -> Result<Value, String> {
        let upper = format!("{}{}Input", name[..1].to_uppercase(), &name[1..]);
        let query =
            format!("mutation($input: {upper}!) {{ {name}(input: $input) {{ {answer} }} }}");
        let payload = json!({ "query": query, "variables": { "input": input } });
        let mut args = vec!["api", "graphql", "--input", "-"];
        if self.host != "github.com" {
            args.extend(["--hostname", &self.host]);
        }
        let out =
            forge::tool_with_input(Forge::GitHub, &self.repo, &args, Some(&payload.to_string()))?;
        serde_json::from_str(&out).map_err(|e| format!("unexpected output of gh: {e}"))
    }

    /// Adds a comment to the conversation.
    pub fn comment(&self, body: &str) -> Result<(), String> {
        let endpoint = format!("repos/{}/issues/{}/comments", self.path, self.number);
        self.api("POST", &endpoint, Some(&json!({ "body": body })))
            .map(|_| ())
    }

    /// Sends the review: the pending one if there is one, with the comments in it.
    pub fn review(&self, verdict: Verdict, body: &str) -> Result<(), String> {
        if let Some(review) = &self.pending_review {
            let event = match verdict {
                Verdict::Comment => "COMMENT",
                Verdict::Approve => "APPROVE",
                Verdict::RequestChanges => "REQUEST_CHANGES",
            };
            let input = json!({ "pullRequestReviewId": review, "event": event, "body": body });
            return self
                .mutate("submitPullRequestReview", input, "clientMutationId")
                .map(|_| ());
        }
        let payload = review_payload(&self.head_oid, verdict, body, &[]);
        self.post_review(&payload)
    }

    fn post_review(&self, payload: &Value) -> Result<(), String> {
        let endpoint = format!("repos/{}/pulls/{}/reviews", self.path, self.number);
        self.api("POST", &endpoint, Some(payload)).map(|_| ())
    }

    /// Sends a comment on a line right away, as a review of its own.
    pub fn comment_now(&self, comment: &LineComment) -> Result<(), String> {
        let payload = review_payload(
            &self.head_oid,
            Verdict::Comment,
            "",
            std::slice::from_ref(comment),
        );
        self.post_review(&payload)
    }

    /// Adds a comment on a line to the pending review, starting one if there is none.
    pub fn add_pending(&self, comment: &LineComment) -> Result<(), String> {
        let review = match &self.pending_review {
            Some(review) => review.clone(),
            None => {
                let answer = self.mutate(
                    "addPullRequestReview",
                    json!({ "pullRequestId": self.pr_id }),
                    "pullRequestReview { id }",
                )?;
                answer
                    .pointer("/data/addPullRequestReview/pullRequestReview/id")
                    .and_then(Value::as_str)
                    .ok_or("GitHub didn't start a review")?
                    .to_owned()
            }
        };
        let input = json!({
            "pullRequestReviewId": review,
            "path": comment.path,
            "line": comment.line,
            "side": side_name(comment.side),
            "body": comment.body,
        });
        self.mutate("addPullRequestReviewThread", input, "clientMutationId")
            .map(|_| ())
    }

    /// Takes a comment out of the pending review; the review goes with its `last` comment.
    pub fn remove_pending(&self, comment: &str, last: bool) -> Result<(), String> {
        if last {
            return self.discard_pending();
        }
        self.mutate(
            "deletePullRequestReviewComment",
            json!({ "id": comment }),
            "clientMutationId",
        )
        .map(|_| ())
    }

    /// Throws the pending review away.
    pub fn discard_pending(&self) -> Result<(), String> {
        let review = self
            .pending_review
            .as_ref()
            .ok_or("There is no pending review")?;
        self.mutate(
            "deletePullRequestReview",
            json!({ "pullRequestReviewId": review }),
            "clientMutationId",
        )
        .map(|_| ())
    }

    pub fn reply(&self, thread: &str, body: &str) -> Result<(), String> {
        let mut input = json!({ "pullRequestReviewThreadId": thread, "body": body });
        if let Some(review) = &self.pending_review {
            input["pullRequestReviewId"] = json!(review);
        }
        self.mutate("addPullRequestReviewThreadReply", input, "clientMutationId")
            .map(|_| ())
    }

    pub fn set_resolved(&self, thread: &str, resolved: bool) -> Result<(), String> {
        let name = if resolved {
            "resolveReviewThread"
        } else {
            "unresolveReviewThread"
        };
        self.mutate(name, json!({ "threadId": thread }), "clientMutationId")
            .map(|_| ())
    }

    pub fn dismiss(&self, review: &str, message: &str) -> Result<(), String> {
        self.mutate(
            "dismissPullRequestReview",
            json!({ "pullRequestReviewId": review, "message": message }),
            "clientMutationId",
        )
        .map(|_| ())
    }

    /// Merges the pull request, and deletes its branch if asked to.
    pub fn merge(&self, method: MergeMethod, delete_branch: bool) -> Result<(), String> {
        let endpoint = format!("repos/{}/pulls/{}/merge", self.path, self.number);
        self.api(
            "PUT",
            &endpoint,
            Some(&merge_payload(&self.head_oid, method)),
        )?;
        if delete_branch {
            let endpoint = format!(
                "repos/{}/git/refs/heads/{}",
                self.path,
                encode_path(&self.head_branch)
            );
            self.api("DELETE", &endpoint, None)
                .map_err(|e| format!("It's merged, but the branch wasn't deleted: {e}"))?;
        }
        Ok(())
    }

    /// Commits a suggested change to the pull request's branch. The file is read as it is in the
    /// commit that was reviewed, so if it has changed since, GitHub refuses instead of the change
    /// landing on the wrong lines.
    pub fn apply_suggestion(&self, suggestion: &Suggestion) -> Result<(), String> {
        let head = self
            .head_repo
            .as_ref()
            .ok_or("The repository of this branch is gone")?;
        let endpoint = format!("repos/{head}/contents/{}", encode_path(&suggestion.path));
        let file = self.api("GET", &format!("{endpoint}?ref={}", self.head_oid), None)?;
        let file: Value =
            serde_json::from_str(&file).map_err(|e| format!("unexpected output of gh: {e}"))?;
        let encoded: String = file
            .get("content")
            .and_then(Value::as_str)
            .ok_or("GitHub didn't send the file: it may be too large")?
            .split_whitespace()
            .collect();
        let bytes = BASE64
            .decode(encoded)
            .map_err(|e| format!("unexpected file from GitHub: {e}"))?;
        let content = String::from_utf8(bytes).map_err(|_| "It isn't a text file".to_owned())?;
        let changed = apply_lines(&content, suggestion.start, suggestion.end, &suggestion.text)?;
        let payload = json!({
            "message": format!("Apply suggestion from @{}", suggestion.author),
            "content": BASE64.encode(changed),
            "sha": file.get("sha"),
            "branch": self.head_branch,
        });
        self.api("PUT", &endpoint, Some(&payload)).map(|_| ())
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
        assert_eq!(merge_blocker(false, "MERGEABLE", "CLEAN", true), None);
        // Failing checks that aren't required don't stop a merge on GitHub either.
        assert_eq!(merge_blocker(false, "MERGEABLE", "UNSTABLE", true), None);
        assert_eq!(merge_blocker(false, "MERGEABLE", "BEHIND", true), None);
    }

    #[test]
    fn what_keeps_a_pull_request_from_being_merged() {
        assert!(merge_blocker(true, "MERGEABLE", "DRAFT", true).is_some());
        assert!(merge_blocker(false, "CONFLICTING", "DIRTY", true).is_some());
        assert!(merge_blocker(false, "UNKNOWN", "UNKNOWN", true).is_some());
        assert!(merge_blocker(false, "MERGEABLE", "BLOCKED", true).is_some());
        assert!(merge_blocker(false, "MERGEABLE", "CLEAN", false).is_some());
    }

    #[test]
    fn finds_a_suggested_change() {
        let body = "Clearer:\n\n```suggestion\nlet a = 1;\nlet b = 2;\n```\nThanks";
        assert_eq!(
            parse_suggestion(body).as_deref(),
            Some("let a = 1;\nlet b = 2;\n")
        );
    }

    #[test]
    fn a_blank_line_is_not_the_same_as_no_line() {
        let blank = parse_suggestion("```suggestion\n\n```").unwrap_or_default();
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, &blank).as_deref(),
            Ok("a\n\nc\n")
        );
        let gone = parse_suggestion("```suggestion\n```").unwrap_or_default();
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, &gone).as_deref(),
            Ok("a\nc\n")
        );
        let ends_blank = parse_suggestion("```suggestion\nB\n\n```").unwrap_or_default();
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, &ends_blank).as_deref(),
            Ok("a\nB\n\nc\n")
        );
    }

    #[test]
    fn spaces_at_the_end_of_a_suggested_line_stay() {
        assert_eq!(
            parse_suggestion("```suggestion\nx  \n```  ").as_deref(),
            Some("x  \n")
        );
    }

    #[test]
    fn a_suggestion_hidden_from_the_reader_is_not_offered() {
        let hidden = "Looks fine\n<!--\n```suggestion\nevil();\n```\n-->";
        assert_eq!(shown_suggestion(hidden, "github.com"), None);
        let decoy = "<!--\n```suggestion\nevil();\n```\n-->\n```suggestion\ngood();\n```";
        assert_eq!(shown_suggestion(decoy, "github.com"), None);
        let inside = "```suggestion\ngood(); <!-- evil(); -->\n```";
        assert_eq!(shown_suggestion(inside, "github.com"), None);
    }

    #[test]
    fn a_suggestion_in_plain_view_is_offered() {
        let body = "Clearer:\n\n```suggestion\nlet a = 1;\n```";
        assert_eq!(
            shown_suggestion(body, "github.com").as_deref(),
            Some("let a = 1;\n")
        );
    }

    #[test]
    fn a_suggestion_can_be_to_remove_the_line() {
        assert_eq!(parse_suggestion("```suggestion\n```").as_deref(), Some(""));
    }

    #[test]
    fn other_code_blocks_are_no_suggestion() {
        assert_eq!(parse_suggestion("```rust\nlet a = 1;\n```"), None);
        assert_eq!(parse_suggestion("no code"), None);
        assert_eq!(parse_suggestion("```suggestion\nnever closed"), None);
    }

    #[test]
    fn windows_line_ends_in_a_comment_make_no_difference() {
        assert_eq!(
            parse_suggestion("```suggestion\r\nx\r\n```\r\n").as_deref(),
            Some("x\n")
        );
    }

    #[test]
    fn replaces_a_line() {
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, "B").as_deref(),
            Ok("a\nB\nc\n")
        );
    }

    #[test]
    fn replaces_several_lines_with_several() {
        assert_eq!(
            apply_lines("a\nb\nc\nd\n", 2, 3, "x\ny\nz").as_deref(),
            Ok("a\nx\ny\nz\nd\n")
        );
    }

    #[test]
    fn an_empty_suggestion_removes_the_line() {
        assert_eq!(apply_lines("a\nb\nc\n", 2, 2, "").as_deref(), Ok("a\nc\n"));
    }

    #[test]
    fn keeps_the_line_ends_of_the_file() {
        assert_eq!(
            apply_lines("a\r\nb\r\nc\r\n", 2, 2, "x\ny").as_deref(),
            Ok("a\r\nx\r\ny\r\nc\r\n")
        );
    }

    #[test]
    fn keeps_a_last_line_without_a_line_end() {
        assert_eq!(apply_lines("a\nb", 2, 2, "B").as_deref(), Ok("a\nB"));
    }

    #[test]
    fn lines_that_are_not_there_are_an_error() {
        assert!(apply_lines("a\n", 3, 3, "x").is_err());
        assert!(apply_lines("a\n", 0, 1, "x").is_err());
        assert!(apply_lines("a\nb\n", 2, 1, "x").is_err());
    }

    #[test]
    fn encodes_a_path_but_not_its_slashes() {
        assert_eq!(encode_path("src/a b#c.rs"), "src/a%20b%23c.rs");
        assert_eq!(encode_path("src/ä.rs"), "src/%C3%A4.rs");
        assert_eq!(encode_path("plain-name_1.2~x"), "plain-name_1.2~x");
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
