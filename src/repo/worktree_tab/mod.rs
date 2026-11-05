//! Worktrees in the tab that opened them. A tab shows one worktree and keeps the others it was
//! on parked in the background, so that coming back finds everything as it was: terminals keep
//! running, operations finish, a half-written commit message stays.

mod switcher;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use super::RepoTab;
use crate::git::Worktree;

pub(crate) use switcher::{CHIP_CLOSE_W, Chip, SWITCHER_W, chip_for, paint_chip, switcher};
pub(super) use switcher::{changes_elsewhere, changes_line, paint_dirty};

/// Idle worktrees kept for coming back to. Those with unfinished work are always kept.
const KEEP_IDLE: usize = 3;
const MAX_BRANCH_CHARS: usize = 26;

/// What can be going on in a tab, which it must not lose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Unfinished {
    pub operation: bool,
    pub terminal: bool,
    pub commit_message: bool,
    pub editor: bool,
    pub review: bool,
}

impl Unfinished {
    /// What to call it next to the worktree's name, if anything is going on.
    pub fn label(self) -> Option<&'static str> {
        if self.operation {
            Some("running")
        } else if self.terminal {
            Some("terminal")
        } else if self.commit_message {
            Some("commit message")
        } else if self.editor {
            Some("editor")
        } else if self.review {
            Some("review draft")
        } else {
            None
        }
    }

    /// Whether some of it goes on by itself, an operation or a terminal, and has an end to wait
    /// for. The rest waits on the user.
    pub fn running(self) -> bool {
        self.operation || self.terminal
    }
}

/// What a tab shows about the worktree it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabLabel {
    pub repo: String,
    /// The branch of a linked worktree; the main worktree shows none.
    pub branch: Option<String>,
}

/// The label of a tab showing `worktrees`' current one; `plain` until they are known.
pub(super) fn tab_label(worktrees: &[Worktree], plain: &str) -> TabLabel {
    let linked = worktrees.iter().find(|w| w.current && !w.main);
    let main = worktrees.iter().find(|w| w.main);
    let (Some(linked), Some(main)) = (linked, main) else {
        return TabLabel {
            repo: plain.to_owned(),
            branch: None,
        };
    };
    let name = main.name();
    let branch = match &linked.branch {
        Some(branch) => branch.clone(),
        None => linked.head.get(..7).unwrap_or(&linked.head).to_owned(),
    };
    TabLabel {
        repo: name.strip_suffix(".git").unwrap_or(&name).to_owned(),
        branch: Some(branch),
    }
}

/// The worktree closing a linked one goes back to, if there is one to go back to.
pub(super) fn main_to_return_to(worktrees: &[Worktree]) -> Option<&Path> {
    worktrees
        .iter()
        .find(|w| w.main && !w.bare && w.prunable.is_none())
        .map(|w| w.path.as_path())
}

/// `name` in at most `max` characters, keeping its end: that is what tells branches apart.
pub(super) fn shorten_middle(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_owned();
    }
    if let (Some(first), Some(last)) = (name.split('/').next(), name.rsplit('/').next())
        && name.contains('/')
    {
        let folders = format!("{first}/…/{last}");
        if folders.chars().count() <= max {
            return folders;
        }
    }
    let room = max.saturating_sub(1);
    let head = room.div_ceil(3);
    let tail = room - head;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

/// `name` shortened as by `shorten_middle`, further if need be, until `width` measures it at no
/// more than `max_w`: some characters are much wider than others.
pub(super) fn fit_branch(name: &str, max_w: f32, width: impl Fn(&str) -> f32) -> String {
    let mut max = MAX_BRANCH_CHARS;
    loop {
        let short = shorten_middle(name, max);
        if max <= 1 || width(&short) <= max_w {
            return short;
        }
        max -= 1;
    }
}

/// Which parked worktrees to let go of, oldest first in `unfinished`: all but the last `keep` of
/// those with nothing unfinished.
pub(super) fn parked_to_drop(unfinished: &[bool], keep: usize) -> Vec<usize> {
    let idle: Vec<usize> = (0..unfinished.len()).filter(|&i| !unfinished[i]).collect();
    let excess = idle.len().saturating_sub(keep);
    idle[..excess].to_vec()
}

/// The worktrees the switcher lists for `query`: the current one, then those with work in the
/// background, then the rest. Folders that are gone can't be opened, nor can a bare repository,
/// which has no files to show, so they are left out.
pub(super) fn picker_entries<'a>(
    worktrees: &'a [Worktree],
    background: &[(PathBuf, &'static str)],
    query: &str,
) -> Vec<&'a Worktree> {
    let query = query.to_lowercase();
    let mut list: Vec<&Worktree> = worktrees
        .iter()
        .filter(|w| w.prunable.is_none() && !w.bare)
        .filter(|w| {
            let branch = w.branch.as_deref().unwrap_or("").to_lowercase();
            branch.contains(&query) || w.name().to_lowercase().contains(&query)
        })
        .collect();
    let in_background = |w: &Worktree| background.iter().any(|(path, _)| *path == w.path);
    list.sort_by_key(|w| {
        if w.current {
            0
        } else if in_background(w) {
            1
        } else {
            2
        }
    });
    list
}

