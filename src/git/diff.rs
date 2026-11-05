//! Patches and the files they touch: of a commit, between two revisions, of an uncommitted file.

use std::path::{Path, PathBuf};

use super::status::{Area, StatusFile};
use super::{END_OPTS, NO_SIGNATURE, now, run, run_ok_codes};

#[derive(Clone, Debug)]
pub struct ChangedFile {
    /// `A`, `M`, `D`, `R`, `C`, `T`…
    pub status: char,
    pub path: String,
    /// Original path for renames and copies.
    pub old_path: Option<String>,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

/// `--name-status -z`: `STATUS\0path\0` or, for renames/copies, `R100\0old\0new\0`.
fn parse_name_status(out: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(status) = it.next() {
        let code = status.chars().next().unwrap_or('?');
        let Some(first) = it.next() else { break };
        let (path, old_path) = if matches!(code, 'R' | 'C') {
            let Some(second) = it.next() else { break };
            (second, Some(first))
        } else {
            (first, None)
        };
        files.push(ChangedFile {
            status: code,
            path: path.to_owned(),
            old_path: old_path.map(str::to_owned),
            added: None,
            removed: None,
        });
    }
    files
}

/// `--numstat -z`: `add\tdel\tpath\0` or, for renames, `add\tdel\t\0old\0new\0`.
fn parse_numstat(out: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut result = Vec::new();
    let mut it = out.split('\0');
    while let Some(entry) = it.next() {
        let entry = entry.trim_start_matches('\n');
        if entry.is_empty() {
            continue;
        }
        let mut parts = entry.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            // Rename: old path, then new path.
            let _old = it.next();
            it.next().unwrap_or_default().to_owned()
        } else {
            path.to_owned()
        };
        // Binary files report `-`.
        result.push((path, a.parse().ok(), d.parse().ok()));
    }
    result
}

/// Fills in the line counts of `files` from `--numstat -z` output.
fn add_line_counts(files: &mut [ChangedFile], numstat: &str) {
    for (path, added, removed) in parse_numstat(numstat) {
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            file.added = added;
            file.removed = removed;
        }
    }
}

/// `show` arguments that list the files `hash` changed (`kind` is `--name-status` or
/// `--numstat`). Merges are shown against their first parent, like most GUIs do.
fn commit_files_args<'a>(kind: &'a str, hash: &'a str) -> [&'a str; 9] {
    [
        "show",
        NO_SIGNATURE,
        "--format=",
        "--diff-merges=first-parent",
        "-M",
        kind,
        "-z",
        END_OPTS,
        hash,
    ]
}

/// The files commit `hash` changed, with their line counts.
pub(super) fn commit_files(repo: &Path, hash: &str) -> Result<Vec<ChangedFile>, String> {
    let status_out = run(Some(repo), &commit_files_args("--name-status", hash))?;
    let numstat_out = run(Some(repo), &commit_files_args("--numstat", hash))?;
    let mut files = parse_name_status(&status_out);
    add_line_counts(&mut files, &numstat_out);
    Ok(files)
}

/// What makes git print a plain patch.
const PATCH_ARGS: [&str; 4] = [
    "--no-color",
    "--no-ext-diff",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// How a patch is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// `-w`.
    pub ignore_ws: bool,
    /// The whole file as context, not just the lines around the changes.
    pub whole_file: bool,
}

impl DiffOptions {
    fn push_args(self, args: &mut Vec<&str>) {
        // A patch as gitcraft reads it and `git apply` takes it back, whatever the user's
        // config says (`color.ui=always`, `diff.external`, `diff.noprefix`).
        args.extend(PATCH_ARGS);
        if self.ignore_ws {
            args.push("-w");
        }
        if self.whole_file {
            // More context than any file has lines: git shows each file as one hunk.
            args.push("--unified=999999999");
        }
    }
}

/// The patch of one file in a commit.
pub fn file_diff(repo: &Path, hash: &str, path: &str, opts: DiffOptions) -> Result<String, String> {
    let mut args = vec![
        "show",
        NO_SIGNATURE,
        "--format=",
        "--diff-merges=first-parent",
        "-M",
    ];
    opts.push_args(&mut args);
    args.extend([END_OPTS, hash, "--", path]);
    // Limited to its new name, a file renamed in the commit looks new from top to bottom.
    let old_path = run(Some(repo), &commit_files_args("--name-status", hash))
        .ok()
        .and_then(|out| parse_name_status(&out).into_iter().find(|f| f.path == path))
        .and_then(|file| file.old_path);
    args.extend(old_path.as_deref());
    run(Some(repo), &args)
}

/// The files that differ from `from` to `to` (a commit, or the working tree when `None`), with
/// their line counts.
pub fn diff_files(repo: &Path, from: &str, to: Option<&str>) -> Result<Vec<ChangedFile>, String> {
    let args = |kind: &'static str| {
        let mut args = vec!["diff", "-M", kind, "-z", END_OPTS, from];
        args.extend(to);
        args.push("--");
        args
    };
    let mut files = parse_name_status(&run(Some(repo), &args("--name-status"))?);
    add_line_counts(&mut files, &run(Some(repo), &args("--numstat"))?);
    Ok(files)
}

