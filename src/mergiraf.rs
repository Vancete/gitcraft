//! Mergiraf (<https://mergiraf.org>), a merge tool that knows the syntax of many languages.
//! gitcraft runs the `mergiraf` command, the one on the PATH or one it downloaded from
//! Mergiraf's own releases when asked to; it never bundles or links it, so Mergiraf's GPL stays
//! with Mergiraf.
//!
//! Besides solving a file in the conflict editor, it can set git up to merge with Mergiraf in
//! every repository (`merge.mergiraf.*` in the global config, a marked block in the global
//! attributes file), and take that back.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Other ways to install it.
pub const DOCS: &str = "https://mergiraf.org/installation.html";
const LATEST: &str = "https://codeberg.org/api/v1/repos/mergiraf/mergiraf/releases/latest";
/// Where its release files are; a download from anywhere else is refused.
const DOWNLOADS: &str = "https://codeberg.org/mergiraf/mergiraf/releases/download/";
const MAX_DOWNLOAD: u64 = 200 << 20;
const EXE: &str = if cfg!(windows) {
    "mergiraf.exe"
} else {
    "mergiraf"
};

/// How long an answer to "is it installed" holds: installing it needs no restart.
const RECHECK: Duration = Duration::from_secs(30);

/// The lines gitcraft adds to the global attributes file are between these.
const BEGIN: &str = "# gitcraft: Mergiraf (begin)";
const END: &str = "# gitcraft: Mergiraf (end)";

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

fn in_merges() -> bool {
    let driver = crate::git::run(
        None,
        &["config", "--global", "--get", "merge.mergiraf.driver"],
    )
    .is_ok_and(|d| !d.trim().is_empty());
    driver
        && attributes_file()
            .and_then(|f| std::fs::read_to_string(f).map_err(|e| e.to_string()))
            .is_ok_and(|text| text.contains("merge=mergiraf"))
}

/// Whether Mergiraf is installed and git set up to use it, as of at most [`RECHECK`] ago.
pub fn state() -> State {
    let Ok(mut cache) = STATE.lock() else {
        return State::default();
    };
    if let Some((when, state)) = cache.as_ref()
        && when.elapsed() < RECHECK
    {
        return state.clone();
    }
    let state = State {
        found: find(),
        in_merges: in_merges(),
    };
    *cache = Some((Instant::now(), state.clone()));
    state
}

pub fn found() -> Option<Found> {
    state().found
}

fn forget() {
    if let Ok(mut cache) = STATE.lock() {
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
    start(ctx, "Downloading Mergiraf…", download);
}

/// Sets git up to merge with Mergiraf in every repository, or takes that back.
pub fn use_in_merges(ctx: &egui::Context, on: bool) {
    if on {
        start(ctx, "Setting git up…", enable);
    } else {
        start(ctx, "Setting git back…", disable);
    }
}

/// The release file for this system. Windows on ARM runs the x86-64 one.
fn asset() -> Option<&'static str> {
    let arm = cfg!(target_arch = "aarch64");
    if cfg!(windows) {
        Some("mergiraf_x86_64-pc-windows-gnu.zip")
    } else if cfg!(target_os = "macos") {
        Some(if arm {
            "mergiraf_aarch64-apple-darwin.tar.gz"
        } else {
            "mergiraf_x86_64-apple-darwin.tar.gz"
        })
    } else if cfg!(target_os = "linux") && (arm || cfg!(target_arch = "x86_64")) {
        Some(if arm {
            "mergiraf_aarch64-unknown-linux-musl.tar.gz"
        } else {
            "mergiraf_x86_64-unknown-linux-musl.tar.gz"
        })
    } else {
        None
    }
}

/// Windows' own tar, which also opens zip files (Git's GNU tar earlier on the PATH doesn't).
fn tar() -> Command {
    let program = if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
            .unwrap_or_else(|| PathBuf::from("tar.exe"))
    } else {
        PathBuf::from("tar")
    };
    quiet(Command::new(program))
}

fn download() -> Result<String, String> {
    let asset = asset().ok_or("Mergiraf publishes no build for this system")?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .user_agent("gitcraft")
        .build();
    let release = agent
        .get(LATEST)
        .call()
        .map_err(|e| format!("Could not ask Codeberg for Mergiraf's latest release: {e}"))?
        .into_string()
        .map_err(|e| format!("Codeberg's answer about Mergiraf: {e}"))?;
    let release: serde_json::Value = serde_json::from_str(&release)
        .map_err(|e| format!("Codeberg's answer about Mergiraf: {e}"))?;
    let url = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == asset)
        .and_then(|a| a["browser_download_url"].as_str())
        .filter(|url| url.starts_with(DOWNLOADS))
        .ok_or_else(|| format!("Mergiraf's latest release has no {asset}"))?;

    let dir = own_dir().ok_or("No folder to keep Mergiraf in")?;
    // Unpacked apart, so only the program is taken from the archive.
    let unpack = dir.join("unpack");
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack).map_err(|e| format!("{}: {e}", unpack.display()))?;
    let mut body = Vec::new();
    agent
        .get(url)
        .call()
        .map_err(|e| format!("Could not download {asset}: {e}"))?
        .into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("Could not download {asset}: {e}"))?;
    if body.len() as u64 > MAX_DOWNLOAD {
        return Err(format!("{asset} is bigger than expected"));
    }
    let archive = unpack.join(asset);
    std::fs::write(&archive, &body).map_err(|e| format!("{}: {e}", archive.display()))?;
    let unpacked = tar()
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpack)
        .output()
        .map_err(|e| format!("Could not run tar: {e}"))?;
    let program = dir.join(EXE);
    let moved = if unpacked.status.success() {
        std::fs::rename(unpack.join(EXE), &program).map_err(|e| format!("{asset}: {e}"))
    } else {
        Err(format!(
            "Could not unpack {asset}: {}",
            String::from_utf8_lossy(&unpacked.stderr).trim()
        ))
    };
    let _ = std::fs::remove_dir_all(&unpack);
    moved?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", program.display()))?;
    }
    let version = probe(&program).ok_or("The downloaded Mergiraf doesn't start")?;
    Ok(format!("Installed {version}"))
}

