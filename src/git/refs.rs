//! Branches, tags, remotes and stashes.

use std::path::Path;

use super::{FS, run};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Short name, e.g. `main` or `origin/main`.
    pub name: String,
    pub target: String,
    pub is_head: bool,
    pub upstream: String,
    /// e.g. `[ahead 1, behind 2]`.
    pub track: String,
}

impl Branch {
    /// Commits ahead of / behind the upstream, from `[ahead 1, behind 2]`.
    pub fn ahead_behind(&self) -> (u32, u32) {
        let inner = self.track.trim_start_matches('[').trim_end_matches(']');
        let mut result = (0, 0);
        for part in inner.split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                result.0 = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                result.1 = n.parse().unwrap_or(0);
            }
        }
        result
    }

    /// The upstream branch was deleted on the remote.
    pub fn upstream_gone(&self) -> bool {
        self.track == "[gone]"
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub local: Vec<Branch>,
    /// Remote name → branches of that remote.
    pub remotes: Vec<(String, Vec<Branch>)>,
    pub tags: Vec<Branch>,
    pub head: String,
    /// Current branch name, or `None` when HEAD is detached / unborn.
    pub current: Option<String>,
}

pub fn refs(repo: &Path) -> Result<Refs, String> {
    // Names are the full ones less `refs/heads/`… and not `%(refname:short)`, which makes a
    // branch `heads/x` when a tag is called `x` too (checking that out detaches HEAD).
    let format = format!(
        "%(refname){FS}%(objectname){FS}%(HEAD){FS}%(upstream:short){FS}%(upstream:track){FS}%(*objectname)"
    );
    let out = run(
        Some(repo),
        &[
            "for-each-ref",
            &format!("--format={format}"),
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    // Remote names can have slashes in them (`my/fork`).
    let remote_names = run(Some(repo), &["remote"]).unwrap_or_default();
    let remote_names: Vec<&str> = remote_names.lines().filter(|r| !r.is_empty()).collect();
    let mut refs = Refs::default();
    for line in out.lines() {
        let f: Vec<&str> = line.split(FS).collect();
        let [full, oid, head, upstream, track, peeled] = f[..] else {
            continue;
        };
        let short = ["refs/heads/", "refs/remotes/", "refs/tags/"]
            .iter()
            .find_map(|p| full.strip_prefix(p))
            .unwrap_or(full);
        // Annotated tags point at a tag object; the commit is the peeled id.
        let target = if peeled.is_empty() { oid } else { peeled };
        let branch = Branch {
            name: short.to_owned(),
            target: target.to_owned(),
            is_head: head == "*",
            upstream: upstream.to_owned(),
            track: track.to_owned(),
        };
        if full.starts_with("refs/heads/") {
            if branch.is_head {
                refs.current = Some(branch.name.clone());
            }
            refs.local.push(branch);
        } else if let Some(rest) = full.strip_prefix("refs/remotes/") {
            if rest.ends_with("/HEAD") {
                continue;
            }
            let remote = remote_of(rest, &remote_names).to_owned();
            match refs.remotes.iter_mut().find(|(r, _)| *r == remote) {
                Some((_, list)) => list.push(branch),
                None => refs.remotes.push((remote, vec![branch])),
            }
        } else if full.starts_with("refs/tags/") {
            refs.tags.push(branch);
        }
    }
    refs.head = head(repo);
    Ok(refs)
}

/// The remote the remote-tracking branch `rest` (`my/fork/main`) is of: the longest of
/// `remotes` it is under, else its first part (a remote since removed from the config).
fn remote_of<'a>(rest: &'a str, remotes: &[&'a str]) -> &'a str {
    remotes
        .iter()
        .copied()
        .filter(|r| {
            rest.strip_prefix(r)
                .is_some_and(|branch| branch.starts_with('/'))
        })
        .max_by_key(|r| r.len())
        .unwrap_or_else(|| rest.split('/').next().unwrap_or_default())
}

/// HEAD's commit hash, or empty before the first commit.
pub fn head(repo: &Path) -> String {
    run(Some(repo), &["rev-parse", "--verify", "-q", "HEAD"])
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    /// `stash@{0}`.
    pub name: String,
    pub hash: String,
    /// The commit the changes were stashed on (the stash commit's first parent).
    pub base: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub date: String,
}

pub fn stashes(repo: &Path) -> Result<Vec<Stash>, String> {
    let out = run(
        Some(repo),
        &[
            "stash",
            "list",
            "--date=format-local:%Y-%m-%d %H:%M",
            &format!("--format=%H{FS}%P{FS}%gs{FS}%an{FS}%ae{FS}%at{FS}%ad"),
        ],
    )?;
    Ok(out
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let f: Vec<&str> = line.split(FS).collect();
            let [hash, parents, message, author, email, time, date] = f[..] else {
                return None;
            };
            // `On main: wip` / `WIP on main: 1234 msg` → the part after the colon reads better.
            let message = message
                .split_once(": ")
                .map(|(_, m)| m)
                .unwrap_or(message)
                .to_owned();
            Some(Stash {
                // Not `%gd`: with a date format it names the stash by its date, which two
                // stashes made in the same minute share.
                name: format!("stash@{{{i}}}"),
                hash: hash.to_owned(),
                base: parents
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                message,
                author: author.to_owned(),
                email: email.to_owned(),
                time: time.parse().unwrap_or(0),
                date: date.to_owned(),
            })
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

pub fn remotes(repo: &Path) -> Result<Vec<Remote>, String> {
    let out = run(Some(repo), &["remote", "-v"])?;
    let mut list: Vec<Remote> = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !list.iter().any(|r| r.name == name) {
            list.push(Remote {
                name: name.to_owned(),
                url: url.to_owned(),
            });
        }
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_with_a_slash() {
        let remotes = ["my", "my/fork", "origin"];
        assert_eq!(remote_of("my/fork/main", &remotes), "my/fork");
        assert_eq!(remote_of("my/main", &remotes), "my");
        assert_eq!(remote_of("origin/a/b", &remotes), "origin");
        assert_eq!(remote_of("gone/x", &remotes), "gone");
    }

    /// A branch and a tag of the same name: the branch is still `x`, not `heads/x`; remote
    /// branches are grouped by their remote even when its name has a slash.
    #[test]
    fn refs_names_and_remotes() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-refs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let git = |args: &[&str]| run(Some(&dir), args);
        for args in [
            &["init", "-q", "-b", "x"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["config", "commit.gpgSign", "false"],
            &["commit", "-q", "--allow-empty", "-m", "one"],
            &["tag", "x"],
            &["remote", "add", "my/fork", "https://example.invalid/a.git"],
            &["update-ref", "refs/remotes/my/fork/main", "HEAD"],
        ] {
            git(args)?;
        }
        let r = refs(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let r = r?;
        assert_eq!(
            r.local.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["x"]
        );
        assert_eq!(r.current.as_deref(), Some("x"));
        assert_eq!(
            r.tags.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["x"]
        );
        assert_eq!(r.remotes.len(), 1);
        assert_eq!(r.remotes[0].0, "my/fork");
        assert_eq!(r.remotes[0].1[0].name, "my/fork/main");
        Ok(())
    }

    /// Two stashes made in the same minute keep names of their own, and each knows its commit.
    #[test]
    fn stashes_are_named_by_number() {
        let dir = std::env::temp_dir().join(format!("gitcraft-stashes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
        ] {
            assert!(git(args).is_ok());
        }
        let file = dir.join("f.txt");
        let _ = std::fs::write(&file, "a\n");
        assert!(git(&["add", "."]).is_ok());
        assert!(git(&["commit", "-q", "-m", "base"]).is_ok());
        let head = git(&["rev-parse", "HEAD"]).unwrap_or_default();
        for text in ["b\n", "c\n"] {
            let _ = std::fs::write(&file, text);
            assert!(git(&["stash", "push", "-q", "-m", text.trim()]).is_ok());
        }
        let list = stashes(&dir).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["stash@{0}", "stash@{1}"]);
        assert_eq!(list[0].message, "c");
        assert!(list.iter().all(|s| s.base == head.trim()));
    }
}
