//! The repository's submodules: where they are, where they come from, and how they stand.

use std::path::Path;

use super::run;

/// How a submodule's checkout compares with the commit the repository records for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmoduleState {
    #[default]
    UpToDate,
    /// Not cloned yet (`git submodule update --init` would).
    NotInitialized,
    /// Another commit is checked out than the one recorded.
    OutOfDate,
    /// It has a merge conflict in the repository.
    Conflicted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Submodule {
    /// Where it is, relative to the repository.
    pub path: String,
    /// Its URL in `.gitmodules`.
    pub url: String,
    /// The commit checked out in it, or the one recorded when it isn't cloned.
    pub hash: String,
    /// `git describe` of that commit, when git gave one.
    pub describe: Option<String>,
    pub state: SubmoduleState,
    /// It has changes of its own (edited or untracked files).
    pub modified: bool,
}

impl SubmoduleState {
    /// What the sidebar says about it; `None` when there is nothing to say.
    pub fn label(self) -> Option<&'static str> {
        match self {
            SubmoduleState::UpToDate => None,
            SubmoduleState::NotInitialized => Some("not initialised"),
            SubmoduleState::OutOfDate => Some("out of date"),
            SubmoduleState::Conflicted => Some("conflict"),
        }
    }
}

/// The submodules, each with how it stands; none when there is no `.gitmodules`.
pub fn submodules(repo: &Path) -> Result<Vec<Submodule>, String> {
    if !repo.join(".gitmodules").is_file() {
        return Ok(Vec::new());
    }
    let mut list = parse_submodule_status(&run(Some(repo), &["submodule", "status"])?);
    // A `.gitmodules` without a submodule entry fails here; the URLs are only extra.
    let urls = run(
        Some(repo),
        &[
            "config",
            "-f",
            ".gitmodules",
            "-z",
            "--get-regexp",
            r"^submodule\..*\.(path|url)$",
        ],
    )
    .map(|out| parse_gitmodules(&out))
    .unwrap_or_default();
    for sub in &mut list {
        if let Some((_, url)) = urls.iter().find(|(path, _)| *path == sub.path) {
            sub.url = url.clone();
        }
    }
    let ready: Vec<&str> = list
        .iter()
        .filter(|s| s.state != SubmoduleState::NotInitialized)
        .map(|s| s.path.as_str())
        .collect();
    if !ready.is_empty() {
        let mut args = vec![
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=none",
            "--",
        ];
        args.extend(&ready);
        if let Ok(out) = run(Some(repo), &args) {
            let modified = modified_submodules(&out);
            for sub in &mut list {
                sub.modified = modified.contains(&sub.path);
            }
        }
    }
    Ok(list)
}

/// `git submodule status`: a state mark, the commit, the path and `(describe)` when known.
pub(super) fn parse_submodule_status(out: &str) -> Vec<Submodule> {
    out.lines()
        .filter_map(|line| {
            let mut chars = line.chars();
            let state = match chars.next()? {
                '-' => SubmoduleState::NotInitialized,
                '+' => SubmoduleState::OutOfDate,
                'U' => SubmoduleState::Conflicted,
                _ => SubmoduleState::UpToDate,
            };
            let (hash, rest) = chars.as_str().split_once(' ')?;
            let (path, describe) = match rest.strip_suffix(')').and_then(|r| r.rsplit_once(" (")) {
                Some((path, describe)) => (path, Some(describe.to_owned())),
                None => (rest, None),
            };
            Some(Submodule {
                path: path.to_owned(),
                hash: hash.to_owned(),
                describe,
                state,
                ..Submodule::default()
            })
        })
        .collect()
}

/// `config -f .gitmodules -z --get-regexp`: `submodule.<name>.path` and `.url` entries, each
/// `key\nvalue\0` (names and paths can have spaces), as (path, url) per submodule.
pub(super) fn parse_gitmodules(out: &str) -> Vec<(String, String)> {
    let mut by_name: Vec<(String, String, String)> = Vec::new();
    for entry in out.split('\0') {
        let Some((key, value)) = entry.split_once('\n') else {
            continue;
        };
        let Some((name, field)) = key
            .strip_prefix("submodule.")
            .and_then(|k| k.rsplit_once('.'))
        else {
            continue;
        };
        let i = match by_name.iter().position(|(n, ..)| n == name) {
            Some(i) => i,
            None => {
                by_name.push((name.to_owned(), String::new(), String::new()));
                by_name.len() - 1
            }
        };
        match field {
            "path" => by_name[i].1 = value.to_owned(),
            "url" => by_name[i].2 = value.to_owned(),
            _ => {}
        }
    }
    by_name
        .into_iter()
        .map(|(_, path, url)| (path, url))
        .collect()
}

