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
    run_inner(dir, args, ok, None)
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
    run_inner(dir, args, &[0], creds)
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
) -> Result<String, String> {
    let mut cmd = git_cmd();
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
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
}
