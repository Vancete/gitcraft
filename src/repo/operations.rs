//! Running git commands in the background, taking in how they ended, and undo / redo.

use std::path::Path;
use std::time::{Duration, Instant};

use super::toolbar::headline;
use super::{Dialog, Msg, OpKind, PendingOp, Refresh, RepoTab, Undo, UndoEntry, ops, spawn};
use crate::git;

/// An operation that takes longer than this gets a notification when it ends.
const LONG_OP: Duration = Duration::from_secs(4);

impl RepoTab {
    /// Runs git commands one after another (stopping at the first failure), then refreshes.
    pub(super) fn run_ops(
        &mut self,
        label: &str,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        kind: OpKind,
        ctx: &egui::Context,
    ) {
        self.run_ops_ex(label, ops, refresh, kind, false, ctx);
    }

    pub(super) fn run_ops_ex(
        &mut self,
        label: &str,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        kind: OpKind,
        quiet: bool,
        ctx: &egui::Context,
    ) {
        if self.busy_op.is_some() {
            return;
        }
        self.busy_op = Some(label.to_owned());
        self.op_started = Some(Instant::now());
        self.note_checkout_move(&ops, ctx);
        if !quiet {
            self.status = None;
        }
        // A quiet one (the background fetch) never asks to sign in, and must not take the place
        // of the operation that did while its sign-in dialog is open.
        if !quiet {
            self.last_op = Some(PendingOp {
                label: label.to_owned(),
                ops: ops.clone(),
                refresh,
                kind: kind.clone(),
                quiet,
            });
        }
        // Nor does a credential helper on its behalf: Git Credential Manager would otherwise
        // open its sign-in window every few minutes while a token has expired.
        let ops = if quiet { unattended(ops) } else { ops };
        let path = self.path.clone();
        let label = label.to_owned();
        let creds = self.credentials.clone();
        spawn(&self.tx, ctx, move || {
            let (result, kind, stopped) = run_steps_counted(&path, &ops, kind, creds.as_ref());
            Msg::OpDone {
                label,
                result,
                refresh,
                kind,
                quiet,
                stopped,
            }
        });
    }

    /// Leaves out of the operation kept to run again after signing in the steps that ran
    /// before step `stopped` failed: they are done, and most would fail a second time (a tag or
    /// remote added again, a branch deleted again), so that the step that needed signing in
    /// would never run.
    pub(super) fn keep_unfinished(&mut self, stopped: usize) {
        if let Some(op) = &mut self.last_op {
            op.ops.drain(..stopped.min(op.ops.len()));
        }
    }

    /// Runs `ops` as an undoable step.
    pub(super) fn run_undoable(
        &mut self,
        label: &str,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        undo: Undo,
        ctx: &egui::Context,
    ) {
        let before = self.refs.head.clone();
        self.run_ops(label, ops, refresh, OpKind::Normal(undo, before), ctx);
    }

    /// Remembers the credentials and runs the operation that needed them again.
    pub(super) fn sign_in(&mut self, creds: git::Credentials, ctx: &egui::Context) {
        self.credentials = Some(creds);
        if let Some(busy) = &self.busy_op
            && let Some(op) = &self.last_op
        {
            // It would not start: say so, rather than drop it.
            self.status = Some((
                format!("{}: run it again once {busy} is done", op.label),
                true,
            ));
        } else if let Some(op) = self.last_op.take() {
            self.run_ops_ex(&op.label, op.ops, op.refresh, op.kind, op.quiet, ctx);
        }
    }

    pub(super) fn undo(&mut self, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        if let Some(entry) = self.undo_stack.pop() {
            let label = format!("Undo {}", entry.label);
            let steps = entry.undo.clone();
            self.run_ops(&label, steps, Refresh::All, OpKind::Undo(entry), ctx);
        }
    }

    pub(super) fn redo(&mut self, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        if let Some(entry) = self.redo_stack.pop() {
            let label = format!("Redo {}", entry.label);
            let steps = entry.redo.clone();
            self.run_ops(&label, steps, Refresh::All, OpKind::Redo(entry), ctx);
        }
    }

