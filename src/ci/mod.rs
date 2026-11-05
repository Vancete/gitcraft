//! CI/CD through the forges' tools: the build status of commits, from GitHub Actions and other
//! checks on GitHub or the pipelines of GitLab CI.

pub(crate) mod github;
pub(crate) mod gitlab;

use std::collections::HashMap;
use std::path::Path;

use crate::forge::{self, Forge};
use crate::git::Remote;
pub use crate::pull_request::{Check, CheckState};

/// The repository on its forge whose builds are read: from `origin` (or the first remote).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub forge: Forge,
    pub host: String,
    /// `owner/repo`, or `group/sub/project`.
    pub path: String,
}

impl Project {
    pub fn of(forge: Forge, remotes: &[Remote]) -> Option<Self> {
        let remote = remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| remotes.first())?;
        let (host, path) = forge::location(&remote.url)?;
        // Both go into the tools' arguments and the API's paths as they are.
        let plain = |text: &str, extra: char| {
            !text.is_empty()
                && !text.starts_with(['-', '.', '/'])
                && text.chars().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') || c == extra
                })
        };
        (plain(&host, ':') && plain(&path, '/')).then_some(Self { forge, host, path })
    }
}

/// How the builds of one commit went, all taken together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: CheckState,
    /// What it is made of, for the tooltip; may be empty.
    pub checks: Vec<Check>,
}

/// "1 failed, 4 passed".
pub fn tally_label(checks: &[Check]) -> String {
    let (passed, failed, running, skipped) = crate::pull_request::tally(checks);
    let parts: Vec<String> = [
        (failed, "failed"),
        (running, "running"),
        (passed, "passed"),
        (skipped, "skipped"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, word)| format!("{n} {word}"))
    .collect();
    parts.join(", ")
}

/// The build status of each of `shas` that has one. Commits without builds are left out.
pub fn statuses(
    repo: &Path,
    project: &Project,
    shas: &[String],
) -> Result<HashMap<String, Status>, String> {
    // They go into a query, so they have to be what they say they are.
    let shas: Vec<&String> = shas.iter().filter(|sha| is_sha(sha)).collect();
    if shas.is_empty() {
        return Ok(HashMap::new());
    }
    match project.forge {
        Forge::GitHub => github::statuses(repo, project, &shas),
        Forge::GitLab => gitlab::statuses(repo, project, &shas),
    }
}

/// Whether `sha` is a full or abbreviated commit id, not anything a tool could take for more.
fn is_sha(sha: &str) -> bool {
    (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_commit_ids_go_to_the_forge() {
        assert!(is_sha("15845e2"));
        assert!(is_sha(&"a".repeat(40)));
        assert!(!is_sha("main"));
        assert!(!is_sha("abc\") { x }"));
        assert!(!is_sha("abc"));
    }

    #[test]
    fn the_project_is_origin() {
        let remotes = [
            Remote {
                name: "up".into(),
                url: "https://github.com/x/y".into(),
            },
            Remote {
                name: "origin".into(),
                url: "git@github.com:o/r.git".into(),
            },
        ];
        let project = Project::of(Forge::GitHub, &remotes);
        assert_eq!(project.map(|p| p.path), Some("o/r".to_owned()));
        assert_eq!(Project::of(Forge::GitHub, &[]), None);
    }

    #[test]
    fn tallies_read_worst_first() {
        let check = |state| Check {
            name: String::new(),
            state,
            detail: String::new(),
            url: String::new(),
        };
        let checks = [
            check(CheckState::Passed),
            check(CheckState::Failed),
            check(CheckState::Passed),
        ];
        assert_eq!(tally_label(&checks), "1 failed, 2 passed");
    }
}
