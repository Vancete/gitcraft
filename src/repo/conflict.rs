//! Three-pane conflict editor: each conflict's "ours" and "theirs" side by side with buttons to
//! pick a side (or both), and the resulting file below, editable by hand.

use std::path::Path;

use egui::{RichText, Sense, Stroke, Ui, vec2};

use super::{Action, Refresh, RepoTab, ops};
use crate::git::InProgress;
use crate::icons;
use crate::theme::{self, Tokens};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Common(String),
    Conflict {
        ours: String,
        base: Option<String>,
        theirs: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
}

pub struct ConflictEditor {
    pub path: String,
    segments: Vec<Segment>,
    choices: Vec<Option<Choice>>,
    pub result: String,
    /// The result was edited by hand; choices no longer rewrite it.
    manual: bool,
    ours_label: String,
    theirs_label: String,
    /// During a rebase "ours" is the branch being rebased onto, "theirs" your own commit.
    rebase: bool,
}

/// Whether `line` (without its line ending) is git's `kind` conflict marker: exactly seven
/// `kind` characters, then nothing or a space and a label. Longer runs (`========` under a
/// Markdown heading, the inner markers of a nested conflict) are content.
fn is_marker(line: &str, kind: char) -> bool {
    let mut chars = line.chars();
    (&mut chars).take(7).filter(|&c| c == kind).count() == 7
        && matches!(chars.next(), None | Some(' '))
}

/// Splits a file with conflict markers into common text and conflicts (diff3 bases kept).
/// A conflict that never ends (no `>>>>>>>` before the end of the file) stays text.
fn parse(text: &str) -> (Vec<Segment>, String, String) {
    enum State {
        Common,
        Ours,
        Base,
        Theirs,
    }
    let mut segments = Vec::new();
    let (mut common, mut ours, mut base, mut theirs) =
        (String::new(), String::new(), None::<String>, String::new());
    let (mut ours_label, mut theirs_label) = (String::new(), String::new());
    // The lines of the conflict being read, markers included, in case it never ends.
    let mut raw = String::new();
    let mut state = State::Common;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        if !matches!(state, State::Common) {
            raw.push_str(line);
        }
        match state {
            State::Common if is_marker(bare, '<') => {
                raw.clear();
                raw.push_str(line);
                if !common.is_empty() {
                    segments.push(Segment::Common(std::mem::take(&mut common)));
                }
                if ours_label.is_empty() {
                    ours_label = bare.trim_start_matches('<').trim().to_owned();
                }
                state = State::Ours;
            }
            State::Common => common.push_str(line),
            State::Ours if is_marker(bare, '|') => {
                base = Some(String::new());
                state = State::Base;
            }
            State::Ours | State::Base if is_marker(bare, '=') => state = State::Theirs,
            State::Ours => ours.push_str(line),
            State::Base => {
                if let Some(b) = &mut base {
                    b.push_str(line);
                }
            }
            State::Theirs if is_marker(bare, '>') => {
                if theirs_label.is_empty() {
                    theirs_label = bare.trim_start_matches('>').trim().to_owned();
                }
                raw.clear();
                segments.push(Segment::Conflict {
                    ours: std::mem::take(&mut ours),
                    base: base.take(),
                    theirs: std::mem::take(&mut theirs),
                });
                state = State::Common;
            }
            State::Theirs => theirs.push_str(line),
        }
    }
    if !matches!(state, State::Common) {
        // Not a conflict after all: keep the text as it was rather than lose it.
        common.push_str(&raw);
    }
    if !common.is_empty() {
        segments.push(Segment::Common(common));
    }
    (segments, ours_label, theirs_label)
}

