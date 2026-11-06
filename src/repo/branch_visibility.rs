//! Branches hidden from the graph, or soloed: the graph then has only the commits of the
//! branches it still shows. Kept per repository with the app.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::RepoTab;
use crate::git::Refs;

/// Which branches the graph shows, by their full ref names (`refs/heads/main`,
/// `refs/remotes/origin/main`). The checked-out branch is always shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Visibility {
    /// Left out of the graph, while nothing is soloed.
    pub hidden: BTreeSet<String>,
    /// When any are: the only branches the graph shows.
    pub solo: BTreeSet<String>,
}

/// The full ref name of the local branch `name`.
pub(crate) fn local_ref(name: &str) -> String {
    format!("refs/heads/{name}")
}

/// The full ref name of the remote-tracking branch `name` (`origin/main`).
pub(crate) fn remote_ref(name: &str) -> String {
    format!("refs/remotes/{name}")
}

fn id(repo: &Path) -> egui::Id {
    egui::Id::new(("branch-visibility", repo))
}

impl Visibility {
    /// What is kept for the repository `repo`.
    pub fn load(ctx: &egui::Context, repo: &Path) -> Self {
        ctx.data_mut(|d| d.get_persisted::<Visibility>(id(repo)))
            .unwrap_or_default()
    }

    pub fn store(&self, ctx: &egui::Context, repo: &Path) {
        ctx.data_mut(|d| d.insert_persisted(id(repo), self.clone()));
    }

    /// Whether some branches are left out.
    pub fn is_filtering(&self) -> bool {
        !self.hidden.is_empty() || !self.solo.is_empty()
    }

    pub fn is_soloing(&self) -> bool {
        !self.solo.is_empty()
    }

    /// Whether the branch `full` is in the graph, unless it is the checked-out one: that one
    /// always is.
    pub fn shows(&self, full: &str) -> bool {
        if self.is_soloing() {
            self.solo.contains(full)
        } else {
            !self.hidden.contains(full)
        }
    }

    /// Shows `full` if it isn't shown, else leaves it out: soloed along with the others while
    /// some are soloed, hidden otherwise.
    pub fn toggle(&mut self, full: &str) {
        let set = if self.is_soloing() {
            &mut self.solo
        } else {
            &mut self.hidden
        };
        if !set.remove(full) {
            set.insert(full.to_owned());
        }
    }

    /// Solos `full` along with the branches already soloed, or unsolos it.
    pub fn toggle_solo(&mut self, full: &str) {
        if !self.solo.remove(full) {
            self.solo.insert(full.to_owned());
        }
    }

    /// Forgets the branches that no longer exist; whether any were.
    pub fn prune(&mut self, refs: &Refs) -> bool {
        let all: BTreeSet<String> = branch_refs(refs).map(|(full, _)| full).collect();
        let before = (self.hidden.len(), self.solo.len());
        self.hidden.retain(|r| all.contains(r));
        self.solo.retain(|r| all.contains(r));
        before != (self.hidden.len(), self.solo.len())
    }

    /// What the graph's `git log` starts from: `None` for every branch, else HEAD and the
    /// branches shown.
    pub fn revisions(&self, refs: &Refs) -> Option<Vec<String>> {
        if !self.is_filtering() {
            return None;
        }
        let head = (!refs.head.is_empty()).then(|| "HEAD".to_owned());
        Some(
            head.into_iter()
                .chain(
                    branch_refs(refs)
                        .filter(|(full, is_head)| !is_head && self.shows(full))
                        .map(|(full, _)| full),
                )
                .collect(),
        )
    }

    /// How many branches the graph shows, out of how many there are.
    pub fn counts(&self, refs: &Refs) -> (usize, usize) {
        branch_refs(refs).fold((0, 0), |(shown, all), (full, is_head)| {
            (shown + usize::from(is_head || self.shows(&full)), all + 1)
        })
    }
}

/// A change to which branches the graph shows.
pub(crate) enum Change {
    /// Hide the branch, or show it again.
    Toggle(String),
    /// Solo the branch along with those already soloed, or unsolo it.
    Solo(String),
    /// Show every branch.
    ShowAll,
}

impl RepoTab {
    /// Changes which branches the graph shows, keeps that, and reads the graph again.
    pub(super) fn change_visibility(&mut self, change: Change, ctx: &egui::Context) {
        match change {
            Change::Toggle(full) => self.visibility.toggle(&full),
            Change::Solo(full) => self.visibility.toggle_solo(&full),
            Change::ShowAll => self.visibility = Visibility::default(),
        }
        self.visibility.store(ctx, self.repo_key());
        self.reload(ctx);
    }

