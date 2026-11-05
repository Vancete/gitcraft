//! Interactive rebase: the commits after a base, newest on top like the graph, each picked,
//! reworded, squashed, fixed up or dropped, and reordered. Starting it writes git's todo list
//! and hands it over as `sequence.editor`; rewords become an `exec git commit --amend -F`.

use std::path::{Path, PathBuf};

use egui::{Stroke, Ui};

use super::widgets;
use super::{Action, RepoTab};
use crate::git::RebaseCommit;
use crate::icons;
use crate::theme::{self, Tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl Step {
    const ALL: [Step; 5] = [
        Step::Pick,
        Step::Reword,
        Step::Squash,
        Step::Fixup,
        Step::Drop,
    ];

    fn label(self) -> &'static str {
        match self {
            Step::Pick => "Pick",
            Step::Reword => "Reword",
            Step::Squash => "Squash",
            Step::Fixup => "Fixup",
            Step::Drop => "Drop",
        }
    }

    fn explain(self) -> &'static str {
        match self {
            Step::Pick => "Keep the commit as it is",
            Step::Reword => "Keep the changes, edit the message",
            Step::Squash => "Meld into the commit below, keeping both messages",
            Step::Fixup => "Meld into the commit below, dropping this message",
            Step::Drop => "Remove the commit and its changes",
        }
    }
}

pub(crate) struct Entry {
    pub commit: RebaseCommit,
    pub step: Step,
    /// The new message, for rewords.
    pub message: String,
}

pub(crate) struct RebaseEditor {
    /// What the commits are replayed onto (a commit or a branch).
    pub onto: String,
    pub onto_label: String,
    /// Oldest first, as git lists them.
    pub entries: Vec<Entry>,
}

impl RebaseEditor {
    pub fn new(onto: String, onto_label: String, commits: Vec<RebaseCommit>) -> Self {
        let entries = commits
            .into_iter()
            .map(|c| Entry {
                message: c.message.clone(),
                commit: c,
                step: Step::Pick,
            })
            .collect();
        Self {
            onto,
            onto_label,
            entries,
        }
    }

    /// Why the rebase can't start as set up, if it can't.
    pub fn problem(&self) -> Option<String> {
        if self.entries.iter().any(|e| e.commit.merge) {
            return Some(
                "These commits include merges, which this editor would flatten. Use git rebase -i --rebase-merges in the terminal."
                    .to_owned(),
            );
        }
        let mut kept = self.entries.iter().filter(|e| e.step != Step::Drop);
        match kept.next() {
            None => {
                return Some("Keep at least one commit (or reset the branch instead).".to_owned());
            }
            Some(e) if matches!(e.step, Step::Squash | Step::Fixup) => {
                return Some(format!(
                    "{} {} has no commit below it to meld into.",
                    e.step.label(),
                    short(&e.commit.hash)
                ));
            }
            _ => {}
        }
        if self
            .entries
            .iter()
            .any(|e| e.step == Step::Reword && e.message.trim().is_empty())
        {
            return Some("A reworded commit needs a message.".to_owned());
        }
        None
    }

    fn changed(&self, e: &Entry) -> bool {
        e.step == Step::Reword && e.message.trim_end() != e.commit.message.trim_end()
    }

    /// Writes the todo list and the new messages to a fresh temporary folder and returns the
    /// git command that runs the rebase with them.
    pub fn command(&self) -> Result<Vec<String>, String> {
        let dir = private_dir(&std::env::temp_dir())
            .map_err(|e| format!("could not prepare the rebase: {e}"))?;
        let write = |name: &str, text: &str| -> Result<PathBuf, String> {
            let path = dir.join(name);
            std::fs::write(&path, text)
                .map_err(|e| format!("could not prepare the rebase: {e}"))?;
            Ok(path)
        };
        let mut files = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            files.push(if self.changed(e) {
                Some(write(&format!("message-{i}.txt"), &e.message)?)
            } else {
                None
            });
        }
        let todo = todo(&self.entries, |i| {
            files.get(i).cloned().flatten().map(|p| sh_path(&p))
        });
        let todo_file = write("todo.txt", &todo)?;
        Ok(vec![
            "-c".to_owned(),
            format!("sequence.editor=cp {}", sh_path(&todo_file)),
            "rebase".to_owned(),
            "-i".to_owned(),
            "--autostash".to_owned(),
            "--end-of-options".to_owned(),
            self.onto.clone(),
        ])
    }
}

