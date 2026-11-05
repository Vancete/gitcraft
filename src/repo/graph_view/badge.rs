//! The pills of the refs column: which branches and tags point at a commit.

use crate::git::Refs;
use crate::repo::sidebar;

/// One pill in the refs column: a branch (local, remote or both), a tag or a detached HEAD.
#[derive(Debug)]
pub(super) struct Badge {
    pub(super) label: String,
    pub(super) kind: BadgeKind,
    /// The branch exists locally.
    pub(super) local: bool,
    /// Remotes that have the branch at this commit.
    pub(super) remotes: Vec<String>,
    /// The checked-out branch (or a detached HEAD).
    pub(super) head: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum BadgeKind {
    Branch,
    Tag,
    DetachedHead,
}

impl Badge {
    /// The name git knows a branch pill by: the local branch, else `remote/branch`.
    pub(super) fn git_ref(&self) -> String {
        match self.remotes.first() {
            Some(remote) if !self.local => format!("{remote}/{}", self.label),
            _ => self.label.clone(),
        }
    }

    pub(super) fn describe(&self) -> String {
        match self.kind {
            BadgeKind::Tag => format!("tag {}", self.label),
            BadgeKind::DetachedHead => "HEAD (detached)".to_owned(),
            BadgeKind::Branch => {
                let mut places = Vec::new();
                if self.local {
                    places.push(if self.head {
                        "local, checked out".to_owned()
                    } else {
                        "local".to_owned()
                    });
                }
                places.extend(self.remotes.iter().cloned());
                format!("{} — {}", self.label, places.join(", "))
            }
        }
    }
}

/// Turns `git log --format=%D` decorations into pills: `x` and `origin/x` at the same commit
/// become one pill marked local and remote.
pub(super) fn badges(decorations: &[String], refs: &Refs) -> Vec<Badge> {
    // The remote whose branch list has it, else the longest remote it is under (`my/fork/main`
    // is `my/fork`'s even when there is a remote `my`), unless it is a local branch's name.
    let remote_of = |d: &str| {
        let remote = refs
            .remotes
            .iter()
            .find(|(_, list)| list.iter().any(|b| b.name == d))
            .map(|(remote, _)| remote.as_str())
            .or_else(|| {
                if refs.local.iter().any(|b| b.name == d) {
                    return None;
                }
                let names: Vec<&str> = refs.remotes.iter().map(|(r, _)| r.as_str()).collect();
                sidebar::remote_of(d, &names)
            })?;
        let branch = sidebar::remote_short(d, remote)?;
        Some((remote.to_owned(), branch.to_owned()))
    };
    fn branch(out: &mut Vec<Badge>, name: &str) -> usize {
        match out
            .iter()
            .position(|b| b.kind == BadgeKind::Branch && b.label == name)
        {
            Some(i) => i,
            None => {
                out.push(Badge {
                    label: name.to_owned(),
                    kind: BadgeKind::Branch,
                    local: false,
                    remotes: Vec::new(),
                    head: false,
                });
                out.len() - 1
            }
        }
    }
    let mut out: Vec<Badge> = Vec::new();
    for d in decorations {
        if let Some(name) = d.strip_prefix("HEAD -> ") {
            let i = branch(&mut out, name);
            if let Some(b) = out.get_mut(i) {
                b.local = true;
                b.head = true;
            }
        } else if d == "HEAD" {
            out.push(Badge {
                label: "HEAD".to_owned(),
                kind: BadgeKind::DetachedHead,
                local: false,
                remotes: Vec::new(),
                head: true,
            });
        } else if let Some(tag) = d.strip_prefix("tag: ") {
            out.push(Badge {
                label: tag.to_owned(),
                kind: BadgeKind::Tag,
                local: false,
                remotes: Vec::new(),
                head: false,
            });
        } else if let Some((remote, name)) = remote_of(d) {
            // `origin/HEAD` only says which branch the remote defaults to.
            if name == "HEAD" {
                continue;
            }
            let i = branch(&mut out, &name);
            if let Some(b) = out.get_mut(i) {
                b.remotes.push(remote);
            }
        } else {
            let i = branch(&mut out, d);
            if let Some(b) = out.get_mut(i) {
                b.local = true;
            }
        }
    }
    // Checked out first, then local branches, remote-only ones, tags.
    out.sort_by_key(|b| match b.kind {
        _ if b.head => 0,
        BadgeKind::Branch if b.local => 1,
        BadgeKind::Branch => 2,
        BadgeKind::Tag => 3,
        BadgeKind::DetachedHead => 4,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    fn refs_with_origin() -> Refs {
        Refs {
            remotes: vec![("origin".to_owned(), Vec::new())],
            ..Refs::default()
        }
    }

    #[test]
    fn remote_twin_folds_into_local_badge() {
        let decos = [
            "origin/main",
            "HEAD -> main",
            "origin/HEAD",
            "origin/dev",
            "tag: v1",
        ]
        .map(String::from);
        let b = badges(&decos, &refs_with_origin());
        let labels: Vec<&str> = b.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, vec!["main", "dev", "v1"]);
        assert!(b[0].head && b[0].local && b[0].remotes == ["origin"]);
        assert!(!b[1].local && b[1].remotes == ["origin"]);
        assert_eq!(b[2].kind, BadgeKind::Tag);
    }

    fn branch(name: &str) -> git::Branch {
        git::Branch {
            name: name.to_owned(),
            target: String::new(),
            is_head: false,
            upstream: String::new(),
            track: String::new(),
        }
    }

    #[test]
    fn a_remote_named_like_the_start_of_another_does_not_take_its_branches() {
        let refs = Refs {
            remotes: vec![
                ("my".to_owned(), vec![branch("my/dev")]),
                ("my/fork".to_owned(), vec![branch("my/fork/main")]),
            ],
            ..Refs::default()
        };
        let b = badges(&["my/fork/main".to_owned()], &refs);
        assert_eq!(b[0].label, "main");
        assert_eq!(b[0].remotes, ["my/fork"]);
        assert_eq!(b[0].git_ref(), "my/fork/main");
        // Not in the lists yet (refs not reloaded): still the longest remote.
        let b = badges(&["my/fork/topic".to_owned()], &refs);
        assert_eq!(
            (b[0].label.as_str(), b[0].remotes.as_slice()),
            ("topic", &["my/fork".to_owned()][..])
        );
    }

    #[test]
    fn a_local_branch_named_under_a_remote_is_local() {
        let refs = Refs {
            local: vec![branch("origin/wip")],
            remotes: vec![("origin".to_owned(), vec![branch("origin/main")])],
            ..Refs::default()
        };
        let b = badges(&["origin/wip".to_owned()], &refs);
        assert_eq!(b[0].label, "origin/wip");
        assert!(b[0].local && b[0].remotes.is_empty());
        assert_eq!(b[0].git_ref(), "origin/wip");
    }

    #[test]
    fn local_branch_with_slash_is_not_remote() {
        let decos = ["feature/x".to_owned()];
        let b = badges(&decos, &refs_with_origin());
        assert!(b[0].kind == BadgeKind::Branch && b[0].local && b[0].remotes.is_empty());
    }
}
