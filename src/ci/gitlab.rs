//! GitLab CI through `glab api`: the pipelines of recent commits and of one commit, and the
//! jobs of a pipeline by stage. GitLab doesn't list the steps of a job.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{Check, Job, Project, Run, Status, time};
use crate::forge::{self, Forge};
use crate::pull_request::{CheckState, at};

/// The state of a pipeline or a job from its `status`.
pub(crate) fn state(status: &str) -> CheckState {
    match status {
        "success" => CheckState::Passed,
        "failed" => CheckState::Failed,
        "running"
        | "pending"
        | "created"
        | "preparing"
        | "waiting_for_resource"
        | "waiting_for_callback"
        | "scheduled"
        | "canceling" => CheckState::Running,
        _ => CheckState::Skipped,
    }
}

/// `projects/group%2Fproject`, how the API names a project by its path.
fn endpoint(project: &Project, rest: &str) -> String {
    format!("projects/{}/{rest}", project.path.replace('/', "%2F"))
}

fn api(repo: &Path, project: &Project, rest: &str) -> Result<String, String> {
    let endpoint = endpoint(project, rest);
    forge::tool(
        Forge::GitLab,
        repo,
        &["api", "--hostname", &project.host, &endpoint],
    )
}

fn list(json: &str) -> Result<Vec<Value>, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected glab output: {e}"))?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// The latest pipelines of the project, of which those of `shas` are taken: one query for
/// every commit, as GitHub's.
pub(super) fn statuses(
    repo: &Path,
    project: &Project,
    shas: &[&String],
) -> Result<HashMap<String, Status>, String> {
    let json = api(repo, project, "pipelines?per_page=100")?;
    parse_statuses(&json, shas)
}

/// The newest pipeline of each of `shas` that has one; GitLab lists the newest first. A short
/// id stands for the commit it begins.
fn parse_statuses(json: &str, shas: &[&String]) -> Result<HashMap<String, Status>, String> {
    let mut out: HashMap<String, Status> = HashMap::new();
    for pipeline in list(json)? {
        let sha = at(&pipeline, "/sha");
        let Some(wanted) = shas
            .iter()
            .find(|s| !sha.is_empty() && sha.starts_with(s.as_str()))
        else {
            continue;
        };
        if out.contains_key(wanted.as_str()) {
            continue;
        }
        let state = state(at(&pipeline, "/status"));
        let number = pipeline.get("iid").and_then(Value::as_u64).unwrap_or(0);
        let check = Check {
            name: format!("Pipeline #{number}"),
            state,
            detail: at(&pipeline, "/ref").to_owned(),
            url: at(&pipeline, "/web_url").to_owned(),
        };
        let status = Status {
            state,
            checks: vec![check],
        };
        out.insert((*wanted).clone(), status);
    }
    Ok(out)
}

pub(super) fn runs(repo: &Path, project: &Project, sha: &str) -> Result<Vec<Run>, String> {
    let json = api(repo, project, &format!("pipelines?sha={sha}&per_page=50"))?;
    Ok(list(&json)?.iter().map(run).collect())
}

fn run(pipeline: &Value) -> Run {
    let number = |key: &str| pipeline.get(key).and_then(Value::as_u64).unwrap_or(0);
    let state = state(at(pipeline, "/status"));
    let name = match at(pipeline, "/name") {
        "" => "Pipeline",
        name => name,
    };
    Run {
        id: number("id"),
        name: name.to_owned(),
        title: at(pipeline, "/ref").to_owned(),
        number: number("iid"),
        attempt: 1,
        event: at(pipeline, "/source").to_owned(),
        branch: at(pipeline, "/ref").to_owned(),
        sha: at(pipeline, "/sha").to_owned(),
        state,
        started: time(at(pipeline, "/created_at")),
        finished: (state != CheckState::Running)
            .then(|| time(at(pipeline, "/updated_at")))
            .flatten(),
        url: at(pipeline, "/web_url").to_owned(),
    }
}

/// The jobs of pipeline `run`, oldest first: their stages come in the pipeline's order then.
pub(super) fn jobs(repo: &Path, project: &Project, run: u64) -> Result<Vec<Job>, String> {
    let json = api(repo, project, &format!("pipelines/{run}/jobs?per_page=100"))?;
    parse_jobs(&json)
}

fn parse_jobs(json: &str) -> Result<Vec<Job>, String> {
    let mut jobs: Vec<Job> = list(json)?
        .iter()
        .map(|job| Job {
            id: job.get("id").and_then(Value::as_u64).unwrap_or(0),
            name: at(job, "/name").to_owned(),
            stage: at(job, "/stage").to_owned(),
            state: state(at(job, "/status")),
            started: time(at(job, "/started_at")),
            finished: time(at(job, "/finished_at")),
            url: at(job, "/web_url").to_owned(),
            steps: Vec::new(),
        })
        .collect();
    jobs.sort_by_key(|job| job.id);
    Ok(jobs)
}

fn post(repo: &Path, project: &Project, rest: &str) -> Result<(), String> {
    let endpoint = endpoint(project, rest);
    let args = ["api", "--hostname", &project.host, "-X", "POST", &endpoint];
    forge::tool(Forge::GitLab, repo, &args).map(|_| ())
}

/// Runs the failed jobs of pipeline `run` again.
pub(super) fn retry(repo: &Path, project: &Project, run: u64) -> Result<(), String> {
    post(repo, project, &format!("pipelines/{run}/retry"))
}

pub(super) fn cancel(repo: &Path, project: &Project, run: u64) -> Result<(), String> {
    post(repo, project, &format!("pipelines/{run}/cancel"))
}

