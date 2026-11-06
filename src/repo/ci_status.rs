//! The build status of the branch tips, pull requests and latest commits, read in the
//! background, and the coloured dot that shows it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egui::{Color32, Id, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, vec2};

use super::{Action, Msg, RepoTab, WIP_HASH};
use crate::ci::{self, CheckState, Project, Status};
use crate::theme::Tokens;

/// So many of the latest commits get a status, besides the tips of branches.
const RECENT: usize = 30;
/// At most so many commits are asked about at a time.
const MAX_SHAS: usize = 120;
/// How soon statuses are read again while a build runs, or after one may have started.
const RUNNING_POLL: Duration = Duration::from_secs(15);
/// Statuses older than this are read again with the next reload, like the pull requests.
const STALE: Duration = Duration::from_secs(120);
pub(super) const DOT_R: f32 = 3.5;

/// The statuses of a tab's commits.
#[derive(Default)]
pub(super) struct CiStatuses {
    /// Where they were read from.
    project: Option<Project>,
    pub(super) by_sha: HashMap<String, Status>,
    /// The commits last asked about.
    asked: HashSet<String>,
    loading: bool,
    loaded: Option<Instant>,
    /// When to read them again: a build runs, or one may only just have started.
    next: Option<Instant>,
    /// Why they couldn't be read (no tool, not logged in…).
    pub(super) error: Option<String>,
    /// The answer on its way was asked of the project before this one: it is dropped.
    superseded: bool,
}

impl CiStatuses {
    /// The repository's builds are now read from `project` (its remotes changed): what was read
    /// from the one before goes, along with the answer still on its way from there. That one
    /// keeps the slot until it comes, so the two can't land in the wrong order.
    fn moved_to(&mut self, project: Option<Project>) {
        let in_flight = self.loading;
        *self = CiStatuses {
            project,
            loading: in_flight,
            superseded: in_flight,
            ..CiStatuses::default()
        };
    }

    fn arrived(&mut self, fresh: bool, result: Result<HashMap<String, Status>, String>) {
        self.loading = false;
        if std::mem::take(&mut self.superseded) {
            // Of the project before: read them again, from this one, right away.
            if self.project.is_some() {
                self.next = Some(Instant::now());
            }
            return;
        }
        match result {
            Ok(by_sha) => {
                let running = by_sha.values().any(|s| s.state == CheckState::Running);
                self.by_sha = by_sha;
                self.error = None;
                if running || fresh {
                    self.next = Some(Instant::now() + RUNNING_POLL);
                }
            }
            // What was read before still holds as well as anything.
            Err(e) => self.error = Some(e),
        }
    }
}

impl RepoTab {
    /// The repository on its forge, whose builds are shown.
    pub(super) fn ci_project(&self) -> Option<Project> {
        Project::of(self.forge?, &self.remotes)
    }

    /// The build status of commit `sha`, if it has one.
    pub(super) fn ci_status(&self, sha: &str) -> Option<&Status> {
        self.ci.by_sha.get(sha)
    }

    /// The commits whose status is worth knowing: HEAD, the branches, the pull requests and the
    /// latest commits.
    fn ci_shas(&self) -> Vec<String> {
        let refs = &self.refs;
        let remote_tips = refs.remotes.iter().flat_map(|(_, list)| list);
        let candidates = std::iter::once(&refs.head)
            .chain(refs.local.iter().map(|b| &b.target))
            .chain(self.prs.iter().map(|pr| &pr.sha))
            .chain(self.history.iter().take(RECENT).map(|c| &c.hash))
            .chain(remote_tips.map(|b| &b.target));
        let mut seen = HashSet::new();
        candidates
            .filter(|sha| !sha.is_empty() && sha.as_str() != WIP_HASH)
            .filter(|sha| seen.insert(sha.as_str()))
            .take(MAX_SHAS)
            .cloned()
            .collect()
    }

    /// Reads the statuses again if they are old, or commits came that weren't asked about
    /// (after a push, a fetch, a new pull request).
    pub(super) fn refresh_ci(&mut self, ctx: &egui::Context) {
        let project = self.ci_project();
        if project != self.ci.project {
            self.ci.moved_to(project);
        }
        if self.ci.project.is_none() {
            return;
        }
        let stale = self.ci.loaded.is_none_or(|t| t.elapsed() > STALE);
        let unasked = self
            .ci_shas()
            .iter()
            .any(|sha| !self.ci.asked.contains(sha));
        if stale || unasked {
            self.load_ci(ctx);
        }
    }

    pub(super) fn load_ci(&mut self, ctx: &egui::Context) {
        let Some(project) = self.ci.project.clone() else {
            return;
        };
        let shas = self.ci_shas();
        if self.ci.loading || shas.is_empty() {
            return;
        }
        // New commits after others were asked about: maybe a push, whose build starts soon.
        let fresh = !self.ci.asked.is_empty() && shas.iter().any(|s| !self.ci.asked.contains(s));
        self.ci.loading = true;
        self.ci.loaded = Some(Instant::now());
        self.ci.next = None;
        self.ci.asked = shas.iter().cloned().collect();
        let path = self.path.clone();
        // Answered all the same if it panics: no statuses are read again until it is.
        let failed = move |e| Msg::CiStatuses(fresh, Err(e));
        super::messages::spawn_or(&self.tx, ctx, failed, move || {
            Msg::CiStatuses(fresh, ci::statuses(&path, &project, &shas))
        });
    }

    pub(super) fn ci_arrived(
        &mut self,
        fresh: bool,
        result: Result<HashMap<String, Status>, String>,
    ) {
        self.ci.arrived(fresh, result);
    }

