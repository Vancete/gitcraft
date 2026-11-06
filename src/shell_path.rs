//! macOS apps opened from the Finder or the Dock get a bare PATH (`/usr/bin:/bin:/usr/sbin:/sbin`),
//! without Homebrew's folders, where git, gh, glab and editors' commands usually are. The
//! commands gitcraft starts get the PATH of the user's login shell instead.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Folders added in any case (Homebrew on Apple silicon, then on Intel).
const FALLBACK: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// The PATH for commands gitcraft runs, when it differs from its own: on macOS the login
/// shell's, elsewhere its own without relative folders. `gh`, `glab` and Mergiraf run in the
/// repository's folder, where an empty entry or `.` (`PATH=$PATH:`) would run a `gh` the
/// repository brought.
pub fn path() -> Option<&'static OsString> {
    static PATH: OnceLock<Option<OsString>> = OnceLock::new();
    PATH.get_or_init(|| {
        if cfg!(target_os = "macos") {
            login_path(false)
        } else {
            absolute_only(&std::env::var_os("PATH").unwrap_or_default())
        }
    })
    .as_ref()
}

/// The PATH for the embedded terminal, when it differs from gitcraft's own: on macOS the login
/// shell's, elsewhere none (the terminal keeps gitcraft's own). Relative folders stay: what the
/// user types there is theirs to run, and a deliberate `node_modules/.bin` must not go missing
/// from their shell. Only the commands gitcraft starts by itself need [`path`].
pub fn terminal_path() -> Option<&'static OsString> {
    static PATH: OnceLock<Option<OsString>> = OnceLock::new();
    PATH.get_or_init(|| {
        if cfg!(target_os = "macos") {
            login_path(true)
        } else {
            None
        }
    })
    .as_ref()
}

/// `own` without its relative folders, when it has any. With none left, the system's own
/// folders: an empty PATH would be the current folder again, the repository's.
fn absolute_only(own: &OsString) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(own)
        .filter(|d| d.is_absolute())
        .collect();
    if dirs.is_empty() {
        dirs = system_dirs();
    }
    let joined = std::env::join_paths(dirs).ok()?;
    (joined != *own).then_some(joined)
}

/// Where a system keeps its programs, for a PATH that had nothing else.
fn system_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|root| vec![PathBuf::from(root).join("System32")])
            .unwrap_or_default()
    } else {
        ["/usr/local/bin", "/usr/bin", "/bin"]
            .map(PathBuf::from)
            .to_vec()
    }
}

/// `program` found in the folders of [`path`] (gitcraft's own PATH without relative folders)
/// when it is a bare name (`bash`), as it is otherwise. A program started in the repository's
/// folder and looked up through a PATH with relative entries could be one the repository
/// brought; the terminal's shell is found here, its PATH left as the user's.
#[cfg(unix)]
pub fn resolve(program: &str) -> String {
    if program.contains('/') {
        return program.to_owned();
    }
    let own = std::env::var_os("PATH").unwrap_or_default();
    let dirs = path().cloned().unwrap_or(own);
    std::env::split_paths(&dirs)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(program))
        .find(|p| p.is_file())
        .map_or_else(|| program.to_owned(), |p| p.display().to_string())
}

/// Gives `cmd` that PATH.
pub fn apply(cmd: &mut Command) {
    if let Some(path) = path() {
        cmd.env("PATH", path);
    }
}

/// The login shell's PATH merged with gitcraft's own, with its relative folders if
/// `keep_relative`. The shell is asked once, for both [`path`] and [`terminal_path`].
fn login_path(keep_relative: bool) -> Option<OsString> {
    static SHELL: OnceLock<OsString> = OnceLock::new();
    let own = std::env::var_os("PATH").unwrap_or_default();
    let shell = SHELL.get_or_init(|| shell_path().unwrap_or_default());
    let joined = merge([shell, &own], keep_relative);
    (joined != own).then_some(joined)
}

/// Every folder of `paths` once, in order, then the fallbacks. Only whole paths unless
/// `keep_relative`: git runs in the repository's folder, where `.` or `bin` would find a `git`
/// the repository brought.
fn merge<const N: usize>(paths: [&OsString; N], keep_relative: bool) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let candidates = paths
        .iter()
        .flat_map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .chain(FALLBACK.iter().map(PathBuf::from));
    for dir in candidates {
        if (keep_relative || dir.is_absolute()) && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Marks the PATH in the login shell's output, which may also hold whatever its profile prints.
const MARK: &str = "_GITCRAFT_PATH_";

/// `$PATH` as the user's login shell sets it, if it answers within a few seconds.
fn shell_path() -> Option<OsString> {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    path_of_shell(&shell)
}

fn path_of_shell(shell: &std::ffi::OsStr) -> Option<OsString> {
    let mut child = Command::new(shell)
        .args(["-l", "-c", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on a thread of its own and stop at the closing mark: a program the profile started
    // in the background can keep the pipe open long after the shell is gone.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
            // A profile's program writing on and on mustn't fill memory.
            if out.len() > 1 << 20 || marked(&String::from_utf8_lossy(&out)).is_some() {
                break;
            }
        }
        let _ = tx.send(marked(&String::from_utf8_lossy(&out)));
    });
    let found = rx.recv_timeout(Duration::from_secs(3)).ok().flatten();
    // A moment to finish (its logout files) once it printed, then it is stopped.
    let grace = Instant::now() + Duration::from_millis(300);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    found.map(OsString::from)
}

