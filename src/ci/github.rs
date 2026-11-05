//! GitHub Actions through `gh`: the rolled-up status of many commits in one GraphQL query, the
//! workflow runs of a commit and the jobs and steps of a run.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{Job, Project, Run, Status, Step, time};
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

/// `owner/repo`, with the host in front when it isn't github.com.
fn repo_arg(project: &Project) -> String {
    if project.host == "github.com" {
        project.path.clone()
    } else {
        format!("{}/{}", project.host, project.path)
    }
}

const RUN_FIELDS: &str = "databaseId,name,workflowName,displayTitle,event,status,conclusion,\
    headBranch,headSha,number,attempt,startedAt,updatedAt,url";

pub(super) fn runs(repo: &Path, project: &Project, sha: &str) -> Result<Vec<Run>, String> {
    let json = forge::tool(
        Forge::GitHub,
        repo,
        &[
            "run",
            "list",
            "-R",
            &repo_arg(project),
            "--commit",
            sha,
            "--limit",
            "50",
            "--json",
            RUN_FIELDS,
        ],
    )?;
    parse_runs(&json)
}

fn parse_runs(json: &str) -> Result<Vec<Run>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("unexpected gh output: {e}"))?;
    let number = |run: &Value, key: &str| run.get(key).and_then(Value::as_u64).unwrap_or(0);
    let runs = v.as_array().map_or(&[][..], Vec::as_slice);
    Ok(runs
        .iter()
        .map(|run| {
            let state = state(at(run, "/status"), at(run, "/conclusion"));
            let name = match at(run, "/name") {
                "" => at(run, "/workflowName"),
                name => name,
            };
            Run {
                id: number(run, "databaseId"),
                name: name.to_owned(),
                title: at(run, "/displayTitle").to_owned(),
                number: number(run, "number"),
                attempt: number(run, "attempt"),
                event: at(run, "/event").to_owned(),
                branch: at(run, "/headBranch").to_owned(),
                sha: at(run, "/headSha").to_owned(),
                state,
                started: time(at(run, "/startedAt")),
                finished: (state != CheckState::Running)
                    .then(|| time(at(run, "/updatedAt")))
                    .flatten(),
                url: at(run, "/url").to_owned(),
            }
        })
        .collect())
}

pub(super) fn jobs(repo: &Path, project: &Project, run: u64) -> Result<Vec<Job>, String> {
    parse_jobs(&run_command(
        repo,
        project,
        "view",
        run,
        &["--json", "jobs"],
    )?)
}

fn parse_jobs(json: &str) -> Result<Vec<Job>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("unexpected gh output: {e}"))?;
    let when = |item: &Value, key: &str| time(at(item, key));
    Ok(items(&v, "/jobs")
        .iter()
        .map(|job| Job {
            id: job.get("databaseId").and_then(Value::as_u64).unwrap_or(0),
            name: at(job, "/name").to_owned(),
            stage: String::new(),
            state: state(at(job, "/status"), at(job, "/conclusion")),
            started: when(job, "/startedAt"),
            finished: when(job, "/completedAt"),
            url: at(job, "/url").to_owned(),
            steps: items(job, "/steps")
                .iter()
                .map(|step| Step {
                    name: at(step, "/name").to_owned(),
                    state: state(at(step, "/status"), at(step, "/conclusion")),
                    started: when(step, "/startedAt"),
                    finished: when(step, "/completedAt"),
                })
                .collect(),
        })
        .collect())
}

/// The run of a check's page, `…/actions/runs/<id>/job/<id>`.
pub(crate) fn run_of_url(url: &str) -> Option<u64> {
    let (_, rest) = url.split_once("/actions/runs/")?;
    rest.split('/').next()?.parse().ok()
}

/// `gh run <command> <run> -R <repo>` and more arguments.
fn run_command(
    repo: &Path,
    project: &Project,
    command: &str,
    run: u64,
    more: &[&str],
) -> Result<String, String> {
    let (run, repo_arg) = (run.to_string(), repo_arg(project));
    let mut args = vec!["run", command, run.as_str(), "-R", repo_arg.as_str()];
    args.extend(more);
    forge::tool(Forge::GitHub, repo, &args)
}

pub(super) fn rerun_failed(repo: &Path, project: &Project, run: u64) -> Result<(), String> {
    run_command(repo, project, "rerun", run, &["--failed"]).map(|_| ())
}

pub(super) fn cancel(repo: &Path, project: &Project, run: u64) -> Result<(), String> {
    run_command(repo, project, "cancel", run, &[]).map(|_| ())
}

