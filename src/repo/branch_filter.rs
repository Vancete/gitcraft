//! Branches hidden from the graph, or shown alone ("solo"), as GitKraken does: in a repository
//! with hundreds of branches, the graph only draws the ones that matter. A hidden branch's own
//! commits leave the graph; commits other branches share stay. Kept per repository.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Full ref names (`refs/heads/x`, `refs/remotes/origin/x`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BranchFilter {
    pub hidden: BTreeSet<String>,
    /// When any, only these are drawn (and HEAD).
    pub solo: BTreeSet<String>,
}

/// What the user asked of a branch's visibility.
pub(crate) enum Change {
    /// The eye: hidden ↔ shown (while soloing: in the solo ↔ out of it).
    Toggle(String),
    Solo(String),
    Unsolo(String),
    /// Everything shown again.
    Clear,
}

pub fn local_ref(name: &str) -> String {
    format!("refs/heads/{name}")
}

/// `origin/main` → `refs/remotes/origin/main`.
pub fn remote_ref(name: &str) -> String {
    format!("refs/remotes/{name}")
}

impl BranchFilter {
    pub fn is_active(&self) -> bool {
        !self.hidden.is_empty() || !self.solo.is_empty()
    }

    pub fn shows(&self, full: &str) -> bool {
        if self.solo.is_empty() {
            !self.hidden.contains(full)
        } else {
            self.solo.contains(full)
        }
    }

    pub fn soloed(&self, full: &str) -> bool {
        self.solo.contains(full)
    }

    pub(crate) fn apply(&mut self, change: Change) {
        match change {
            Change::Toggle(full) if !self.solo.is_empty() => {
                if !self.solo.remove(&full) {
                    self.solo.insert(full);
                }
            }
            Change::Toggle(full) => {
                if !self.hidden.remove(&full) {
                    self.hidden.insert(full);
                }
            }
            Change::Solo(full) => {
                self.hidden.remove(&full);
                self.solo.insert(full);
            }
            Change::Unsolo(full) => {
                self.solo.remove(&full);
            }
            Change::Clear => *self = BranchFilter::default(),
        }
    }

    /// What `git log` is given to walk: every ref but the hidden ones (and the stash), or the
    /// soloed ones and HEAD. Refs that no longer exist are skipped.
    pub fn revs(&self) -> Vec<String> {
        let mut revs = vec!["--ignore-missing".to_owned()];
        if self.solo.is_empty() {
            revs.push("--exclude=refs/stash".to_owned());
            revs.extend(self.hidden.iter().map(|r| format!("--exclude={r}")));
            revs.push("--all".to_owned());
        } else {
            revs.extend(self.solo.iter().cloned());
            revs.push("HEAD".to_owned());
        }
        revs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_and_soloing() {
        let mut f = BranchFilter::default();
        let (a, b) = (local_ref("a"), remote_ref("origin/b"));
        assert!(f.shows(&a) && !f.is_active());
        f.apply(Change::Toggle(a.clone()));
        assert!(!f.shows(&a) && f.shows(&b));
        assert!(f.revs().contains(&"--exclude=refs/heads/a".to_owned()));
        // Soloing shows only the solo, whatever was hidden; the eye then adds to the solo.
        f.apply(Change::Solo(b.clone()));
        assert!(f.shows(&b) && !f.shows(&a));
        f.apply(Change::Toggle(a.clone()));
        assert!(f.shows(&a));
        assert_eq!(
            f.revs(),
            [
                "--ignore-missing",
                "refs/heads/a",
                "refs/remotes/origin/b",
                "HEAD"
            ]
        );
        f.apply(Change::Clear);
        assert!(!f.is_active());
    }
}
