//! One file: its contents at a revision, its history and blame, and ignoring it.

use std::path::Path;

use super::{END_OPTS, FS, LITERAL_PATHS, NO_SIGNATURE, RS, UTF8, not_an_option, run, run_bytes};

/// The contents of `path` at `rev` (`HEAD`, a hash, or `""` for the index), or from disk when
/// `rev` is `None`.
pub fn file_at(repo: &Path, rev: Option<&str>, path: &str) -> Result<Vec<u8>, String> {
    match rev {
        None => std::fs::read(repo.join(path)).map_err(|e| format!("{path}: {e}")),
        // The index as `:0:path`: `:path` of a file named `1:x` would be stage 1 of `x`.
        Some("") => run_bytes(Some(repo), &["show", END_OPTS, &format!(":0:{path}")]),
        Some(rev) => run_bytes(Some(repo), &["show", END_OPTS, &format!("{rev}:{path}")]),
    }
}

/// Every file in the tree of commit `rev`, sorted by path.
pub fn tree_files(repo: &Path, rev: &str) -> Result<Vec<String>, String> {
    let out = run(
        Some(repo),
        &[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            "--full-tree",
            END_OPTS,
            rev,
        ],
    )?;
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Adds `pattern` as a line of the repository's top `.gitignore` (created if missing). `false`
/// when it was already there.
pub fn add_to_gitignore(repo: &Path, pattern: &str) -> Result<bool, String> {
    let file = repo.join(".gitignore");
    let existing = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("could not read .gitignore: {e}")),
    };
    match gitignore_with(&existing, pattern) {
        Some(updated) => std::fs::write(&file, updated)
            .map(|_| true)
            .map_err(|e| format!("could not write .gitignore: {e}")),
        None => Ok(false),
    }
}

/// `existing` with `pattern` appended on a line of its own (keeping its line endings), or `None`
/// when the line is already there.
fn gitignore_with(existing: &str, pattern: &str) -> Option<String> {
    if existing.lines().any(|l| l.trim() == pattern) {
        return None;
    }
    let nl = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = existing.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    out.push_str(pattern);
    out.push_str(nl);
    Some(out)
}

/// Whether git tracks `path` (so ignoring it also needs `git rm --cached`).
pub fn is_tracked(repo: &Path, path: &str) -> bool {
    let args = [LITERAL_PATHS, "ls-files", "--error-unmatch", "--", path];
    run(Some(repo), &args).is_ok()
}

/// One commit in a file's history, with the file's path at that commit (renames are followed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileCommit {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    pub path: String,
}

/// Every commit that changed `path` up to `rev` (HEAD when `None`), newest first, following
/// renames.
pub fn file_history(repo: &Path, rev: Option<&str>, path: &str) -> Result<Vec<FileCommit>, String> {
    let format = format!("--format={RS}%H{FS}%an{FS}%ae{FS}%at{FS}%s");
    // `-z`: names as they are, not quoted (`"a\"b.txt"`) when they have quotes or tabs in them.
    let mut args = vec![
        LITERAL_PATHS,
        "log",
        NO_SIGNATURE,
        UTF8,
        "--follow",
        "--name-only",
        "-z",
        &format,
        END_OPTS,
    ];
    if let Some(rev) = rev {
        args.push(rev);
    }
    args.extend(["--", path]);
    let out = run(Some(repo), &args)?;
    Ok(parse_file_history(&out))
}

/// `log --name-only -z`: `RS fields\0`, then `\npath\0` for the file.
fn parse_file_history(out: &str) -> Vec<FileCommit> {
    out.split(RS)
        .filter_map(|record| {
            let mut lines = record.split('\0').map(|l| l.trim_start_matches('\n'));
            let mut f = lines.next()?.split(FS);
            let mut c = FileCommit {
                hash: f.next()?.to_owned(),
                author: f.next().unwrap_or_default().to_owned(),
                email: f.next().unwrap_or_default().to_owned(),
                time: f.next().and_then(|t| t.parse().ok()).unwrap_or(0),
                summary: f.next().unwrap_or_default().to_owned(),
                path: String::new(),
            };
            c.path = lines.rfind(|l| !l.is_empty())?.to_owned();
            Some(c)
        })
        .collect()
}

/// One line of `git blame`: who last changed it, in which commit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    pub text: String,
}

impl BlameLine {
    /// Lines changed in the working tree and not committed yet.
    pub fn uncommitted(&self) -> bool {
        self.hash.bytes().all(|b| b == b'0')
    }
}

/// `git blame` of `path`, as of `rev` (the working tree when `None`).
pub fn blame(repo: &Path, rev: Option<&str>, path: &str) -> Result<Vec<BlameLine>, String> {
    // Names and summaries in UTF-8, whatever `i18n.logOutputEncoding` says (`blame` has no
    // `--encoding`).
    let mut args = vec!["-c", "i18n.logOutputEncoding=UTF-8", "blame", "--porcelain"];
    if let Some(rev) = rev {
        // `blame` takes no `--end-of-options`.
        args.push(not_an_option(rev)?);
    }
    args.extend(["--", path]);
    match run(Some(repo), &args) {
        // `blame.ignoreRevsFile` set for all repositories (`.git-blame-ignore-revs`) names a
        // file this one may not have, and git then blames nothing at all.
        Err(e) if e.contains("could not open object name list") => {
            args.insert(4, "--no-ignore-revs-file");
            Ok(parse_blame(&run(Some(repo), &args)?))
        }
        out => Ok(parse_blame(&out?)),
    }
}