    /// Takes in how an operation ended: the status line, undo history, sign-in, a notification
    /// for a long one, then re-reads what it changed (and updates the submodules after a
    /// checkout, if asked to).
    pub(super) fn op_done(
        &mut self,
        label: String,
        result: Result<String, String>,
        refresh: Refresh,
        kind: OpKind,
        quiet: bool,
        ctx: &egui::Context,
    ) {
        self.busy_op = None;
        let long = self
            .op_started
            .take()
            .is_some_and(|t| t.elapsed() > LONG_OP);
        match &result {
            Ok(_) => {
                if label == "Commit" || label == "Amend" || label.starts_with("Continue ") {
                    self.commit_summary.clear();
                    self.commit_body.clear();
                    self.amend = false;
                }
                // Quiet success for staging, which happens all the time.
                if refresh == Refresh::All && !quiet {
                    self.status = Some((format!("{label}: done"), false));
                }
                if label.contains("etch") {
                    self.load_prs(ctx);
                }
                self.record_undo(&label, kind);
            }
            Err(e) if git::is_auth_error(e) && !quiet => {
                // Typed credentials that were refused are forgotten.
                let refused = self.credentials.take().is_some();
                self.status = Some((format!("{label}: sign-in needed"), true));
                let url = self
                    .default_remote()
                    .and_then(|r| self.remotes.iter().find(|x| x.name == r))
                    .map(|r| r.url.clone())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::credentials(&url, refused));
            }
            Err(e) => {
                self.focus_wip = e.contains("CONFLICT") || e.contains("could not apply");
                self.status = Some((format!("{label} failed: {e}"), true));
                // A failed undo stays undoable.
                match kind {
                    OpKind::Undo(entry) => self.undo_stack.push(entry),
                    OpKind::Redo(entry) => self.redo_stack.push(entry),
                    OpKind::Normal(..) => {}
                }
            }
        }
        if long && !quiet {
            self.finished.push(match &result {
                Ok(_) => (format!("{} · {label}: done", self.name), false),
                Err(e) => (
                    format!("{} · {label} failed: {}", self.name, headline(e)),
                    true,
                ),
            });
        }
        match refresh {
            Refresh::Status => self.reload_status(ctx),
            Refresh::All => self.reload(ctx),
        }
        self.follow_checkout_move(result.is_ok(), ctx);
    }

    fn record_undo(&mut self, label: &str, kind: OpKind) {
        match kind {
            OpKind::Normal(Undo::None, _) => {}
            OpKind::Normal(Undo::HeadMove { mode }, heads) => {
                let (before, after) = heads.split_once('\u{0}').unwrap_or(("", ""));
                if before.is_empty() || after.is_empty() || before == after {
                    return;
                }
                self.undo_stack.push(UndoEntry {
                    label: label.to_owned(),
                    undo: ops(&[&["reset", mode, before]]),
                    redo: ops(&[&["reset", mode, after]]),
                    heads: Some((before.to_owned(), after.to_owned())),
                });
                self.redo_stack.clear();
            }
            OpKind::Normal(Undo::Custom { undo, redo }, _) => {
                self.undo_stack.push(UndoEntry {
                    label: label.to_owned(),
                    undo,
                    redo,
                    heads: None,
                });
                self.redo_stack.clear();
            }
            OpKind::Undo(entry) => self.redo_stack.push(entry),
            OpKind::Redo(entry) => self.undo_stack.push(entry),
        }
        // Plenty for a session.
        if self.undo_stack.len() > 50 {
            self.undo_stack.remove(0);
        }
    }
}

/// [`run_steps_counted`] without where it stopped.
#[cfg(test)]
pub(super) fn run_steps(
    path: &Path,
    ops: &[Vec<String>],
    kind: OpKind,
    creds: Option<&git::Credentials>,
) -> (Result<String, String>, OpKind) {
    let (result, kind, _) = run_steps_counted(path, ops, kind, creds);
    (result, kind)
}

