//! Profiles: separate sets of open tabs and recent repositories (work, a client, personal…),
//! each with an optional projects folder, a commit identity, a GitHub account and extra
//! environment variables for the git, `gh` and terminal processes started while it is active.
//!
//! Only one profile is active at a time and only its tabs are open, so the processes it starts
//! read what to add from one global: [`activate`] sets it, the `apply_*` functions use it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The profile used when none is chosen; its tabs and recent list are the ones from before
/// profiles existed.
pub const DEFAULT_ID: u64 = 0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// Stable key of its saved tabs (the name can change).
    pub id: u64,
    pub name: String,
    /// Projects folder: where clones go and the Open dialog starts, and whose repositories
    /// the start page lists. `~` is the home folder. Empty for none.
    pub folder: String,
    /// Commit identity; empty fields leave git's own (global, `includeIf`) in place. A
    /// repository's own `user.name` (`git config --local`) still wins.
    pub git_name: String,
    pub git_email: String,
    /// The `gh` account (login) used for pull requests and HTTPS pushes to GitHub. `gh auth
    /// login` each account first; empty uses gh's active one.
    pub github_user: String,
    /// `KEY=VALUE` per line, set for git, gh and terminals: `GIT_SSH_COMMAND`,
    /// `AZURE_CONFIG_DIR`, `AZURE_DEVOPS_EXT_PAT`… Lines starting with `#` are ignored.
    pub env: String,
}

impl Workspace {
    pub fn folder_path(&self) -> Option<PathBuf> {
        let f = self.folder.trim();
        (!f.is_empty()).then(|| expand_home(f))
    }

    /// Whether `path` is inside its projects folder.
    pub fn contains(&self, path: &Path) -> bool {
        self.folder_path().is_some_and(|f| {
            let f = std::fs::canonicalize(&f).unwrap_or(f);
            path.starts_with(&f)
        })
    }

    pub fn env_vars(&self) -> Vec<(String, String)> {
        parse_env(&self.env)
    }

    fn has_identity(&self) -> bool {
        !self.git_name.trim().is_empty() || !self.git_email.trim().is_empty()
    }
}

/// The id a new profile gets: one more than the largest so far.
pub fn next_id(list: &[Workspace]) -> u64 {
    list.iter().map(|w| w.id).max().unwrap_or(DEFAULT_ID) + 1
}

/// The profile whose folder holds `path`, the deepest folder winning.
pub fn owning(list: &[Workspace], path: &Path) -> Option<u64> {
    list.iter()
        .filter(|w| w.contains(path))
        .max_by_key(|w| w.folder_path().map_or(0, |f| f.components().count()))
        .map(|w| w.id)
}

pub fn expand_home(s: &str) -> PathBuf {
    match s.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(s),
    }
}

fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            let v = v.trim().trim_matches('"');
            let valid = !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            valid.then(|| {
                let v = if v.starts_with("~/") {
                    expand_home(v).display().to_string()
                } else {
                    v.to_owned()
                };
                (k.to_owned(), v)
            })
        })
        .collect()
}

/// The git repositories in `folder` and one level below it (`folder/x`, `folder/group/x`),
/// sorted by path. Hidden folders and `node_modules` are skipped.
pub fn repos_in(folder: &Path) -> Vec<PathBuf> {
    fn children(dir: &Path) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        rd.filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.starts_with('.') && n != "node_modules")
            })
            .collect()
    }
    let is_repo = |p: &Path| p.join(".git").exists();
    let mut out = Vec::new();
    for child in children(folder) {
        if is_repo(&child) {
            out.push(child);
        } else {
            out.extend(children(&child).into_iter().filter(|p| is_repo(p)));
        }
    }
    out.sort();
    out
}

struct Active {
    ws: Workspace,
    /// `gh auth token --user …`, asked once (None: not yet).
    gh_token: Option<Option<String>>,
    /// Whether a repository sets its own `user.name`, by the folder git runs in.
    local_identity: HashMap<PathBuf, bool>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Active>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes `ws` the profile processes are started for (`None`: add nothing). Setting the same
/// profile again keeps what was found out about it.
pub fn activate(ws: Option<&Workspace>) {
    let mut guard = lock();
    if guard.as_ref().map(|a| &a.ws) == ws {
        return;
    }
    *guard = ws.map(|ws| Active {
        ws: ws.clone(),
        gh_token: None,
        local_identity: HashMap::new(),
    });
}

/// The token of `user`'s `gh` login on github.com.
pub fn github_token(user: &str) -> Result<String, String> {
    let mut cmd = Command::new("gh");
    cmd.args(["auth", "token", "--hostname", "github.com", "--user", user]);
    cmd.env("GH_PROMPT_DISABLED", "1");
    cmd.env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN");
    crate::shell_path::apply(&mut cmd);
    hide_window(&mut cmd);
    let out = cmd.output().map_err(|_| "gh isn't installed".to_owned())?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if out.status.success() && !token.is_empty() {
        Ok(token)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if err.is_empty() {
            format!("gh has no login for {user} (run gh auth login)")
        } else {
            err
        })
    }
}