/// The PATH between the marks of the shell's output.
fn marked(out: &str) -> Option<String> {
    let (_, rest) = out.split_once(MARK)?;
    let (path, _) = rest.split_once(MARK)?;
    // As printed: a folder's name may start or end with a space.
    (!path.trim().is_empty()).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_kept_once_in_order() {
        let shell = std::env::join_paths(["/opt/homebrew/bin", "/usr/bin"]).unwrap_or_default();
        let own = std::env::join_paths(["/usr/bin", "/bin"]).unwrap_or_default();
        let merged: Vec<PathBuf> = std::env::split_paths(&merge([&shell, &own], false)).collect();
        assert_eq!(
            merged,
            ["/opt/homebrew/bin", "/usr/bin", "/bin", "/usr/local/bin"].map(PathBuf::from)
        );
    }

    #[cfg(unix)]
    #[test]
    fn folders_relative_to_the_repository_are_left_out() {
        let shell = OsString::from(".:bin:/usr/bin::node_modules/.bin");
        let merged: Vec<PathBuf> =
            std::env::split_paths(&merge([&shell, &OsString::new()], false)).collect();
        assert_eq!(
            merged,
            ["/usr/bin", "/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from)
        );
    }

    /// The terminal's PATH is the user's own: a deliberate `node_modules/.bin` stays there.
    #[cfg(unix)]
    #[test]
    fn the_terminal_keeps_relative_folders() {
        let shell = OsString::from(".:/usr/bin:node_modules/.bin");
        let own = OsString::from("/usr/bin:bin");
        let merged: Vec<PathBuf> = std::env::split_paths(&merge([&shell, &own], true)).collect();
        assert_eq!(
            merged,
            [
                ".",
                "/usr/bin",
                "node_modules/.bin",
                "bin",
                "/opt/homebrew/bin",
                "/usr/local/bin"
            ]
            .map(PathBuf::from)
        );
    }

    /// Outside macOS the terminal gets gitcraft's PATH as it is, relative folders included,
    /// while the commands gitcraft starts get it without them.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn outside_macos_the_terminal_keeps_the_apps_own_path() {
        assert_eq!(terminal_path(), None);
        let own = std::env::var_os("PATH").unwrap_or_default();
        let tools = path().unwrap_or(&own);
        assert!(std::env::split_paths(tools).all(|d| d.is_absolute()));
    }

    #[cfg(unix)]
    #[test]
    fn relative_folders_are_left_out_everywhere() {
        let own = OsString::from("/usr/bin:/bin:");
        assert_eq!(absolute_only(&own), Some(OsString::from("/usr/bin:/bin")));
        let own = OsString::from(".:/usr/bin::bin");
        assert_eq!(absolute_only(&own), Some(OsString::from("/usr/bin")));
        assert_eq!(absolute_only(&OsString::from("/usr/bin:/bin")), None);
        // Nothing absolute left: the system's folders, not an empty PATH (the current folder).
        assert_eq!(
            absolute_only(&OsString::from(".:bin")),
            Some(OsString::from("/usr/local/bin:/usr/bin:/bin"))
        );
    }

    /// A shell named without its folder is found among absolute folders, never in the
    /// repository the terminal starts in.
    #[cfg(unix)]
    #[test]
    fn a_bare_shell_name_is_found_in_an_absolute_folder() {
        let found = resolve("sh");
        assert!(std::path::Path::new(&found).is_absolute(), "{found}");
        assert_eq!(resolve("/bin/zsh"), "/bin/zsh");
    }

    #[test]
    fn the_path_is_read_between_the_marks() {
        let out =
            format!("Welcome back!\nLast login: today\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}");
        assert_eq!(marked(&out).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(marked("/usr/bin:/bin"), None);
        assert_eq!(marked(&format!("{MARK}/usr/bin")), None);
        assert_eq!(marked(&format!("{MARK}{MARK}")), None);
        assert_eq!(
            marked(&format!("{MARK}/usr/bin:/opt/my tools {MARK}")).as_deref(),
            Some("/usr/bin:/opt/my tools ")
        );
    }

    /// A profile that prints and leaves a program holding the pipe still gives the PATH, soon.
    #[cfg(unix)]
    #[test]
    fn reads_a_noisy_shell_without_waiting_for_its_children() {
        let dir = std::env::temp_dir().join(format!("gitcraft-shellpath-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let shell = dir.join("fake-shell");
        // Ignores `-l -c ...`: prints a greeting, the marked PATH, and leaves `sleep` behind.
        let script = format!(
            "#!/bin/sh\necho hello\nprintf '{MARK}/a/bin:/b/bin{MARK}'\nsleep 30 &\nexit 0\n"
        );
        let _ = std::fs::write(&shell, script);
        let _ = Command::new("chmod").arg("+x").arg(&shell).status();
        let started = std::time::Instant::now();
        let got = path_of_shell(shell.as_os_str());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, Some(OsString::from("/a/bin:/b/bin")));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
