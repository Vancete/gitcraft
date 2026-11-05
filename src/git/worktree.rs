//! The repository's worktrees.

use std::path::{Path, PathBuf};

use super::run;
use super::status::parse_status;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    /// Short branch name, `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
    /// Lock reason (may be empty) when locked.
    pub locked: Option<String>,
    /// Why git would prune it (its folder is gone…).
    pub prunable: Option<String>,
    /// The main worktree (the first one git lists).
    pub main: bool,
    /// The worktree this tab shows.
    pub current: bool,
    /// How many files have uncommitted changes; `None` when not known (bare, missing…).
    pub changes: Option<usize>,
}

impl Worktree {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

pub fn worktrees(repo: &Path) -> Result<Vec<Worktree>, String> {
    let mut list = match run(Some(repo), &["worktree", "list", "--porcelain", "-z"]) {
        Ok(out) => parse_worktrees(&out, '\0'),
        // `-z` is git 2.36+; before, one line per field (a path with a line break in it is
        // misread there).
        Err(_) => parse_worktrees(
            &run(Some(repo), &["worktree", "list", "--porcelain"])?,
            '\n',
        ),
    };
    let here = std::fs::canonicalize(repo).ok();
    for wt in &mut list {
        wt.current = here.is_some() && std::fs::canonicalize(&wt.path).ok() == here;
    }
    // One `status` per worktree, side by side. `--no-optional-locks` so that looking doesn't take
    // the index lock from under whatever runs in that worktree.
    std::thread::scope(|scope| {
        for wt in list.iter_mut().filter(|w| !w.bare && w.prunable.is_none()) {
            scope.spawn(|| {
                let args = [
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                ];
                wt.changes = run(Some(&wt.path), &args)
                    .ok()
                    .map(|out| parse_status(&out).changed_files());
            });
        }
    });
    Ok(list)
}

/// `worktree list --porcelain -z`: `key value\0` lines, an empty line between worktrees
/// (`sep` is `\n` without `-z`).
fn parse_worktrees(out: &str, sep: char) -> Vec<Worktree> {
    let mut list: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in out.split(sep) {
        if line.is_empty() {
            list.extend(current.take());
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            list.extend(current.take());
            current = Some(Worktree {
                path: PathBuf::from(value),
                main: list.is_empty(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(wt) = current.as_mut() else { continue };
        match key {
            "HEAD" => wt.head = value.to_owned(),
            "branch" => {
                wt.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_owned(),
                )
            }
            "bare" => wt.bare = true,
            "locked" => wt.locked = Some(value.to_owned()),
            "prunable" => wt.prunable = Some(value.to_owned()),
            _ => {}
        }
    }
    list.extend(current);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_list() {
        let out = "worktree C:/src/app\0HEAD abc\0branch refs/heads/main\0\0\
                   worktree C:/src/app-fix\0HEAD def\0detached\0locked moving disks\0\0\
                   worktree C:/gone\0HEAD 123\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0";
        let list = parse_worktrees(out, '\0');
        assert_eq!(list.len(), 3);
        // Git before 2.36 has no `-z`: the same, a field per line.
        assert_eq!(parse_worktrees(&out.replace('\0', "\n"), '\n'), list);
        assert!(list[0].main && !list[1].main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch, None);
        assert_eq!(list[1].locked.as_deref(), Some("moving disks"));
        assert!(list[2].prunable.is_some());
        assert_eq!(list[1].name(), "app-fix");
    }
}
