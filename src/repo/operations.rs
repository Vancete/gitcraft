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
        let path = self.path.clone();
        let label = label.to_owned();
        let creds = self.credentials.clone();
        spawn(&self.tx, ctx, move || {
            let (result, kind) = run_steps(&path, &ops, kind, creds.as_ref());
            Msg::OpDone {
                label,
                result,
                refresh,
                kind,
                quiet,
            }
        });
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
    /// for a long one, then re-reads what it changed.
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
                if label == "Commit" || label == "Amend" || label == "Continue rebase" {
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
                });
                self.redo_stack.clear();
            }
            OpKind::Normal(Undo::Custom { undo, redo }, _) => {
                self.undo_stack.push(UndoEntry {
                    label: label.to_owned(),
                    undo,
                    redo,
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

/// Runs git commands one after another, stopping at the first failure; `kind` comes back with
/// what undoing needs, as found on disk.
pub(super) fn run_steps(
    path: &Path,
    ops: &[Vec<String>],
    kind: OpKind,
    creds: Option<&git::Credentials>,
) -> (Result<String, String>, OpKind) {
    // Where HEAD really is now, not where the last reload saw it: a commit made in the
    // terminal since must not be undone along with this.
    let head_before =
        matches!(kind, OpKind::Normal(Undo::HeadMove { .. }, _)).then(|| git::head(path));
    let pushes_stash = ops.first().is_some_and(|op| {
        let stash = stash_push_args();
        op.len() >= stash.len() && op.iter().zip(stash).all(|(a, b)| a == b)
    });
    let stash_before = pushes_stash.then(|| git::resolve(path, "refs/stash").ok());
    let mut result = Ok(String::new());
    for op in ops {
        let args: Vec<&str> = op.iter().map(String::as_str).collect();
        result = git::run_with(Some(path), &args, creds);
        if result.is_err() {
            break;
        }
    }
    let kind = match kind {
        // For HEAD moves, the new HEAD is what redo goes back to.
        OpKind::Normal(Undo::HeadMove { mode }, before) if result.is_ok() => {
            let before = head_before.filter(|h| !h.is_empty()).unwrap_or(before);
            OpKind::Normal(
                Undo::HeadMove { mode },
                format!("{before}\u{0}{}", git::head(path)),
            )
        }
        // "No local changes to save" makes no stash: undoing would pop an older one.
        OpKind::Normal(Undo::Custom { .. }, before)
            if stash_before.is_some_and(|b| b == git::resolve(path, "refs/stash").ok()) =>
        {
            OpKind::Normal(Undo::None, before)
        }
        other => other,
    };
    (result, kind)
}

/// How the app stashes (and how `run_ops_ex` knows an operation for one).
pub(super) fn stash_push_args() -> [&'static str; 3] {
    ["stash", "push", "--include-untracked"]
}
