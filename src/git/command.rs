//! Running git: the environment every call gets, credentials for remotes, errors.

use std::path::Path;
use std::process::Command;

fn git_cmd() -> Command {
    let mut cmd = Command::new("git");
    // Never block on an interactive prompt: credential helpers with a GUI still work.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    // `rebase --continue`, `merge --continue`… would otherwise open an editor nobody sees.
    cmd.env("GIT_EDITOR", "true");
    // Interactive rebases set `sequence.editor` themselves; an inherited one would win.
    cmd.env_remove("GIT_SEQUENCE_EDITOR");
    // Messages in English whatever the user's language: errors are told apart by their words
    // (`is_auth_error`…). `C` in LANGUAGE wins over LC_ALL / LANG for messages only, so the
    // character set (and what hooks see of it) stays the user's.
    cmd.env("LANGUAGE", "C");
    crate::shell_path::apply(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs git in `dir` and returns stdout, or the error output.
pub fn run(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    run_ok_codes(dir, args, &[0])
}

/// Runs git and returns stdout as raw bytes (file contents: images…).
pub fn run_bytes(dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = git_cmd();
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Starts git in `dir` with its output piped, for a caller that reads it bit by bit.
pub(super) fn spawn_piped(dir: &Path, args: &[&str]) -> Result<std::process::Child, String> {
    let mut cmd = git_cmd();
    cmd.arg("-C").arg(dir).args(args);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.spawn().map_err(|e| format!("could not run git: {e}"))
}

/// Like [`run`], but exit codes in `ok` count as success (`diff --no-index` exits 1 on changes).
pub fn run_ok_codes(dir: Option<&Path>, args: &[&str], ok: &[i32]) -> Result<String, String> {
    run_inner(dir, args, ok, None, false)
}

/// A username and password (or token) for an HTTPS remote, typed into gitcraft.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// A credential helper that answers git's `get` from the environment, so the secret never shows
/// up in a command line. git tries the user's own helpers first and, when the operation works,
/// asks them to store what this one gave.
/// `printf '%s'`, not `echo`: the `sh` git runs it with (dash…) would read backslashes in them
/// as escapes (`CORP\nancy` broken in two lines, `\c` cutting the password short).
const ENV_HELPER: &str = "credential.helper=!f() { test \"$1\" = get && printf 'username=%s\\npassword=%s\\n' \"$GITCRAFT_GIT_USERNAME\" \"$GITCRAFT_GIT_PASSWORD\"; }; f";

/// [`run`], answering a credential request with `creds` when given.
pub fn run_with(
    dir: Option<&Path>,
    args: &[&str],
    creds: Option<&Credentials>,
) -> Result<String, String> {
    run_inner(dir, args, &[0], creds, false)
}

/// [`run_with`] for an operation nobody is waiting on (the background fetch): it asks nobody
/// for credentials (see [`unattended`]). Helpers that answer from what they stored still do,
/// as do credentials typed into the tab (`creds`).
pub fn run_unattended(
    dir: Option<&Path>,
    args: &[&str],
    creds: Option<&Credentials>,
) -> Result<String, String> {
    run_inner(dir, args, &[0], creds, true)
}

/// Sets `cmd` (before its subcommand) to open no sign-in window: no credential helper window
/// (Git Credential Manager and git ≥ 2.46 heed `credential.interactive`), nor git's askpass
/// program. git reads `GIT_ASKPASS` before `core.askPass` and `SSH_ASKPASS`, so an inherited one
/// (VS Code's terminal, ksshaskpass) would win over an empty `core.askPass`, and its being set
/// but empty makes git skip all three. ssh asks on the terminal the app was started from, if
/// any (a passphrase, a new host key: the fetch would wait there for good), or else runs
/// `SSH_ASKPASS` itself: on Unix it is made to ask a program that answers nothing, `false`,
/// and nothing else; where that isn't there, never to run one.
fn unattended(cmd: &mut Command) {
    cmd.args(["-c", "credential.interactive=false", "-c", "core.askPass="]);
    cmd.env("GIT_ASKPASS", "");
    if cfg!(unix) {
        cmd.env("SSH_ASKPASS_REQUIRE", "force");
        cmd.env("SSH_ASKPASS", "false");
    } else {
        cmd.env("SSH_ASKPASS_REQUIRE", "never");
    }
}

/// Whether `err` says git needed a username / password it couldn't get or that were refused.
pub fn is_auth_error(err: &str) -> bool {
    let e = err.to_lowercase();
    [
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "invalid username or password",
        "http basic: access denied",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
    ]
    .iter()
    .any(|s| e.contains(s))
}

fn run_inner(
    dir: Option<&Path>,
    args: &[&str],
    ok: &[i32],
    creds: Option<&Credentials>,
    quiet: bool,
) -> Result<String, String> {
    let mut cmd = git_cmd();
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    if quiet {
        unattended(&mut cmd);
    }
    if let Some(c) = creds {
        cmd.args(["-c", ENV_HELPER]);
        cmd.env("GITCRAFT_GIT_USERNAME", &c.username);
        cmd.env("GITCRAFT_GIT_PASSWORD", &c.password);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.code().is_some_and(|c| ok.contains(&c)) {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        // Merges and pulls report conflicts on stdout and the summary on stderr.
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let err = match (stderr.is_empty(), stdout.is_empty()) {
            (true, true) => format!("git {} failed ({})", args.join(" "), out.status),
            (false, true) => stderr,
            (true, false) => stdout,
            (false, false) => format!("{stderr}\n{stdout}"),
        };
        Err(err)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::Stdio;

    use super::*;

    /// What git gets from the helper is what was typed, backslashes (`DOMAIN\user`) included.
    #[test]
    fn credentials_reach_git_as_typed() -> Result<(), String> {
        let (user, pass) = ("CORP\\nancy", "p\\c%s$x 'q\"");
        let mut child = git_cmd()
            // Only this helper: the user's own would answer first.
            .args([
                "-c",
                "credential.helper=",
                "-c",
                ENV_HELPER,
                "credential",
                "fill",
            ])
            .env("GITCRAFT_GIT_USERNAME", user)
            .env("GITCRAFT_GIT_PASSWORD", pass)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(b"protocol=https\nhost=example.invalid\n\n")
                .map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains(&format!("username={user}\n")), "{text}");
        assert!(text.contains(&format!("password={pass}\n")), "{text}");
        Ok(())
    }

    #[cfg(unix)]
    fn scratch(name: &str) -> Result<std::path::PathBuf, String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-cmd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    /// A script that leaves `marker` behind when run, then prints `x` (an askpass answer).
    #[cfg(unix)]
    fn marking_script(dir: &Path, marker: &Path) -> Result<std::path::PathBuf, String> {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("askpass.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\necho x\n", marker.display()),
        )
        .map_err(|e| e.to_string())?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        Ok(script)
    }

    /// Whether git, asked for a credential (quietly or not) in an environment that sets
    /// `SSH_ASKPASS` (KDE's opens a window) and maybe `GIT_ASKPASS` (VS Code's terminal), ran
    /// the askpass program.
    #[cfg(unix)]
    fn askpass_asked(dir: &Path, quiet: bool, git_askpass: bool) -> Result<bool, String> {
        let marker = dir.join("asked");
        let _ = std::fs::remove_file(&marker);
        let script = marking_script(dir, &marker)?;
        let mut cmd = git_cmd();
        // Not the user's own config, whose `core.askPass` would stand in for the script.
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        // What the app inherited, which `unattended` then has to override.
        cmd.env("SSH_ASKPASS", &script);
        if git_askpass {
            cmd.env("GIT_ASKPASS", &script);
        } else {
            cmd.env_remove("GIT_ASKPASS");
        }
        if quiet {
            unattended(&mut cmd);
        }
        let mut child = cmd
            .args(["-c", "credential.helper=", "credential", "fill"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"protocol=https\nhost=example.invalid\n\n");
        }
        child.wait().map_err(|e| e.to_string())?;
        Ok(marker.exists())
    }

    /// The background fetch asks nobody to sign in, every few minutes, through a window: not
    /// even with a `GIT_ASKPASS` in the environment, which git reads before `core.askPass`.
    #[cfg(unix)]
    #[test]
    fn a_quiet_operation_opens_no_sign_in_window() -> Result<(), String> {
        let dir = scratch("unattended")?;
        let asked = [(false, false), (true, false), (false, true), (true, true)]
            .map(|(quiet, git_askpass)| askpass_asked(&dir, quiet, git_askpass));
        let _ = std::fs::remove_dir_all(&dir);
        // Without it, git does ask (or the test would prove nothing).
        assert_eq!(asked, [Ok(true), Ok(false), Ok(true), Ok(false)]);
        Ok(())
    }

    /// Nor does ssh, which would ask on a terminal or run `SSH_ASKPASS` itself.
    #[cfg(unix)]
    #[test]
    fn a_quiet_operation_tells_ssh_to_ask_nobody() -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unattended-ssh")?;
        let seen = dir.join("seen");
        let ssh = dir.join("ssh.sh");
        std::fs::write(
            &ssh,
            format!(
                "#!/bin/sh\nprintf '%s %s' \"$SSH_ASKPASS_REQUIRE\" \"$SSH_ASKPASS\" > '{}'\nexit 1\n",
                seen.display()
            ),
        )
        .map_err(|e| e.to_string())?;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        let mut cmd = git_cmd();
        cmd.env("GIT_SSH_COMMAND", &ssh)
            .env_remove("SSH_ASKPASS_REQUIRE");
        unattended(&mut cmd);
        let out = cmd
            .args(["ls-remote", "ssh://example.invalid/repo.git"])
            .output()
            .map_err(|e| e.to_string())?;
        let required = std::fs::read_to_string(&seen);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!out.status.success());
        // Asked through `false`, which answers nothing, never on a terminal.
        assert_eq!(required.map_err(|e| e.to_string())?, "force false");
        Ok(())
    }

    /// Credentials typed into the tab still answer a quiet operation.
    #[test]
    fn typed_credentials_answer_a_quiet_operation() -> Result<(), String> {
        let mut cmd = git_cmd();
        unattended(&mut cmd);
        let mut child = cmd
            .args([
                "-c",
                "credential.helper=",
                "-c",
                ENV_HELPER,
                "credential",
                "fill",
            ])
            .env("GITCRAFT_GIT_USERNAME", "nancy")
            .env("GITCRAFT_GIT_PASSWORD", "secret")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(b"protocol=https\nhost=example.invalid\n\n")
                .map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("username=nancy\n"), "{text}");
        assert!(text.contains("password=secret\n"), "{text}");
        Ok(())
    }
}
