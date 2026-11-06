//! Carrying out the actions the views asked for.

use std::path::Path;
use std::time::Instant;

use super::{
    Action, Compare, DiffKey, DiffView, Msg, OpKind, OpenRequest, Refresh, RepoTab, Undo, diff,
    ops, spawn,
};
use crate::git;
use crate::settings::Settings;

impl RepoTab {
    pub(crate) fn apply(&mut self, action: Action, ctx: &egui::Context) {
        let plain = OpKind::plain();
        match action {
            Action::Select(row) => {
                self.close_pr();
                self.ci_view = None;
                self.select(row, ctx);
            }
            Action::SelectHash(hash) => {
                self.close_pr();
                self.ci_view = None;
                self.goto(&hash, ctx);
            }
            Action::SelectStash(i) => {
                self.close_pr();
                self.ci_view = None;
                self.select_stash(i, ctx);
            }
            Action::OpenDiff(key) => self.open_diff(key, ctx),
            Action::OpenPr(number) => {
                // It refuses while an editor takes the big area: the builds shown stay then.
                let requests = self.pr_requests;
                self.open_pr(number, ctx);
                if self.pr_requests != requests {
                    self.ci_view = None;
                }
            }
            Action::ClosePr => {
                self.close_pr();
                self.diff = None;
            }
            Action::ReloadPr => self.load_pr(ctx),
            Action::Pr(act) => self.pr_act(act, ctx),
            Action::OpenCi { sha, run } => self.open_ci(sha, run, ctx),
            Action::CloseCi => self.ci_view = None,
            Action::Ci(act) => self.ci_act(act, ctx),
            Action::CloseDiff => {
                self.diff = None;
                self.conflict = None;
            }
            Action::Ops {
                label,
                ops,
                refresh,
            } => self.run_ops(&label, ops, refresh, plain, ctx),
            Action::UndoableOps {
                label,
                ops,
                refresh,
                undo,
            } => self.run_undoable(&label, ops, refresh, undo, ctx),
            Action::Checkout(name) => self.checkout(&name, ctx),
            Action::CheckoutPull(name) => self.checkout_pull(&name, ctx),
            Action::CheckoutRemote(name) => self.checkout_remote(&name, ctx),
            Action::Dialog(dialog) => self.dialog = Some(dialog),
            Action::Commit => self.commit(ctx),
            Action::Fetch => {
                self.last_fetch = Instant::now();
                self.run_ops(
                    "Fetch",
                    ops(&[&["fetch", "--all", "--prune"]]),
                    Refresh::All,
                    plain,
                    ctx,
                )
            }
            Action::Pull => self.pull(Settings::get(ctx).pull_mode, ctx),
            Action::PullWith(mode) => self.pull(mode, ctx),
            Action::Push => self.push(ctx),
            Action::Reload => self.reload(ctx),
            Action::LoadMore => self.load_more(ctx),
            Action::Undo => self.undo(ctx),
            Action::Redo => self.redo(ctx),
            Action::StashPush(message) => self.stash_push(&message, ctx),
            Action::OpenTab(path) => self.open_request = Some(OpenRequest::NewTab(path)),
            Action::OpenWorktree(path) => self.open_request = Some(OpenRequest::Worktree(path)),
            Action::ToggleBranches => {
                let mut settings = Settings::get(ctx);
                settings.show_branches = !settings.show_branches;
                settings.set(ctx);
            }
            Action::BranchVisibility(change) => self.change_visibility(change, ctx),
            Action::OpenBranches(section) => {
                let mut settings = Settings::get(ctx);
                settings.show_branches = true;
                settings.set(ctx);
                match section {
                    Some(key) => self.sidebar_reveal = Some(key),
                    None => self.focus_branch_filter = true,
                }
            }
            Action::ToggleDetail => {
                let mut settings = Settings::get(ctx);
                settings.show_detail = !settings.show_detail;
                settings.set(ctx);
            }
            Action::ToggleTerminal => self.toggle_terminal(ctx),
            Action::NewTerminal { shell, cwd } => self.start_terminal(shell, cwd, ctx),
            Action::CloseTerminal(i) => self.close_terminal(i),
            Action::SelectTerminal(i) => self.active_terminal = i,
            Action::ToggleSearch => self.toggle_search(),
            Action::SearchStep(d) => self.search_step(d, ctx),
            Action::CreatePr {
                branch,
                base,
                title,
                body,
                draft,
            } => self.create_pr(branch, base, title, body, draft, ctx),
            Action::OpenUrl(url) => {
                if crate::pull_request::is_web_url(&url) {
                    let _ = open::that_detached(url);
                }
            }
            Action::OpenPath(rel) => {
                let _ = open::that_detached(self.path.join(rel));
            }
            Action::OpenInEditor(rel) => {
                let editor = Settings::get(ctx).editor;
                if let Err(e) = open_in_editor(&editor, &self.path.join(rel)) {
                    self.status = Some((e, true));
                }
            }
            Action::SetIdentity(name, email) => self.run_ops(
                "Set identity",
                ops(&[
                    &["config", "--local", "user.name", &name],
                    &["config", "--local", "user.email", &email],
                ]),
                Refresh::All,
                plain,
                ctx,
            ),
            Action::SetDiffOptions {
                split,
                ignore_ws,
                whole_file,
            } => self.set_diff_options(split, ignore_ws, whole_file, ctx),
            Action::DiffAsFile(on) => self.show_diff_as_file(on, ctx),
            Action::DiffStep(delta) => {
                if let Some(view) = self.diff.as_mut().filter(|_| !self.diff_as_file) {
                    view.step_change(Settings::get(ctx).diff_split, delta);
                }
            }
            Action::ViewAllFiles(on) => self.all_files.on = on,
            Action::FileView { path, rev, tab } => self.open_file_view(path, rev, tab, ctx),
            Action::CloseFileView => self.file_view = None,
            Action::InteractiveRebase { onto, label } => self.interactive_rebase(onto, label, ctx),
            Action::Reword { hash, message } => self.reword_commit(hash, message, ctx),
            Action::EditMessage(hash) => self.edit_message(hash, ctx),
            Action::SavePatch { hash, summary } => self.save_patch(hash, &summary, ctx),
            Action::ApplyPatch => self.apply_patch(ctx),
            Action::QuickEdit { edit, hash } => self.quick_edit(edit, hash, ctx),
            Action::RunRebase => self.run_rebase(ctx),
            Action::CloseRebase => self.rebase = None,
            Action::SignIn(creds) => self.sign_in(creds, ctx),
            Action::Ignore { pattern, path } => self.ignore(&pattern, &path, ctx),
            Action::PickLines { pick, lines } => self.pick_lines(pick, &lines, ctx),
            Action::Compare {
                base,
                base_label,
                target,
                target_label,
                since_fork,
            } => self.compare(
                Compare {
                    base,
                    base_label,
                    target,
                    target_label,
                    since_fork,
                    data: None,
                    request: 0,
                },
                ctx,
            ),
            Action::CompareSinceFork(on) => {
                if let Some(c) = &mut self.compare {
                    c.since_fork = on;
                    self.reload_compare(ctx);
                }
            }
            Action::SwapCompare => self.swap_compare(ctx),
            Action::CloseCompare => {
                self.compare = None;
                self.close_range_diff();
            }
            Action::SearchCode(on) => {
                if self.search.code != on {
                    self.search.code = on;
                    self.run_search(ctx);
                }
                self.search.focus = true;
            }
            Action::SolveWithMergiraf => self.solve_with_mergiraf(ctx),
            Action::SaveConflict => self.save_conflict(ctx),
        }
    }

