//! CI/CD through the forges' tools: the build status of commits, and the runs (GitHub Actions)
//! or pipelines (GitLab CI) of one commit with their jobs and steps.

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

/// One workflow run (GitHub) or pipeline (GitLab).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub id: u64,
    /// The workflow's name, or "Pipeline".
    pub name: String,
    /// The commit's title (GitHub) or the ref built (GitLab).
    pub title: String,
    pub number: u64,
    pub attempt: u64,
    /// What started it: `push`, `pull_request`, `schedule`…
    pub event: String,
    pub branch: String,
    pub sha: String,
    pub state: CheckState,
    pub started: Option<i64>,
    pub finished: Option<i64>,
    pub url: String,
}

/// One job of a run, in its stage (GitLab; empty on GitHub).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub stage: String,
    pub state: CheckState,
    pub started: Option<i64>,
    pub finished: Option<i64>,
    pub url: String,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub state: CheckState,
    pub started: Option<i64>,
    pub finished: Option<i64>,
}

/// Seconds from `started` to `finished`, or to `now` while it runs.
pub fn duration(started: Option<i64>, finished: Option<i64>, now: i64) -> Option<i64> {
    let started = started?;
    Some((finished.unwrap_or(now) - started).max(0))
}

/// `1h 2m`, `3m 4s`, `5s`.
pub fn duration_label(seconds: i64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
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

/// The runs of commit `sha`, newest first.
pub fn runs(repo: &Path, project: &Project, sha: &str) -> Result<Vec<Run>, String> {
    if !is_sha(sha) {
        return Err("not a commit id".to_owned());
    }
    match project.forge {
        Forge::GitHub => github::runs(repo, project, sha),
        Forge::GitLab => gitlab::runs(repo, project, sha),
    }
}

/// The jobs of run `run`.
pub fn jobs(repo: &Path, project: &Project, run: u64) -> Result<Vec<Job>, String> {
    match project.forge {
        Forge::GitHub => github::jobs(repo, project, run),
        Forge::GitLab => gitlab::jobs(repo, project, run),
    }
}

/// Runs the jobs of run `run` that failed again.
pub fn rerun_failed(repo: &Path, project: &Project, run: u64) -> Result<(), String> {
    match project.forge {
        Forge::GitHub => github::rerun_failed(repo, project, run),
        Forge::GitLab => gitlab::retry(repo, project, run),
    }
}

/// Stops run `run`.
pub fn cancel(repo: &Path, project: &Project, run: u64) -> Result<(), String> {
    match project.forge {
        Forge::GitHub => github::cancel(repo, project, run),
        Forge::GitLab => gitlab::cancel(repo, project, run),
    }
}

/// What the failed `jobs` of run `run` wrote, as plain text.
pub fn failed_log(
    repo: &Path,
    project: &Project,
    run: u64,
    jobs: &[Job],
) -> Result<String, String> {
    let log = match project.forge {
        Forge::GitHub => github::failed_log(repo, project, run)?,
        Forge::GitLab => gitlab::failed_log(repo, project, jobs)?,
    };
    Ok(plain_text(&log))
}

/// So much of the end of a log is kept: where it failed is at the end.
const MAX_LOG: usize = 256 * 1024;

/// `log` without terminal colours and carriage returns, and only its end if it's long.
fn plain_text(log: &str) -> String {
    let mut out = String::with_capacity(log.len().min(MAX_LOG));
    let mut chars = log.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // An escape sequence `ESC [ … letter`.
            '\u{1b}' => {
                if chars.next_if_eq(&'[').is_some() {
                    while chars.next().is_some_and(|c| !c.is_ascii_alphabetic()) {}
                }
            }
            '\r' => {
                // `\r\r\n` (text written twice over on Windows) ends the line all the same.
                while chars.next_if_eq(&'\r').is_some() {}
                // A carriage return alone starts the line over, as on a terminal: what a
                // progress bar overwrote, GitLab's `section_start:…` marks, are not to be read.
                // Followed by nothing but colours up to the line's end (`\r\x1b[0m\n`), it
                // overwrites nothing.
                if !only_colours_to_line_end(chars.clone()) {
                    out.truncate(out.rfind('\n').map_or(0, |n| n + 1));
                }
            }
            c => out.push(c),
        }
    }
    if out.len() <= MAX_LOG {
        return out;
    }
    let mut cut = out.len() - MAX_LOG;
    while !out.is_char_boundary(cut) {
        cut += 1;
    }
    let start = out[cut..].find('\n').map_or(cut, |n| cut + n + 1);
    format!("… (the start is left out)\n{}", &out[start..])
}

