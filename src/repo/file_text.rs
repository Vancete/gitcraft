//! A whole file as it is at a commit, in the index or on disk: read, coloured and shown with
//! its line numbers. For the file view's File tab, and diffs switched to the file.

use std::path::Path;

use egui::{Align2, Rect, Sense, Ui, pos2, vec2};

use super::DiffKey;
use super::highlight::{self, Spans};
use crate::git::{self, Area};
use crate::theme::{self, Tokens};

const LINE_H: f32 = 19.0;
const NUM_W: f32 = 56.0;
/// Bytes looked at for a NUL, which makes a file binary (as git decides it).
const SNIFF: usize = 8000;

pub(crate) struct FileText {
    /// The lines, tabs expanded; empty for a binary file.
    lines: Vec<String>,
    spans: Option<Vec<Spans>>,
    /// Characters in the longest line, for the width to scroll.
    longest: usize,
    /// How big a binary file is, which isn't shown as text.
    binary: Option<usize>,
    /// Read from before the change: the change deleted the file.
    pub(crate) before: bool,
}

impl FileText {
    pub(crate) fn new(bytes: &[u8], path: &str, dark: bool) -> Self {
        if bytes[..bytes.len().min(SNIFF)].contains(&0) {
            return Self {
                lines: Vec::new(),
                spans: None,
                longest: 0,
                binary: Some(bytes.len()),
                before: false,
            };
        }
        let lines = split_lines(&String::from_utf8_lossy(bytes));
        let spans = highlight::highlight(path, lines.iter().map(String::as_str), dark);
        let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        Self {
            lines,
            spans,
            longest,
            binary: None,
            before: false,
        }
    }
}

