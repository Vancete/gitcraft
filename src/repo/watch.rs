//! Watching the repository's folder (and a linked worktree's git folders) for changes made
//! elsewhere.

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use super::RepoTab;

impl RepoTab {
    /// Watches the folder so changes made elsewhere (editor, terminal) show up by themselves.
    pub(super) fn watch(&mut self, ctx: &egui::Context) {
        use notify::Watcher;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        // A linked worktree keeps its HEAD and index, and the repository its refs, outside its
        // folder: watched too, or commits and checkouts made elsewhere would never show.
        let linked = linked_git_dirs(&self.path);
        let git_dirs: Vec<PathBuf> = linked
            .iter()
            .flat_map(|(own, common)| [own.clone(), common.clone()])
            .collect();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                if let Some(git_dir) = fs_change(path, &git_dirs) {
                    let _ = tx.send(git_dir);
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
            }
        };
        if let Ok(mut watcher) = notify::recommended_watcher(handler)
            && watcher
                .watch(&self.path, notify::RecursiveMode::Recursive)
                .is_ok()
        {
            if let Some((own, common)) = &linked {
                use notify::RecursiveMode::{NonRecursive, Recursive};
                let _ = watcher.watch(own, Recursive);
                if common != own {
                    // Not all of it: objects are written all the time.
                    let _ = watcher.watch(common, NonRecursive);
                    let _ = watcher.watch(&common.join("refs"), Recursive);
                }
            }
            self.watcher = Some(watcher);
            self.fs_rx = Some(rx);
        }
    }
}

/// A linked worktree's (or submodule's) own git folder and the repository's common one, both
/// outside `worktree`, which has a `.git` file pointing there. `None` for a `.git` folder.
pub(super) fn linked_git_dirs(worktree: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot_git = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let own = dot_git
        .lines()
        .find_map(|l| l.strip_prefix("gitdir:"))?
        .trim();
    let own = std::fs::canonicalize(worktree.join(own)).ok()?;
    let common = std::fs::read_to_string(own.join("commondir"))
        .ok()
        .and_then(|c| std::fs::canonicalize(own.join(c.trim())).ok())
        .unwrap_or_else(|| own.clone());
    Some((own, common))
}

/// What a change on disk at `path` calls for: `Some(true)` a full reload (git's data moved),
/// `Some(false)` re-reading the working tree, `None` nothing. `git_dirs`: git folders outside
/// the worktree that are watched too.
pub(super) fn fs_change(path: &Path, git_dirs: &[PathBuf]) -> Option<bool> {
    // Refs, HEAD, the index and merge state matter; objects and logs don't.
    let relevant = |inside: &str| {
        inside.starts_with("refs")
            || inside == "HEAD"
            || inside == "index"
            || inside == "packed-refs"
            || inside.ends_with("_HEAD")
            || inside.starts_with("rebase-")
    };
    if let Some(inside) = git_dirs.iter().find_map(|d| path.strip_prefix(d).ok()) {
        let inside = inside.to_string_lossy().replace('\\', "/");
        return relevant(&inside).then_some(true);
    }
    let s = path.to_string_lossy().replace('\\', "/");
    match s.split_once("/.git/") {
        Some((_, inside)) => relevant(inside).then_some(true),
        None => Some(false),
    }
}
