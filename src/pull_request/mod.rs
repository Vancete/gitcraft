//! One pull request in full, through `gh`: its description, build status, commits and comments,
//! and the changes it proposes. Only GitHub for now; the list of `forge.rs` covers GitLab too.

mod html;
mod markdown;
mod parse;
mod tables;

use std::path::Path;

use crate::forge::{self, Forge};
use crate::git::{self, ChangedFile, Credentials, Remote};
use crate::pr_threads::{self, PrExtras};

pub use markdown::{safe_blocks, safe_markdown};
pub(crate) use parse::{at, check, items, login, parse_details, parse_time};
pub use tables::Block;

const FIELDS: &str = "number,title,body,state,isDraft,author,url,baseRefName,baseRefOid,\
    headRefName,headRefOid,createdAt,updatedAt,additions,deletions,changedFiles,mergeable,\
    mergeStateStatus,reviewDecision,labels,statusCheckRollup,commits,comments,reviews";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    Passed,
    Failed,
    Running,
    /// Skipped, cancelled by a newer run, neutral: neither good nor bad.
    #[default]
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    /// The workflow it belongs to or the status's description; may be empty.
    pub detail: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCommit {
    pub oid: String,
    pub headline: String,
    pub author: String,
}

/// Which version of a line a comment is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffSide {
    /// As it was: a removed line.
    Left,
    /// As it is now: an added line, or one that didn't change.
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Comment,
    /// What the review said: "approved", "requested changes"…
    Review(&'static str),
    /// On a line of a file.
    Inline {
        path: String,
        line: Option<u64>,
        side: DiffSide,
        /// The line has changed since, so it isn't where the comment says any more.
        outdated: bool,
        resolved: bool,
    },
}

/// One thing said on the pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub author: String,
    pub at: i64,
    pub body: String,
    pub url: String,
    /// The review this is, if it can be dismissed.
    pub review_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrDetails {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub draft: bool,
    pub author: String,
    pub url: String,
    pub base_branch: String,
    pub base_oid: String,
    pub head_branch: String,
    pub head_oid: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub mergeable: String,
    pub merge_state: String,
    pub review_decision: String,
    pub labels: Vec<String>,
    pub checks: Vec<Check>,
    pub commits: Vec<PrCommit>,
    /// Oldest first.
    pub timeline: Vec<Entry>,
    /// The comments on lines couldn't be read: why.
    pub inline_error: Option<String>,
    /// Threads on lines and what the person logged in may do.
    pub extras: PrExtras,
}

/// The sent comments of the threads, for the conversation.
fn line_entries(extras: &PrExtras) -> Vec<Entry> {
    extras
        .threads
        .iter()
        .flat_map(|thread| {
            thread
                .comments
                .iter()
                .filter(|comment| !comment.pending)
                .map(move |comment| Entry {
                    kind: EntryKind::Inline {
                        path: thread.path.clone(),
                        line: thread.line,
                        side: thread.side,
                        outdated: thread.outdated,
                        resolved: thread.resolved,
                    },
                    author: comment.author.clone(),
                    at: comment.at,
                    body: comment.body.clone(),
                    url: comment.url.clone(),
                    review_id: None,
                })
        })
        .collect()
}

/// What the page says about the reviews so far.
pub fn reviews_label(decision: &str, open: bool, timeline: &[Entry]) -> &'static str {
    let reviewed = timeline
        .iter()
        .any(|entry| matches!(entry.kind, EntryKind::Review(_) | EntryKind::Inline { .. }));
    match decision {
        "APPROVED" => "Approved",
        "CHANGES_REQUESTED" => "Changes requested",
        "REVIEW_REQUIRED" => "Review required",
        _ if reviewed => "Reviewed",
        _ if open => "No reviews yet",
        _ => "No reviews",
    }
}

/// The changes a pull request proposes, as its web page shows them: from the merge base.
#[derive(Clone, Debug)]
pub struct PrFiles {
    pub from: String,
    pub to: String,
    pub files: Vec<ChangedFile>,
}