    /// Side by side and the whole file are kept for next time; whitespace is per tab.
    fn set_diff_options(
        &mut self,
        split: bool,
        ignore_ws: bool,
        whole_file: bool,
        ctx: &egui::Context,
    ) {
        let mut settings = Settings::get(ctx);
        let reload = ignore_ws != self.diff_ignore_ws || whole_file != settings.diff_whole_file;
        settings.diff_split = split;
        settings.diff_whole_file = whole_file;
        settings.set(ctx);
        self.diff_ignore_ws = ignore_ws;
        if reload {
            self.reopen_diff(ctx);
        }
    }

    /// Adds `pattern` to `.gitignore`, and stops tracking `path` if git tracks it.
    fn ignore(&mut self, pattern: &str, path: &str, ctx: &egui::Context) {
        match git::add_to_gitignore(&self.path, pattern) {
            Err(e) => self.status = Some((e, true)),
            Ok(added) => {
                if git::is_tracked(&self.path, path) {
                    self.run_ops(
                        &format!("Ignore {pattern}"),
                        ops(&[&[
                            "--literal-pathspecs",
                            "rm",
                            "-r",
                            "--cached",
                            "--quiet",
                            "--",
                            path,
                        ]]),
                        Refresh::Status,
                        OpKind::plain(),
                        ctx,
                    );
                } else {
                    let msg = if added {
                        format!("Added {pattern} to .gitignore")
                    } else {
                        format!("{pattern} was already in .gitignore")
                    };
                    self.status = Some((msg, false));
                    self.reload_status(ctx);
                }
            }
        }
    }

