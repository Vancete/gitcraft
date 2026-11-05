//! Reading what `gh pr view` says about a pull request into its details.

use serde_json::Value;

use super::{Check, CheckState, Entry, EntryKind, PrCommit, PrDetails};
use crate::pr_threads::PrExtras;

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
        extras: PrExtras::default(),
    })
}

/// The string at `pointer`, or nothing.
pub(crate) fn at<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The array at `pointer`, or nothing.
pub(crate) fn items<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The account at `pointer`: a deleted one has none, and GitHub calls those "ghost".
pub(crate) fn login(v: &Value, pointer: &str) -> String {
    match at(v, pointer) {
        "" => "ghost".to_owned(),
        name => name.strip_prefix("app/").unwrap_or(name).to_owned(),
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
        review_id: None,
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
    let dismissable = matches!(at(v, "/state"), "APPROVED" | "CHANGES_REQUESTED");
    let id = at(v, "/id");
    Some(Entry {
        kind: EntryKind::Review(verdict),
        author: login(v, "/author/login"),
        at: parse_time(at(v, "/submittedAt")).unwrap_or(0),
        body: body.to_owned(),
        url: at(v, "/url").to_owned(),
        review_id: (dismissable && !id.is_empty()).then(|| id.to_owned()),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::tally;

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
            {"id": "PRR_di", "author": {"login": "di"}, "body": "", "state": "APPROVED",
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
    fn approvals_and_requests_for_changes_can_be_dismissed() {
        let pr = parsed();
        let approval = pr.timeline.iter().find(|e| e.author == "di");
        assert_eq!(
            approval.and_then(|e| e.review_id.as_deref()),
            Some("PRR_di")
        );
        // Without an id there's nothing to refer to.
        let request = pr.timeline.iter().find(|e| e.author == "cy");
        assert_eq!(request.and_then(|e| e.review_id.as_deref()), None);
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
    fn an_app_goes_by_its_name_alone() {
        let v: Value = serde_json::json!({"author": {"login": "app/github-actions"}});
        assert_eq!(login(&v, "/author/login"), "github-actions");
        let person: Value = serde_json::json!({"author": {"login": "ana"}});
        assert_eq!(login(&person, "/author/login"), "ana");
    }
}
