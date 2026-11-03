//! What only GitHub's GraphQL API tells about a pull request: review threads with their state,
//! the pending review of the person logged in, and what they may do.

use std::path::Path;

use serde_json::{Value, json};

use crate::forge::{self, Forge};
use crate::pull_request::{DiffSide, at, items, login, parse_time};

const QUERY: &str = "query($owner: String!, $name: String!, $number: Int!, $after: String) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed deleteBranchOnMerge viewerPermission
    pullRequest(number: $number) {
      id headRefOid viewerCanMergeAsAdmin
      headRepository { nameWithOwner }
      pending: reviews(first: 1, states: PENDING) { nodes { id } }
      reviewThreads(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id isResolved isOutdated path line originalLine diffSide startLine startDiffSide
          viewerCanResolve viewerCanUnresolve viewerCanReply
          comments(first: 100) {
            pageInfo { hasNextPage }
            nodes { id body createdAt url state author { login } }
          }
        }
      }
    }
  }
}";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadComment {
    pub id: String,
    pub author: String,
    pub at: i64,
    pub body: String,
    pub url: String,
    /// In the pending review of the person logged in: nobody else sees it yet.
    pub pending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub path: String,
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    pub side: DiffSide,
    pub outdated: bool,
    pub resolved: bool,
    pub can_resolve: bool,
    pub can_unresolve: bool,
    pub can_reply: bool,
    pub comments: Vec<ThreadComment>,
    /// It starts on the old version of the file and ends on the new one.
    pub starts_left: bool,
    /// It has more comments than were read.
    pub more_comments: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrExtras {
    pub pr_id: String,
    pub viewer: Option<String>,
    pub permission: String,
    pub merge_allowed: bool,
    pub squash_allowed: bool,
    pub rebase_allowed: bool,
    pub delete_branch_on_merge: bool,
    /// `owner/name` of where the proposed branch lives; `None` when that repository is gone.
    pub head_repo: Option<String>,
    pub pending_review: Option<String>,
    /// The commit the threads' line numbers belong to.
    pub head_oid: String,
    /// The person logged in may merge past the repository's rules.
    pub can_bypass_rules: bool,
    pub threads: Vec<Thread>,
    /// Where the next page of threads starts, when there is one.
    next_page: Option<String>,
}

impl PrExtras {
    /// Whether the person logged in may push and merge here.
    pub fn can_write(&self) -> bool {
        matches!(self.permission.as_str(), "WRITE" | "MAINTAIN" | "ADMIN")
    }

    /// The comments waiting in the pending review.
    pub fn pending_comments(&self) -> impl Iterator<Item = (&Thread, &ThreadComment)> {
        self.threads.iter().flat_map(|thread| {
            thread
                .comments
                .iter()
                .filter(|comment| comment.pending)
                .map(move |comment| (thread, comment))
        })
    }
}

/// Reads all of it for the pull request `number` of `owner/name`.
pub fn fetch(repo: &Path, host: &str, path: &str, number: u64) -> Result<PrExtras, String> {
    let (owner, name) = path.split_once('/').ok_or("unexpected repository name")?;
    let mut extras: Option<PrExtras> = None;
    let mut after: Option<String> = None;
    loop {
        let payload = json!({
            "query": QUERY,
            "variables": { "owner": owner, "name": name, "number": number, "after": after },
        });
        let mut args = vec!["api", "graphql", "--input", "-"];
        if host != "github.com" {
            args.extend(["--hostname", host]);
        }
        let out = forge::tool_with_input(Forge::GitHub, repo, &args, Some(&payload.to_string()))?;
        let page = parse(&out)?;
        after = page.next_page.clone();
        extras = Some(match extras {
            None => page,
            Some(mut all) => {
                all.threads.extend(page.threads);
                all
            }
        });
        if after.is_none() {
            return extras.ok_or_else(|| "no answer".to_owned());
        }
    }
}