/// A new folder under `parent` that only this user can enter. Never one that already exists:
/// in a shared temporary folder, another user could have made it to swap the todo list (whose
/// `exec` lines git runs) for their own.
fn private_dir(parent: &Path) -> std::io::Result<PathBuf> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    let base = format!(
        "gitcraft-rebase-{}-{}",
        std::process::id(),
        crate::git::now()
    );
    let mut n = 0u32;
    loop {
        let dir = parent.join(if n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        });
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => n += 1,
            Err(e) => return Err(e),
        }
    }
}

/// git's todo list for `entries` (oldest first). `message_file(i)` is the quoted path of entry
/// `i`'s new message when it was reworded.
fn todo(entries: &[Entry], message_file: impl Fn(usize) -> Option<String>) -> String {
    let mut out = String::new();
    for (i, e) in entries.iter().enumerate() {
        let verb = match e.step {
            Step::Pick | Step::Reword => "pick",
            Step::Squash => "squash",
            Step::Fixup => "fixup",
            Step::Drop => "drop",
        };
        out.push_str(&format!("{verb} {} {}\n", e.commit.hash, e.commit.summary));
        if let Some(file) = message_file(i) {
            out.push_str(&format!(
                "exec git commit --amend --allow-empty --cleanup=whitespace -F {file}\n"
            ));
        }
    }
    out
}

/// `path` for git's shell (`sh`, also on Windows): forward slashes, single-quoted.
fn sh_path(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    format!("'{}'", p.replace('\'', r"'\''"))
}

fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

