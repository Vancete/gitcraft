//! A pull request to say something on, and the `gh api` calls that do the saying.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use super::suggestion::{Suggestion, apply_lines};
use super::{
    LineComment, MergeMethod, Verdict, event_name, merge_payload, review_payload, side_name,
};
use crate::forge::{self, Forge};
use crate::pull_request::{PrDetails, repo_of_url};

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
            let event = event_name(verdict);
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
        if self.pr_id.is_empty() {
            return Err("Couldn't read all of this pull request. Load it again.".to_owned());
        }
        let review = match &self.pending_review {
            Some(review) => review.clone(),
            None => {
                // On the commit shown: without it, GitHub takes the newest, which the line
                // numbers may not be of.
                let answer = self.mutate(
                    "addPullRequestReview",
                    json!({ "pullRequestId": self.pr_id, "commitOID": self.head_oid }),
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
        let added = self
            .mutate("addPullRequestReviewThread", input, "clientMutationId")
            .map(|_| ());
        if added.is_err() && self.pending_review.is_none() {
            // A review started for this and left empty would only stand in the way of the next try.
            let _ = self.mutate(
                "deletePullRequestReview",
                json!({ "pullRequestReviewId": review }),
                "clientMutationId",
            );
        }
        added
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
            let branch = encode_path(&self.head_branch);
            // Commits pushed since the merge would lose their branch. GitHub deletes without
            // asking which commit it's on, so this is checked first; a push can still come in
            // between, just in a much smaller window.
            let answer = self.api(
                "GET",
                &format!("repos/{}/git/ref/heads/{branch}", self.path),
                None,
            );
            if !branch_to_delete(answer.as_deref().map_err(String::as_str), &self.head_oid)? {
                return Ok(());
            }
            let endpoint = format!("repos/{}/git/refs/heads/{branch}", self.path);
            if let Err(e) = self.api("DELETE", &endpoint, None)
                && !branch_was_gone(&e)
            {
                return Err(format!("It's merged, but the branch wasn't deleted: {e}"));
            }
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
        let content = decode_file(&file)?;
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

/// The text of a file as the contents API sends it.
pub(crate) fn decode_file(file: &Value) -> Result<String, String> {
    // Above 1 MB the answer has no content, whatever the file holds.
    if file.get("encoding").and_then(Value::as_str) == Some("none") {
        return Err("GitHub didn't send the file: it's too large".to_owned());
    }
    let encoded: String = file
        .get("content")
        .and_then(Value::as_str)
        .ok_or("GitHub didn't send the file: it may be too large")?
        .split_whitespace()
        .collect();
    let bytes = BASE64
        .decode(encoded)
        .map_err(|e| format!("unexpected file from GitHub: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "It isn't a text file".to_owned())
}

/// Whether a failed deletion only says the branch is gone already.
pub(crate) fn branch_was_gone(error: &str) -> bool {
    error.contains("Reference does not exist")
}

/// The commit a branch is on, from what GitHub answers when asked for its ref.
pub(crate) fn ref_tip(answer: &str) -> Option<String> {
    let v: Value = serde_json::from_str(answer).ok()?;
    v.pointer("/object/sha")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Whether the branch of a merged pull request goes, from what asking for its ref answered: only
/// while it's still on the merged commit. One that couldn't be checked is kept.
pub(crate) fn branch_to_delete(answer: Result<&str, &str>, head_oid: &str) -> Result<bool, String> {
    match answer {
        Ok(answer) => match ref_tip(answer) {
            Some(tip) if tip == head_oid => Ok(true),
            Some(_) => {
                Err("It's merged, but the branch has new commits, so it was kept".to_owned())
            }
            None => Err(
                "It's merged, but the branch was kept: GitHub didn't say which commit it's on"
                    .to_owned(),
            ),
        },
        // Gone already: nothing to delete.
        Err(e) if e.contains("HTTP 404") => Ok(false),
        Err(e) => Err(format!(
            "It's merged, but the branch was kept, as its commit couldn't be checked: {e}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::DiffSide;

    #[test]
    fn a_file_too_large_for_the_api_is_said_to_be() {
        let large = json!({"content": "", "encoding": "none"});
        assert!(decode_file(&large).is_err_and(|e| e.contains("too large")));
        let small = json!({"content": "aGk=\n", "encoding": "base64"});
        assert_eq!(decode_file(&small).as_deref(), Ok("hi"));
    }

    #[test]
    fn a_branch_gone_already_is_what_deleting_it_wanted() {
        assert!(branch_was_gone("gh: Reference does not exist (HTTP 422)"));
        assert!(!branch_was_gone("gh: Must have admin rights (HTTP 403)"));
    }

    #[test]
    fn reads_which_commit_a_branch_is_on() {
        let answer = r#"{"ref":"refs/heads/b","object":{"sha":"abc123","type":"commit"}}"#;
        assert_eq!(ref_tip(answer).as_deref(), Some("abc123"));
        assert_eq!(ref_tip(r#"{"message":"Not Found"}"#), None);
        assert_eq!(ref_tip("not json"), None);
    }

    #[test]
    fn a_merged_branch_goes_only_while_on_the_merged_commit() {
        let on = r#"{"object":{"sha":"abc"}}"#;
        assert_eq!(branch_to_delete(Ok(on), "abc"), Ok(true));
        let moved = r#"{"object":{"sha":"def"}}"#;
        assert!(branch_to_delete(Ok(moved), "abc").is_err_and(|e| e.contains("new commits")));
        assert_eq!(
            branch_to_delete(Err("gh: Not Found (HTTP 404)"), "abc"),
            Ok(false)
        );
        // Not knowing which commit it's on, it isn't deleted.
        assert!(branch_to_delete(Err("gh: Bad credentials (HTTP 401)"), "abc").is_err());
        assert!(branch_to_delete(Err("connection reset"), "abc").is_err());
        assert!(branch_to_delete(Ok("not json"), "abc").is_err());
    }

    fn target_without_extras() -> Target {
        Target {
            repo: PathBuf::new(),
            host: "github.com".to_owned(),
            path: "o/r".to_owned(),
            number: 1,
            head_oid: "h".to_owned(),
            head_branch: "b".to_owned(),
            head_repo: None,
            pr_id: String::new(),
            pending_review: None,
        }
    }

    #[test]
    fn nothing_goes_out_without_knowing_the_pull_request() {
        let comment = LineComment {
            path: "a".to_owned(),
            line: 1,
            side: DiffSide::Right,
            body: "x".to_owned(),
        };
        let error = target_without_extras().add_pending(&comment);
        assert!(error.is_err_and(|e| e.contains("Load it again")));
    }

    #[test]
    fn encodes_a_path_but_not_its_slashes() {
        assert_eq!(encode_path("src/a b#c.rs"), "src/a%20b%23c.rs");
        assert_eq!(encode_path("src/ä.rs"), "src/%C3%A4.rs");
        assert_eq!(encode_path("plain-name_1.2~x"), "plain-name_1.2~x");
    }
}