pub(crate) fn parse(json: &str) -> Result<PrExtras, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected output of gh: {e}"))?;
    let repository = v
        .pointer("/data/repository")
        .ok_or("GitHub doesn't know this repository")?;
    let pr = repository
        .get("pullRequest")
        .filter(|pr| !pr.is_null())
        .ok_or("GitHub doesn't know this pull request")?;
    let allowed = |key: &str| {
        repository
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let threads = pr.pointer("/reviewThreads");
    let has_next = threads
        .and_then(|t| t.pointer("/pageInfo/hasNextPage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let viewer = at(&v, "/data/viewer/login");
    let head_repo = at(pr, "/headRepository/nameWithOwner");
    let pending = at(pr, "/pending/nodes/0/id");
    Ok(PrExtras {
        pr_id: at(pr, "/id").to_owned(),
        viewer: (!viewer.is_empty()).then(|| viewer.to_owned()),
        permission: at(repository, "/viewerPermission").to_owned(),
        merge_allowed: allowed("mergeCommitAllowed"),
        squash_allowed: allowed("squashMergeAllowed"),
        rebase_allowed: allowed("rebaseMergeAllowed"),
        delete_branch_on_merge: allowed("deleteBranchOnMerge"),
        head_repo: (!head_repo.is_empty()).then(|| head_repo.to_owned()),
        pending_review: (!pending.is_empty()).then(|| pending.to_owned()),
        head_oid: at(pr, "/headRefOid").to_owned(),
        can_bypass_rules: pr
            .get("viewerCanMergeAsAdmin")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        threads: items(pr, "/reviewThreads/nodes")
            .iter()
            .map(thread)
            .collect(),
        next_page: threads
            .and_then(|t| t.pointer("/pageInfo/endCursor"))
            .and_then(Value::as_str)
            .filter(|_| has_next)
            .map(str::to_owned),
    })
}

fn thread(v: &Value) -> Thread {
    let number = |key: &str| v.pointer(key).and_then(Value::as_u64);
    let flag = |key: &str| v.pointer(key).and_then(Value::as_bool).unwrap_or(false);
    Thread {
        id: at(v, "/id").to_owned(),
        path: at(v, "/path").to_owned(),
        line: number("/line").or_else(|| number("/originalLine")),
        start_line: number("/startLine"),
        side: if at(v, "/diffSide") == "LEFT" {
            DiffSide::Left
        } else {
            DiffSide::Right
        },
        outdated: flag("/isOutdated"),
        resolved: flag("/isResolved"),
        can_resolve: flag("/viewerCanResolve"),
        can_unresolve: flag("/viewerCanUnresolve"),
        can_reply: flag("/viewerCanReply"),
        starts_left: at(v, "/startDiffSide") == "LEFT",
        more_comments: flag("/comments/pageInfo/hasNextPage"),
        comments: items(v, "/comments/nodes")
            .iter()
            .map(|c| ThreadComment {
                id: at(c, "/id").to_owned(),
                author: login(c, "/author/login"),
                at: parse_time(at(c, "/createdAt")).unwrap_or(0),
                body: at(c, "/body").to_owned(),
                url: at(c, "/url").to_owned(),
                pending: at(c, "/state") == "PENDING",
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_page_needs_to_be_careful() {
        let extras = parse(
            r#"{"data": {"viewer": {"login": "me"}, "repository": {"pullRequest": {
                "id": "PR", "headRefOid": "abc", "viewerCanMergeAsAdmin": true,
                "reviewThreads": {"nodes": [{
                    "id": "T", "path": "a.rs", "line": 3, "startLine": 2, "diffSide": "RIGHT",
                    "startDiffSide": "LEFT",
                    "comments": {"pageInfo": {"hasNextPage": true}, "nodes": []}
                }]}
            }}}}"#,
        );
        let extras = extras.unwrap_or_default();
        assert_eq!(extras.head_oid, "abc");
        assert!(extras.can_bypass_rules);
        assert!(extras.threads[0].starts_left);
        assert!(extras.threads[0].more_comments);
    }

    /// What GitHub answered for the throwaway repository: one thread from a bot, one comment of
    /// the viewer in a pending review.
    const ANSWER: &str = r#"{"data": {
        "viewer": {"login": "JKamsker"},
        "repository": {
            "mergeCommitAllowed": true, "squashMergeAllowed": true, "rebaseMergeAllowed": false,
            "deleteBranchOnMerge": true, "viewerPermission": "ADMIN",
            "pullRequest": {
                "id": "PR_abc",
                "headRepository": {"nameWithOwner": "JKamsker/gitcraft-review-test"},
                "pending": {"nodes": [{"id": "PRR_pending"}]},
                "reviewThreads": {
                    "pageInfo": {"hasNextPage": true, "endCursor": "CURSOR"},
                    "nodes": [
                        {"id": "PRRT_1", "isResolved": false, "isOutdated": false, "path": "src/calc.py",
                         "line": 9, "originalLine": 9, "diffSide": "RIGHT", "startLine": 9,
                         "viewerCanResolve": true, "viewerCanUnresolve": false, "viewerCanReply": true,
                         "comments": {"nodes": [
                            {"id": "PRRC_1", "body": "Clearer:\n\n```suggestion\nx\n```",
                             "createdAt": "2026-11-02T21:46:30Z", "url": "https://g/1",
                             "state": "SUBMITTED", "author": {"login": "github-actions"}},
                            {"id": "PRRC_2", "body": "Mine", "createdAt": "2026-11-02T21:47:12Z",
                             "url": "https://g/2", "state": "PENDING", "author": {"login": "JKamsker"}}
                         ]}},
                        {"id": "PRRT_2", "isResolved": true, "isOutdated": true, "path": "a.py",
                         "line": null, "originalLine": 3, "diffSide": "LEFT", "startLine": null,
                         "viewerCanResolve": false, "viewerCanUnresolve": true, "viewerCanReply": false,
                         "comments": {"nodes": [
                            {"id": "PRRC_3", "body": "Old", "createdAt": "2026-11-02T21:00:00Z",
                             "url": "https://g/3", "state": "SUBMITTED", "author": null}
                         ]}}
                    ]
                }
            }
        }
    }}"#;

    fn parsed() -> PrExtras {
        parse(ANSWER).unwrap_or_default()
    }

    #[test]
    fn reads_who_may_do_what() {
        let extras = parsed();
        assert_eq!(extras.pr_id, "PR_abc");
        assert_eq!(extras.viewer.as_deref(), Some("JKamsker"));
        assert!(extras.can_write());
        assert!(extras.merge_allowed && extras.squash_allowed && !extras.rebase_allowed);
        assert!(extras.delete_branch_on_merge);
        assert_eq!(
            extras.head_repo.as_deref(),
            Some("JKamsker/gitcraft-review-test")
        );
        assert_eq!(extras.pending_review.as_deref(), Some("PRR_pending"));
    }

    #[test]
    fn only_some_permissions_may_write() {
        for (permission, may) in [
            ("READ", false),
            ("TRIAGE", false),
            ("WRITE", true),
            ("MAINTAIN", true),
            ("ADMIN", true),
        ] {
            let extras = PrExtras {
                permission: permission.to_owned(),
                ..PrExtras::default()
            };
            assert_eq!(extras.can_write(), may, "{permission}");
        }
    }

    #[test]
    fn reads_threads_and_their_comments() {
        let extras = parsed();
        let thread = &extras.threads[0];
        assert_eq!(thread.id, "PRRT_1");
        assert_eq!(
            (thread.path.as_str(), thread.line),
            ("src/calc.py", Some(9))
        );
        assert_eq!(thread.side, DiffSide::Right);
        assert!(!thread.resolved && !thread.outdated);
        assert!(thread.can_resolve && !thread.can_unresolve && thread.can_reply);
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.comments[0].author, "github-actions");
        assert!(!thread.comments[0].pending);
        assert!(thread.comments[1].pending);
    }

    #[test]
    fn an_outdated_thread_keeps_the_line_it_was_on() {
        let extras = parsed();
        let thread = &extras.threads[1];
        assert_eq!(thread.line, Some(3));
        assert_eq!(thread.side, DiffSide::Left);
        assert!(thread.resolved && thread.outdated && thread.can_unresolve);
        assert_eq!(thread.comments[0].author, "ghost");
    }

    #[test]
    fn finds_the_comments_waiting_in_the_pending_review() {
        let extras = parsed();
        let waiting: Vec<&str> = extras
            .pending_comments()
            .map(|(_, comment)| comment.id.as_str())
            .collect();
        assert_eq!(waiting, ["PRRC_2"]);
    }

    #[test]
    fn knows_when_there_are_more_threads() {
        assert_eq!(parsed().next_page.as_deref(), Some("CURSOR"));
    }

    #[test]
    fn a_pull_request_that_does_not_exist_is_an_error() {
        let answer = r#"{"data": {"viewer": {"login": "a"}, "repository": {"pullRequest": null}}}"#;
        assert!(parse(answer).is_err());
        assert!(parse("oops").is_err());
    }
}