/// The active profile's token, asking gh the first time (outside the lock: gh takes a moment).
fn token() -> Option<String> {
    let user = {
        let guard = lock();
        let active = guard.as_ref()?;
        if let Some(t) = &active.gh_token {
            return t.clone();
        }
        active.ws.github_user.trim().to_owned()
    };
    if user.is_empty() {
        return None;
    }
    let token = github_token(&user).ok();
    if let Some(active) = lock().as_mut()
        && active.ws.github_user.trim() == user
    {
        active.gh_token = Some(token.clone());
    }
    token
}

/// Whether the repository git runs in sets its own `user.name`.
fn local_identity(dir: &Path) -> bool {
    if let Some(known) = lock()
        .as_ref()
        .and_then(|a| a.local_identity.get(dir).copied())
    {
        return known;
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["config", "--local", "--get", "user.name"]);
    crate::shell_path::apply(&mut cmd);
    hide_window(&mut cmd);
    let local = cmd.output().is_ok_and(|o| o.status.success());
    if let Some(active) = lock().as_mut() {
        active.local_identity.insert(dir.to_owned(), local);
    }
    local
}

/// The variables a process of the active profile gets; `dir`: the repository it runs in (the
/// identity is left out where the repository has its own).
fn vars(dir: Option<&Path>, identity: bool) -> Vec<(String, String)> {
    let Some(ws) = lock().as_ref().map(|a| a.ws.clone()) else {
        return Vec::new();
    };
    let mut out = ws.env_vars();
    if !ws.github_user.trim().is_empty()
        && let Some(t) = token()
    {
        out.push(("GH_TOKEN".to_owned(), t));
    }
    if identity && ws.has_identity() && !dir.is_some_and(local_identity) {
        // Command-line scope config (like `git -c`), so it beats the global config and
        // `includeIf`; a repository's own identity was checked above.
        let base: usize = std::env::var("GIT_CONFIG_COUNT")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let mut n = base;
        for (key, value) in [("user.name", &ws.git_name), ("user.email", &ws.git_email)] {
            let value = value.trim();
            if !value.is_empty() {
                out.push((format!("GIT_CONFIG_KEY_{n}"), key.to_owned()));
                out.push((format!("GIT_CONFIG_VALUE_{n}"), value.to_owned()));
                n += 1;
            }
        }
        out.push(("GIT_CONFIG_COUNT".to_owned(), n.to_string()));
    }
    out
}

/// For a git command run in `dir` with `args`.
pub fn apply_git(cmd: &mut Command, dir: Option<&Path>, args: &[&str]) {
    // Setting or unsetting the identity: read it again next time, and don't let the profile's
    // stand in for what is being changed.
    let config = args.first() == Some(&"config");
    if config && let (Some(dir), Some(active)) = (dir, lock().as_mut()) {
        active.local_identity.remove(dir);
    }
    for (k, v) in vars(dir, !config) {
        cmd.env(k, v);
    }
}

/// For `gh` / `glab`.
pub fn apply_tool(cmd: &mut Command) {
    for (k, v) in vars(None, false) {
        cmd.env(k, v);
    }
}

/// The variables for a terminal opened in `cwd`.
pub fn terminal_vars(cwd: Option<&Path>) -> Vec<(String, String)> {
    vars(cwd, true)
}

fn hide_window(_cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        _cmd.creation_flags(0x0800_0000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_lines() {
        let vars = parse_env(
            "# comment\nGIT_SSH_COMMAND=ssh -i ~/.ssh/work\n\nexport AZURE_CONFIG_DIR=\"/x/y\"\nbad line\n1=ok?\nBAD-KEY=1",
        );
        assert_eq!(
            vars,
            vec![
                (
                    "GIT_SSH_COMMAND".to_owned(),
                    "ssh -i ~/.ssh/work".to_owned()
                ),
                ("AZURE_CONFIG_DIR".to_owned(), "/x/y".to_owned()),
                ("1".to_owned(), "ok?".to_owned()),
            ]
        );
    }

    #[test]
    fn deepest_folder_owns() {
        let ws = |id, folder: &str| Workspace {
            id,
            folder: folder.to_owned(),
            ..Default::default()
        };
        let list = [
            ws(1, "/nonexistent/a"),
            ws(2, "/nonexistent/a/b"),
            ws(3, ""),
        ];
        assert_eq!(owning(&list, Path::new("/nonexistent/a/b/repo")), Some(2));
        assert_eq!(owning(&list, Path::new("/nonexistent/a/repo")), Some(1));
        assert_eq!(owning(&list, Path::new("/elsewhere/repo")), None);
        assert_eq!(next_id(&list), 4);
        assert_eq!(next_id(&[]), 1);
    }

    #[test]
    fn finds_repositories_two_levels_down() {
        let root = std::env::temp_dir().join(format!("gitcraft-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for p in [
            "one/.git",
            "group/two/.git",
            "group/plain",
            ".hidden/x/.git",
        ] {
            std::fs::create_dir_all(root.join(p)).ok();
        }
        let found = repos_in(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(found, vec![root.join("group/two"), root.join("one")]);
    }
}