/// The logs of those of `jobs` that failed, one after the other.
pub(super) fn failed_log(repo: &Path, project: &Project, jobs: &[Job]) -> Result<String, String> {
    let mut log = String::new();
    for job in jobs.iter().filter(|job| job.state == CheckState::Failed) {
        let trace = api(repo, project, &format!("jobs/{}/trace", job.id))?;
        log.push_str(&format!(
            "──── {} ({}) ────\n{trace}\n",
            job.name, job.stage
        ));
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_of_pipelines_and_jobs() {
        assert_eq!(state("success"), CheckState::Passed);
        assert_eq!(state("failed"), CheckState::Failed);
        for running in ["running", "pending", "created", "preparing", "scheduled"] {
            assert_eq!(state(running), CheckState::Running);
        }
        for neutral in ["canceled", "skipped", "manual"] {
            assert_eq!(state(neutral), CheckState::Skipped);
        }
    }

    #[test]
    fn projects_go_by_their_path() {
        let project = Project {
            forge: Forge::GitLab,
            host: "gitlab.com".to_owned(),
            path: "group/sub/project".to_owned(),
        };
        assert_eq!(
            endpoint(&project, "pipelines"),
            "projects/group%2Fsub%2Fproject/pipelines"
        );
    }

    const PIPELINES: &str = r#"[
        {"id":48,"iid":13,"project_id":1,"status":"running","source":"push","ref":"main",
         "sha":"a91957a858320c0e17f3a0eca7cfacbff50ea29a",
         "web_url":"https://gitlab.com/g/p/-/pipelines/48",
         "created_at":"2026-11-02T20:37:34.085Z","updated_at":"2026-11-02T20:40:00.000Z"},
        {"id":47,"iid":12,"project_id":1,"status":"failed","source":"push","ref":"main",
         "sha":"a91957a858320c0e17f3a0eca7cfacbff50ea29a","name":"Build",
         "web_url":"https://gitlab.com/g/p/-/pipelines/47",
         "created_at":"2026-11-02T20:00:00.000+01:00","updated_at":"2026-11-02T20:05:00.000+01:00"},
        {"id":46,"iid":11,"project_id":1,"status":"success","source":"merge_request_event",
         "ref":"refs/merge-requests/3/head","sha":"eb94b618fb5865b26e80fdd8ae531b7a63ad851a",
         "web_url":"https://gitlab.com/g/p/-/pipelines/46",
         "created_at":"2026-11-01T10:00:00Z","updated_at":"2026-11-01T10:09:00Z"}
    ]"#;

    #[test]
    fn the_newest_pipeline_of_a_commit_is_its_status() {
        let wanted = [
            "a91957a858320c0e17f3a0eca7cfacbff50ea29a".to_owned(),
            "eb94b61".to_owned(),
            "0000000".to_owned(),
        ];
        let wanted: Vec<&String> = wanted.iter().collect();
        let statuses = parse_statuses(PIPELINES, &wanted).unwrap_or_default();
        assert_eq!(statuses.len(), 2);
        let newest = &statuses["a91957a858320c0e17f3a0eca7cfacbff50ea29a"];
        assert_eq!(newest.state, CheckState::Running);
        assert_eq!(newest.checks[0].name, "Pipeline #13");
        assert_eq!(statuses["eb94b61"].state, CheckState::Passed);
    }

    #[test]
    fn reads_the_pipelines_of_a_commit() {
        let runs: Vec<Run> = list(PIPELINES)
            .unwrap_or_default()
            .iter()
            .map(run)
            .collect();
        assert_eq!(runs.len(), 3);
        assert_eq!((runs[0].id, runs[0].number), (48, 13));
        assert_eq!(runs[0].name, "Pipeline");
        assert_eq!(runs[0].finished, None);
        assert_eq!(runs[1].name, "Build");
        assert_eq!(runs[1].state, CheckState::Failed);
        assert_eq!(
            runs[1].finished.zip(runs[1].started).map(|(f, s)| f - s),
            Some(300)
        );
        assert_eq!(runs[2].event, "merge_request_event");
    }

    #[test]
    fn jobs_come_in_the_order_of_their_stages() {
        // GitLab lists the newest job first.
        let json = r#"[
            {"id":9,"status":"pending","stage":"deploy","name":"pages","started_at":null,
             "finished_at":null,"web_url":"https://gitlab.com/g/p/-/jobs/9"},
            {"id":8,"status":"failed","stage":"test","name":"rspec","web_url":"u8",
             "started_at":"2026-11-02T20:01:00.000Z","finished_at":"2026-11-02T20:03:30.000Z"},
            {"id":7,"status":"success","stage":"build","name":"compile","web_url":"u7",
             "started_at":"2026-11-02T20:00:00.000Z","finished_at":"2026-11-02T20:01:00.000Z"}
        ]"#;
        let jobs = parse_jobs(json).unwrap_or_default();
        let stages: Vec<&str> = jobs.iter().map(|j| j.stage.as_str()).collect();
        assert_eq!(stages, ["build", "test", "deploy"]);
        assert_eq!(jobs[1].state, CheckState::Failed);
        assert_eq!(
            jobs[1].finished.zip(jobs[1].started).map(|(f, s)| f - s),
            Some(150)
        );
        assert_eq!(jobs[2].started, None);
        assert!(jobs.iter().all(|j| j.steps.is_empty()));
    }

    #[test]
    fn output_that_is_not_json_is_an_error() {
        assert!(parse_statuses("oops", &[]).is_err());
        assert!(parse_jobs("oops").is_err());
    }
}