impl RepoTab {
    fn work(&mut self) -> Unfinished {
        Unfinished {
            operation: self.busy_op.is_some(),
            terminal: self.terminals.iter_mut().any(|t| !t.has_exited()),
            commit_message: !self.commit_summary.trim().is_empty()
                || !self.commit_body.trim().is_empty()
                || self.reword.is_some(),
            editor: self.rebase.is_some() || self.conflict.is_some(),
            review: self.has_pr_draft(),
        }
    }

    fn unfinished(&mut self) -> Option<&'static str> {
        self.work().label()
    }

    /// What the tab strip shows for this tab.
    pub(crate) fn label(&self, worktrees_in_same_tab: bool) -> TabLabel {
        if worktrees_in_same_tab {
            tab_label(&self.worktrees, &self.name)
        } else {
            TabLabel {
                repo: self.name.clone(),
                branch: None,
            }
        }
    }

    /// The repository this tab belongs to, whichever of its worktrees it shows: what is kept
    /// about the sidebar is the same for all of them.
    pub(super) fn repo_key(&self) -> &Path {
        self.worktrees
            .iter()
            .find(|w| w.main)
            .map_or(self.path.as_path(), |w| w.path.as_path())
    }

    /// Takes over from `from`, the same repository's view this one follows, so that the name and
    /// the sidebar are right from the first frame and a sign-in isn't asked for again.
    pub(crate) fn continue_from(&mut self, from: &RepoTab) {
        let here = std::fs::canonicalize(&self.path).ok();
        self.worktrees = from
            .worktrees
            .iter()
            .cloned()
            .map(|mut w| {
                w.current = here.is_some() && std::fs::canonicalize(&w.path).ok() == here;
                w
            })
            .collect();
        if from.credentials.is_some() {
            self.credentials = from.credentials.clone();
        }
    }

    /// Coming back to this view from the background: catch up, and show the selected commit
    /// again, as the graph's scroll position is the tab's and not the view's.
    pub(crate) fn resume(&mut self, ctx: &egui::Context) {
        self.reload(ctx);
        self.scroll_to = self.selected;
    }

    /// The worktrees with work going on in the background, and what that work is.
    pub(crate) fn background_work(&self) -> &[(PathBuf, &'static str)] {
        &self.parked_status
    }

    /// Keeps the parked worktrees going, as background tabs are.
    pub(crate) fn poll_parked(&mut self, ctx: &egui::Context) {
        let mut status = Vec::new();
        for parked in &mut self.parked {
            parked.poll(ctx);
            self.finished.append(&mut parked.finished);
            if let Some(work) = parked.unfinished() {
                status.push((parked.path.clone(), work));
            }
        }
        self.parked_status = status;
    }

    /// Parks `view`, the worktree this tab just moved away from, and lets go of what isn't worth
    /// keeping any more.
    pub(crate) fn park(&mut self, view: Box<RepoTab>) {
        self.parked.push(view);
        // A worktree whose folder is gone can't be gone back to, but what still runs in it is
        // seen through first.
        self.parked
            .retain_mut(|parked| parked.path.is_dir() || parked.work().running());
        let unfinished: Vec<bool> = self
            .parked
            .iter_mut()
            .map(|parked| parked.unfinished().is_some())
            .collect();
        for i in parked_to_drop(&unfinished, KEEP_IDLE).into_iter().rev() {
            self.parked.remove(i);
        }
    }

    /// Takes over the parked worktrees of `old`, the view this one follows, and parks `old`.
    pub(crate) fn adopt_parked(&mut self, mut old: Box<RepoTab>) {
        self.parked = std::mem::take(&mut old.parked);
        self.park(old);
    }

    /// The worktree that closing this one goes back to.
    pub(crate) fn main_worktree(&self) -> Option<&Path> {
        main_to_return_to(&self.worktrees)
    }

    /// Takes over the parked worktrees of `from`, the view this one replaces for good.
    pub(crate) fn inherit_parked(&mut self, from: &mut RepoTab) {
        self.parked = std::mem::take(&mut from.parked);
    }

    /// Takes the parked view of the folder `path`, if this tab has one.
    pub(crate) fn take_parked(&mut self, path: &Path) -> Option<Box<RepoTab>> {
        let i = self.parked.iter().position(|parked| parked.path == path)?;
        Some(self.parked.remove(i))
    }

    /// For closing this view: the latest parked worktree with unfinished work, with the other
    /// parked ones handed over to it.
    pub(crate) fn successor_with_work(&mut self) -> Option<Box<RepoTab>> {
        let i = self
            .parked
            .iter_mut()
            .rposition(|parked| parked.unfinished().is_some())?;
        let mut next = self.parked.remove(i);
        next.parked = std::mem::take(&mut self.parked);
        Some(next)
    }
}
