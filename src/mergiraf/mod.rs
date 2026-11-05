//! Mergiraf (<https://mergiraf.org>), a merge tool that knows the syntax of many languages.
//! gitcraft runs the `mergiraf` command, the one on the PATH or one it downloaded from
//! Mergiraf's own releases when asked to; it never bundles or links it, so Mergiraf's GPL stays
//! with Mergiraf.
//!
//! Besides solving a file in the conflict editor, it can set git up to merge with Mergiraf in
//! every repository (`merge.mergiraf.*` in the global config, a marked block in the global
//! attributes file), and take that back.

mod download;
mod setup;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Other ways to install it.
pub const DOCS: &str = "https://mergiraf.org/installation.html";
const EXE: &str = if cfg!(windows) {
    "mergiraf.exe"
} else {
    "mergiraf"
};

/// How long an answer to "is it installed" holds: installing it needs no restart.
const RECHECK: Duration = Duration::from_secs(30);

/// A Mergiraf that runs.
#[derive(Clone, Debug)]
pub struct Found {
    pub program: PathBuf,
    /// Its version line ("mergiraf 0.20.0").
    pub version: String,
    /// The one gitcraft downloaded, not one on the PATH.
    pub own: bool,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub found: Option<Found>,
    /// Git merges with Mergiraf, in every repository.
    pub in_merges: bool,
}