fn parse_blame(out: &str) -> Vec<BlameLine> {
    use std::collections::HashMap;
    // The porcelain format describes each commit once, the first time it shows up.
    let mut known: HashMap<String, BlameLine> = HashMap::new();
    let mut lines = Vec::new();
    let mut current: Option<BlameLine> = None;
    for line in out.lines() {
        if let Some(text) = line.strip_prefix('\t') {
            if let Some(mut c) = current.take() {
                known.entry(c.hash.clone()).or_insert_with(|| c.clone());
                c.text = text.to_owned();
                lines.push(c);
            }
            continue;
        }
        let Some(c) = current.as_mut() else {
            let hash = line.split(' ').next().unwrap_or_default();
            // SHA-1, or SHA-256 in a repository made with `--object-format=sha256`.
            if matches!(hash.len(), 40 | 64) && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                current = Some(known.get(hash).cloned().unwrap_or_else(|| BlameLine {
                    hash: hash.to_owned(),
                    ..BlameLine::default()
                }));
            }
            continue;
        };
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => c.author = value.to_owned(),
            "author-mail" => c.email = value.trim_matches(['<', '>']).to_owned(),
            "author-time" => c.time = value.parse().unwrap_or(0),
            "summary" => c.summary = value.to_owned(),
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `1:x` in the index is that file, not stage 1 of `x`; `[ab].txt` isn't tracked because
    /// `a.txt` is.
    #[test]
    fn odd_names_in_the_index() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-odd-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let git = |args: &[&str]| run(Some(&dir), args);
        git(&["init", "-q"])?;
        std::fs::write(dir.join("a.txt"), "a\n").map_err(|e| e.to_string())?;
        git(&["add", "a.txt"])?;
        // No such file on disk: `:` isn't allowed in Windows file names.
        let oid = git(&["hash-object", "-w", "a.txt"])?;
        let info = format!("100644,{},1:x", oid.trim());
        git(&["update-index", "--add", "--cacheinfo", &info])?;
        let staged = file_at(&dir, Some(""), "1:x");
        let tracked = (is_tracked(&dir, "[ab].txt"), is_tracked(&dir, "a.txt"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(staged?, b"a\n");
        assert_eq!(tracked, (false, true));
        Ok(())
    }

    /// A `blame.ignoreRevsFile` this repository hasn't got doesn't keep the file from its blame.
    #[test]
    fn blame_without_the_ignore_revs_file() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-blame-revs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let git = |args: &[&str]| run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["config", "commit.gpgSign", "false"],
            &["config", "blame.ignoreRevsFile", ".git-blame-ignore-revs"],
        ] {
            git(args)?;
        }
        std::fs::write(dir.join("a.txt"), "one\n").map_err(|e| e.to_string())?;
        git(&["add", "a.txt"])?;
        git(&["commit", "-qm", "first"])?;
        let lines = blame(&dir, None, "a.txt");
        let _ = std::fs::remove_dir_all(&dir);
        let lines = lines?;
        assert_eq!(lines.len(), 1);
        assert_eq!(
            (lines[0].summary.as_str(), lines[0].text.as_str()),
            ("first", "one")
        );
        Ok(())
    }

    #[test]
    fn gitignore_lines() {
        assert_eq!(gitignore_with("", "/a.log").as_deref(), Some("/a.log\n"));
        assert_eq!(
            gitignore_with("target\n", "*.log").as_deref(),
            Some("target\n*.log\n")
        );
        // No trailing newline, CRLF kept.
        assert_eq!(
            gitignore_with("a\r\nb", "c").as_deref(),
            Some("a\r\nb\r\nc\r\n")
        );
        assert_eq!(gitignore_with("x\n*.log\n", "*.log"), None);
    }

    #[test]
    fn file_history_follows_renames() {
        let out = format!(
            "{RS}aaa{FS}Ann{FS}ann@x{FS}200{FS}Rename\0\nsrc/new \"q\".rs\0{RS}bbb{FS}Bob{FS}bob@x{FS}100{FS}Add\0\nold.rs\0"
        );
        let h = parse_file_history(&out);
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0].hash.as_str(), h[0].path.as_str()),
            ("aaa", "src/new \"q\".rs")
        );
        assert_eq!(
            (h[1].author.as_str(), h[1].time, h[1].path.as_str()),
            ("Bob", 100, "old.rs")
        );
    }

    #[test]
    fn blame_reuses_commit_headers() {
        let a = "a".repeat(40);
        let z = "0".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor Ann\nauthor-mail <ann@x>\nauthor-time 100\nsummary First\nfilename f\n\tone\n\
             {a} 2 2\n\ttwo\n\
             {z} 3 3 1\nauthor Not Committed Yet\nauthor-mail <not.committed.yet>\nauthor-time 200\nsummary Version of f from f\nfilename f\n\tthree\n"
        );
        let b = parse_blame(&out);
        assert_eq!(b.len(), 3);
        assert_eq!(
            (
                b[0].author.as_str(),
                b[0].email.as_str(),
                b[0].text.as_str()
            ),
            ("Ann", "ann@x", "one")
        );
        assert_eq!(
            (b[1].summary.as_str(), b[1].time, b[1].text.as_str()),
            ("First", 100, "two")
        );
        assert!(!b[1].uncommitted() && b[2].uncommitted());
    }

    #[test]
    fn blame_of_a_sha256_repository() {
        let a = "a".repeat(64);
        let out = format!("{a} 1 1 1\nauthor Ann\nsummary First\nfilename f\n\tone\n");
        let b = parse_blame(&out);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].hash.len(), b[0].author.as_str()), (64, "Ann"));
    }
}
