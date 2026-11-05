//! Thin wrapper over the `git` command line. Every call shells out to the installed git, so
//! gitcraft behaves exactly like the user's own git (config, credentials, hooks).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Field and record separators for `--format` strings (unit/record separator control chars).
const FS: char = '\x1f';
const RS: char = '\x1e';

/// How many commits the graph loads at a time; scrolling to the end loads the next page.
pub const LOG_PAGE: usize = 1500;

/// For `log` and `show`: `log.showSignature` would print each commit's signature check (or
/// "No signature") into the output that is parsed, and run gpg for every commit.
const NO_SIGNATURE: &str = "--no-show-signature";

/// Ends the options: what follows is a revision even when it starts with `-` (a tag named
/// `--output=x` fetched from a remote, a ref from a forge's answer…), not an option.
const END_OPTS: &str = "--end-of-options";

/// For commands that don't take [`END_OPTS`] (`rev-parse` before git 2.30, `blame`): a
/// revision that would be read as an option is refused.
fn not_an_option(rev: &str) -> Result<&str, String> {
    if rev.starts_with('-') {
        Err(format!("{rev} is not a revision"))
    } else {
        Ok(rev)
    }
}

fn git_cmd() -> Command {
    let mut cmd = Command::new("git");
    // Never block on an interactive prompt: credential helpers with a GUI still work.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    // `rebase --continue`, `merge --continue`… would otherwise open an editor nobody sees.
    cmd.env("GIT_EDITOR", "true");
    // Interactive rebases set `sequence.editor` themselves; an inherited one would win.
    cmd.env_remove("GIT_SEQUENCE_EDITOR");
    // Messages in English whatever the user's language: errors are told apart by their words
    // (`is_auth_error`…). `C` in LANGUAGE wins over LC_ALL / LANG for messages only, so the
    // character set (and what hooks see of it) stays the user's.
    cmd.env("LANGUAGE", "C");
    crate::shell_path::apply(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs git in `dir` and returns stdout, or the error output.
pub fn run(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    run_ok_codes(dir, args, &[0])
}

/// Runs git and returns stdout as raw bytes (file contents: images…).
pub fn run_bytes(dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = git_cmd();
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Like [`run`], but exit codes in `ok` count as success (`diff --no-index` exits 1 on changes).
pub fn run_ok_codes(dir: Option<&Path>, args: &[&str], ok: &[i32]) -> Result<String, String> {
    run_inner(dir, args, ok, None)
}

/// A username and password (or token) for an HTTPS remote, typed into gitcraft.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// A credential helper that answers git's `get` from the environment, so the secret never shows
/// up in a command line. git tries the user's own helpers first and, when the operation works,
/// asks them to store what this one gave.
const ENV_HELPER: &str = "credential.helper=!f() { test \"$1\" = get && echo \"username=$GITCRAFT_GIT_USERNAME\" && echo \"password=$GITCRAFT_GIT_PASSWORD\"; }; f";

/// [`run`], answering a credential request with `creds` when given.
pub fn run_with(
    dir: Option<&Path>,
    args: &[&str],
    creds: Option<&Credentials>,
) -> Result<String, String> {
    run_inner(dir, args, &[0], creds)
}

/// Whether `err` says git needed a username / password it couldn't get or that were refused.
pub fn is_auth_error(err: &str) -> bool {
    let e = err.to_lowercase();
    [
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "invalid username or password",
        "http basic: access denied",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
    ]
    .iter()
    .any(|s| e.contains(s))
}

fn run_inner(
    dir: Option<&Path>,
    args: &[&str],
    ok: &[i32],
    creds: Option<&Credentials>,
) -> Result<String, String> {
    let mut cmd = git_cmd();
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    if let Some(c) = creds {
        cmd.args(["-c", ENV_HELPER]);
        cmd.env("GITCRAFT_GIT_USERNAME", &c.username);
        cmd.env("GITCRAFT_GIT_PASSWORD", &c.password);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.code().is_some_and(|c| ok.contains(&c)) {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        // Merges and pulls report conflicts on stdout and the summary on stderr.
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let err = match (stderr.is_empty(), stdout.is_empty()) {
            (true, true) => format!("git {} failed ({})", args.join(" "), out.status),
            (false, true) => stderr,
            (true, false) => stdout,
            (false, false) => format!("{stderr}\n{stdout}"),
        };
        Err(err)
    }
}

/// `git clone url dest`, run from `dest`'s parent.
pub fn clone(url: &str, dest: &Path, creds: Option<&Credentials>) -> Result<(), String> {
    let dest = dest.to_string_lossy();
    run_with(None, &["clone", "--", url, &dest], creds).map(|_| ())
}

/// The folder name `git clone` would pick for `url`.
pub fn clone_dir_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or_default();
    last.strip_suffix(".git").unwrap_or(last).to_owned()
}

/// `git --version`, or an error if git isn't installed / not on PATH.
pub fn version() -> Result<String, String> {
    run(None, &["--version"]).map(|s| s.trim().to_owned())
}

/// The top-level folder of the repository containing `path`.
pub fn toplevel(path: &Path) -> Result<PathBuf, String> {
    let out = run(Some(path), &["rev-parse", "--show-toplevel"])
        .map_err(|_| format!("{} is not inside a git repository", path.display()))?;
    Ok(PathBuf::from(out.trim()))
}

pub fn init(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("could not create folder: {e}"))?;
    run(Some(path), &["init"]).map(|_| ())
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, Default)]
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
    refs.head = run(Some(repo), &["rev-parse", "--verify", "-q", "HEAD"])
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
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

#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub date: String,
    pub summary: String,
    /// The rest of the message on one line (paragraphs joined by spaces), possibly empty.
    pub body: String,
    /// Decorations (`HEAD -> main`, `origin/main`, `tag: v1`), as git prints them.
    pub refs: Vec<String>,
}