/// Something gitcraft is doing with Mergiraf in the background, or the last thing it did.
#[derive(Clone, Debug)]
pub enum Job {
    Idle,
    Running(&'static str),
    Done(String),
    Failed(String),
}

static STATE: Mutex<Option<(Instant, State)>> = Mutex::new(None);
static JOB: Mutex<Job> = Mutex::new(Job::Idle);

fn quiet(mut cmd: Command) -> Command {
    crate::shell_path::apply(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The folder gitcraft downloads Mergiraf into.
fn own_dir() -> Option<PathBuf> {
    eframe::storage_dir("gitcraft").map(|d| d.join("mergiraf"))
}

fn probe(program: &Path) -> Option<String> {
    quiet(Command::new(program))
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The one on the PATH first: the user looks after that one.
fn find() -> Option<Found> {
    let on_path = PathBuf::from("mergiraf");
    if let Some(version) = probe(&on_path) {
        return Some(Found {
            program: on_path,
            version,
            own: false,
        });
    }
    let own = own_dir()?.join(EXE);
    let version = probe(&own)?;
    Some(Found {
        program: own,
        version,
        own: true,
    })
}

/// Whether Mergiraf is installed and git set up to use it, as of about [`RECHECK`] ago. Asked
/// every frame (the conflict editor, the settings): only the first answer is worked out here,
/// later ones are refreshed in the background, the old answer standing meanwhile, and `ctx`
/// repainted when the new one is in.
pub fn state(ctx: &egui::Context) -> State {
    // Remembered for any refresh, also one started where there is no UI (`solve`): the UI
    // may have shown the old answer while it ran.
    if let Ok(mut repaint) = REPAINT.lock()
        && repaint.is_none()
    {
        *repaint = Some(ctx.clone());
    }
    current()
}

/// The UI to repaint when a background refresh is in.
static REPAINT: Mutex<Option<egui::Context>> = Mutex::new(None);

/// [`state`], for callers without a UI.
fn current() -> State {
    let Ok(mut cache) = STATE.lock() else {
        return State::default();
    };
    match cache.as_ref() {
        Some((when, state)) => {
            if when.elapsed() >= RECHECK && !REFRESHING.swap(true, Ordering::AcqRel) {
                let generation = FORGOTTEN.load(Ordering::Acquire);
                std::thread::spawn(move || {
                    let fresh = read_state();
                    // Not over what a `forget` meanwhile said is out of date.
                    if let Ok(mut cache) = STATE.lock()
                        && FORGOTTEN.load(Ordering::Acquire) == generation
                    {
                        *cache = Some((Instant::now(), fresh));
                    }
                    REFRESHING.store(false, Ordering::Release);
                    let repaint = REPAINT.lock().ok().and_then(|ctx| ctx.clone());
                    if let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                });
            }
            state.clone()
        }
        None => {
            let state = read_state();
            *cache = Some((Instant::now(), state.clone()));
            state
        }
    }
}

/// A background refresh of [`STATE`] is running.
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// How many times [`forget`] dropped the answer.
static FORGOTTEN: AtomicU32 = AtomicU32::new(0);

fn read_state() -> State {
    State {
        found: find(),
        in_merges: setup::in_merges(),
    }
}

/// The Mergiraf that runs, as [`state`] has it.
pub fn found(ctx: &egui::Context) -> Option<Found> {
    state(ctx).found
}

fn forget() {
    if let Ok(mut cache) = STATE.lock() {
        FORGOTTEN.fetch_add(1, Ordering::AcqRel);
        *cache = None;
    }
}

pub fn job() -> Job {
    JOB.lock().map(|j| j.clone()).unwrap_or(Job::Idle)
}

fn start(ctx: &egui::Context, doing: &'static str, work: fn() -> Result<String, String>) {
    let Ok(mut job) = JOB.lock() else { return };
    if matches!(*job, Job::Running(_)) {
        return;
    }
    *job = Job::Running(doing);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = work();
        forget();
        if let Ok(mut job) = JOB.lock() {
            *job = match result {
                Ok(done) => Job::Done(done),
                Err(e) => Job::Failed(e),
            };
        }
        ctx.request_repaint();
    });
}

/// Downloads the latest Mergiraf release for this system into gitcraft's folder.
pub fn install(ctx: &egui::Context) {
    start(ctx, "Downloading Mergiraf…", download::download);
}

/// Sets git up to merge with Mergiraf in every repository, or takes that back.
pub fn use_in_merges(ctx: &egui::Context, on: bool) {
    if on {
        start(ctx, "Setting git up…", setup::enable);
    } else {
        start(ctx, "Setting git back…", setup::disable);
    }
}

/// What Mergiraf makes of the conflicts in `path`, a file of the repository at `repo`: the file
/// with every conflict it could solve solved, the others still marked. The file itself is left
/// as it is.
pub fn solve(repo: &Path, path: &str) -> Result<String, String> {
    let found = current().found.ok_or("Mergiraf isn't installed")?;
    let out = quiet(Command::new(&found.program))
        .current_dir(repo)
        .args(["solve", "--stdout", "--", path])
        .output()
        .map_err(|e| format!("Could not run mergiraf: {e}"))?;
    // 0: all solved; 2: some conflicts remain, still a file to show.
    let solved = out.status.success() || (out.status.code() == Some(2) && !out.stdout.is_empty());
    if solved {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(error_message(&String::from_utf8_lossy(&out.stderr), path))
    }
}

/// Mergiraf's last log line ("WARN Mergiraf: Could not find a supported language for 'x'"),
/// without its level and name.
fn error_message(stderr: &str, path: &str) -> String {
    let line = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
    match line {
        Some(line) => {
            let line = ["ERROR ", "WARN ", "INFO "]
                .iter()
                .find_map(|level| line.strip_prefix(level))
                .unwrap_or(line);
            format!(
                "Mergiraf: {}",
                line.strip_prefix("Mergiraf: ").unwrap_or(line)
            )
        }
        None => format!("Mergiraf could not solve {path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_lose_their_log_level() {
        assert_eq!(
            error_message(
                "WARN Mergiraf: Could not find a supported language for 'notes.txt'\n",
                "notes.txt"
            ),
            "Mergiraf: Could not find a supported language for 'notes.txt'"
        );
        assert_eq!(error_message("ERROR boom", "f"), "Mergiraf: boom");
        assert_eq!(error_message("  \n", "f"), "Mergiraf could not solve f");
    }
}