/// Reads the pull request numbered `number` of the repository at `repo`.
pub fn details(repo: &Path, number: u64) -> Result<PrDetails, String> {
    let details = read_details(repo, number)?;
    // It's read in two goes. A push in between would put the threads' line numbers on another
    // commit than the one the page takes for the pull request's, so that goes again.
    if read_in_step(&details) {
        return Ok(details);
    }
    let details = read_details(repo, number)?;
    if read_in_step(&details) {
        Ok(details)
    } else {
        Err("The pull request changed while it was read. Load it again.".to_owned())
    }
}

/// Whether both goes of reading `details` saw the same commit (or the threads weren't read).
fn read_in_step(details: &PrDetails) -> bool {
    details.inline_error.is_some() || details.extras.head_oid == details.head_oid
}

fn read_details(repo: &Path, number: u64) -> Result<PrDetails, String> {
    let view = forge::tool(
        Forge::GitHub,
        repo,
        &["pr", "view", &number.to_string(), "--json", FIELDS],
    )?;
    let mut details = parse_details(&view)?;
    match repo_of_url(&details.url) {
        Some((host, path)) => match pr_threads::fetch(repo, &host, &path, number) {
            Ok(extras) => {
                details.timeline.extend(line_entries(&extras));
                details.extras = extras;
            }
            Err(e) => details.inline_error = Some(e),
        },
        None => details.inline_error = Some("unexpected pull request URL".to_owned()),
    }
    details.timeline.sort_by_key(|entry| entry.at);
    Ok(details)
}

/// Finds the changes of `details`, fetching its commits first if they aren't here yet.
pub fn files(
    repo: &Path,
    details: &PrDetails,
    remotes: &[Remote],
    credentials: Option<&Credentials>,
) -> Result<PrFiles, String> {
    // They go to git as arguments, so they have to be what they say they are.
    if !is_oid(&details.head_oid) || !is_oid(&details.base_oid) {
        return Err("GitHub didn't say which commits the pull request is between".to_owned());
    }
    let has = |oid: &str| {
        git::run(
            Some(repo),
            &["cat-file", "-e", &format!("{oid}^{{commit}}")],
        )
        .is_ok()
    };
    if !has(&details.head_oid) || !has(&details.base_oid) {
        let remote = remote_for(remotes, &details.url)
            .ok_or("no remote of this repository is the one the pull request belongs to")?;
        // No refs and no FETCH_HEAD: nothing here changes but the objects being available.
        let head = format!("refs/pull/{}/head", details.number);
        let base = format!("refs/heads/{}", details.base_branch);
        // The branches may have moved on since the details were read (a force push even leaves
        // the old commits out), so the very commits are asked for too.
        let mut args = vec![
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            remote,
            &head,
            &base,
        ];
        args.extend([details.head_oid.as_str(), details.base_oid.as_str()]);
        git::run_with(Some(repo), &args, credentials)?;
    }
    let from = git::merge_base(repo, &details.base_oid, &details.head_oid)?;
    let files = git::diff_files(repo, &from, Some(&details.head_oid))?;
    Ok(PrFiles {
        from,
        to: details.head_oid.clone(),
        files,
    })
}

/// Whether `oid` is a full commit id (SHA-1 or SHA-256), not anything git could take for an option.
fn is_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

/// How many checks passed, failed, are running and were skipped.
pub fn tally(checks: &[Check]) -> (usize, usize, usize, usize) {
    let count = |state| checks.iter().filter(|c| c.state == state).count();
    (
        count(CheckState::Passed),
        count(CheckState::Failed),
        count(CheckState::Running),
        count(CheckState::Skipped),
    )
}

/// The host and `owner/repo` of a pull request's URL.
pub(crate) fn repo_of_url(url: &str) -> Option<(String, String)> {
    forge::location(url.split_once("/pull/")?.0)
}

/// The remote of the repository a pull request's URL belongs to. GitHub doesn't mind the case of
/// owner and name, so a clone of `Owner/Repo` may well be from `owner/repo`.
pub(crate) fn remote_for<'a>(remotes: &'a [Remote], url: &str) -> Option<&'a str> {
    let (host, path) = repo_of_url(url)?;
    remotes
        .iter()
        .find(|remote| {
            forge::location(&remote.url)
                .is_some_and(|(h, p)| h == host && p.eq_ignore_ascii_case(&path))
        })
        .map(|remote| remote.name.as_str())
}