    /// Reads the statuses again when it's time to; called every frame.
    pub(super) fn poll_ci(&mut self, ctx: &egui::Context) {
        let Some(next) = self.ci.next else { return };
        let now = Instant::now();
        if now >= next {
            self.load_ci(ctx);
        } else {
            ctx.request_repaint_after(next - now);
        }
    }
}

/// The colour of a build state.
pub(crate) fn state_color(t: &Tokens, state: CheckState) -> Color32 {
    match state {
        CheckState::Passed => t.success,
        CheckState::Failed => t.danger,
        CheckState::Running => t.warning,
        CheckState::Skipped => t.text_faint,
    }
}

/// Paints the dot of `state` around `center`: filled, or a ring while it runs.
pub(super) fn paint_dot(
    painter: &Painter,
    t: &Tokens,
    center: Pos2,
    radius: f32,
    state: CheckState,
) {
    let color = state_color(t, state);
    if state == CheckState::Running {
        painter.circle_stroke(center, radius - 0.5, Stroke::new(1.6, color));
    } else {
        painter.circle_filled(center, radius, color);
    }
}

/// What a dot's tooltip says: the tally, then the checks that need attention.
pub(super) fn tooltip(status: &Status) -> String {
    let mut text = match ci::tally_label(&status.checks) {
        tally if tally.is_empty() => format!("CI: {}", state_word(status.state)),
        tally => format!("CI: {tally}"),
    };
    let mut worst: Vec<_> = status
        .checks
        .iter()
        .filter(|c| matches!(c.state, CheckState::Failed | CheckState::Running))
        .collect();
    worst.sort_by_key(|c| c.state != CheckState::Failed);
    for check in worst.iter().take(8) {
        text.push_str(&format!("\n{}  {}", state_word(check.state), check.name));
    }
    if worst.len() > 8 {
        text.push_str(&format!("\n… and {} more", worst.len() - 8));
    }
    text.push_str("\nClick for details");
    text
}

pub(crate) fn state_word(state: CheckState) -> &'static str {
    match state {
        CheckState::Passed => "passed",
        CheckState::Failed => "failed",
        CheckState::Running => "running",
        CheckState::Skipped => "skipped",
    }
}

/// A clickable dot for `status` of commit `sha` around `center`; clicked, it asks for the
/// commit's builds in the big area.
pub(super) fn dot(ui: &Ui, id: Id, center: Pos2, sha: &str, status: &Status) -> Option<Action> {
    let t = Tokens::get(ui.ctx());
    let rect = Rect::from_center_size(center, vec2(DOT_R * 2.0 + 6.0, DOT_R * 2.0 + 6.0));
    let resp: Response = ui.interact(rect, id, Sense::click());
    let radius = if resp.hovered() { DOT_R + 1.0 } else { DOT_R };
    paint_dot(ui.painter(), &t, center, radius, status.state);
    let clicked = resp.on_hover_text(tooltip(status)).clicked();
    clicked.then(|| Action::OpenCi {
        sha: sha.to_owned(),
        run: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::Check;

    fn check(name: &str, state: CheckState) -> Check {
        Check {
            name: name.to_owned(),
            state,
            detail: String::new(),
            url: format!("https://ci/{name}"),
        }
    }

    #[test]
    fn the_tooltip_tallies_and_names_what_failed_first() {
        let status = Status {
            state: CheckState::Failed,
            checks: vec![
                check("test", CheckState::Running),
                check("lint", CheckState::Failed),
                check("docs", CheckState::Passed),
            ],
        };
        assert_eq!(
            tooltip(&status),
            "CI: 1 failed, 1 running, 1 passed\nfailed  lint\nrunning  test\nClick for details"
        );
    }

    fn project(path: &str) -> Project {
        Project {
            forge: crate::forge::Forge::GitHub,
            host: "github.com".to_owned(),
            path: path.to_owned(),
        }
    }

    fn passed() -> Status {
        Status {
            state: CheckState::Passed,
            checks: Vec::new(),
        }
    }

    #[test]
    fn statuses_asked_of_the_project_before_are_dropped() {
        let mut ci = CiStatuses {
            project: Some(project("me/fork")),
            loading: true,
            ..CiStatuses::default()
        };
        // The remote is pointed elsewhere while the fork's statuses are being read.
        ci.moved_to(Some(project("org/upstream")));
        assert!(ci.loading, "no second read while the first is on its way");
        let fork = HashMap::from([("abc".to_owned(), passed())]);
        ci.arrived(false, Ok(fork));
        assert!(
            ci.by_sha.is_empty(),
            "the fork's statuses aren't upstream's"
        );
        assert!(!ci.loading);
        assert!(ci.next.is_some(), "read again from upstream");
        // Upstream's own answer is taken.
        let upstream = HashMap::from([("abc".to_owned(), passed())]);
        ci.arrived(false, Ok(upstream));
        assert_eq!(ci.by_sha.len(), 1);
    }

    #[test]
    fn a_project_change_with_nothing_on_its_way_starts_afresh() {
        let mut ci = CiStatuses {
            project: Some(project("me/fork")),
            by_sha: HashMap::from([("abc".to_owned(), passed())]),
            ..CiStatuses::default()
        };
        ci.moved_to(Some(project("org/upstream")));
        assert!(ci.by_sha.is_empty());
        assert!(!ci.loading);
        ci.arrived(false, Ok(HashMap::from([("abc".to_owned(), passed())])));
        assert_eq!(ci.by_sha.len(), 1);
    }

    #[test]
    fn a_status_without_checks_says_its_state() {
        let status = Status {
            state: CheckState::Passed,
            checks: Vec::new(),
        };
        assert_eq!(tooltip(&status), "CI: passed\nClick for details");
    }
}
