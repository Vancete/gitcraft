//! One pull request in full, through `gh`: its description, build status, commits and comments,
//! and the changes it proposes. Only GitHub for now; the list of `forge.rs` covers GitLab too.

use std::path::Path;

use serde_json::Value;

use crate::forge::{self, Forge};
use crate::git::{self, ChangedFile, Credentials, Remote};

const FIELDS: &str = "number,title,body,state,isDraft,author,url,baseRefName,baseRefOid,\
    headRefName,headRefOid,createdAt,updatedAt,additions,deletions,changedFiles,mergeable,\
    mergeStateStatus,reviewDecision,labels,statusCheckRollup,commits,comments,reviews";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Passed,
    Failed,
    Running,
    /// Skipped, cancelled by a newer run, neutral: neither good nor bad.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Comment,
    /// What the review said: "approved", "requested changes"…
    Review(&'static str),
    /// On a line of a file.
    Inline {
        path: String,
        line: Option<u64>,
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
    let view = forge::tool(
        Forge::GitHub,
        repo,
        &["pr", "view", &number.to_string(), "--json", FIELDS],
    )?;
    let mut details = parse_details(&view)?;
    match inline_comments(repo, &details) {
        Ok(entries) => details.timeline.extend(entries),
        Err(e) => details.inline_error = Some(e),
    }
    details.timeline.sort_by_key(|entry| entry.at);
    Ok(details)
}

fn inline_comments(repo: &Path, details: &PrDetails) -> Result<Vec<Entry>, String> {
    let (host, path) = repo_of_url(&details.url).ok_or("unexpected pull request URL")?;
    let endpoint = format!(
        "repos/{path}/pulls/{}/comments?per_page=100",
        details.number
    );
    let mut args = vec!["api", endpoint.as_str(), "--paginate", "--slurp"];
    if host != "github.com" {
        args.extend(["--hostname", host.as_str()]);
    }
    Ok(parse_inline_comments(&forge::tool(
        Forge::GitHub,
        repo,
        &args,
    )?))
}

/// Finds the changes of `details`, fetching its commits first if they aren't here yet.
pub fn files(
    repo: &Path,
    details: &PrDetails,
    remotes: &[Remote],
    credentials: Option<&Credentials>,
) -> Result<PrFiles, String> {
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
        git::run_with(
            Some(repo),
            &[
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                remote,
                &head,
                &base,
            ],
            credentials,
        )?;
    }
    let from = git::merge_base(repo, &details.base_oid, &details.head_oid)?;
    let files = git::diff_files(repo, &from, Some(&details.head_oid))?;
    Ok(PrFiles {
        from,
        to: details.head_oid.clone(),
        files,
    })
}

pub(crate) fn parse_details(json: &str) -> Result<PrDetails, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected output of gh: {e}"))?;
    let text = |key: &str| at(&v, key).to_owned();
    let number = |key: &str| v.pointer(key).and_then(Value::as_u64).unwrap_or(0);
    let time = |key: &str| parse_time(at(&v, key)).unwrap_or(0);

    let mut timeline: Vec<Entry> = items(&v, "/comments").iter().map(comment).collect();
    timeline.extend(items(&v, "/reviews").iter().filter_map(review));
    timeline.sort_by_key(|entry| entry.at);
    Ok(PrDetails {
        number: number("/number"),
        title: text("/title"),
        body: text("/body"),
        state: text("/state"),
        draft: v
            .pointer("/isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        author: login(&v, "/author/login"),
        url: text("/url"),
        base_branch: text("/baseRefName"),
        base_oid: text("/baseRefOid"),
        head_branch: text("/headRefName"),
        head_oid: text("/headRefOid"),
        created_at: time("/createdAt"),
        updated_at: time("/updatedAt"),
        additions: number("/additions"),
        deletions: number("/deletions"),
        changed_files: number("/changedFiles"),
        mergeable: text("/mergeable"),
        merge_state: text("/mergeStateStatus"),
        review_decision: text("/reviewDecision"),
        labels: items(&v, "/labels")
            .iter()
            .map(|label| at(label, "/name").to_owned())
            .collect(),
        checks: items(&v, "/statusCheckRollup").iter().map(check).collect(),
        commits: items(&v, "/commits").iter().map(commit).collect(),
        timeline,
        inline_error: None,
    })
}