/// The global attributes file git reads.
fn attributes_file() -> Result<PathBuf, String> {
    if let Ok(path) = crate::git::run(
        None,
        &[
            "config",
            "--global",
            "--path",
            "--get",
            "core.attributesFile",
        ],
    ) && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path.trim()));
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(config).join("git").join("attributes"));
    }
    std::env::var_os("HOME")
        .filter(|d| !d.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("git")
                .join("attributes")
        })
        .ok_or_else(|| "No home folder for git's attributes file".to_owned())
}

/// `text` without gitcraft's block, then with `lines` as the block when given.
fn with_block(text: &str, lines: Option<&str>) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        match line.trim() {
            BEGIN => inside = true,
            END if inside => inside = false,
            _ if !inside => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    if let Some(lines) = lines {
        out.push_str(BEGIN);
        out.push('\n');
        for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(END);
        out.push('\n');
    }
    out
}

/// How git's merge driver calls it: by name when it is on the PATH, else by its full path.
fn driver(found: &Found) -> String {
    let program = if found.own {
        format!(
            "\"{}\"",
            found.program.display().to_string().replace('\\', "/")
        )
    } else {
        "mergiraf".to_owned()
    };
    format!("{program} merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L")
}

fn enable() -> Result<String, String> {
    let found = find().ok_or("Mergiraf isn't installed")?;
    let out = quiet(Command::new(&found.program))
        .args(["languages", "--gitattributes"])
        .output()
        .map_err(|e| format!("Could not run mergiraf: {e}"))?;
    let languages = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() || !languages.contains("merge=mergiraf") {
        return Err("Mergiraf didn't list the files it merges".to_owned());
    }
    let git = |args: &[&str]| crate::git::run(None, args).map(drop);
    git(&["config", "--global", "merge.mergiraf.name", "mergiraf"])?;
    git(&[
        "config",
        "--global",
        "merge.mergiraf.driver",
        &driver(&found),
    ])?;
    // Mergiraf needs the base of each conflict, which git's default markers leave out.
    if crate::git::run(
        None,
        &["config", "--global", "--get", "merge.conflictStyle"],
    )
    .is_err()
    {
        git(&["config", "--global", "merge.conflictStyle", "diff3"])?;
    }
    let file = attributes_file()?;
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&file, with_block(&text, Some(&languages)))
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok("Git merges with Mergiraf now, in every repository".to_owned())
}

fn disable() -> Result<String, String> {
    let file = attributes_file()?;
    if let Ok(text) = std::fs::read_to_string(&file) {
        std::fs::write(&file, with_block(&text, None))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    // Fails only when there is no such section, which is the point.
    let _ = crate::git::run(
        None,
        &["config", "--global", "--remove-section", "merge.mergiraf"],
    );
    Ok("Git merges as it did before Mergiraf".to_owned())
}

/// What Mergiraf makes of the conflicts in `path`, a file of the repository at `repo`: the file
/// with every conflict it could solve solved, the others still marked. The file itself is left
/// as it is.
pub fn solve(repo: &Path, path: &str) -> Result<String, String> {
    let found = found().ok_or("Mergiraf isn't installed")?;
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

    #[test]
    fn the_block_is_added_replaced_and_taken_out_alone() {
        let mine = "*.png binary\n";
        let added = with_block(mine, Some("*.rs merge=mergiraf\r\n\n*.go merge=mergiraf\n"));
        assert_eq!(
            added,
            format!("*.png binary\n{BEGIN}\n*.rs merge=mergiraf\n*.go merge=mergiraf\n{END}\n")
        );
        let again = with_block(
            &format!("{added}*.pdf binary\n"),
            Some("*.py merge=mergiraf"),
        );
        assert_eq!(
            again,
            format!("*.png binary\n*.pdf binary\n{BEGIN}\n*.py merge=mergiraf\n{END}\n")
        );
        assert_eq!(with_block(&again, None), "*.png binary\n*.pdf binary\n");
        assert_eq!(with_block("", None), "");
    }

    #[test]
    fn the_driver_names_its_own_copy_by_path() {
        let found = |program: &str, own| Found {
            program: PathBuf::from(program),
            version: String::new(),
            own,
        };
        assert_eq!(
            driver(&found("mergiraf", false)),
            "mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L"
        );
        assert!(
            driver(&found(r"C:\Users\me\gitcraft\mergiraf.exe", true))
                .starts_with("\"C:/Users/me/gitcraft/mergiraf.exe\" merge --git ")
        );
    }
}