impl RepoTab {
    pub(super) fn rebase_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        let branch = self
            .refs
            .current
            .clone()
            .unwrap_or_else(|| "HEAD".to_owned());
        let Some(editor) = &mut self.rebase else {
            return;
        };

        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "git-merge", 16.0, t.icon);
                    ui.label(
                        egui::RichText::new(format!(
                            "Interactive rebase · {branch} onto {}",
                            editor.onto_label
                        ))
                        .font(theme::medium(13.0)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Cancel (Esc)").clicked() {
                            acts.push(Action::CloseRebase);
                        }
                    });
                });
            });

        let problem = editor.problem();
        egui::Panel::bottom("rebase_footer")
            .frame(
                egui::Frame::new()
                    .fill(t.chrome)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0, t.separator)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    match &problem {
                        Some(p) => {
                            ui.colored_label(t.danger, p);
                        }
                        None => {
                            let n = editor.entries.len();
                            ui.label(
                                egui::RichText::new(format!(
                                    "{n} commit{}, newest on top · Ctrl+Z undoes the rebase",
                                    if n == 1 { "" } else { "s" }
                                ))
                                .color(t.text_dim),
                            );
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary_button(
                            ui,
                            "Start rebase",
                            idle && problem.is_none(),
                            120.0,
                        )
                        .clicked()
                        {
                            acts.push(Action::RunRebase);
                        }
                        if ui.button("Cancel").clicked() {
                            acts.push(Action::CloseRebase);
                        }
                    });
                });
            });

        let n = editor.entries.len();
        let mut swap: Option<(usize, usize)> = None;
        egui::ScrollArea::vertical()
            .id_salt("rebase_list")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.add_space(6.0);
                // Newest first, like the graph; `i` indexes git's oldest-first order.
                for i in (0..n).rev() {
                    let Some(e) = editor.entries.get_mut(i) else {
                        continue;
                    };
                    ui.push_id(&e.commit.hash, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.add_enabled_ui(i + 1 < n, |ui| {
                                if icons::small_button(ui, "chevron-up", "Move up (later)")
                                    .clicked()
                                {
                                    swap = Some((i, i + 1));
                                }
                            });
                            ui.add_enabled_ui(i > 0, |ui| {
                                if icons::small_button(ui, "chevron-down", "Move down (earlier)")
                                    .clicked()
                                {
                                    swap = Some((i, i - 1));
                                }
                            });
                            egui::ComboBox::from_id_salt("step")
                                .width(84.0)
                                .selected_text(e.step.label())
                                .show_ui(ui, |ui| {
                                    for step in Step::ALL {
                                        ui.selectable_value(&mut e.step, step, step.label())
                                            .on_hover_text(step.explain());
                                    }
                                });
                            let dim = e.step == Step::Drop;
                            ui.label(
                                egui::RichText::new(short(&e.commit.hash))
                                    .font(theme::mono(11.5))
                                    .color(t.text_faint),
                            );
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                            widgets::avatar(
                                ui,
                                rect.center(),
                                9.0,
                                &e.commit.author,
                                &e.commit.email,
                            );
                            let mut text = egui::RichText::new(&e.commit.summary).color(if dim {
                                t.text_faint
                            } else {
                                t.text
                            });
                            if dim {
                                text = text.strikethrough();
                            }
                            ui.label(text);
                            if matches!(e.step, Step::Squash | Step::Fixup) {
                                ui.label(
                                    egui::RichText::new("↓ into the commit below")
                                        .color(t.accent)
                                        .small(),
                                );
                            }
                        });
                        if e.step == Step::Reword {
                            ui.horizontal(|ui| {
                                ui.add_space(70.0);
                                ui.add(
                                    egui::TextEdit::multiline(&mut e.message)
                                        .desired_rows(3)
                                        .desired_width(ui.available_width() - 16.0),
                                );
                            });
                        }
                    });
                    ui.add_space(2.0);
                    ui.separator();
                }
            });
        if let Some((a, b)) = swap {
            editor.entries.swap(a, b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(hash: &str, step: Step) -> Entry {
        Entry {
            commit: RebaseCommit {
                hash: hash.to_owned(),
                summary: format!("summary {hash}"),
                message: format!("summary {hash}"),
                ..RebaseCommit::default()
            },
            step,
            message: format!("summary {hash}"),
        }
    }

    #[test]
    fn todo_lines() {
        let entries = vec![
            entry("a1", Step::Pick),
            entry("b2", Step::Reword),
            entry("c3", Step::Squash),
            entry("d4", Step::Drop),
        ];
        let todo = todo(&entries, |i| (i == 1).then(|| "'/tmp/m 1.txt'".to_owned()));
        assert_eq!(
            todo,
            "pick a1 summary a1\npick b2 summary b2\nexec git commit --amend --allow-empty --cleanup=whitespace -F '/tmp/m 1.txt'\nsquash c3 summary c3\ndrop d4 summary d4\n"
        );
    }

    #[test]
    fn problems() {
        let mut ed = RebaseEditor::new("x".into(), "x".into(), Vec::new());
        ed.entries = vec![entry("a1", Step::Drop), entry("b2", Step::Fixup)];
        assert!(ed.problem().is_some_and(|p| p.contains("below")));
        ed.entries[1].step = Step::Pick;
        assert_eq!(ed.problem(), None);
        ed.entries[1].step = Step::Drop;
        assert!(ed.problem().is_some());
        ed.entries[1].step = Step::Reword;
        ed.entries[1].message = "  ".into();
        assert!(ed.problem().is_some());
    }

    #[test]
    fn private_dirs_are_new() {
        let parent = std::env::temp_dir().join(format!("gitcraft-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&parent);
        let a = private_dir(&parent);
        let b = private_dir(&parent);
        assert!(a.is_ok() && b.is_ok());
        if let (Ok(a), Ok(b)) = (&a, &b) {
            assert_ne!(a, b);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(a).map(|m| m.permissions().mode() & 0o777);
                assert_eq!(mode.ok(), Some(0o700));
            }
        }
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn shell_paths() {
        assert_eq!(
            sh_path(Path::new(r"C:\Temp\it's\todo.txt")),
            r"'C:/Temp/it'\''s/todo.txt'"
        );
    }
}
