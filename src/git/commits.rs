//! Commits: the graph's log, one commit's details, searches, the commits a rebase rewrites, and
//! how revisions relate.

use std::collections::HashSet;
use std::path::Path;

use super::ChangedFile;
use super::diff::commit_files;
use super::{END_OPTS, FS, NO_SIGNATURE, RS, not_an_option, run};

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

/// The commits of every branch (stashes aside), or of the revisions `from` when given, newest
/// first, in an order fit for drawing the graph: `limit` of them after the first `skip`.
pub fn log(
    repo: &Path,
    from: Option<&[String]>,
    skip: usize,
    limit: usize,
) -> Result<Vec<Commit>, String> {
    // No revisions would make git log HEAD.
    if from.is_some_and(|revs| revs.is_empty()) {
        return Ok(Vec::new());
    }
    let limit = format!("--max-count={limit}");
    let out = match run(Some(repo), &log_args(from, skip, Some(&limit)).as_args()) {
        Ok(out) => out,
        // A freshly initialised repository has no commits yet.
        Err(e) if e.contains("does not have any commits") => String::new(),
        Err(e) => return Err(e),
    };
    Ok(out.split(RS).filter_map(parse_record).collect())
}

/// The arguments of `log` / [`LogStream`]: the revisions `from` (else every branch), after the
/// first `skip`, at most `limit` (`--max-count=n`) when given.
fn log_args<'a>(from: Option<&'a [String]>, skip: usize, limit: Option<&'a str>) -> LogArgs<'a> {
    LogArgs {
        format: format!("--format=%H{FS}%P{FS}%an{FS}%ae{FS}%at{FS}%ad{FS}%s{FS}%D{FS}%b{RS}"),
        skip: format!("--skip={skip}"),
        limit,
        from,
    }
}

struct LogArgs<'a> {
    format: String,
    skip: String,
    limit: Option<&'a str>,
    from: Option<&'a [String]>,
}

impl LogArgs<'_> {
    fn as_args(&self) -> Vec<&str> {
        let mut args = vec![
            "log",
            NO_SIGNATURE,
            "--date-order",
            "--date=format-local:%Y-%m-%d %H:%M",
            &self.skip,
        ];
        args.extend(self.limit);
        args.push(&self.format);
        push_revisions(&mut args, self.from);
        args
    }
}

/// One commit of `log`'s output, between record separators.
fn parse_record(record: &str) -> Option<Commit> {
    let record = record.trim_start_matches('\n');
    if record.is_empty() {
        return None;
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
        return None;
    };
    Some(Commit {
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
    })
}

/// Older history page after page from one `git log` left running, as [`log`] gives it. A
/// fresh `log --skip=n` would walk the n commits skipped (with `--date-order`, all history)
/// again for every page.
pub struct LogStream {
    /// `None` for no revisions at all, which have no history (as with [`log`]).
    git: Option<RunningLog>,
}

struct RunningLog {
    child: std::process::Child,
    out: std::io::BufReader<std::process::ChildStdout>,
    /// What git says on its error output, read as it comes (unread, a full pipe would stop
    /// git, and with it the output read here).
    err: Option<std::thread::JoinHandle<String>>,
}

impl LogStream {
    /// The commits of `from` (else every branch) after the first `skip`.
    pub fn start(repo: &Path, from: Option<&[String]>, skip: usize) -> Result<Self, String> {
        // No revisions would make git log HEAD.
        if from.is_some_and(|revs| revs.is_empty()) {
            return Ok(Self { git: None });
        }
        let mut child = super::command::spawn_piped(repo, &log_args(from, skip, None).as_args())?;
        let out = child
            .stdout
            .take()
            .map(std::io::BufReader::new)
            .ok_or("could not read git log")?;
        let err = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            })
        });
        Ok(Self {
            git: Some(RunningLog { child, out, err }),
        })
    }

    /// The next `n` commits; fewer at the end of history.
    pub fn next_page(&mut self, n: usize) -> Result<Vec<Commit>, String> {
        use std::io::BufRead;
        let Some(git) = &mut self.git else {
            return Ok(Vec::new());
        };
        let mut page = Vec::with_capacity(n);
        let mut record = Vec::new();
        while page.len() < n {
            record.clear();
            let read = git
                .out
                .read_until(RS as u8, &mut record)
                .map_err(|e| format!("could not read git log: {e}"))?;
            if read == 0 {
                git.check_exit()?;
                break;
            }
            let text = String::from_utf8_lossy(&record);
            page.extend(parse_record(text.trim_end_matches(RS)));
        }
        Ok(page)
    }
}

