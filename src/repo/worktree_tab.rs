//! Worktrees in the tab that opened them. A tab shows one worktree and keeps the others it was
//! on parked in the background, so that coming back finds everything as it was: terminals keep
//! running, operations finish, a half-written commit message stays.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align2, Galley, Rect, Response, Sense, Ui, pos2, vec2};

use super::RepoTab;
use crate::git::Worktree;
use crate::icons;
use crate::theme::{self, Tokens};

/// Idle worktrees kept for coming back to. Those with unfinished work are always kept.
const KEEP_IDLE: usize = 3;
const MAX_BRANCH_CHARS: usize = 26;
pub(crate) const SWITCHER_W: f32 = 340.0;
const SWITCHER_ROW_H: f32 = 26.0;
const SWITCHER_LIST_H: f32 = 340.0;

/// What can be going on in a tab, which it must not lose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Unfinished {
    pub operation: bool,
    pub terminal: bool,
    pub commit_message: bool,
    pub editor: bool,
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
        } else {
            None
        }
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

/// Which parked worktrees to let go of, oldest first in `unfinished`: all but the last `keep` of
/// those with nothing unfinished.
pub(super) fn parked_to_drop(unfinished: &[bool], keep: usize) -> Vec<usize> {
    let idle: Vec<usize> = (0..unfinished.len()).filter(|&i| !unfinished[i]).collect();
    let excess = idle.len().saturating_sub(keep);
    idle[..excess].to_vec()
}

/// The worktrees the switcher lists for `query`: the current one, then those with work in the
/// background, then the rest. Folders that are gone can't be opened, so they are left out.
pub(super) fn picker_entries<'a>(
    worktrees: &'a [Worktree],
    background: &[(PathBuf, &'static str)],
    query: &str,
) -> Vec<&'a Worktree> {
    let query = query.to_lowercase();
    let mut list: Vec<&Worktree> = worktrees
        .iter()
        .filter(|w| w.prunable.is_none())
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
    fn unfinished(&mut self) -> Option<&'static str> {
        Unfinished {
            operation: self.busy_op.is_some(),
            terminal: self.terminals.iter_mut().any(|t| !t.has_exited()),
            commit_message: !self.commit_summary.trim().is_empty()
                || !self.commit_body.trim().is_empty()
                || self.reword.is_some(),
            editor: self.rebase.is_some() || self.conflict.is_some(),
        }
        .label()
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
            if let Some(done) = parked.finished.take() {
                self.finished.get_or_insert(done);
            }
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
        self.parked.retain(|parked| parked.path.is_dir());
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

/// The chip after a tab's name while it is on a linked worktree: the branch, then the button
/// that closes the worktree.
#[derive(Clone)]
pub(crate) struct Chip {
    branch: Arc<Galley>,
    pub full_name: String,
    pub width: f32,
}

/// Width of the end of the chip with the button that closes the worktree.
pub(crate) const CHIP_CLOSE_W: f32 = 20.0;

/// The chip for `label`, if it has one: the main worktree has none.
pub(crate) fn chip_for(ui: &Ui, label: &TabLabel) -> Option<Chip> {
    let name = label.branch.as_deref()?;
    let t = Tokens::get(ui.ctx());
    let branch = ui.painter().layout_no_wrap(
        shorten_middle(name, MAX_BRANCH_CHARS),
        egui::FontId::proportional(11.5),
        t.text_dim,
    );
    let width = branch.size().x + 22.0 + CHIP_CLOSE_W;
    Some(Chip {
        branch,
        full_name: name.to_owned(),
        width,
    })
}

/// Draws the chip at `rect`. Only the active tab's chip reacts: `hovered` is about that.
pub(crate) fn paint_chip(ui: &Ui, rect: Rect, chip: &Chip, active: bool, hovered: bool) {
    let t = Tokens::get(ui.ctx());
    let fill = if hovered {
        t.pressed
    } else if active {
        t.field
    } else {
        t.hover.gamma_multiply(0.5)
    };
    ui.painter().rect_filled(rect, t.radius_sm, fill);
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 12.0, y), vec2(12.0, 12.0)),
        "git-branch",
        11.0,
        t.text_faint,
    );
    ui.painter().galley(
        pos2(rect.left() + 21.0, y - chip.branch.size().y / 2.0),
        chip.branch.clone(),
        t.text_dim,
    );
}