/// The string at `pointer`, or nothing.
fn at<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The array at `pointer`, or nothing.
fn items<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The account at `pointer`: a deleted one has none, and GitHub calls those "ghost".
fn login(v: &Value, pointer: &str) -> String {
    match at(v, pointer) {
        "" => "ghost".to_owned(),
        name => name.to_owned(),
    }
}

fn check(v: &Value) -> Check {
    if at(v, "/__typename") == "StatusContext" {
        let state = match at(v, "/state") {
            "SUCCESS" => CheckState::Passed,
            "FAILURE" | "ERROR" => CheckState::Failed,
            "PENDING" | "EXPECTED" => CheckState::Running,
            _ => CheckState::Skipped,
        };
        return Check {
            name: at(v, "/context").to_owned(),
            state,
            detail: at(v, "/description").to_owned(),
            url: at(v, "/targetUrl").to_owned(),
        };
    }
    let state = match (at(v, "/status"), at(v, "/conclusion")) {
        ("COMPLETED", "SUCCESS") => CheckState::Passed,
        ("COMPLETED", "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED") => {
            CheckState::Failed
        }
        ("COMPLETED", _) => CheckState::Skipped,
        _ => CheckState::Running,
    };
    Check {
        name: at(v, "/name").to_owned(),
        state,
        detail: at(v, "/workflowName").to_owned(),
        url: at(v, "/detailsUrl").to_owned(),
    }
}

fn commit(v: &Value) -> PrCommit {
    let author = match at(v, "/authors/0/login") {
        "" => at(v, "/authors/0/name"),
        name => name,
    };
    PrCommit {
        oid: at(v, "/oid").to_owned(),
        headline: at(v, "/messageHeadline").to_owned(),
        author: author.to_owned(),
    }
}

fn comment(v: &Value) -> Entry {
    Entry {
        kind: EntryKind::Comment,
        author: login(v, "/author/login"),
        at: parse_time(at(v, "/createdAt")).unwrap_or(0),
        body: at(v, "/body").to_owned(),
        url: at(v, "/url").to_owned(),
    }
}

/// A review, if it says anything: one that only holds comments on lines has no text of its own.
fn review(v: &Value) -> Option<Entry> {
    let body = at(v, "/body");
    let verdict = match at(v, "/state") {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "had a review dismissed",
        "COMMENTED" if !body.is_empty() => "reviewed",
        _ => return None,
    };
    Some(Entry {
        kind: EntryKind::Review(verdict),
        author: login(v, "/author/login"),
        at: parse_time(at(v, "/submittedAt")).unwrap_or(0),
        body: body.to_owned(),
        url: at(v, "/url").to_owned(),
    })
}

/// `gh api --paginate --slurp`: an array of pages.
pub(crate) fn parse_inline_comments(json: &str) -> Vec<Entry> {
    let Ok(Value::Array(pages)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    pages
        .iter()
        .flat_map(|page| page.as_array().map_or(&[][..], Vec::as_slice))
        .map(|v| {
            let line = |key: &str| v.pointer(key).and_then(Value::as_u64);
            Entry {
                kind: EntryKind::Inline {
                    path: at(v, "/path").to_owned(),
                    line: line("/line").or_else(|| line("/original_line")),
                },
                author: login(v, "/user/login"),
                at: parse_time(at(v, "/created_at")).unwrap_or(0),
                body: at(v, "/body").to_owned(),
                url: at(v, "/html_url").to_owned(),
            }
        })
        .collect()
}

/// Seconds since the epoch of a UTC time like `2026-11-02T20:37:34Z`.
pub(crate) fn parse_time(iso: &str) -> Option<i64> {
    let (date, time) = iso.strip_suffix('Z')?.split_once('T')?;
    let numbers = |text: &str, separator: char| -> Option<[i64; 3]> {
        let mut parts = text.split(separator).map(|n| n.parse::<i64>().ok());
        Some([parts.next()??, parts.next()??, parts.next()??])
    };
    let [year, month, day] = numbers(date, '-')?;
    let [hour, minute, second] = numbers(time.split('.').next()?, ':')?;
    // Days from the civil calendar, counted from the March before: leap days come last then.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
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

/// The remote of the repository a pull request's URL belongs to.
pub(crate) fn remote_for<'a>(remotes: &'a [Remote], url: &str) -> Option<&'a str> {
    let repo = repo_of_url(url)?;
    remotes
        .iter()
        .find(|remote| forge::location(&remote.url).as_ref() == Some(&repo))
        .map(|remote| remote.name.as_str())
}