/// The commits of every branch (stashes aside), newest first, in an order fit for drawing the
/// graph: `limit` of them after the first `skip`.
pub fn log(repo: &Path, skip: usize, limit: usize) -> Result<Vec<Commit>, String> {
    let format = format!("--format=%H{FS}%P{FS}%an{FS}%ae{FS}%at{FS}%ad{FS}%s{FS}%D{FS}%b{RS}");
    let limit = format!("--max-count={limit}");
    let skip = format!("--skip={skip}");
    let out = match run(
        Some(repo),
        &[
            "log",
            NO_SIGNATURE,
            "--exclude=refs/stash",
            "--all",
            "--date-order",
            "--date=format-local:%Y-%m-%d %H:%M",
            &skip,
            &limit,
            &format,
        ],
    ) {
        Ok(out) => out,
        // A freshly initialised repository has no commits yet.
        Err(e) if e.contains("does not have any commits") => String::new(),
        Err(e) => return Err(e),
    };
    let mut commits = Vec::new();
    for record in out.split(RS) {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let f: Vec<&str> = record.split(FS).collect();
        let [
            hash,
            parents,
            author,
            email,
            time,
            date,
            summary,
            decor,
            body,
        ] = f[..]
        else {
            continue;
        };
        commits.push(Commit {
            hash: hash.to_owned(),
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            author: author.to_owned(),
            email: email.to_owned(),
            time: time.parse().unwrap_or(0),
            date: date.to_owned(),
            summary: summary.to_owned(),
            body: one_line(body),
            refs: decor
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        });
    }
    Ok(commits)
}

/// A commit body as one line: whitespace runs (line breaks included) become single spaces. Long
/// bodies are cut, the graph only shows their start.
fn one_line(body: &str) -> String {
    const MAX_CHARS: usize = 400;
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_CHARS)
        .collect()
}

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

#[derive(Clone, Debug)]
pub struct CommitDetail {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub author_date: String,
    pub committer: String,
    pub commit_date: String,
    pub message: String,
    pub files: Vec<ChangedFile>,
    /// Whether the checked-out branch (HEAD) contains it, so its message can be edited.
    pub in_head: bool,
    /// Whether HEAD has merges after it, which rewording it by a rebase would flatten.
    pub merges_after: bool,
}