/// The lines of `text`, without their line endings and with tabs four spaces wide.
fn split_lines(text: &str) -> Vec<String> {
    text.split_terminator('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).replace('\t', "    "))
        .collect()
}

/// `path` at `rev` (`""`: the index; `None`: on disk), coloured; on a background thread.
pub(crate) fn load(
    repo: &Path,
    rev: Option<&str>,
    path: &str,
    dark: bool,
) -> Result<FileText, String> {
    git::file_at(repo, rev, path).map(|bytes| FileText::new(&bytes, path, dark))
}

/// The file of a diff as it is after the change, or before it when the change deleted it.
pub(crate) fn load_for_diff(repo: &Path, key: &DiffKey, dark: bool) -> Result<FileText, String> {
    let (path, after, before): (&str, Option<String>, Option<Option<String>>) = match key {
        DiffKey::Commit { hash, path } => {
            (path, Some(hash.clone()), Some(Some(format!("{hash}^"))))
        }
        DiffKey::Work(file) => match file.area {
            Area::Staged => (
                &file.path,
                Some(String::new()),
                Some(Some("HEAD".to_owned())),
            ),
            Area::Unstaged => (&file.path, None, Some(Some(String::new()))),
            Area::Untracked | Area::Conflicted => (&file.path, None, None),
        },
        DiffKey::Range { from, to, path } => (path, to.clone(), Some(Some(from.clone()))),
    };
    match load(repo, after.as_deref(), path, dark) {
        Err(e) => {
            let Some(before) = before else { return Err(e) };
            let mut text = load(repo, before.as_deref(), path, dark).map_err(|_| e)?;
            text.before = true;
            Ok(text)
        }
        ok => ok,
    }
}

/// The file, or a spinner, the error or a note in its place. `id`: what keeps its scrolling.
pub(crate) fn show(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    text: Option<&Result<FileText, String>>,
) {
    let t = Tokens::get(ui.ctx());
    let data = match text {
        None => {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        }
        Some(Err(e)) => {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.colored_label(t.danger, e);
            });
            return;
        }
        Some(Ok(data)) => data,
    };
    let note = match data.binary {
        Some(size) => Some(format!("Binary file, {size} bytes")),
        None if data.lines.is_empty() => Some("Empty file".to_owned()),
        None => None,
    };
    if let Some(note) = note {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new(note).color(t.text_dim));
        });
        return;
    }
    let font = theme::mono(12.0);
    let char_w = ui
        .painter()
        .layout_no_wrap("M".to_owned(), font.clone(), t.text)
        .size()
        .x;
    let width = (NUM_W + 16.0 + data.longest as f32 * char_w + 24.0).max(ui.available_width());
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::both()
        .id_salt(("file_text", id))
        .auto_shrink(false)
        .show_rows(ui, LINE_H, data.lines.len(), |ui, range| {
            for i in range {
                let Some(line) = data.lines.get(i) else {
                    continue;
                };
                let (rect, _) = ui.allocate_exact_size(vec2(width, LINE_H), Sense::hover());
                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                let gutter = Rect::from_min_size(rect.min, vec2(NUM_W, LINE_H));
                p.rect_filled(gutter, 0.0, t.card.gamma_multiply(0.6));
                let mid = rect.center().y;
                p.text(
                    pos2(gutter.right() - 8.0, mid),
                    Align2::RIGHT_CENTER,
                    (i + 1).to_string(),
                    theme::mono(11.0),
                    t.text_faint,
                );
                let x = gutter.right() + 10.0;
                match data.spans.as_ref().and_then(|s| s.get(i)) {
                    Some(spans) => {
                        let mut job = egui::text::LayoutJob::default();
                        for (color, text) in spans {
                            job.append(
                                text,
                                0.0,
                                egui::TextFormat {
                                    font_id: font.clone(),
                                    color: *color,
                                    ..Default::default()
                                },
                            );
                        }
                        let galley = ui.painter().layout_job(job);
                        p.galley(pos2(x, mid - galley.size().y / 2.0), galley, t.text);
                    }
                    None => {
                        p.text(
                            pos2(x, mid),
                            Align2::LEFT_CENTER,
                            line,
                            font.clone(),
                            t.text,
                        );
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_lose_their_endings_and_tabs() {
        assert_eq!(split_lines("a\r\n\tb\nc"), ["a", "    b", "c"]);
        assert_eq!(split_lines("a\n\n"), ["a", ""]);
        assert!(split_lines("").is_empty());
    }

    #[test]
    fn a_nul_makes_a_file_binary() {
        let text = FileText::new(b"PNG\0\x01", "a.png", true);
        assert_eq!(text.binary, Some(5));
        assert!(text.lines.is_empty());
        let text = FileText::new(b"fn main() {}\n", "main.rs", true);
        assert_eq!((text.binary, text.lines.len(), text.longest), (None, 1, 12));
        assert!(text.spans.is_some());
    }

    #[test]
    fn a_diff_shows_the_file_after_the_change_or_before_its_deletion() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-file-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["config", "commit.gpgSign", "false"],
        ] {
            git(args)?;
        }
        std::fs::write(dir.join("a.txt"), "one\n").map_err(|e| e.to_string())?;
        git(&["add", "-A"])?;
        git(&["commit", "-qm", "one"])?;
        std::fs::write(dir.join("a.txt"), "two\n").map_err(|e| e.to_string())?;
        git(&["commit", "-qam", "two"])?;
        git(&["rm", "-q", "a.txt"])?;
        git(&["commit", "-qm", "gone"])?;
        let commit = |rev: &str| -> Result<DiffKey, String> {
            Ok(DiffKey::Commit {
                hash: git(&["rev-parse", rev])?.trim().to_owned(),
                path: "a.txt".to_owned(),
            })
        };
        let changed = load_for_diff(&dir, &commit("HEAD~1")?, true)?;
        let deleted = load_for_diff(&dir, &commit("HEAD")?, true)?;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            (changed.lines, changed.before),
            (vec!["two".to_owned()], false)
        );
        assert_eq!(
            (deleted.lines, deleted.before),
            (vec!["two".to_owned()], true)
        );
        Ok(())
    }
}