/// Whether `rest` holds nothing up to its line's end (or its own end) but escape sequences
/// that only set colours: not text, nor an erase (`ESC [ K`, `ESC [ J`), which a carriage
/// return before it means to clear the line with.
fn only_colours_to_line_end(mut rest: std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    loop {
        match rest.next() {
            None | Some('\n') => return true,
            Some('\u{1b}') if rest.next_if_eq(&'[').is_some() => {
                let end = rest.find(char::is_ascii_alphabetic);
                if matches!(end, None | Some('K' | 'J')) {
                    return false;
                }
            }
            Some(_) => return false,
        }
    }
}

/// Whether `sha` is a full or abbreviated commit id, not anything a tool could take for more.
fn is_sha(sha: &str) -> bool {
    (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Seconds since the epoch of a time like `2026-11-02T20:37:34Z` or `…34.085+01:00`; GitHub
/// writes `0001-01-01T00:00:00Z` for a time that hasn't come yet.
fn time(text: &str) -> Option<i64> {
    let text = text.trim();
    let split = text
        .len()
        .checked_sub(6)
        .and_then(|at| Some((text.get(..at)?, text.get(at..)?)));
    let (local, offset) = match split {
        Some((local, zone)) if zone.starts_with(['+', '-']) && zone.get(3..4) == Some(":") => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..)?.parse().ok()?;
            (format!("{local}Z"), sign * (hours * 3600 + minutes * 60))
        }
        _ => (text.to_owned(), 0),
    };
    let at = crate::pull_request::parse_time(&local)? - offset;
    (at > 0).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_lose_their_colours() {
        let log = "test\tRun\t\u{1b}[31merror\u{1b}[0m: it broke\r\nnext\n";
        assert_eq!(plain_text(log), "test\tRun\terror: it broke\nnext\n");
        let long = "line\n".repeat(MAX_LOG / 4);
        let kept = plain_text(&long);
        assert!(kept.starts_with("… (the start is left out)\nline\n"));
        assert!(kept.len() < MAX_LOG + 40);
    }

    #[test]
    fn what_a_log_wrote_over_is_left_out() {
        let gitlab = "\u{1b}[0Ksection_start:1730000000:prepare_executor\r\u{1b}[0K\u{1b}[0;mPreparing\n\
                      Downloading 10%\r30%\r100%\n\
                      \u{1b}[0Ksection_end:1730000001:prepare_executor\r\u{1b}[0K\n\
                      done\r\n\
                      twice\r\r\nend\r";
        assert_eq!(plain_text(gitlab), "Preparing\n100%\n\ndone\ntwice\nend");
    }

    /// A colour reset between a carriage return and the line's end overwrites nothing.
    #[test]
    fn a_line_ended_by_a_colour_reset_stays() {
        let log = "error details\r\u{1b}[0m\nnext\r\u{1b}[0m";
        assert_eq!(plain_text(log), "error details\nnext");
    }

    #[test]
    fn reads_times_in_any_zone() {
        assert_eq!(time("2026-11-02T20:37:34Z"), Some(1_793_651_854));
        assert_eq!(time("2026-11-02T20:37:34.085Z"), Some(1_793_651_854));
        assert_eq!(time("2026-11-02T21:37:34.000+01:00"), Some(1_793_651_854));
        assert_eq!(time("2026-11-02T19:37:34-01:00"), Some(1_793_651_854));
        assert_eq!(time("0001-01-01T00:00:00Z"), None);
        assert_eq!(time(""), None);
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(duration_label(5), "5s");
        assert_eq!(duration_label(185), "3m 5s");
        assert_eq!(duration_label(3725), "1h 2m");
        assert_eq!(duration(Some(10), Some(4), 100), Some(0));
        assert_eq!(duration(Some(10), None, 100), Some(90));
        assert_eq!(duration(None, Some(4), 100), None);
    }

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