/// Runs git commands one after another, stopping at the first failure; `kind` comes back with
/// what undoing needs, as found on disk. Also says which step it stopped at (`ops.len()` when
/// all ran): the steps before it are done, and running the operation again (after signing in)
/// starts there.
pub(super) fn run_steps_counted(
    path: &Path,
    ops: &[Vec<String>],
    kind: OpKind,
    creds: Option<&git::Credentials>,
) -> (Result<String, String>, OpKind, usize) {
    // Undoing a HEAD move resets the branch checked out now: after a checkout or a commit made
    // elsewhere since, that would throw away commits that aren't the operation's.
    let expected = match &kind {
        OpKind::Undo(entry) => entry.heads.as_ref().map(|(_, after)| after),
        OpKind::Redo(entry) => entry.heads.as_ref().map(|(before, _)| before),
        OpKind::Normal(..) => None,
    };
    if let Some(expected) = expected {
        let now = git::head(path);
        if &now != expected {
            return (
                Err(format!(
                    "HEAD has moved since (it is at {}, not {}): resetting it would lose what \
                     was done after",
                    super::short(&now),
                    super::short(expected)
                )),
                kind,
                0,
            );
        }
    }
    // Where HEAD really is now, not where the last reload saw it: a commit made in the
    // terminal since must not be undone along with this.
    let head_before =
        matches!(kind, OpKind::Normal(Undo::HeadMove { .. }, _)).then(|| git::head(path));
    let pushes_stash = ops.first().is_some_and(|op| {
        let stash = stash_push_args();
        op.len() >= stash.len() && op.iter().zip(stash).all(|(a, b)| a == b)
    });
    let stash_before = pushes_stash.then(|| git::resolve(path, "refs/stash").ok());
    // Checked before anything runs, so a refused undo changes nothing.
    if let Err(e) = check_expected(path, ops) {
        return (Err(e), kind, 0);
    }
    let mut result = Ok(String::new());
    let mut stopped = ops.len();
    for (i, op) in ops.iter().enumerate() {
        if expected_ref(op).is_some() {
            continue;
        }
        let args: Vec<&str> = op.iter().map(String::as_str).collect();
        result = git::run_with(Some(path), &args, creds);
        if result.is_err() {
            stopped = i;
            break;
        }
    }
    // "No local changes to save" makes no stash: undoing would pop an older one.
    let no_stash = stash_before.is_some_and(|b| b == git::resolve(path, "refs/stash").ok());
    let kind = match kind {
        // For HEAD moves, the new HEAD is what redo goes back to.
        OpKind::Normal(Undo::HeadMove { mode }, before) if result.is_ok() => {
            let before = head_before.filter(|h| !h.is_empty()).unwrap_or(before);
            OpKind::Normal(
                Undo::HeadMove { mode },
                format!("{before}\u{0}{}", git::head(path)),
            )
        }
        OpKind::Normal(Undo::Custom { .. }, before) if no_stash => {
            OpKind::Normal(Undo::None, before)
        }
        // The way back expects the refs where this left them.
        OpKind::Normal(Undo::Custom { mut undo, redo }, before) if result.is_ok() => {
            fill_expected(path, &mut undo);
            OpKind::Normal(Undo::Custom { undo, redo }, before)
        }
        OpKind::Undo(mut entry) if result.is_ok() => {
            fill_expected(path, &mut entry.redo);
            OpKind::Undo(entry)
        }
        // Redone with nothing to stash, the undo keeps expecting the stash it popped, and so
        // refuses rather than pop an older one.
        OpKind::Redo(mut entry) if result.is_ok() && !no_stash => {
            fill_expected(path, &mut entry.undo);
            OpKind::Redo(entry)
        }
        other => other,
    };
    (result, kind, stopped)
}

/// Marks a step that runs no git (no argument can hold a NUL).
const EXPECT: &str = "\u{0}expect";