/// The paths of submodules with changes of their own, from `status --porcelain=v2 -z`, whose
/// third field is `S<commit><tracked><untracked>` for a submodule (`N...` otherwise).
pub(super) fn modified_submodules(out: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut entries = out.split('\0');
    while let Some(entry) = entries.next() {
        // The fields before the path: ordinary, renamed (and the old path after it), unmerged.
        let fields = match entry.chars().next() {
            Some('1') => 8,
            Some('2') => {
                entries.next();
                9
            }
            Some('u') => 10,
            _ => continue,
        };
        let parts: Vec<&str> = entry.splitn(fields + 1, ' ').collect();
        let (Some(sub), Some(path)) = (parts.get(2), parts.get(fields)) else {
            continue;
        };
        let mut marks = sub.chars();
        if marks.next() == Some('S') && marks.skip(1).any(|c| c == 'M' || c == 'U') {
            found.push((*path).to_owned());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submodule_status_lines() {
        let out = " 1111111111111111111111111111111111111111 lib (v1.0)\n\
                   -2222222222222222222222222222222222222222 vendor/not yet\n\
                   +3333333333333333333333333333333333333333 my lib (heads/main)\n\
                   U4444444444444444444444444444444444444444 torn\n";
        let list = parse_submodule_status(out);
        assert_eq!(list.len(), 4);
        assert_eq!(
            (list[0].path.as_str(), list[0].describe.as_deref()),
            ("lib", Some("v1.0"))
        );
        assert_eq!(list[0].state, SubmoduleState::UpToDate);
        assert_eq!(list[0].hash, "1".repeat(40));
        assert_eq!(
            (list[1].path.as_str(), list[1].describe.as_deref()),
            ("vendor/not yet", None)
        );
        assert_eq!(list[1].state, SubmoduleState::NotInitialized);
        assert_eq!(list[2].path, "my lib");
        assert_eq!(list[2].state, SubmoduleState::OutOfDate);
        assert_eq!(list[3].state, SubmoduleState::Conflicted);
    }

    #[test]
    fn gitmodules_paths_and_urls() {
        let out = "submodule.lib.path\nlib\0\
                   submodule.lib.url\nhttps://example.com/lib.git\0\
                   submodule.a b.c.path\nvendor/a b\0\
                   submodule.a b.c.url\n../a.git\0";
        assert_eq!(
            parse_gitmodules(out),
            vec![
                ("lib".to_owned(), "https://example.com/lib.git".to_owned()),
                ("vendor/a b".to_owned(), "../a.git".to_owned()),
            ]
        );
    }

    #[test]
    fn submodules_with_changes_of_their_own() {
        let h = "0".repeat(40);
        let out = format!(
            "1 .M S.M. 160000 160000 160000 {h} {h} edited\0\
             1 .M SC.. 160000 160000 160000 {h} {h} moved on\0\
             1 .M S..U 160000 160000 160000 {h} {h} untracked\0\
             2 R. N... 100644 100644 100644 {h} {h} R100 new name\0old name\0\
             1 .M N... 100644 100644 100644 {h} {h} file.txt\0"
        );
        assert_eq!(modified_submodules(&out), vec!["edited", "untracked"]);
    }

    /// Against real repositories: each state as `git submodule status` and `status` give it.
    #[test]
    fn submodule_states_in_a_repository() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("gitcraft-submodules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let git = |dir: &Path, args: &[&str]| {
            let mut all = vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "protocol.file.allow=always",
            ];
            all.extend(args);
            run(Some(dir), &all)
        };
        for name in ["lib", "other", "top"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            git(&dir, &["init", "-q"])?;
            std::fs::write(dir.join("a.txt"), "a\n").map_err(|e| e.to_string())?;
            git(&dir, &["add", "-A"])?;
            git(&dir, &["commit", "-qm", "one"])?;
        }
        let lib = root.join("lib");
        std::fs::write(lib.join("a.txt"), "b\n").map_err(|e| e.to_string())?;
        git(&lib, &["commit", "-qam", "two"])?;
        let top = root.join("top");
        assert!(submodules(&top)?.is_empty());
        git(&top, &["submodule", "add", "-q", "../lib", "my lib"])?;
        git(
            &top,
            &["submodule", "add", "-q", "../other", "vendor/other"],
        )?;
        git(&top, &["commit", "-qm", "subs"])?;

        let list = submodules(&top)?;
        assert_eq!(list.len(), 2, "{list:?}");
        assert_eq!(list[0].path, "my lib");
        assert_eq!(list[0].url, "../lib");
        assert!(
            list.iter()
                .all(|s| s.state == SubmoduleState::UpToDate && !s.modified)
        );

        git(&top.join("my lib"), &["checkout", "-q", "HEAD~1"])?;
        std::fs::write(top.join("vendor/other/a.txt"), "edited\n").map_err(|e| e.to_string())?;
        let list = submodules(&top)?;
        assert_eq!(list[0].state, SubmoduleState::OutOfDate);
        assert!(!list[0].modified);
        assert_eq!(list[1].state, SubmoduleState::UpToDate);
        assert!(list[1].modified);

        let clone = root.join("clone");
        git(&root, &["clone", "-q", "top", "clone"])?;
        let list = submodules(&clone)?;
        assert!(
            list.iter()
                .all(|s| s.state == SubmoduleState::NotInitialized),
            "{list:?}"
        );
        git(&clone, &["submodule", "update", "--init", "--recursive"])?;
        assert!(
            submodules(&clone)?
                .iter()
                .all(|s| s.state == SubmoduleState::UpToDate)
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