/// Whether a link from a pull request may be opened: a click must not run a local file or some
/// other program's address.
pub fn is_web_url(url: &str) -> bool {
    let url = url.trim_start().to_lowercase();
    url.starts_with("https://") || url.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_repository_of_a_pull_request() {
        assert_eq!(
            repo_of_url("https://github.com/o/r/pull/12"),
            Some(("github.com".to_owned(), "o/r".to_owned()))
        );
        assert_eq!(repo_of_url("https://github.com/o/r"), None);
    }

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_owned(),
            url: url.to_owned(),
        }
    }

    #[test]
    fn finds_the_remote_of_a_pull_request_whatever_its_url_looks_like() {
        let remotes = [
            remote("fork", "https://github.com/me/r.git"),
            remote("up", "git@github.com:o/r.git"),
        ];
        assert_eq!(
            remote_for(&remotes, "https://github.com/o/r/pull/1"),
            Some("up")
        );
        assert_eq!(remote_for(&remotes, "https://github.com/x/y/pull/1"), None);
        assert_eq!(
            remote_for(&remotes, "https://github.com/O/R/pull/1"),
            Some("up")
        );
    }

    #[test]
    fn only_full_commit_ids_go_to_git() {
        assert!(is_oid(&"a1".repeat(20)));
        assert!(is_oid(&"F".repeat(64)));
        assert!(!is_oid(""));
        assert!(!is_oid("h0"));
        assert!(!is_oid(&format!("--upload-pack={}", "a".repeat(26))));
        assert!(!is_oid(&"g".repeat(40)));
    }

    fn entry(kind: EntryKind) -> Entry {
        Entry {
            kind,
            author: "ana".to_owned(),
            at: 0,
            body: String::new(),
            url: String::new(),
            review_id: None,
        }
    }

    #[test]
    fn a_review_without_a_verdict_still_counts_as_a_review() {
        let reviewed = [entry(EntryKind::Review("reviewed"))];
        assert_eq!(reviews_label("", true, &reviewed), "Reviewed");
        let on_a_line = [entry(EntryKind::Inline {
            path: "a.rs".to_owned(),
            line: Some(1),
            side: DiffSide::Right,
            outdated: false,
            resolved: false,
        })];
        assert_eq!(reviews_label("", true, &on_a_line), "Reviewed");
        let talk = [entry(EntryKind::Comment)];
        assert_eq!(reviews_label("", true, &talk), "No reviews yet");
    }

    #[test]
    fn a_verdict_says_more_than_that_there_was_a_review() {
        let reviewed = [entry(EntryKind::Review("approved"))];
        assert_eq!(reviews_label("APPROVED", true, &reviewed), "Approved");
    }

    #[test]
    fn no_review_is_coming_on_a_finished_pull_request() {
        assert_eq!(reviews_label("", false, &[]), "No reviews");
    }

    #[test]
    fn threads_read_on_another_commit_are_not_shown_on_this_one() {
        let mut details = PrDetails {
            head_oid: "h1".to_owned(),
            ..PrDetails::default()
        };
        details.extras.head_oid = "h1".to_owned();
        assert!(read_in_step(&details));
        details.extras.head_oid = "h0".to_owned();
        assert!(!read_in_step(&details));
        // Without threads there are no line numbers to put on the wrong commit.
        details.inline_error = Some("offline".to_owned());
        assert!(read_in_step(&details));
    }

    #[test]
    fn only_web_links_may_be_opened() {
        assert!(is_web_url("https://github.com/o/r"));
        assert!(is_web_url("HTTP://example.com"));
        assert!(!is_web_url("file:///C:/Windows/System32/calc.exe"));
        assert!(!is_web_url("javascript:alert(1)"));
        assert!(!is_web_url("ms-settings:"));
        assert!(!is_web_url(""));
    }
}