/// The switcher's contents: a search field and the worktrees. Returns the one picked.
pub(crate) fn switcher(
    ui: &mut Ui,
    tab: &RepoTab,
    query: &mut String,
    wants_focus: &mut bool,
) -> Option<PathBuf> {
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
    let field = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(query)
                    .hint_text("Search worktrees")
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(8, 4)),
            )
        })
        .inner;
    if *wants_focus {
        field.request_focus();
        *wants_focus = false;
    }
    let background = tab.background_work();
    let entries = picker_entries(&tab.worktrees, background, query);
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        picked = entries.iter().find(|w| !w.current).map(|w| w.path.clone());
    }
    if entries.is_empty() {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("No worktree matches").color(t.text_dim));
            });
        return picked;
    }
    egui::ScrollArea::vertical()
        .id_salt("worktree-switcher")
        .max_height(SWITCHER_LIST_H)
        .auto_shrink([false, true])
        .show_rows(ui, SWITCHER_ROW_H, entries.len(), |ui, rows| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for wt in &entries[rows] {
                let work = background
                    .iter()
                    .find(|(path, _)| *path == wt.path)
                    .map(|(_, work)| *work);
                if switcher_row(ui, wt, work).clicked() && !wt.current {
                    picked = Some(wt.path.clone());
                }
            }
        });
    picked
}