pub fn commit_detail(repo: &Path, hash: &str) -> Result<CommitDetail, String> {
    let format = format!("--format=%H{FS}%P{FS}%an{FS}%ae{FS}%ad{FS}%cn{FS}%cd{FS}%B");
    let out = run(
        Some(repo),
        &[
            "show",
            NO_SIGNATURE,
            "-s",
            "--date=format-local:%Y-%m-%d %H:%M:%S",
            &format,
            END_OPTS,
            hash,
        ],
    )?;
    let f: Vec<&str> = out.splitn(8, FS).collect();
    let [h, parents, author, email, adate, committer, cdate, message] = f[..] else {
        return Err("unexpected output from git show".to_owned());
    };

    // Merges are shown against their first parent, like most GUIs do.
    let diff_args = |kind: &'static str| {
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
    };
    let status_out = run(Some(repo), &diff_args("--name-status"))?;
    let numstat_out = run(Some(repo), &diff_args("--numstat"))?;

    let mut files = parse_name_status(&status_out);
    for (path, added, removed) in parse_numstat(&numstat_out) {
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            file.added = added;
            file.removed = removed;
        }
    }

    let in_head = run(Some(repo), &["merge-base", "--is-ancestor", h, "HEAD"]).is_ok();
    let merges_after = in_head
        && run(
            Some(repo),
            &["rev-list", "--merges", "--count", &format!("{h}..HEAD")],
        )
        .is_ok_and(|n| n.trim() != "0");
    Ok(CommitDetail {
        hash: h.to_owned(),
        parents: parents.split_whitespace().map(str::to_owned).collect(),
        author: author.to_owned(),
        email: email.to_owned(),
        author_date: adate.to_owned(),
        committer: committer.to_owned(),
        commit_date: cdate.to_owned(),
        message: message.trim_end().to_owned(),
        files,
        in_head,
        merges_after,
    })
}