/// The file for the current choices; undecided conflicts keep their markers.
fn render(segments: &[Segment], choices: &[Option<Choice>], labels: (&str, &str)) -> String {
    let mut out = String::new();
    let mut k = 0;
    for seg in segments {
        match seg {
            Segment::Common(text) => out.push_str(text),
            Segment::Conflict { ours, base, theirs } => {
                match choices.get(k).copied().flatten() {
                    Some(Choice::Ours) => out.push_str(ours),
                    Some(Choice::Theirs) => out.push_str(theirs),
                    Some(Choice::OursThenTheirs) => {
                        out.push_str(ours);
                        out.push_str(theirs);
                    }
                    Some(Choice::TheirsThenOurs) => {
                        out.push_str(theirs);
                        out.push_str(ours);
                    }
                    None => {
                        out.push_str(&format!("<<<<<<< {}\n", labels.0));
                        out.push_str(ours);
                        if let Some(base) = base {
                            out.push_str("||||||| base\n");
                            out.push_str(base);
                        }
                        out.push_str("=======\n");
                        out.push_str(theirs);
                        out.push_str(&format!(">>>>>>> {}\n", labels.1));
                    }
                }
                k += 1;
            }
        }
    }
    out
}

fn start_markers(text: &str) -> usize {
    text.lines()
        .filter(|l| is_marker(l.trim_end_matches('\r'), '<'))
        .count()
}

impl ConflictEditor {
    /// Opens `path` if it still has conflict markers.
    pub fn open(repo: &Path, path: &str, state: Option<InProgress>) -> Option<Self> {
        let text = std::fs::read_to_string(repo.join(path)).ok()?;
        let (segments, ours_label, theirs_label) = parse(&text);
        let conflicts = segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .count();
        if conflicts == 0 {
            return None;
        }
        Some(Self {
            path: path.to_owned(),
            choices: vec![None; conflicts],
            segments,
            result: text,
            manual: false,
            ours_label,
            theirs_label,
            rebase: state == Some(InProgress::Rebase),
        })
    }

    fn choose(&mut self, k: usize, choice: Choice) {
        if let Some(c) = self.choices.get_mut(k) {
            *c = if *c == Some(choice) {
                None
            } else {
                Some(choice)
            };
        }
        self.manual = false;
        self.result = render(
            &self.segments,
            &self.choices,
            (&self.ours_label, &self.theirs_label),
        );
    }

    fn unresolved(&self) -> usize {
        // A start marker that never closed is kept as plain text: not a conflict to resolve.
        let kept_as_text: usize = self
            .segments
            .iter()
            .map(|s| match s {
                Segment::Common(text) => start_markers(text),
                Segment::Conflict { .. } => 0,
            })
            .sum();
        start_markers(&self.result).saturating_sub(kept_as_text)
    }

    pub fn save(&self, repo: &Path) -> Result<(), String> {
        let left = self.unresolved();
        if left > 0 {
            return Err(format!(
                "{left} conflict{} still unresolved in {}",
                if left == 1 { "" } else { "s" },
                self.path
            ));
        }
        std::fs::write(repo.join(&self.path), &self.result)
            .map_err(|e| format!("Could not save {}: {e}", self.path))
    }

    /// Names for the two sides as the user thinks of them.
    fn side_names(&self) -> (String, String) {
        if self.rebase {
            (
                format!("Upstream ({})", short(&self.ours_label)),
                format!("Your commit ({})", short(&self.theirs_label)),
            )
        } else {
            (
                format!("Ours ({})", short(&self.ours_label)),
                format!("Theirs ({})", short(&self.theirs_label)),
            )
        }
    }
}

fn short(label: &str) -> String {
    let s = label.split_whitespace().next().unwrap_or(label);
    if s.chars().count() > 28 {
        format!("{}…", s.chars().take(27).collect::<String>())
    } else {
        s.to_owned()
    }
}

impl RepoTab {
    pub(super) fn conflict_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        let Some(ed) = &mut self.conflict else { return };
        let (ours_name, theirs_name) = ed.side_names();
        let total = ed.choices.len();
        let left = ed.unresolved();