/// Whether a link from a pull request may be opened: a click must not run a local file or some
/// other program's address.
pub fn is_web_url(url: &str) -> bool {
    let url = url.trim_start().to_lowercase();
    url.starts_with("https://") || url.starts_with("http://")
}

/// `body` made safe and fit to show: without HTML comments (what templates leave behind), with
/// `<img>` tags as images, and images from anywhere but `host` as links, so that reading a pull
/// request doesn't make the app contact a server its author chose.
pub fn safe_markdown(body: &str, host: &str) -> String {
    let body = without_comments(body);
    let body = img_tags_to_markdown(&body);
    images_to_links(&body, host)
}

/// The most images in one row of a table of `markdown`: those have to share the width.
pub fn images_per_row(markdown: &str) -> usize {
    markdown
        .lines()
        .filter(|line| line.trim_start().starts_with('|'))
        .map(|line| line.matches("![").count())
        .max()
        .unwrap_or(0)
        .max(1)
}

fn without_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn img_tags_to_markdown(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<img") {
        let Some(len) = rest[start..].find('>') else {
            break;
        };
        out.push_str(&rest[..start]);
        let tag = &rest[start..start + len];
        if let Some(src) = attribute(tag, "src") {
            out.push_str(&format!(
                "![{}]({src})",
                attribute(tag, "alt").unwrap_or("")
            ));
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// The value of `name="…"` in an HTML tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

fn images_to_links(body: &str, host: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("![") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let parts = after.find(']').and_then(|close| {
            let target = after[close + 1..].strip_prefix('(')?;
            let end = target.find(')')?;
            Some((close, &target[..end], close + 2 + end + 1))
        });
        let Some((close, target, used)) = parts else {
            out.push_str("![");
            rest = after;
            continue;
        };
        let alt = &after[..close];
        let url = target.split_whitespace().next().unwrap_or("");
        if is_trusted_image(url, host) {
            out.push_str(&format!("![{alt}]({target})"));
        } else {
            let label = if alt.is_empty() { "image" } else { alt };
            out.push_str(&format!("[{label}]({target})"));
        }
        rest = &after[used..];
    }
    out.push_str(rest);
    out
}

fn is_trusted_image(url: &str, host: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let image_host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    image_host == host || (host == "github.com" && image_host.ends_with(".githubusercontent.com"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"{
        "number": 12, "title": "Add search", "body": "Adds **search**.", "state": "OPEN",
        "isDraft": true, "author": {"login": "ana"}, "url": "https://github.com/o/r/pull/12",
        "baseRefName": "master", "baseRefOid": "b0", "headRefName": "feat/search",
        "headRefOid": "h0", "createdAt": "2026-11-02T20:00:00Z", "updatedAt": "2026-11-02T21:00:00Z",
        "additions": 10, "deletions": 2, "changedFiles": 3, "mergeable": "CONFLICTING",
        "mergeStateStatus": "DIRTY", "reviewDecision": "CHANGES_REQUESTED",
        "labels": [{"name": "bug"}, {"name": "ui"}],
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "test", "workflowName": "CI", "status": "COMPLETED",
             "conclusion": "SUCCESS", "detailsUrl": "https://ci/1"},
            {"__typename": "CheckRun", "name": "lint", "workflowName": "CI", "status": "COMPLETED",
             "conclusion": "FAILURE", "detailsUrl": "https://ci/2"},
            {"__typename": "CheckRun", "name": "e2e", "workflowName": "CI", "status": "IN_PROGRESS",
             "conclusion": "", "detailsUrl": "https://ci/3"},
            {"__typename": "CheckRun", "name": "docs", "workflowName": "CI", "status": "COMPLETED",
             "conclusion": "SKIPPED", "detailsUrl": "https://ci/4"},
            {"__typename": "CheckRun", "name": "old", "workflowName": "CI", "status": "COMPLETED",
             "conclusion": "TIMED_OUT", "detailsUrl": "https://ci/5"},
            {"__typename": "StatusContext", "context": "ci/legacy", "state": "PENDING",
             "description": "Waiting", "targetUrl": "https://ci/6"},
            {"__typename": "StatusContext", "context": "ci/ok", "state": "SUCCESS",
             "description": "", "targetUrl": "https://ci/7"}
        ],
        "commits": [{"oid": "c1", "messageHeadline": "feat: search",
                     "authors": [{"login": "ana", "name": "Ana"}]}],
        "comments": [{"author": {"login": "bo"}, "body": "Nice", "createdAt": "2026-11-02T20:30:00Z",
                      "url": "https://github.com/o/r/pull/12#c1"}],
        "reviews": [
            {"author": {"login": "cy"}, "body": "Please fix", "state": "CHANGES_REQUESTED",
             "submittedAt": "2026-11-02T20:10:00Z"},
            {"author": {"login": "cy"}, "body": "", "state": "COMMENTED",
             "submittedAt": "2026-11-02T20:11:00Z"},
            {"author": {"login": "di"}, "body": "", "state": "APPROVED",
             "submittedAt": "2026-11-02T20:50:00Z"}
        ]
    }"#;

    fn parsed() -> PrDetails {
        parse_details(VIEW).unwrap_or_default()
    }

    #[test]
    fn reads_the_pull_request_itself() {
        let pr = parsed();
        assert_eq!(pr.number, 12);
        assert_eq!(pr.title, "Add search");
        assert_eq!(pr.body, "Adds **search**.");
        assert!(pr.draft);
        assert_eq!(pr.author, "ana");
        assert_eq!(
            (pr.base_branch.as_str(), pr.base_oid.as_str()),
            ("master", "b0")
        );
        assert_eq!(
            (pr.head_branch.as_str(), pr.head_oid.as_str()),
            ("feat/search", "h0")
        );
        assert_eq!((pr.additions, pr.deletions, pr.changed_files), (10, 2, 3));
        assert_eq!(pr.mergeable, "CONFLICTING");
        assert_eq!(pr.review_decision, "CHANGES_REQUESTED");
        assert_eq!(pr.labels, ["bug", "ui"]);
        assert_eq!(pr.commits[0].headline, "feat: search");
        assert_eq!(pr.commits[0].author, "ana");
    }

    #[test]
    fn output_that_is_not_json_is_an_error() {
        assert!(parse_details("oops").is_err());
    }

    #[test]
    fn understands_both_kinds_of_check() {
        let pr = parsed();
        let states: Vec<(&str, CheckState)> = pr
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.state))
            .collect();
        assert_eq!(
            states,
            [
                ("test", CheckState::Passed),
                ("lint", CheckState::Failed),
                ("e2e", CheckState::Running),
                ("docs", CheckState::Skipped),
                ("old", CheckState::Failed),
                ("ci/legacy", CheckState::Running),
                ("ci/ok", CheckState::Passed),
            ]
        );
        assert_eq!(pr.checks[0].url, "https://ci/1");
        assert_eq!(pr.checks[0].detail, "CI");
        assert_eq!(pr.checks[5].detail, "Waiting");
    }

    #[test]
    fn counts_the_checks_by_outcome() {
        assert_eq!(tally(&parsed().checks), (2, 2, 2, 1));
    }

    #[test]
    fn the_timeline_has_comments_and_reviews_that_say_something() {
        let pr = parsed();
        let said: Vec<(&str, &EntryKind)> = pr
            .timeline
            .iter()
            .map(|e| (e.author.as_str(), &e.kind))
            .collect();
        assert_eq!(
            said,
            [
                ("cy", &EntryKind::Review("requested changes")),
                ("bo", &EntryKind::Comment),
                ("di", &EntryKind::Review("approved")),
            ]
        );
    }

    #[test]
    fn reads_comments_on_lines_across_pages() {
        let json = r#"[[{"user": {"login": "ana"}, "body": "Typo", "path": "src/a.rs", "line": 7,
                         "created_at": "2026-11-02T20:00:00Z", "html_url": "https://g/1"}],
                       [{"user": {"login": "bo"}, "body": "Old", "path": "src/b.rs", "line": null,
                         "original_line": 3, "created_at": "2026-11-02T20:05:00Z",
                         "html_url": "https://g/2"},
                        {"user": null, "body": "Gone", "path": "c.rs", "line": null,
                         "created_at": "2026-11-02T20:06:00Z", "html_url": "https://g/3"}]]"#;
        let entries = parse_inline_comments(json);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].author, "ana");
        assert_eq!(
            entries[0].kind,
            EntryKind::Inline {
                path: "src/a.rs".to_owned(),
                line: Some(7)
            }
        );
        assert_eq!(
            entries[1].kind,
            EntryKind::Inline {
                path: "src/b.rs".to_owned(),
                line: Some(3)
            }
        );
        assert_eq!(
            entries[2].kind,
            EntryKind::Inline {
                path: "c.rs".to_owned(),
                line: None
            }
        );
    }

    #[test]
    fn inline_comments_that_are_not_json_are_none() {
        assert_eq!(parse_inline_comments("oops"), Vec::new());
    }

    #[test]
    fn parses_utc_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_time("2026-11-02T20:37:34Z"), Some(1_793_651_854));
        assert_eq!(parse_time("not a time"), None);
        assert_eq!(parse_time(""), None);
    }

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
    }

    #[test]
    fn images_from_the_pull_requests_host_stay() {
        let body = "![shot](https://github.com/user-attachments/assets/a)";
        assert_eq!(safe_markdown(body, "github.com"), body);
        let raw = "![shot](https://raw.githubusercontent.com/me/r/1/a.png)";
        assert_eq!(safe_markdown(raw, "github.com"), raw);
    }

    #[test]
    fn images_from_elsewhere_become_links() {
        assert_eq!(
            safe_markdown("a ![pixel](https://evil.example/x.png) b", "github.com"),
            "a [pixel](https://evil.example/x.png) b"
        );
        // Only the host counts, not what the address merely contains.
        assert_eq!(
            safe_markdown("![x](https://github.com.evil.example/x.png)", "github.com"),
            "[x](https://github.com.evil.example/x.png)"
        );
        assert_eq!(
            safe_markdown("![x](http://github.com/x.png)", "github.com"),
            "[x](http://github.com/x.png)"
        );
    }

    #[test]
    fn an_image_link_without_text_still_has_something_to_click() {
        assert_eq!(
            safe_markdown("![](https://evil.example/x.png)", "github.com"),
            "[image](https://evil.example/x.png)"
        );
    }

    #[test]
    fn img_tags_become_images_by_the_same_rule() {
        assert_eq!(
            safe_markdown(
                r#"<img width="400" alt="Shot" src="https://github.com/user-attachments/assets/a" />"#,
                "github.com"
            ),
            "![Shot](https://github.com/user-attachments/assets/a)"
        );
        assert_eq!(
            safe_markdown(r#"<img src="https://evil.example/x.png">"#, "github.com"),
            "[image](https://evil.example/x.png)"
        );
    }

    #[test]
    fn html_comments_are_left_out() {
        assert_eq!(
            safe_markdown("a<!-- template\nhint -->b", "github.com"),
            "ab"
        );
    }

    #[test]
    fn plain_text_with_exclamation_marks_is_left_alone() {
        let body = "Wow![not an image] and [link](https://a.example) ![unfinished](";
        assert_eq!(safe_markdown(body, "github.com"), body);
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

    #[test]
    fn counts_the_images_sharing_a_table_row() {
        assert_eq!(images_per_row("no tables, ![a](b) ![c](d)"), 1);
        assert_eq!(
            images_per_row(
                "| A | B |
|---|---|
| ![a](b) | ![c](d) |"
            ),
            2
        );
        assert_eq!(images_per_row("| just | text |"), 1);
        assert_eq!(
            images_per_row(
                "  | ![a](b) |
| ![a](b) | ![c](d) | ![e](f) |"
            ),
            3
        );
    }

    #[test]
    fn enterprise_hosts_trust_themselves() {
        let body = "![a](https://git.corp.example/x.png)";
        assert_eq!(safe_markdown(body, "git.corp.example"), body);
    }
}