/// `--name-status -z`: `STATUS\0path\0` or, for renames/copies, `R100\0old\0new\0`.
fn parse_name_status(out: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(status) = it.next() {
        let code = status.chars().next().unwrap_or('?');
        let Some(first) = it.next() else { break };
        if matches!(code, 'R' | 'C') {
            let Some(second) = it.next() else { break };
            files.push(ChangedFile {
                status: code,
                path: second.to_owned(),
                old_path: Some(first.to_owned()),
                added: None,
                removed: None,
            });
        } else {
            files.push(ChangedFile {
                status: code,
                path: first.to_owned(),
                old_path: None,
                added: None,
                removed: None,
            });
        }
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
    let old_path = run(
        Some(repo),
        &[
            "show",
            NO_SIGNATURE,
            "--format=",
            "--diff-merges=first-parent",
            "-M",
            "--name-status",
            "-z",
            END_OPTS,
            hash,
        ],
    )
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
    for (path, added, removed) in parse_numstat(&run(Some(repo), &args("--numstat"))?) {
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            file.added = added;
            file.removed = removed;
        }
    }
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

/// The best common ancestor of two commits.
pub fn merge_base(repo: &Path, a: &str, b: &str) -> Result<String, String> {
    run(Some(repo), &["merge-base", END_OPTS, a, b]).map(|s| s.trim().to_owned())
}

/// The full hash of a branch, tag or abbreviated hash.
pub fn resolve(repo: &Path, rev: &str) -> Result<String, String> {
    not_an_option(rev)?;
    run(
        Some(repo),
        &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
    )
    .map(|s| s.trim().to_owned())
    .map_err(|_| format!("{rev} is not a commit"))
}

/// How many commits `a` has that `b` hasn't, and the other way round.
pub fn count_apart(repo: &Path, a: &str, b: &str) -> Result<(u32, u32), String> {
    let out = run(
        Some(repo),
        &[
            "rev-list",
            "--left-right",
            "--count",
            END_OPTS,
            &format!("{a}...{b}"),
        ],
    )?;
    let mut n = out.split_whitespace().map(|x| x.parse().unwrap_or(0));
    Ok((n.next().unwrap_or(0), n.next().unwrap_or(0)))
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

/// Which side of the index a working-tree file is listed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Staged,
    Unstaged,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusFile {
    pub path: String,
    pub old_path: Option<String>,
    /// Porcelain status letter for this side (`M`, `A`, `D`, `R`, `?`, `U`…).
    pub code: char,
    pub area: Area,
}

/// An operation git left half done (conflicts to resolve).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    CherryPick,
    Revert,
    Rebase,
}

impl InProgress {
    pub fn label(self) -> &'static str {
        match self {
            InProgress::Merge => "Merge",
            InProgress::CherryPick => "Cherry-pick",
            InProgress::Revert => "Revert",
            InProgress::Rebase => "Rebase",
        }
    }

    /// The git subcommand whose `--abort` cancels it.
    pub fn command(self) -> &'static str {
        match self {
            InProgress::Merge => "merge",
            InProgress::CherryPick => "cherry-pick",
            InProgress::Revert => "revert",
            InProgress::Rebase => "rebase",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkStatus {
    pub staged: Vec<StatusFile>,
    pub unstaged: Vec<StatusFile>,
    pub conflicted: Vec<StatusFile>,
    pub in_progress: Option<InProgress>,
    /// Message git prepared for the pending merge / cherry-pick / revert commit.
    pub prepared_message: Option<String>,
}

impl WorkStatus {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty() && self.conflicted.is_empty()
    }

    /// How many files have uncommitted changes; one both staged and changed again counts once.
    pub fn changed_files(&self) -> usize {
        let mut paths: Vec<&str> = (self.staged.iter())
            .chain(&self.unstaged)
            .chain(&self.conflicted)
            .map(|f| f.path.as_str())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    /// Short branch name, `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
    /// Lock reason (may be empty) when locked.
    pub locked: Option<String>,
    /// Why git would prune it (its folder is gone…).
    pub prunable: Option<String>,
    /// The main worktree (the first one git lists).
    pub main: bool,
    /// The worktree this tab shows.
    pub current: bool,
    /// How many files have uncommitted changes; `None` when not known (bare, missing…).
    pub changes: Option<usize>,
}

impl Worktree {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

pub fn worktrees(repo: &Path) -> Result<Vec<Worktree>, String> {
    let mut list = match run(Some(repo), &["worktree", "list", "--porcelain", "-z"]) {
        Ok(out) => parse_worktrees(&out, '\0'),
        // `-z` is git 2.36+; before, one line per field (a path with a line break in it is
        // misread there).
        Err(_) => parse_worktrees(
            &run(Some(repo), &["worktree", "list", "--porcelain"])?,
            '\n',
        ),
    };
    let here = std::fs::canonicalize(repo).ok();
    for wt in &mut list {
        wt.current = here.is_some() && std::fs::canonicalize(&wt.path).ok() == here;
    }
    // One `status` per worktree, side by side. `--no-optional-locks` so that looking doesn't take
    // the index lock from under whatever runs in that worktree.
    std::thread::scope(|scope| {
        for wt in list.iter_mut().filter(|w| !w.bare && w.prunable.is_none()) {
            scope.spawn(|| {
                let args = [
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                ];
                wt.changes = run(Some(&wt.path), &args)
                    .ok()
                    .map(|out| parse_status(&out).changed_files());
            });
        }
    });
    Ok(list)
}

/// `worktree list --porcelain -z`: `key value\0` lines, an empty line between worktrees
/// (`sep` is `\n` without `-z`).
fn parse_worktrees(out: &str, sep: char) -> Vec<Worktree> {
    let mut list: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in out.split(sep) {
        if line.is_empty() {
            list.extend(current.take());
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            list.extend(current.take());
            current = Some(Worktree {
                path: PathBuf::from(value),
                main: list.is_empty(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(wt) = current.as_mut() else { continue };
        match key {
            "HEAD" => wt.head = value.to_owned(),
            "branch" => {
                wt.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_owned(),
                )
            }
            "bare" => wt.bare = true,
            "locked" => wt.locked = Some(value.to_owned()),
            "prunable" => wt.prunable = Some(value.to_owned()),
            _ => {}
        }
    }
    list.extend(current);
    list
}

pub fn status(repo: &Path) -> Result<WorkStatus, String> {
    let out = run(
        Some(repo),
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut status = parse_status(&out);
    for dir in ["rebase-merge", "rebase-apply"] {
        if let Ok(path) = run(Some(repo), &["rev-parse", "--git-path", dir])
            && repo.join(path.trim()).is_dir()
        {
            status.in_progress = Some(InProgress::Rebase);
        }
    }
    for (state, head) in [
        (InProgress::Merge, "MERGE_HEAD"),
        (InProgress::CherryPick, "CHERRY_PICK_HEAD"),
        (InProgress::Revert, "REVERT_HEAD"),
    ] {
        if status.in_progress.is_none()
            && run(Some(repo), &["rev-parse", "-q", "--verify", head]).is_ok()
        {
            status.in_progress = Some(state);
            break;
        }
    }
    if status.in_progress.is_some()
        && let Ok(path) = run(Some(repo), &["rev-parse", "--git-path", "MERGE_MSG"])
    {
        let path = repo.join(path.trim());
        status.prepared_message = std::fs::read_to_string(path).ok().map(|m| {
            // Drop git's `#` comment lines.
            m.lines()
                .filter(|l| !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned()
        });
    }
    Ok(status)
}

/// `status --porcelain=v1 -z`: `XY path\0`, renames and copies `XY new\0old\0`.
fn parse_status(out: &str) -> WorkStatus {
    let mut status = WorkStatus::default();
    let mut it = out.split('\0');
    while let Some(entry) = it.next() {
        let mut chars = entry.chars();
        let (Some(x), Some(y), Some(' ')) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        let path = chars.as_str().to_owned();
        // A rename in the working tree (of a file added with `--intent-to-add`) is `Y`'s.
        let old_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            it.next().map(str::to_owned)
        } else {
            None
        };
        let file = |code, area| StatusFile {
            path: path.clone(),
            old_path: old_path.clone(),
            code,
            area,
        };
        let conflicted = x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D');
        if conflicted {
            status.conflicted.push(file('U', Area::Conflicted));
        } else if x == '?' {
            status.unstaged.push(file('?', Area::Untracked));
        } else if x == '!' {
            continue;
        } else {
            if x != ' ' {
                status.staged.push(file(x, Area::Staged));
            }
            if y != ' ' {
                status.unstaged.push(file(y, Area::Unstaged));
            }
        }
    }
    status
}

/// The patch of one uncommitted file.
pub fn work_diff(repo: &Path, file: &StatusFile, opts: DiffOptions) -> Result<String, String> {
    let path = file.path.as_str();
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
    match file.area {
        Area::Staged | Area::Unstaged | Area::Conflicted => run(Some(repo), &args),
        // An untracked file has no history: show it as all added.
        Area::Untracked => run_ok_codes(
            Some(repo),
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--no-index",
                "--",
                "/dev/null",
                path,
            ],
            &[0, 1],
        ),
    }
}

/// The contents of `path` at `rev` (`HEAD`, a hash, or `""` for the index), or from disk when
/// `rev` is `None`.
pub fn file_at(repo: &Path, rev: Option<&str>, path: &str) -> Result<Vec<u8>, String> {
    match rev {
        None => std::fs::read(repo.join(path)).map_err(|e| format!("{path}: {e}")),
        Some(rev) => run_bytes(Some(repo), &["show", END_OPTS, &format!("{rev}:{path}")]),
    }
}

/// HEAD's commit hash, or empty before the first commit.
pub fn head(repo: &Path) -> String {
    run(Some(repo), &["rev-parse", "--verify", "-q", "HEAD"])
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// The full message of HEAD (to amend it).
pub fn head_message(repo: &Path) -> Result<String, String> {
    run(Some(repo), &["log", NO_SIGNATURE, "-1", "--format=%B"]).map(|s| s.trim_end().to_owned())
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

/// Commits whose message or author matches `query` (case-insensitive), or whose hash starts
/// with it, newest first.
pub fn search(repo: &Path, query: &str) -> Result<Vec<String>, String> {
    let mut hashes: Vec<String> = Vec::new();
    let base = [
        "log",
        NO_SIGNATURE,
        "--exclude=refs/stash",
        "--all",
        "--date-order",
        "-i",
        "--format=%H",
    ];
    for filter in [format!("--grep={query}"), format!("--author={query}")] {
        let mut args = base.to_vec();
        args.push("--fixed-strings");
        args.push(&filter);
        if let Ok(out) = run(Some(repo), &args) {
            for h in out.lines() {
                if !hashes.iter().any(|x| x == h) {
                    hashes.push(h.to_owned());
                }
            }
        }
    }
    let q = query.trim();
    if q.len() >= 4
        && q.chars().all(|c| c.is_ascii_hexdigit())
        && let Ok(h) = run(
            Some(repo),
            &["rev-parse", "--verify", "-q", &format!("{q}^{{commit}}")],
        )
    {
        hashes.insert(0, h.trim().to_owned());
    }
    Ok(hashes)
}

/// Commits that added or removed `query` in a file's contents (`git log -S`), newest first.
pub fn search_code(repo: &Path, query: &str) -> Result<Vec<String>, String> {
    let pickaxe = format!("-S{query}");
    let out = run(
        Some(repo),
        &[
            "log",
            NO_SIGNATURE,
            "--exclude=refs/stash",
            "--all",
            "--date-order",
            "--format=%H",
            &pickaxe,
        ],
    )?;
    Ok(out.lines().map(str::to_owned).collect())
}

/// The name and email commits are made with here, and whether they're set for this repository
/// only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
    pub local: bool,
}

pub fn identity(repo: &Path) -> Identity {
    let get = |args: &[&str]| run(Some(repo), args).map(|s| s.trim().to_owned()).ok();
    Identity {
        name: get(&["config", "user.name"]).unwrap_or_default(),
        email: get(&["config", "user.email"]).unwrap_or_default(),
        local: get(&["config", "--local", "user.name"]).is_some(),
    }
}

/// A commit an interactive rebase would rewrite.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RebaseCommit {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub summary: String,
    /// The whole message (summary and body).
    pub message: String,
    pub merge: bool,
}

/// The commits `git rebase -i onto` would list (`onto..HEAD`), oldest first.
pub fn rebase_commits(repo: &Path, onto: &str) -> Result<Vec<RebaseCommit>, String> {
    let format = format!("--format={RS}%H{FS}%P{FS}%an{FS}%ae{FS}%s{FS}%B");
    let range = format!("{onto}..HEAD");
    let out = run(
        Some(repo),
        &["log", NO_SIGNATURE, "--reverse", &format, END_OPTS, &range],
    )?;
    Ok(parse_rebase_commits(&out))
}

fn parse_rebase_commits(out: &str) -> Vec<RebaseCommit> {
    out.split(RS)
        .filter_map(|record| {
            let mut f = record.splitn(6, FS);
            let hash = f.next()?.trim().to_owned();
            if hash.is_empty() {
                return None;
            }
            Some(RebaseCommit {
                hash,
                merge: f.next()?.split_whitespace().count() > 1,
                author: f.next()?.to_owned(),
                email: f.next()?.to_owned(),
                summary: f.next()?.to_owned(),
                message: f.next()?.trim_end().to_owned(),
            })
        })
        .collect()
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
    run(Some(repo), &["ls-files", "--error-unmatch", "--", path]).is_ok()
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
        "log",
        NO_SIGNATURE,
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
    let mut args = vec!["blame", "--porcelain"];
    if let Some(rev) = rev {
        // `blame` takes no `--end-of-options`.
        args.push(not_an_option(rev)?);
    }
    args.extend(["--", path]);
    Ok(parse_blame(&run(Some(repo), &args)?))
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

/// "3 minutes ago", "2 days ago"… for `time` seen from `now` (both Unix seconds).
pub fn relative_time(time: i64, now: i64) -> String {
    let secs = (now - time).max(0);
    let (n, unit) = match secs {
        0..60 => return "just now".to_owned(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        86_400..604_800 => (secs / 86_400, "day"),
        604_800..2_629_800 => (secs / 604_800, "week"),
        2_629_800..31_557_600 => (secs / 2_629_800, "month"),
        _ => (secs / 31_557_600, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
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

    /// The user's config doesn't change what gitcraft reads, and a ref that looks like an option
    /// (a tag fetched from somewhere) is read as a ref.
    #[test]
    fn hostile_config_and_option_like_refs() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-hostile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let git = |args: &[&str]| run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["config", "commit.gpgSign", "false"],
            &["config", "core.autocrlf", "false"],
            &["config", "color.ui", "always"],
            &["config", "diff.external", "false"],
            &["config", "diff.noprefix", "true"],
            &["config", "log.showSignature", "true"],
        ] {
            git(args)?;
        }
        let name = "q\"x.txt";
        std::fs::write(dir.join(name), "a\n").map_err(|e| e.to_string())?;
        git(&["add", "-A"])?;
        git(&["commit", "-qm", "one"])?;
        std::fs::write(dir.join(name), "b\n").map_err(|e| e.to_string())?;
        git(&["commit", "-qam", "two"])?;
        let tag = "--output=written";
        git(&["update-ref", &format!("refs/tags/{tag}"), "HEAD~1"])?;
        let head = head(&dir);

        let commits = log(&dir, 0, 10)?;
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, head);
        let patch = file_diff(&dir, &head, name, DiffOptions::default())?;
        assert!(patch.starts_with("diff --git \"a/q"), "{patch}");
        assert!(!patch.contains('\x1b'), "{patch}");
        let range = range_file_diff(&dir, tag, Some(&head), name, DiffOptions::default())?;
        assert!(range.contains("+b"), "{range}");
        assert_eq!(diff_files(&dir, tag, None)?.len(), 1);
        assert_eq!(count_apart(&dir, tag, "HEAD")?, (0, 1));
        assert!(merge_base(&dir, tag, "HEAD").is_ok());
        assert_eq!(rebase_commits(&dir, tag)?.len(), 1);
        assert_eq!(file_at(&dir, Some(tag), name)?, b"a\n");
        assert!(resolve(&dir, tag).is_err());
        assert!(blame(&dir, Some(tag), name).is_err());
        assert!(!dir.join("written").exists());
        std::fs::write(dir.join(name), "c\n").map_err(|e| e.to_string())?;
        let file = StatusFile {
            path: name.to_owned(),
            old_path: None,
            code: 'M',
            area: Area::Unstaged,
        };
        let work = work_diff(&dir, &file, DiffOptions::default())?;
        assert!(work.contains("+c") && !work.contains('\x1b'), "{work}");
        let history = file_history(&dir, None, name)?;
        assert_eq!(history.len(), 2);
        // A rename is shown as one, in a commit and staged.
        git(&["mv", name, "moved.txt"])?;
        let staged = StatusFile {
            path: "moved.txt".to_owned(),
            old_path: Some(name.to_owned()),
            code: 'R',
            area: Area::Staged,
        };
        let work = work_diff(&dir, &staged, DiffOptions::default())?;
        assert!(work.contains("rename from"), "{work}");
        git(&["commit", "-qm", "three"])?;
        let moved = run(Some(&dir), &["rev-parse", "HEAD"])?;
        let patch = file_diff(&dir, moved.trim(), "moved.txt", DiffOptions::default())?;
        assert!(patch.contains("rename from"), "{patch}");
        assert_eq!(head_message(&dir)?, "three");
        git(&["reset", "-q", "--hard", "HEAD~1"])?;
        assert!(history.iter().all(|c| c.path == name), "{history:?}");
        assert_eq!(head_message(&dir)?, "two");
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

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
    fn status_sides_renames_and_conflicts() {
        let s =
            parse_status("MM both.rs\0R  new.rs\0old.rs\0?? fresh.txt\0UU clash.rs\0 D gone.rs\0");
        let staged: Vec<_> = s.staged.iter().map(|f| (f.code, f.path.as_str())).collect();
        assert_eq!(staged, vec![('M', "both.rs"), ('R', "new.rs")]);
        assert_eq!(s.staged[1].old_path.as_deref(), Some("old.rs"));
        let unstaged: Vec<_> = s
            .unstaged
            .iter()
            .map(|f| (f.code, f.path.as_str()))
            .collect();
        assert_eq!(
            unstaged,
            vec![('M', "both.rs"), ('?', "fresh.txt"), ('D', "gone.rs")]
        );
        assert_eq!(s.conflicted.len(), 1);
    }

    /// A file added with `--intent-to-add` under a new name is renamed on the working tree's
    /// side: its old name follows, and isn't an entry of its own.
    #[test]
    fn status_rename_in_the_working_tree() {
        let s = parse_status(" R new.txt\0ab c\0M  other.rs\0");
        let unstaged: Vec<_> = s
            .unstaged
            .iter()
            .map(|f| (f.code, f.path.as_str(), f.old_path.as_deref()))
            .collect();
        assert_eq!(unstaged, vec![('R', "new.txt", Some("ab c"))]);
        let staged: Vec<_> = s.staged.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(staged, vec!["other.rs"]);
    }

    #[test]
    fn blame_of_a_sha256_repository() {
        let a = "a".repeat(64);
        let out = format!("{a} 1 1 1\nauthor Ann\nsummary First\nfilename f\n\tone\n");
        let b = parse_blame(&out);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].hash.len(), b[0].author.as_str()), (64, "Ann"));
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(1000, 1030), "just now");
        assert_eq!(relative_time(0, 60), "1 minute ago");
        assert_eq!(relative_time(0, 2 * 3_600), "2 hours ago");
        assert_eq!(relative_time(0, 5 * 86_400), "5 days ago");
        assert_eq!(relative_time(0, 3 * 604_800), "3 weeks ago");
        assert_eq!(relative_time(0, 400 * 86_400), "1 year ago");
        assert_eq!(relative_time(100, 50), "just now");
    }

    #[test]
    fn worktree_list() {
        let out = "worktree C:/src/app\0HEAD abc\0branch refs/heads/main\0\0\
                   worktree C:/src/app-fix\0HEAD def\0detached\0locked moving disks\0\0\
                   worktree C:/gone\0HEAD 123\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0";
        let list = parse_worktrees(out, '\0');
        assert_eq!(list.len(), 3);
        // Git before 2.36 has no `-z`: the same, a field per line.
        assert_eq!(parse_worktrees(&out.replace('\0', "\n"), '\n'), list);
        assert!(list[0].main && !list[1].main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch, None);
        assert_eq!(list[1].locked.as_deref(), Some("moving disks"));
        assert!(list[2].prunable.is_some());
        assert_eq!(list[1].name(), "app-fix");
    }

    #[test]
    fn a_file_staged_and_changed_again_counts_once() {
        let out = " M a.txt\0MM b.txt\0?? new.txt\0R  c.txt\0old-c.txt\0UU d.txt\0";
        assert_eq!(parse_status(out).changed_files(), 5);
        assert_eq!(WorkStatus::default().changed_files(), 0);
    }

    #[test]
    fn clone_folder_names() {
        assert_eq!(clone_dir_name("https://github.com/a/b.git"), "b");
        assert_eq!(clone_dir_name("git@github.com:a/b.git"), "b");
        assert_eq!(clone_dir_name("C:\\repos\\thing\\"), "thing");
        assert_eq!(clone_dir_name("git@host:proj"), "proj");
    }

    #[test]
    fn bodies_fit_on_one_line() {
        assert_eq!(
            one_line(
                "First line
second.

- a
- b
"
            ),
            "First line second. - a - b"
        );
        assert_eq!(one_line(""), "");
        assert_eq!(one_line(&"x ".repeat(500)).chars().count(), 400);
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
