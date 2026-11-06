//! Opening, closing, moving and reopening tabs. The folders of the last repository tabs closed
//! are kept for Ctrl+Shift+T.

use std::path::{Path, PathBuf};

use super::panels::Toast;
use super::{App, Tab};

/// How many closed tabs can be reopened.
const MAX_CLOSED: usize = 10;

impl App {
    pub(super) fn close_tab(&mut self, i: usize, ctx: &egui::Context) {
        if i >= self.tabs.len() {
            return;
        }
        // Work still going on in a parked worktree isn't closed with a click on what is showing.
        if let Some(Tab::Repo(r)) = self.tabs.get_mut(i)
            && let Some(next) = r.successor_with_work()
        {
            *r = next;
            r.resume(ctx);
            let name = r.label(true);
            let name = name.branch.unwrap_or(name.repo);
            self.active = i;
            self.toasts.push(Toast::new(
                format!("{name} still has work going on, so the tab now shows it"),
                false,
            ));
            return;
        }
        if let Tab::Repo(r) = self.tabs.remove(i) {
            remember_closed(&mut self.closed, &r.path);
        }
        // A drag going on holds a tab by its index, which now names another one.
        self.tab_drag = None;
        if self.tabs.is_empty() {
            self.tabs.push(Tab::Home);
        }
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    /// Opens the repository of the tab closed last whose folder is still there.
    pub(super) fn reopen_closed_tab(&mut self, ctx: &egui::Context) {
        while let Some(path) = self.closed.pop() {
            if path.is_dir() {
                self.open_repo(&path, ctx);
                return;
            }
        }
    }

    /// Moves tab `from` to index `to`; the same tab stays active.
    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = if self.active == from {
            to
        } else if from < self.active && to >= self.active {
            self.active - 1
        } else if from > self.active && to <= self.active {
            self.active + 1
        } else {
            self.active
        };
    }

    pub(super) fn new_tab(&mut self) {
        self.tabs.push(Tab::Home);
        self.active = self.tabs.len() - 1;
    }
}

/// Puts `path` on top of the closed tabs, once, dropping the oldest past `MAX_CLOSED`.
fn remember_closed(closed: &mut Vec<PathBuf>, path: &Path) {
    closed.retain(|p| p != path);
    closed.push(path.to_owned());
    if closed.len() > MAX_CLOSED {
        closed.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_closed_is_on_top_once() {
        let mut closed = Vec::new();
        for p in ["a", "b", "a"] {
            remember_closed(&mut closed, Path::new(p));
        }
        assert_eq!(closed, [PathBuf::from("b"), PathBuf::from("a")]);
    }

    #[test]
    fn only_the_last_few_are_kept() {
        let mut closed = Vec::new();
        for i in 0..MAX_CLOSED + 3 {
            remember_closed(&mut closed, Path::new(&i.to_string()));
        }
        assert_eq!(closed.len(), MAX_CLOSED);
        assert_eq!(closed[0], PathBuf::from("3"));
    }
}