/// The patch of one file from `from` to `to` (the working tree when `None`).
pub fn range_file_diff(
    repo: &Path,
    from: &str,
    to: Option<&str>,
    path: &str,
    opts: DiffOptions,
) -> Result<String, String> {
    let mut args = vec!["diff", "-M"];
    opts.push_args(&mut args);
    args.extend([END_OPTS, from]);
    args.extend(to);
    args.extend(["--", path]);
    // Limited to its new name, a renamed file looks new from top to bottom.
    let old_path = diff_files(repo, from, to)
        .ok()
        .and_then(|files| files.into_iter().find(|file| file.path == path))
        .and_then(|file| file.old_path);
    args.extend(old_path.as_deref());
    run(Some(repo), &args)
}

/// The patch of one uncommitted file.
pub fn work_diff(repo: &Path, file: &StatusFile, opts: DiffOptions) -> Result<String, String> {
    let path = file.path.as_str();
    if file.area == Area::Untracked {
        // An untracked file has no history: show it as all added.
        let mut args = vec!["diff"];
        args.extend(PATCH_ARGS);
        args.extend(["--no-index", "--", "/dev/null", path]);
        return run_ok_codes(Some(repo), &args, &[0, 1]);
    }
    let mut args: Vec<&str> = match file.area {
        Area::Staged => vec!["diff", "--cached", "-M"],
        _ => vec!["diff"],
    };
    opts.push_args(&mut args);
    args.extend(["--", path]);
    // A staged rename limited to its new name would look like a new file.
    if file.area == Area::Staged {
        args.extend(file.old_path.as_deref());
    }
    run(Some(repo), &args)
}

/// Saves `patch` to a temporary file for `git apply`, which reads it from there (so the
/// operation is a plain list of git commands, which undo can replay). Patches older than a day
/// are cleared on the way.
pub fn save_patch(patch: &str) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    // One folder per user: it is closed to everyone else below, so a shared one would leave
    // every user but its first unable to stage a hunk.
    let user: String = ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.is_empty()))
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let dir = std::env::temp_dir().join(if user.is_empty() {
        "gitcraft-patches".to_owned()
    } else {
        format!("gitcraft-patches-{user}")
    });
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // `/tmp` is everyone's: a folder someone else made there (or a link to one) could have the
    // patch read, or swapped before `git apply` reads it. Only its owner can close it to others.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let not_ours = |e: std::io::Error| format!("{} isn't gitcraft's: {e}", dir.display());
        if std::fs::symlink_metadata(&dir)
            .map_err(not_ours)?
            .file_type()
            .is_symlink()
        {
            return Err(format!("{} is a link", dir.display()));
        }
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(not_ours)?;
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 24 * 3600);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{}-{}-{n}.patch", std::process::id(), now()));
    // A new file, never one already there (or a link someone left in its place).
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .write_all(patch.as_bytes())
    };
    write().map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn a_renamed_file_shows_what_changed_rather_than_all_of_it() -> Result<(), String> {
        let text = |e: std::io::Error| e.to_string();
        let dir = std::env::temp_dir().join(format!("gitcraft-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(text)?;
        let git = |args: &[&str]| -> Result<String, String> {
            let out = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(["-c", "core.autocrlf=false"])
                .args(args)
                .current_dir(&dir)
                .output()
                .map_err(text)?;
            assert!(out.status.success(), "{args:?}");
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        git(&["init", "-q"])?;
        let lines: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.join("old.txt"), &lines).map_err(text)?;
        git(&["add", "-A"])?;
        git(&["commit", "-qm", "one"])?;
        let from = git(&["rev-parse", "HEAD"])?;
        std::fs::remove_file(dir.join("old.txt")).map_err(text)?;
        std::fs::write(dir.join("new.txt"), lines.replace("line 7\n", "seven\n")).map_err(text)?;
        git(&["add", "-A"])?;
        git(&["commit", "-qm", "two"])?;
        let to = git(&["rev-parse", "HEAD"])?;
        let diff = range_file_diff(&dir, &from, Some(&to), "new.txt", DiffOptions::default())?;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(diff.contains("rename from old.txt"), "{diff}");
        assert!(diff.contains("+seven"), "{diff}");
        assert!(!diff.contains("+line 1\n"), "{diff}");
        Ok(())
    }

    #[test]
    fn name_status_with_rename() {
        let files = parse_name_status("M\0a.txt\0R090\0old.rs\0new.rs\0D\0gone\0");
        assert_eq!(files.len(), 3);
        assert_eq!(files[1].status, 'R');
        assert_eq!(files[1].path, "new.rs");
        assert_eq!(files[1].old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn numstat_with_rename_and_binary() {
        let stats = parse_numstat("1\t2\ta.txt\0-\t-\timg.png\0\n3\t0\t\0old.rs\0new.rs\0");
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[0], ("a.txt".to_owned(), Some(1), Some(2)));
        assert_eq!(stats[1], ("img.png".to_owned(), None, None));
        assert_eq!(stats[2], ("new.rs".to_owned(), Some(3), Some(0)));
    }
}