    /// Stages, unstages or discards the chosen lines of the open working-tree diff.
    fn pick_lines(
        &mut self,
        pick: diff::Pick,
        lines: &std::collections::BTreeSet<usize>,
        ctx: &egui::Context,
    ) {
        let Some(DiffView {
            key: DiffKey::Work(file),
            data: Some(Ok(data)),
            ..
        }) = &self.diff
        else {
            return;
        };
        let Some(patch) = diff::partial_patch(&data.header, &data.lines, lines, pick) else {
            return;
        };
        let patch = match git::save_patch(&patch) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                self.status = Some((e, true));
                return;
            }
        };
        let n = lines.len();
        let label = format!(
            "{} {n} line{} of {}",
            pick.verb(),
            if n == 1 { "" } else { "s" },
            file.path
        );
        let mut args = pick.args().to_vec();
        args.push(&patch);
        if pick == diff::Pick::Discard {
            // Applying the same patch forward brings the lines back.
            let undo = Undo::Custom {
                undo: ops(&[&["apply", &patch]]),
                redo: ops(&[&args]),
            };
            self.run_undoable(&label, ops(&[&args]), Refresh::Status, undo, ctx);
        } else {
            self.run_ops(&label, ops(&[&args]), Refresh::Status, OpKind::plain(), ctx);
        }
    }

    /// Asks Mergiraf to solve what it can of the open conflict.
    fn solve_with_mergiraf(&mut self, ctx: &egui::Context) {
        if let Some(ed) = self.conflict.as_mut().filter(|ed| !ed.solving) {
            ed.solving = true;
            let (repo, file) = (self.path.clone(), ed.path.clone());
            spawn(&self.tx, ctx, move || {
                let result = crate::mergiraf::solve(&repo, &file);
                Msg::Mergiraf(file, result)
            });
        }
    }

    /// Takes Mergiraf's answer into the conflict editor (if it's still on that file), and says
    /// how much it solved.
    pub(super) fn mergiraf_done(&mut self, path: String, result: Result<String, String>) {
        let Some(ed) = self.conflict.as_mut().filter(|ed| ed.path == path) else {
            return;
        };
        ed.solving = false;
        self.status = Some(match result {
            Ok(text) => {
                let before = ed.on_disk;
                let left = ed.take_solved(text);
                let solved = before.saturating_sub(left);
                let message = match (solved, left) {
                    (0, _) => format!("Mergiraf could not solve the conflicts in {path}"),
                    (_, 0) => format!(
                        "Mergiraf solved every conflict in {path}: check the result, then save"
                    ),
                    _ => format!(
                        "Mergiraf solved {solved} of {before} conflicts in {path}; {left} left \
                         to choose"
                    ),
                };
                (message, false)
            }
            Err(e) => (e, true),
        });
    }

    /// Writes the conflict editor's file and marks it resolved.
    fn save_conflict(&mut self, ctx: &egui::Context) {
        let Some(editor) = &self.conflict else { return };
        match editor.save(&self.path) {
            Ok(()) => {
                let file = editor.path.clone();
                self.run_ops(
                    "Mark resolved",
                    ops(&[&["--literal-pathspecs", "add", "--", &file]]),
                    Refresh::Status,
                    OpKind::plain(),
                    ctx,
                );
            }
            Err(e) => self.status = Some((e, true)),
        }
    }
}

/// Runs the user's editor command with `path` (through `cmd` on Windows, where editors are
/// usually `.cmd` scripts).
pub(crate) fn open_in_editor(editor: &str, path: &Path) -> Result<(), String> {
    let editor = if editor.trim().is_empty() {
        "code"
    } else {
        editor.trim()
    };
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        // The path in quotes and through a variable: a file named `a&calc` must not run `calc`,
        // and one named `%COMSPEC%` must not open another (cmd expands `%…%` even in quotes,
        // but not again inside what a variable held). `/S` takes off only the outer quotes,
        // leaving `editor "path"`.
        let mut target = path.as_os_str().to_owned();
        // A backslash before the closing quote would escape it for the editor (the folder
        // itself ends with one): doubled, it is just a backslash.
        if target.to_string_lossy().ends_with('\\') {
            target.push("\\");
        }
        c.env("GITCRAFT_OPEN_PATH", target);
        let mut line = std::ffi::OsString::from("/S /C \"");
        line.push(editor);
        line.push(" \"%GITCRAFT_OPEN_PATH%\"\"");
        c.raw_arg(line);
        c.creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        // A command with arguments (`code -n`, `flatpak run …`), as `cmd` reads it on Windows:
        // through the shell, the path still one argument (`$1`) it does not read. A program
        // whose own path has a space in it is run as it is.
        let mut c = if editor.contains(char::is_whitespace) && !Path::new(editor).exists() {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(format!("{editor} \"$1\"")).arg("sh");
            c
        } else {
            std::process::Command::new(editor)
        };
        c.arg(path);
        crate::shell_path::apply(&mut c);
        c
    };
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not run {editor}: {e}"))?;
    // Reaped once it ends (most editor commands hand the file over and exit at once), or each
    // file opened would leave a zombie process for as long as the app runs.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