fn switcher_row(ui: &mut Ui, wt: &Worktree, work: Option<&str>) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), SWITCHER_ROW_H), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    if wt.current {
        icons::paint(
            ui,
            Rect::from_center_size(pos2(rect.left() + 18.0, y), vec2(14.0, 14.0)),
            "check",
            13.0,
            t.accent,
        );
    }
    let mut right = rect.right() - 10.0;
    let tag = work.or(wt.main.then_some("main"));
    if let Some(tag) = tag {
        let color = if work.is_some() {
            t.accent
        } else {
            t.text_faint
        };
        let text = ui.painter().text(
            pos2(right, y),
            Align2::RIGHT_CENTER,
            tag,
            theme::medium(11.0),
            color,
        );
        right = text.left() - 8.0;
        if work.is_some() {
            ui.painter()
                .circle_filled(pos2(right + 2.0, y), 3.0, t.accent);
            right -= 6.0;
        }
    }
    let name = wt
        .branch
        .clone()
        .unwrap_or_else(|| format!("{} ({})", wt.name(), wt.head.get(..7).unwrap_or(&wt.head)));
    let font = if wt.current {
        theme::semibold(12.5)
    } else {
        egui::FontId::proportional(12.5)
    };
    ui.painter().with_clip_rect(rect.with_max_x(right)).text(
        pos2(rect.left() + 32.0, y),
        Align2::LEFT_CENTER,
        name,
        font,
        t.text,
    );
    resp.on_hover_text(wt.path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn main_tree(path: &str) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: Some("main".to_owned()),
            head: "aaaaaaaaaaaa".to_owned(),
            main: true,
            ..Worktree::default()
        }
    }

    fn linked(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
            head: "0123456789abcdef".to_owned(),
            current: true,
            ..Worktree::default()
        }
    }

    #[test]
    fn a_linked_worktree_shows_the_repository_and_its_branch() {
        let list = [
            main_tree("/code/acme"),
            linked("/code/wt/t3-0219", Some("feature/search")),
        ];
        assert_eq!(
            tab_label(&list, "t3-0219"),
            TabLabel {
                repo: "acme".to_owned(),
                branch: Some("feature/search".to_owned()),
            }
        );
    }

    #[test]
    fn the_main_worktree_shows_no_branch() {
        let main = Worktree {
            current: true,
            ..main_tree("/code/acme")
        };
        let other = Worktree {
            current: false,
            ..linked("/code/wt/a", Some("a"))
        };
        assert_eq!(
            tab_label(&[main, other], "acme"),
            TabLabel {
                repo: "acme".to_owned(),
                branch: None,
            }
        );
    }

    #[test]
    fn a_detached_worktree_shows_its_short_hash_as_the_branch() {
        let list = [main_tree("/code/acme"), linked("/code/wt/check", None)];
        assert_eq!(tab_label(&list, "check").branch.as_deref(), Some("0123456"));
    }

    #[test]
    fn a_bare_repository_is_named_without_its_suffix() {
        let list = [main_tree("/code/acme.git"), linked("/code/wt/a", Some("a"))];
        assert_eq!(tab_label(&list, "a").repo, "acme");
    }

    #[test]
    fn the_plain_name_stands_in_until_the_worktrees_are_known() {
        assert_eq!(tab_label(&[], "t3-0219").repo, "t3-0219");
    }

    #[test]
    fn closing_a_worktree_goes_back_to_the_main_one() {
        let list = [main_tree("/code/acme"), linked("/code/wt/a", Some("a"))];
        assert_eq!(main_to_return_to(&list), Some(Path::new("/code/acme")));
    }

    #[test]
    fn a_bare_repository_has_no_main_worktree_to_go_back_to() {
        let bare = Worktree {
            bare: true,
            ..main_tree("/code/acme.git")
        };
        let list = [bare, linked("/code/wt/a", Some("a"))];
        assert_eq!(main_to_return_to(&list), None);
    }

    #[test]
    fn a_main_worktree_whose_folder_is_gone_cannot_be_gone_back_to() {
        let gone = Worktree {
            prunable: Some("gone".to_owned()),
            ..main_tree("/code/acme")
        };
        assert_eq!(main_to_return_to(&[gone]), None);
    }

    #[test]
    fn a_short_branch_stays_as_it_is() {
        assert_eq!(shorten_middle("docs/tutorial", 20), "docs/tutorial");
    }

    #[test]
    fn a_long_branch_keeps_its_first_folder_and_its_end() {
        assert_eq!(
            shorten_middle("feature/plugins/marketplace/search-ui", 24),
            "feature/…/search-ui"
        );
    }

    #[test]
    fn a_long_name_without_folders_keeps_more_of_its_end() {
        let short = shorten_middle("worktree-agent-a5b4669dd1a48230", 20);
        assert_eq!(short.chars().count(), 20);
        assert!(short.starts_with("worktre"), "{short}");
        assert!(short.ends_with("1a48230"), "{short}");
        assert!(short.contains('…'), "{short}");
    }

    #[test]
    fn folders_that_would_not_fit_are_cut_like_any_long_name() {
        let short = shorten_middle("a-long-folder-name/another-very-long-branch-name", 16);
        assert_eq!(short.chars().count(), 16);
        assert!(short.ends_with("anch-name"), "{short}");
    }

    #[test]
    fn shortening_counts_characters_not_bytes() {
        let short = shorten_middle("äöüäöüäöüäöüäöüäöü", 10);
        assert_eq!(short.chars().count(), 10);
    }

    #[test]
    fn parked_worktrees_with_unfinished_work_are_never_let_go_of() {
        assert_eq!(parked_to_drop(&[true, true, true], 0), Vec::<usize>::new());
    }

    #[test]
    fn only_the_most_recent_idle_worktrees_are_kept() {
        // Oldest first: index 0, 1 and 3 are idle, and only the last 2 of those stay.
        assert_eq!(parked_to_drop(&[false, false, true, false], 2), vec![0]);
    }

    #[test]
    fn idle_worktrees_within_the_limit_all_stay() {
        assert_eq!(parked_to_drop(&[false, false], 3), Vec::<usize>::new());
    }

    fn entries<'a>(
        list: &'a [Worktree],
        background: &[(PathBuf, &'static str)],
        query: &str,
    ) -> Vec<&'a str> {
        picker_entries(list, background, query)
            .iter()
            .map(|w| w.branch.as_deref().unwrap_or(""))
            .collect()
    }

    fn other(path: &str, branch: &str) -> Worktree {
        Worktree {
            current: false,
            ..linked(path, Some(branch))
        }
    }

    #[test]
    fn the_switcher_lists_the_current_one_then_background_work_then_the_rest() {
        let list = [
            main_tree("/code/acme"),
            other("/wt/a", "a"),
            other("/wt/b", "b"),
            linked("/wt/c", Some("c")),
        ];
        let background = [(PathBuf::from("/wt/b"), "terminal")];
        assert_eq!(entries(&list, &background, ""), ["c", "b", "main", "a"]);
    }

    #[test]
    fn the_switcher_finds_branches_and_folders_whatever_the_case() {
        let list = [
            main_tree("/code/acme"),
            other("/wt/t3-0219", "feature/Search"),
            other("/wt/t3-0abc", "docs/intro"),
        ];
        assert_eq!(entries(&list, &[], "SEARCH"), ["feature/Search"]);
        assert_eq!(entries(&list, &[], "0abc"), ["docs/intro"]);
    }

    #[test]
    fn the_switcher_leaves_out_folders_that_are_gone() {
        let gone = Worktree {
            prunable: Some("gitdir file points to non-existent location".to_owned()),
            ..other("/wt/gone", "gone")
        };
        let list = [main_tree("/code/acme"), gone];
        assert_eq!(entries(&list, &[], ""), ["main"]);
    }

    #[test]
    fn nothing_going_on_means_the_tab_can_move() {
        assert_eq!(Unfinished::default().label(), None);
    }

    #[test]
    fn each_kind_of_unfinished_work_keeps_the_tab_as_it_is() {
        let cases = [
            Unfinished {
                operation: true,
                ..Unfinished::default()
            },
            Unfinished {
                terminal: true,
                ..Unfinished::default()
            },
            Unfinished {
                commit_message: true,
                ..Unfinished::default()
            },
            Unfinished {
                editor: true,
                ..Unfinished::default()
            },
        ];
        for unfinished in cases {
            assert!(unfinished.label().is_some(), "{unfinished:?}");
        }
    }
}
