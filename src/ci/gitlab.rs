//! GitLab CI through `glab api`: the pipelines of recent commits.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{Check, Project, Status};
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
    fn output_that_is_not_json_is_an_error() {
        assert!(parse_statuses("oops", &[]).is_err());
    }
}