    /// Takes what is kept for the repository, now that the worktrees read tell which one it is,
    /// with `log_from` what the history just read was logged from.
    pub(super) fn take_visibility(&mut self, log_from: Option<Vec<String>>, ctx: &egui::Context) {
        let mut visibility = Visibility::load(ctx, self.repo_key());
        if visibility.prune(&self.refs) {
            visibility.store(ctx, self.repo_key());
        }
        // Changed while it was read, or read before the repository was known.
        if visibility.revisions(&self.refs) != log_from {
            self.reload_again = true;
        }
        self.visibility = visibility;
        self.log_from = log_from;
    }
}

/// Every branch, local and remote, by its full ref name, and whether it is checked out.
fn branch_refs(refs: &Refs) -> impl Iterator<Item = (String, bool)> + '_ {
    let local = refs.local.iter().map(|b| (local_ref(&b.name), b.is_head));
    let remote = refs
        .remotes
        .iter()
        .flat_map(|(_, list)| list)
        .map(|b| (remote_ref(&b.name), false));
    local.chain(remote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Branch;

    fn branch(name: &str, is_head: bool) -> Branch {
        Branch {
            name: name.to_owned(),
            target: String::new(),
            is_head,
            upstream: String::new(),
            track: String::new(),
        }
    }

    fn refs() -> Refs {
        Refs {
            local: vec![branch("main", true), branch("feat/a", false)],
            remotes: vec![(
                "origin".to_owned(),
                vec![branch("origin/main", false), branch("origin/feat/b", false)],
            )],
            head: "abc".to_owned(),
            current: Some("main".to_owned()),
            ..Refs::default()
        }
    }

    #[test]
    fn nothing_hidden_logs_every_branch() {
        let v = Visibility::default();
        assert_eq!(v.revisions(&refs()), None);
        assert_eq!(v.counts(&refs()), (4, 4));
    }

    #[test]
    fn hidden_branches_are_left_out_of_the_log() {
        let mut v = Visibility::default();
        v.toggle("refs/remotes/origin/feat/b");
        assert_eq!(
            v.revisions(&refs()),
            Some(vec![
                "HEAD".to_owned(),
                "refs/heads/feat/a".to_owned(),
                "refs/remotes/origin/main".to_owned(),
            ])
        );
        assert_eq!(v.counts(&refs()), (3, 4));
        v.toggle("refs/remotes/origin/feat/b");
        assert_eq!(v.revisions(&refs()), None);
    }

    #[test]
    fn soloed_branches_are_the_only_ones_logged_besides_head() {
        let mut v = Visibility::default();
        v.toggle("refs/heads/feat/a");
        v.toggle_solo("refs/remotes/origin/main");
        assert_eq!(
            v.revisions(&refs()),
            Some(vec![
                "HEAD".to_owned(),
                "refs/remotes/origin/main".to_owned()
            ])
        );
        assert_eq!(v.counts(&refs()), (2, 4));
        // While soloing, the eye solos another branch too.
        v.toggle("refs/heads/feat/a");
        assert!(v.shows("refs/heads/feat/a"));
        assert_eq!(v.counts(&refs()), (3, 4));
        // Unsoloing all of them brings back the hidden ones' state.
        v.toggle_solo("refs/heads/feat/a");
        v.toggle_solo("refs/remotes/origin/main");
        assert!(!v.shows("refs/heads/feat/a"));
        assert!(v.shows("refs/remotes/origin/main"));
    }

    #[test]
    fn the_checked_out_branch_is_logged_through_head() {
        let mut v = Visibility::default();
        v.toggle("refs/heads/main");
        assert_eq!(v.counts(&refs()), (4, 4));
        let revs = v.revisions(&refs()).unwrap_or_default();
        assert!(!revs.contains(&"refs/heads/main".to_owned()));
        assert_eq!(revs[0], "HEAD");
    }

    #[test]
    fn hiding_everything_on_an_unborn_branch_logs_nothing() {
        let mut v = Visibility::default();
        v.toggle_solo("refs/heads/gone");
        let unborn = Refs::default();
        assert_eq!(v.revisions(&unborn), Some(Vec::new()));
    }

    #[test]
    fn deleted_branches_are_forgotten() {
        let mut v = Visibility::default();
        v.toggle("refs/heads/feat/a");
        v.toggle("refs/heads/gone");
        v.toggle_solo("refs/remotes/origin/gone");
        assert!(v.prune(&refs()));
        assert_eq!(v.hidden.len(), 1);
        assert!(!v.is_soloing());
        assert!(!v.prune(&refs()));
    }
}
