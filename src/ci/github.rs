//! GitHub Actions through `gh`: the rolled-up status of many commits in one GraphQL query.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{Project, Status};
use crate::forge::{self, Forge};
use crate::pull_request::{CheckState, at, check, items};

/// What the rollup of a commit (or a commit status of the older kind) says, in GraphQL's
/// upper case: `SUCCESS`, `FAILURE`, `PENDING`…
pub(crate) fn rollup_state(state: &str) -> CheckState {
    match state.to_ascii_uppercase().as_str() {
        "SUCCESS" => CheckState::Passed,
        "FAILURE" | "ERROR" => CheckState::Failed,
        "PENDING" | "EXPECTED" => CheckState::Running,
        _ => CheckState::Skipped,
    }
}

/// The state of a check run, a workflow run, a job or a step from its status and conclusion,
/// upper case (GraphQL) or lower case (REST and `gh run`) alike.
pub(crate) fn state(status: &str, conclusion: &str) -> CheckState {
    let status = status.to_ascii_uppercase();
    let conclusion = conclusion.to_ascii_uppercase();
    match (status.as_str(), conclusion.as_str()) {
        ("COMPLETED", "SUCCESS") => CheckState::Passed,
        ("COMPLETED", "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED") => {
            CheckState::Failed
        }
        ("COMPLETED", _) => CheckState::Skipped,
        _ => CheckState::Running,
    }
}

/// What a commit's rollup is read with: its state, and what it is made of for the tooltip.
const ROLLUP: &str = "state contexts(first: 50) { nodes { __typename \
    ... on CheckRun { name status conclusion detailsUrl \
        checkSuite { workflowRun { workflow { name } } } } \
    ... on StatusContext { context state description targetUrl } } }";

/// So many commits go into one query.
const PER_QUERY: usize = 40;

pub(super) fn statuses(
    repo: &Path,
    project: &Project,
    shas: &[&String],
) -> Result<HashMap<String, Status>, String> {
    let (owner, name) = project
        .path
        .split_once('/')
        .ok_or("not a GitHub repository")?;
    // GraphQL wants full commit ids.
    let shas: Vec<&String> = shas
        .iter()
        .copied()
        .filter(|sha| matches!(sha.len(), 40 | 64))
        .collect();
    let mut out = HashMap::new();
    for chunk in shas.chunks(PER_QUERY) {
        let query = format!("query={}", query(chunk));
        let (owner, name) = (format!("owner={owner}"), format!("name={name}"));
        let mut args = vec!["api", "graphql", "-f", &query, "-f", &owner, "-f", &name];
        if project.host != "github.com" {
            args.extend(["--hostname", project.host.as_str()]);
        }
        let json = forge::tool(Forge::GitHub, repo, &args)?;
        out.extend(parse_statuses(&json)?);
    }
    Ok(out)
}

/// One aliased `object` per commit, all in the one repository.
fn query(shas: &[&String]) -> String {
    let mut query = String::from(
        "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) {",
    );
    for (i, sha) in shas.iter().enumerate() {
        query.push_str(&format!(
            " c{i}: object(oid: \"{sha}\") {{ ... on Commit {{ oid statusCheckRollup {{ {ROLLUP} }} }} }}"
        ));
    }
    query.push_str(" } }");
    query
}

fn parse_statuses(json: &str) -> Result<HashMap<String, Status>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("unexpected gh output: {e}"))?;
    let Some(commits) = v.pointer("/data/repository").and_then(Value::as_object) else {
        return Ok(HashMap::new());
    };
    Ok(commits
        .values()
        .filter_map(|commit| {
            // No rollup: nothing ran on that commit (or it isn't on GitHub at all).
            let rollup = commit.get("statusCheckRollup").filter(|r| !r.is_null())?;
            let checks = items(rollup, "/contexts/nodes")
                .iter()
                .map(|node| {
                    let mut check = check(node);
                    if check.detail.is_empty() {
                        check.detail = at(node, "/checkSuite/workflowRun/workflow/name").to_owned();
                    }
                    check
                })
                .collect();
            let status = Status {
                state: rollup_state(at(rollup, "/state")),
                checks,
            };
            Some((at(commit, "/oid").to_owned(), status))
        })
        .filter(|(oid, _)| !oid.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_read_the_same_in_either_case() {
        assert_eq!(state("completed", "success"), CheckState::Passed);
        assert_eq!(state("COMPLETED", "SUCCESS"), CheckState::Passed);
        assert_eq!(state("completed", "timed_out"), CheckState::Failed);
        assert_eq!(state("completed", "cancelled"), CheckState::Skipped);
        assert_eq!(state("in_progress", ""), CheckState::Running);
        assert_eq!(state("queued", ""), CheckState::Running);
        assert_eq!(rollup_state("ERROR"), CheckState::Failed);
        assert_eq!(rollup_state("EXPECTED"), CheckState::Running);
        assert_eq!(rollup_state("success"), CheckState::Passed);
    }

    #[test]
    fn one_query_asks_for_every_commit() {
        let shas = ["a".repeat(40), "b".repeat(40)];
        let shas: Vec<&String> = shas.iter().collect();
        let query = query(&shas);
        assert!(query.contains(&format!("c0: object(oid: \"{}\")", "a".repeat(40))));
        assert!(query.contains(&format!("c1: object(oid: \"{}\")", "b".repeat(40))));
        assert!(query.contains("repository(owner: $owner, name: $name)"));
    }

    #[test]
    fn reads_the_rollup_of_each_commit() {
        let json = r#"{"data":{"repository":{
            "c0":{"oid":"aaa","statusCheckRollup":{"state":"FAILURE","contexts":{"nodes":[
                {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"FAILURE",
                 "detailsUrl":"https://github.com/o/r/actions/runs/1/job/2",
                 "checkSuite":{"workflowRun":{"workflow":{"name":"CI"}}}},
                {"__typename":"CheckRun","name":"test","status":"IN_PROGRESS","conclusion":null,
                 "detailsUrl":"https://github.com/o/r/actions/runs/1/job/3",
                 "checkSuite":{"workflowRun":{"workflow":{"name":"CI"}}}},
                {"__typename":"StatusContext","context":"CodeRabbit","state":"SUCCESS",
                 "description":"Review skipped","targetUrl":null}]}}},
            "c1":{"oid":"bbb","statusCheckRollup":null},
            "c2":null}}}"#;
        let statuses = parse_statuses(json).unwrap_or_default();
        assert_eq!(statuses.len(), 1);
        let status = &statuses["aaa"];
        assert_eq!(status.state, CheckState::Failed);
        let checks: Vec<(&str, CheckState, &str)> = status
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.state, c.detail.as_str()))
            .collect();
        assert_eq!(
            checks,
            [
                ("lint", CheckState::Failed, "CI"),
                ("test", CheckState::Running, "CI"),
                ("CodeRabbit", CheckState::Passed, "Review skipped"),
            ]
        );
    }

    #[test]
    fn output_that_is_not_json_is_an_error() {
        assert!(parse_statuses("oops").is_err());
    }
}