        // Header.
        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "git-merge", 16.0, t.danger);
                    ui.label(RichText::new(&ed.path).font(theme::medium(13.0)));
                    super::widgets::chip(
                        ui,
                        &format!(
                            "{} of {total} resolved",
                            total.saturating_sub(left).min(total)
                        ),
                        if left == 0 { t.success } else { t.warning },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::CloseDiff);
                        }
                        if super::widgets::primary_button(
                            ui,
                            "Save and mark resolved",
                            idle && left == 0,
                            0.0,
                        )
                        .on_disabled_hover_text("Resolve every conflict first")
                        .clicked()
                        {
                            acts.push(Action::SaveConflict);
                        }
                        // Whole file: git's own sides (swapped during a rebase).
                        let (mine, theirs) = if ed.rebase {
                            ("--theirs", "--ours")
                        } else {
                            ("--ours", "--theirs")
                        };
                        for (label, side, tip) in [
                            ("Use theirs", theirs, "Take their version of the whole file"),
                            ("Use mine", mine, "Keep your version of the whole file"),
                        ] {
                            if ui
                                .add_enabled(idle, egui::Button::new(label))
                                .on_hover_text(tip)
                                .clicked()
                            {
                                acts.push(Action::Ops {
                                    label: format!("{label} for {}", ed.path),
                                    ops: ops(&[
                                        &["checkout", side, "--", &ed.path],
                                        &["add", "--", &ed.path],
                                    ]),
                                    refresh: Refresh::Status,
                                });
                            }
                        }
                    });
                });
            });

        // Bottom: the result.
        egui::Panel::bottom("conflict_result")
            .resizable(true)
            .default_size(ui.available_height() * 0.42)
            .size_range(90.0..=f32::INFINITY)
            .frame(
                egui::Frame::new()
                    .fill(t.card)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("RESULT")
                            .font(theme::semibold(10.5))
                            .color(t.text_faint),
                    );
                    if ed.manual {
                        ui.label(RichText::new("edited by hand").color(t.warning).small());
                        if ui.small_button("Back to choices").clicked() {
                            ed.manual = false;
                            ed.result = render(
                                &ed.segments,
                                &ed.choices,
                                (&ed.ours_label, &ed.theirs_label),
                            );
                        }
                    }
                });
                egui::ScrollArea::both()
                    .id_salt("conflict_result_scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let edit = ui.add(
                            egui::TextEdit::multiline(&mut ed.result)
                                .font(theme::mono(12.0))
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(8),
                        );
                        if edit.changed() {
                            ed.manual = true;
                        }
                    });
            });

        // Top: each conflict, ours | theirs, with the choice buttons.
        egui::ScrollArea::vertical()
            .id_salt("conflict_list")
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut k = 0;
                let mut pick = None;
                for seg in &ed.segments {
                    let Segment::Conflict { ours, theirs, .. } = seg else {
                        continue;
                    };
                    let chosen = ed.choices.get(k).copied().flatten();
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("Conflict {} of {total}", k + 1))
                                        .font(theme::semibold(12.0)),
                                );
                                ui.add_space(8.0);
                                for (choice, label) in [
                                    (Choice::Ours, ours_name.as_str()),
                                    (Choice::Theirs, theirs_name.as_str()),
                                    (Choice::OursThenTheirs, "Both (ours first)"),
                                    (Choice::TheirsThenOurs, "Both (theirs first)"),
                                ] {
                                    let on = chosen == Some(choice);
                                    let text = if on {
                                        RichText::new(label).color(t.accent_text)
                                    } else {
                                        RichText::new(label)
                                    };
                                    let b = egui::Button::new(text).fill(if on {
                                        t.accent
                                    } else {
                                        t.field
                                    });
                                    if ui.add(b).clicked() {
                                        pick = Some((k, choice));
                                    }
                                }
                            });
                            ui.add_space(4.0);
                            ui.columns(2, |cols| {
                                for (col, (name, text, side)) in cols.iter_mut().zip([
                                    (ours_name.as_str(), ours, Choice::Ours),
                                    (theirs_name.as_str(), theirs, Choice::Theirs),
                                ]) {
                                    let picked = matches!(
                                        (chosen, side),
                                        (Some(Choice::Ours), Choice::Ours)
                                            | (Some(Choice::Theirs), Choice::Theirs)
                                    ) || matches!(
                                        chosen,
                                        Some(Choice::OursThenTheirs | Choice::TheirsThenOurs)
                                    );
                                    let border = if picked { t.accent } else { t.field_border };
                                    egui::Frame::new()
                                        .fill(t.field)
                                        .stroke(Stroke::new(1.0, border))
                                        .corner_radius(t.radius)
                                        .inner_margin(egui::Margin::same(8))
                                        .show(col, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(
                                                RichText::new(name).color(t.text_faint).small(),
                                            );
                                            let body = if text.is_empty() {
                                                "(nothing)"
                                            } else {
                                                text.trim_end_matches(['\r', '\n'])
                                            };
                                            // No wrapping: wrapped code loses its indentation.
                                            egui::ScrollArea::horizontal()
                                                .id_salt(("conflict-side", k, side == Choice::Ours))
                                                .show(ui, |ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(body)
                                                                .font(theme::mono(12.0)),
                                                        )
                                                        .extend(),
                                                    );
                                                });
                                        });
                                }
                            });
                        });
                    let (sep, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, t.separator);
                    k += 1;
                }
                if let Some((k, choice)) = pick {
                    ed.choose(k, choice);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "a\n<<<<<<< HEAD\nours 1\n=======\ntheirs 1\n>>>>>>> feature\nb\n<<<<<<< HEAD\nx\n||||||| base\nbase\n=======\ny\n>>>>>>> feature\nc\n";

    #[test]
    fn parses_conflicts_and_labels() {
        let (segs, ours, theirs) = parse(FILE);
        assert_eq!((ours.as_str(), theirs.as_str()), ("HEAD", "feature"));
        let conflicts: Vec<_> = segs
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .collect();
        assert_eq!(conflicts.len(), 2);
        assert_eq!(
            conflicts[1],
            &Segment::Conflict {
                ours: "x\n".into(),
                base: Some("base\n".into()),
                theirs: "y\n".into()
            }
        );
    }

    #[test]
    fn renders_choices_and_keeps_undecided_markers() {
        let (segs, o, t) = parse(FILE);
        let out = render(
            &segs,
            &[Some(Choice::Theirs), Some(Choice::OursThenTheirs)],
            (&o, &t),
        );
        assert_eq!(out, "a\ntheirs 1\nb\nx\ny\nc\n");
        let partial = render(&segs, &[Some(Choice::Ours), None], (&o, &t));
        assert!(partial.starts_with("a\nours 1\nb\n<<<<<<< HEAD\nx\n"));
        assert!(partial.contains("||||||| base\nbase\n=======\ny\n>>>>>>> feature\n"));
    }

    #[test]
    fn unterminated_conflict_stays_text() {
        // One real conflict, then a stray start marker (a fixture, a doc about git…).
        let text =
            "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\nc\n<<<<<<< not a conflict\nd\n=======\ne\n";
        let (segs, o, t) = parse(text);
        let conflicts = segs
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .count();
        assert_eq!(conflicts, 1);
        assert_eq!(
            render(&segs, &[Some(Choice::Ours)], (&o, &t)),
            "a\nx\nc\n<<<<<<< not a conflict\nd\n=======\ne\n"
        );
        // Once the real one is chosen, nothing is left to resolve.
        let mut ed = ConflictEditor {
            path: "f".into(),
            choices: vec![None],
            segments: segs,
            result: text.into(),
            manual: false,
            ours_label: o,
            theirs_label: t,
            rebase: false,
        };
        assert_eq!(ed.unresolved(), 1);
        ed.choose(0, Choice::Ours);
        assert_eq!(ed.unresolved(), 0);
    }

    #[test]
    fn longer_runs_are_content() {
        // A Markdown heading underline and nested (longer) markers don't split the conflict.
        let text = "<<<<<<< HEAD\nTitle\n========\n<<<<<<<< inner\n=======\ny\n>>>>>>> b\n";
        let (segs, o, t) = parse(text);
        assert_eq!(
            segs,
            vec![Segment::Conflict {
                ours: "Title\n========\n<<<<<<<< inner\n".into(),
                base: None,
                theirs: "y\n".into(),
            }]
        );
        assert_eq!(render(&segs, &[Some(Choice::Theirs)], (&o, &t)), "y\n");
        assert!(is_marker("<<<<<<<", '<'));
        assert!(is_marker(">>>>>>> feature", '>'));
        assert!(!is_marker("<<<<<<<x", '<'));
        assert!(!is_marker("======", '='));
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let crlf = "a\r\n<<<<<<< HEAD\r\nmine\r\n=======\r\nyours\r\n>>>>>>> b\r\nz\r\n";
        let (segs, o, t) = parse(crlf);
        assert_eq!(
            render(&segs, &[Some(Choice::Ours)], (&o, &t)),
            "a\r\nmine\r\nz\r\n"
        );
    }
}