/// A step of an undo (or redo) that runs nothing, but refuses the whole of it unless the ref
/// `name` is still where the operation it undoes left it (it learns where when that runs): so
/// that undoing a stash pops that stash and not one pushed since, and undoing a pull does not
/// put back a branch that got commits since.
pub(super) fn expect_ref(name: &str) -> Vec<String> {
    vec![EXPECT.to_owned(), name.to_owned(), String::new()]
}

/// A step that runs nothing, but refuses the whole operation unless `name` is at `commit`.
pub(super) fn expect_ref_at(name: &str, commit: &str) -> Vec<String> {
    vec![EXPECT.to_owned(), name.to_owned(), commit.to_owned()]
}

/// The ref and the commit an `expect_ref` step expects.
fn expected_ref(op: &[String]) -> Option<(&str, &str)> {
    match op {
        [mark, name, value] if mark == EXPECT => Some((name, value)),
        _ => None,
    }
}

fn check_expected(path: &Path, ops: &[Vec<String>]) -> Result<(), String> {
    for (name, value) in ops.iter().filter_map(|op| expected_ref(op)) {
        let now = git::resolve(path, name).unwrap_or_default();
        if now != value {
            let what = name.strip_prefix("refs/heads/").unwrap_or(name);
            return Err(if name == "refs/stash" {
                "The newest stash is not the one this made any more".to_owned()
            } else {
                format!(
                    "{what} has moved since (it is at {}, not {}): going ahead would lose \
                     what was done after",
                    super::short(&now),
                    super::short(value)
                )
            });
        }
    }
    Ok(())
}

fn fill_expected(path: &Path, ops: &mut [Vec<String>]) {
    for op in ops {
        if let Some((name, _)) = expected_ref(op) {
            let now = git::resolve(path, name).unwrap_or_default();
            if let Some(value) = op.get_mut(2) {
                *value = now;
            }
        }
    }
}

/// How the app stashes (and how `run_ops_ex` knows an operation for one).
pub(super) fn stash_push_args() -> [&'static str; 3] {
    ["stash", "push", "--include-untracked"]
}

