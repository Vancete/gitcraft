//! Watching the repository's folder (and a linked worktree's git folders) for changes made
//! elsewhere.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use super::RepoTab;
use super::messages::{Msg, spawn};

impl RepoTab {
    /// Watches the folder so changes made elsewhere (editor, terminal) show up by themselves.
    /// Setting that up walks every folder of the worktree (a big `node_modules` is tens of
    /// thousands), so it is done in the background; the watcher comes back as a message.
    pub(super) fn watch(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.fs_rx = Some(rx);
        let path = self.path.clone();
        let events_ctx = ctx.clone();
        spawn(&self.tx, ctx, move || {
            Msg::Watcher(start_watcher(&path, tx, events_ctx).map(Box::new))
        });
    }
}

/// A watcher of the worktree `path` (and of a linked worktree's git folders) that sends what
/// each change calls for (see [`fs_change`]) to `tx`.
fn start_watcher(
    path: &Path,
    tx: Sender<FsChange>,
    ctx: egui::Context,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    // A linked worktree keeps its HEAD and index, and the repository its refs, outside its
    // folder: watched too, or commits and checkouts made elsewhere would never show.
    let linked = linked_git_dirs(path);
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
            if let Some(change) = fs_change(path, &git_dirs) {
                let _ = tx.send(change);
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    };
    // Git keeps a symbolic link as a link: nothing behind one changes what it reports, and
    // following them (pnpm's `node_modules` is full of them) walks the same folders over.
    let config = notify::Config::default().with_follow_symlinks(false);
    let mut watcher = notify::RecommendedWatcher::new(handler, config).ok()?;
    watcher.watch(path, notify::RecursiveMode::Recursive).ok()?;
    if let Some((own, common)) = &linked {
        use notify::RecursiveMode::{NonRecursive, Recursive};
        let _ = watcher.watch(own, Recursive);
        if common != own {
            // Not all of it: objects are written all the time.
            let _ = watcher.watch(common, NonRecursive);
            let _ = watcher.watch(&common.join("refs"), Recursive);
            // Where a repository made with `--ref-format=reftable` keeps them.
            let _ = watcher.watch(&common.join("reftable"), Recursive);
        }
    }
    Some(watcher)
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

/// What a change on disk calls for, from the least to the most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FsChange {
    /// A file of the worktree: re-reading its status.
    Files,
    /// The index (staging writes it, and so does any other tool's `git status`) or
    /// `.gitmodules`: re-reading the status and the submodules, whose commits the index holds.
    Index,
    /// Refs, HEAD, merge state: a full reload.
    Git,
}

/// What a tab reads again for a change on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reread {
    /// Everything (a full reload).
    All,
    Status,
    Submodules,
}

impl FsChange {
    /// What to read again for this change.
    pub(super) fn rereads(self) -> &'static [Reread] {
        match self {
            FsChange::Git => &[Reread::All],
            FsChange::Index => &[Reread::Status, Reread::Submodules],
            FsChange::Files => &[Reread::Status],
        }
    }
}

/// What a change on disk at `path` calls for (`None`: nothing). `git_dirs`: git folders
/// outside the worktree that are watched too.
pub(super) fn fs_change(path: &Path, git_dirs: &[PathBuf]) -> Option<FsChange> {
    // Objects and logs call for nothing. FETCH_HEAD (every fetch, even one that brings
    // nothing) and ORIG_HEAD (written along with HEAD) say nothing the refs don't.
    let relevant = |inside: &str| {
        if inside == "index" {
            Some(FsChange::Index)
        } else {
            (inside.starts_with("refs")
                || inside.starts_with("reftable")
                || inside == "HEAD"
                || inside == "packed-refs"
                || (inside.ends_with("_HEAD") && inside != "FETCH_HEAD" && inside != "ORIG_HEAD")
                || inside.starts_with("rebase-"))
            .then_some(FsChange::Git)
        }
    };
    if let Some(inside) = git_dirs.iter().find_map(|d| path.strip_prefix(d).ok()) {
        let inside = inside.to_string_lossy().replace('\\', "/");
        return relevant(&inside);
    }
    let s = path.to_string_lossy().replace('\\', "/");
    match s.split_once("/.git/") {
        Some((_, inside)) => relevant(inside),
        None if path.file_name().is_some_and(|n| n == ".gitmodules") => Some(FsChange::Index),
        None => Some(FsChange::Files),
    }
}