impl RunningLog {
    /// At the end of the output: whether git ended well, and what it said when it didn't.
    fn check_exit(&mut self) -> Result<(), String> {
        let status = self
            .child
            .wait()
            .map_err(|e| format!("could not run git: {e}"))?;
        let err = self
            .err
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        // A freshly initialised repository has no commits yet.
        if status.success() || err.contains("does not have any commits") {
            return Ok(());
        }
        let err = err.trim();
        Err(if err.is_empty() {
            format!("git log failed ({status})")
        } else {
            err.to_owned()
        })
    }
}

impl Drop for RunningLog {
    fn drop(&mut self) {
        // Not needed any more (history reloaded, the tab closed): don't leave it waiting. Its
        // error output then ends, and with it the thread reading that.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
    let files = commit_files(repo, hash)?;

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

/// The full message of HEAD (to amend it).
pub fn head_message(repo: &Path) -> Result<String, String> {
    run(Some(repo), &["log", NO_SIGNATURE, "-1", "--format=%B"]).map(|s| s.trim_end().to_owned())
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

/// Where a log starts, the last of its arguments: the revisions `from` when given, else every
/// branch (stashes aside).
fn push_revisions<'a>(args: &mut Vec<&'a str>, from: Option<&'a [String]>) {
    match from {
        Some(revs) => {
            args.push(END_OPTS);
            args.extend(revs.iter().map(String::as_str));
            args.push("--");
        }
        None => args.extend(["--exclude=refs/stash", "--all"]),
    }
}

/// Commits whose message or author matches `query` (case-insensitive), or whose hash starts
/// with it, newest first; among those of the revisions `from` when given, as `log`.
pub fn search(repo: &Path, from: Option<&[String]>, query: &str) -> Result<Vec<String>, String> {
    if from.is_some_and(|revs| revs.is_empty()) {
        return Ok(Vec::new());
    }
    let mut hashes: Vec<String> = Vec::new();
    // A short query matches most of history: a linear check per hash would be quadratic.
    let mut seen: HashSet<String> = HashSet::new();
    let base = ["log", NO_SIGNATURE, "--date-order", "-i", "--format=%H"];
    for filter in [format!("--grep={query}"), format!("--author={query}")] {
        let mut args = base.to_vec();
        args.push("--fixed-strings");
        args.push(&filter);
        push_revisions(&mut args, from);
        if let Ok(out) = run(Some(repo), &args) {
            for h in out.lines() {
                if seen.insert(h.to_owned()) {
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
        // First, and once: a tag `2024` may be on a commit whose message has `2024` in it.
        let h = h.trim();
        hashes.retain(|x| x != h);
        hashes.insert(0, h.to_owned());
    }
    Ok(hashes)
}

/// Commits that added or removed `query` in a file's contents (`git log -S`), newest first;
/// among those of the revisions `from` when given, as `log`.
pub fn search_code(
    repo: &Path,
    from: Option<&[String]>,
    query: &str,
) -> Result<Vec<String>, String> {
    if from.is_some_and(|revs| revs.is_empty()) {
        return Ok(Vec::new());
    }
    let pickaxe = format!("-S{query}");
    let mut args = vec!["log", NO_SIGNATURE, "--date-order", "--format=%H", &pickaxe];
    push_revisions(&mut args, from);
    let out = run(Some(repo), &args)?;
    Ok(out.lines().map(str::to_owned).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