/// `ops` set to ask nobody for credentials: no credential helper window (Git Credential Manager
/// and git ≥ 2.46 heed `credential.interactive`), nor git's own askpass program (an empty
/// `core.askPass` also keeps it from falling back to `SSH_ASKPASS`). Helpers that answer from
/// what they stored still do, as do credentials typed into the tab.
fn unattended(ops: Vec<Vec<String>>) -> Vec<Vec<String>> {
    ops.into_iter()
        .map(|op| {
            if expected_ref(&op).is_some() {
                return op;
            }
            ["-c", "credential.interactive=false", "-c", "core.askPass="]
                .into_iter()
                .map(str::to_owned)
                .chain(op)
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::super::commands::checkout_pull_undo;
    use super::super::{OpKind, Undo, UndoEntry, ops};
    use super::{run_steps, stash_push_args};
    use crate::git;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitcraft-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
        ] {
            assert!(git::run(Some(&dir), args).is_ok());
        }
        dir
    }

    /// Whether git, asked for a credential with `op`'s settings, ran the askpass program
    /// (an `SSH_ASKPASS` like KDE's, which would open a window).
    #[cfg(unix)]
    fn askpass_asked(dir: &Path, op: &[String]) -> Result<bool, String> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("askpass.sh");
        let marker = dir.join("asked");
        let _ = std::fs::remove_file(&marker);
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\necho x\n", marker.display()),
        )
        .map_err(|e| e.to_string())?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new("git")
            .args(op)
            .env("SSH_ASKPASS", &script)
            .env_remove("GIT_ASKPASS")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"protocol=https\nhost=example.invalid\n\n");
        }
        child.wait().map_err(|e| e.to_string())?;
        Ok(marker.exists())
    }

    /// The background fetch asks nobody to sign in, every few minutes, through a window.
    #[cfg(unix)]
    #[test]
    fn a_quiet_operation_opens_no_sign_in_window() -> Result<(), String> {
        let dir = repo("unattended");
        let fill = ops(&[&["-c", "credential.helper=", "credential", "fill"]]);
        let plain = fill.first().cloned().unwrap_or_default();
        let quiet = super::unattended(fill).first().cloned().unwrap_or_default();
        let asked_plain = askpass_asked(&dir, &plain);
        let asked_quiet = askpass_asked(&dir, &quiet);
        let _ = std::fs::remove_dir_all(&dir);
        // Without it, git does ask (or the test would prove nothing).
        assert_eq!(asked_plain, Ok(true));
        assert_eq!(asked_quiet, Ok(false));
        Ok(())
    }

    #[test]
    fn a_quiet_operation_keeps_its_expected_refs() {
        let op = super::expect_ref("refs/stash");
        assert_eq!(super::unattended(vec![op.clone()]), vec![op]);
    }

    fn entry(kind: OpKind) -> Option<UndoEntry> {
        match kind {
            OpKind::Normal(Undo::Custom { undo, redo }, _) => Some(UndoEntry {
                label: "op".to_owned(),
                undo,
                redo,
                heads: None,
            }),
            _ => None,
        }
    }

    fn stash(dir: &Path, file: &str) -> Option<UndoEntry> {
        let _ = std::fs::write(dir.join(file), "x\n");
        let push = ops(&[&stash_push_args()]);
        let undo = Undo::Custom {
            undo: [super::expect_ref("refs/stash")]
                .into_iter()
                .chain(ops(&[&["stash", "pop", "--index"]]))
                .collect(),
            redo: push.clone(),
        };
        let (result, kind) = run_steps(dir, &push, OpKind::Normal(undo, String::new()), None);
        assert!(result.is_ok());
        entry(kind)
    }

    /// Undoing a stash pops that stash, not one pushed since (in the terminal).
    #[test]
    fn undoing_a_stash_refuses_once_another_was_pushed() {
        let dir = repo("stash-newer");
        let first = stash(&dir, "a.txt");
        let _ = std::fs::write(dir.join("b.txt"), "y\n");
        let other = git::run(Some(&dir), &["stash", "push", "-u"]);
        let refused = first
            .as_ref()
            .map(|e| run_steps(&dir, &e.undo, OpKind::Undo(e.clone()), None).0);
        let count = git::run(Some(&dir), &["stash", "list"]).map(|s| s.lines().count());
        // Popped by hand: now the newest is this one again, and undoing goes through.
        let popped = git::run(Some(&dir), &["stash", "pop"]);
        let _ = std::fs::remove_file(dir.join("b.txt"));
        let undone = first
            .as_ref()
            .map(|e| run_steps(&dir, &e.undo, OpKind::Undo(e.clone()), None));
        let restored = dir.join("a.txt").exists();
        // Redone, the undo expects the new stash.
        let redone = undone.and_then(|(r, kind)| match kind {
            OpKind::Undo(e) if r.is_ok() => {
                Some(run_steps(&dir, &e.redo.clone(), OpKind::Redo(e), None))
            }
            _ => None,
        });
        let again = redone.and_then(|(r, kind)| match kind {
            OpKind::Redo(e) if r.is_ok() => {
                Some(run_steps(&dir, &e.undo.clone(), OpKind::Undo(e), None).0)
            }
            _ => None,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(other.is_ok());
        assert!(matches!(refused, Some(Err(_))));
        assert_eq!(count, Ok(2));
        assert!(popped.is_ok());
        assert!(restored);
        assert!(matches!(again, Some(Ok(_))));
    }

    /// Redoing a stash with nothing left to stash: undoing it then pops no older stash.
    #[test]
    fn a_redone_stash_that_saved_nothing_is_not_popped() {
        let dir = repo("stash-redo-nothing");
        let _ = std::fs::write(dir.join("old.txt"), "o\n");
        let older = git::run(Some(&dir), &["stash", "push", "-u"]);
        let first = stash(&dir, "a.txt");
        let undone = first
            .map(|e| run_steps(&dir, &e.undo.clone(), OpKind::Undo(e), None))
            .and_then(|(r, kind)| r.ok().and(Some(kind)));
        // The changes popped back go elsewhere: redoing stashes nothing.
        let _ = std::fs::remove_file(dir.join("a.txt"));
        let redone = undone.and_then(|kind| match kind {
            OpKind::Undo(e) => Some(run_steps(&dir, &e.redo.clone(), OpKind::Redo(e), None)),
            _ => None,
        });
        let again = redone.and_then(|(r, kind)| match kind {
            OpKind::Redo(e) if r.is_ok() => {
                Some(run_steps(&dir, &e.undo.clone(), OpKind::Undo(e), None).0)
            }
            _ => None,
        });
        let count = git::run(Some(&dir), &["stash", "list"]).map(|s| s.lines().count());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(older.is_ok());
        assert!(matches!(again, Some(Err(_))));
        assert_eq!(count, Ok(1));
    }

    /// A quick edit's rebase does not start once HEAD moved past the commits it read.
    #[test]
    fn an_expected_head_that_moved_stops_the_operation() {
        let dir = repo("expect-head");
        let read = git::head(&dir);
        let made = git::run(
            Some(&dir),
            &["commit", "-q", "--allow-empty", "-m", "since"],
        );
        let since = git::head(&dir);
        let step = ops(&[&["commit", "-q", "--allow-empty", "-m", "op"]]);
        let guarded = |at: &str| {
            [super::expect_ref_at("HEAD", at)]
                .into_iter()
                .chain(step.clone())
                .collect::<Vec<_>>()
        };
        let (refused, _) = run_steps(&dir, &guarded(&read), OpKind::plain(), None);
        let still = git::head(&dir);
        let (ran, _) = run_steps(&dir, &guarded(&since), OpKind::plain(), None);
        let after = git::head(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(made.is_ok());
        assert!(refused.is_err());
        assert_eq!(still, since);
        assert!(ran.is_ok());
        assert_ne!(after, since);
    }

    /// Undoing "Checkout and pull" does not put the branch back over commits made on it since.
    #[test]
    fn undoing_checkout_and_pull_keeps_commits_made_since() {
        let dir = repo("pull-since");
        let was = git::head(&dir);
        let steps = [
            &["checkout", "-q", "-b", "side"][..],
            &["commit", "-q", "--allow-empty", "-m", "pulled"],
        ];
        for args in steps {
            assert!(git::run(Some(&dir), args).is_ok());
        }
        // As if `main` had been checked out and pulled to `pulled` from `side`.
        let pulled = git::head(&dir);
        let reset = git::run(Some(&dir), &["branch", "-f", "main", &pulled]);
        let checkout = ops(&[&["checkout", "main", "--"]]);
        let undo = checkout_pull_undo("side", "main", &was, &checkout);
        let (result, kind) = run_steps(&dir, &checkout, OpKind::Normal(undo, String::new()), None);
        let entry = entry(kind);
        // Then a commit on main.
        let more = git::run(Some(&dir), &["commit", "-q", "--allow-empty", "-m", "mine"]);
        let mine = git::head(&dir);
        let refused = entry
            .as_ref()
            .map(|e| run_steps(&dir, &e.undo, OpKind::Undo(e.clone()), None).0);
        let main_now = git::resolve(&dir, "refs/heads/main");
        let head_now = git::head(&dir);
        // Without it, undoing goes through.
        let back = git::run(Some(&dir), &["reset", "-q", "--hard", &pulled]);
        let undone = entry
            .as_ref()
            .map(|e| run_steps(&dir, &e.undo, OpKind::Undo(e.clone()), None).0);
        let main_after = git::resolve(&dir, "refs/heads/main");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(reset.is_ok() && result.is_ok() && more.is_ok() && back.is_ok());
        assert!(matches!(refused, Some(Err(_))));
        assert_eq!(main_now, Ok(mine.clone()));
        assert_eq!(head_now, mine);
        assert!(matches!(undone, Some(Ok(_))));
        assert_eq!(main_after, Ok(was));
    }
}