pub(super) fn failed_log(repo: &Path, project: &Project, run: u64) -> Result<String, String> {
    run_command(repo, project, "view", run, &["--log-failed"])
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
    fn checks_of_actions_name_their_run() {
        let url = "https://github.com/o/r/actions/runs/41751854039/job/126318570033";
        assert_eq!(run_of_url(url), Some(41_751_854_039));
        assert_eq!(run_of_url("https://github.com/o/r/actions/runs/7"), Some(7));
        assert_eq!(run_of_url("https://ci.example.org/build/7"), None);
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
    fn reads_the_runs_of_a_commit() {
        let json = r#"[
            {"attempt":2,"conclusion":"failure","databaseId":41714780997,"displayTitle":"Forges",
             "event":"pull_request","headBranch":"feat/x","headSha":"9ef2ba0","name":"CI",
             "number":8,"startedAt":"2026-11-03T21:33:32Z","status":"completed",
             "updatedAt":"2026-11-03T21:48:41Z",
             "url":"https://github.com/o/r/actions/runs/41714780997",
             "workflowName":".github/workflows/ci.yml"},
            {"attempt":1,"conclusion":"","databaseId":7,"displayTitle":"Docs","event":"push",
             "headBranch":"main","headSha":"9ef2ba0","name":"","number":9,
             "startedAt":"2026-11-03T21:40:00Z","status":"in_progress",
             "updatedAt":"2026-11-03T21:41:00Z","url":"https://github.com/o/r/actions/runs/7",
             "workflowName":"Docs"}
        ]"#;
        let runs = parse_runs(json).unwrap_or_default();
        assert_eq!(runs.len(), 2);
        let run = &runs[0];
        assert_eq!((run.id, run.number, run.attempt), (41_714_780_997, 8, 2));
        assert_eq!(run.name, "CI");
        assert_eq!(run.state, CheckState::Failed);
        assert_eq!(
            (run.event.as_str(), run.branch.as_str()),
            ("pull_request", "feat/x")
        );
        assert_eq!(run.finished.zip(run.started).map(|(f, s)| f - s), Some(909));
        // Still running: it has no end yet, and goes by its workflow's name.
        assert_eq!(runs[1].name, "Docs");
        assert_eq!(runs[1].state, CheckState::Running);
        assert_eq!(runs[1].finished, None);
    }

    #[test]
    fn reads_the_jobs_and_steps_of_a_run() {
        let json = r#"{"jobs":[
            {"completedAt":"2026-11-03T21:48:39Z","conclusion":"cancelled","databaseId":1,
             "name":"test (macos-latest)","startedAt":"2026-11-03T21:33:37Z","status":"completed",
             "steps":[],"url":"https://github.com/o/r/actions/runs/9/job/1"},
            {"completedAt":"0001-01-01T00:00:00Z","conclusion":"","databaseId":2,
             "name":"test (ubuntu-latest)","startedAt":"2026-11-03T20:40:53Z",
             "status":"in_progress","url":"https://github.com/o/r/actions/runs/9/job/2",
             "steps":[
                {"completedAt":"2026-11-03T20:40:55Z","conclusion":"success","name":"Set up job",
                 "number":1,"startedAt":"2026-11-03T20:40:54Z","status":"completed"},
                {"completedAt":"0001-01-01T00:00:00Z","conclusion":"","name":"Test","number":2,
                 "startedAt":"2026-11-03T20:40:55Z","status":"in_progress"},
                {"completedAt":"0001-01-01T00:00:00Z","conclusion":"","name":"Post","number":3,
                 "startedAt":"0001-01-01T00:00:00Z","status":"pending"}]}
        ]}"#;
        let jobs = parse_jobs(json).unwrap_or_default();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].state, CheckState::Skipped);
        assert!(jobs[0].steps.is_empty());
        let job = &jobs[1];
        assert_eq!((job.id, job.state), (2, CheckState::Running));
        assert_eq!(job.finished, None);
        let steps: Vec<(&str, CheckState, bool)> = job
            .steps
            .iter()
            .map(|s| (s.name.as_str(), s.state, s.started.is_some()))
            .collect();
        assert_eq!(
            steps,
            [
                ("Set up job", CheckState::Passed, true),
                ("Test", CheckState::Running, true),
                ("Post", CheckState::Running, false),
            ]
        );
    }

    #[test]
    fn output_that_is_not_json_is_an_error() {
        assert!(parse_statuses("oops").is_err());
        assert!(parse_runs("oops").is_err());
        assert!(parse_jobs("oops").is_err());
    }
}
