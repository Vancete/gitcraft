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

/// The PATH for commands gitcraft runs, when it differs from its own (macOS only).
pub fn path() -> Option<&'static OsString> {
    static PATH: OnceLock<Option<OsString>> = OnceLock::new();
    PATH.get_or_init(|| {
        if cfg!(target_os = "macos") {
            login_path()
        } else {
            None
        }
    })
    .as_ref()
}

/// Gives `cmd` that PATH.
pub fn apply(cmd: &mut Command) {
    if let Some(path) = path() {
        cmd.env("PATH", path);
    }
}

fn login_path() -> Option<OsString> {
    let own = std::env::var_os("PATH").unwrap_or_default();
    let shell = shell_path().unwrap_or_default();
    let joined = merge([&shell, &own]);
    (joined != own).then_some(joined)
}

/// Every folder of `paths` once, in order, then the fallbacks.
fn merge<const N: usize>(paths: [&OsString; N]) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let candidates = paths
        .iter()
        .flat_map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .chain(FALLBACK.iter().map(PathBuf::from));
    for dir in candidates {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
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
            if marked(&String::from_utf8_lossy(&out)).is_some() {
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
    let path = path.trim();
    (!path.is_empty()).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_kept_once_in_order() {
        let shell = std::env::join_paths(["/opt/homebrew/bin", "/usr/bin"]).unwrap_or_default();
        let own = std::env::join_paths(["/usr/bin", "/bin"]).unwrap_or_default();
        let merged: Vec<PathBuf> = std::env::split_paths(&merge([&shell, &own])).collect();
        assert_eq!(
            merged,
            ["/opt/homebrew/bin", "/usr/bin", "/bin", "/usr/local/bin"].map(PathBuf::from)
        );
    }

    #[test]
    fn the_path_is_read_between_the_marks() {
        let out =
            format!("Welcome back!\nLast login: today\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}");
        assert_eq!(marked(&out).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(marked("/usr/bin:/bin"), None);
        assert_eq!(marked(&format!("{MARK}/usr/bin")), None);
        assert_eq!(marked(&format!("{MARK}{MARK}")), None);
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
