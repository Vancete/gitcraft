//! Three-pane conflict editor: each conflict's "ours" and "theirs" side by side with buttons to
//! pick a side (or both), and the resulting file below, editable by hand.

mod view;

use std::path::Path;

use crate::git::InProgress;

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
    /// Mergiraf is working on the file.
    pub solving: bool,
    /// How many conflicts the file has on disk, which Mergiraf starts from.
    pub on_disk: usize,
    /// The file as it was opened: while it stays so, a refresh keeps the choices made.
    opened: String,
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

/// The lines of `text` that look like one of git's conflict markers.
fn marker_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| ['<', '|', '=', '>'].iter().any(|&kind| is_marker(l, kind)))
}

/// How many conflicts are left in `result`: each whole one, and what is left of those taken
/// apart by hand (a marker deleted, the others kept). `segments`: the file as it was parsed,
/// whose marker-like lines that are text (a start marker that never closed, a heading's
/// `=======` on one side) may stay.
fn unresolved_in(result: &str, segments: &[Segment]) -> usize {
    use std::collections::HashMap;
    let mut text_markers: HashMap<&str, usize> = HashMap::new();
    for seg in segments {
        let texts = match seg {
            Segment::Common(text) => [Some(text), None, None],
            Segment::Conflict { ours, base, theirs } => [Some(ours), base.as_ref(), Some(theirs)],
        };
        for line in texts.into_iter().flatten().flat_map(|t| marker_lines(t)) {
            *text_markers.entry(line).or_default() += 1;
        }
    }
    let (now, _, _) = parse(result);
    let mut whole = 0;
    // Marker lines outside a whole conflict that the file didn't have as text, by kind.
    let (mut starts, mut middles, mut ends) = (0, 0, 0);
    for seg in &now {
        let text = match seg {
            Segment::Conflict { .. } => {
                whole += 1;
                continue;
            }
            Segment::Common(text) => text,
        };
        for line in marker_lines(text) {
            match text_markers.get_mut(line) {
                Some(n) if *n > 0 => *n -= 1,
                _ if line.starts_with('<') => starts += 1,
                _ if line.starts_with('>') => ends += 1,
                _ => middles += 1,
            }
        }
    }
    // A conflict taken apart keeps its start or its end marker, or at least a middle one.
    whole + starts.max(ends).max(usize::from(middles > 0))
}

impl ConflictEditor {
    /// Opens `path` if it still has conflict markers.
    pub fn open(repo: &Path, path: &str, state: Option<InProgress>) -> Option<Self> {
        // Not through a link: a merge can leave one from the other side here.
        let text = String::from_utf8(crate::git::worktree_file(&repo.join(path)).ok()?).ok()?;
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
            opened: text.clone(),
            result: text,
            manual: false,
            ours_label,
            theirs_label,
            rebase: state == Some(InProgress::Rebase),
            solving: false,
            on_disk: conflicts,
        })
    }

    /// Whether the file in the repository at `repo` is no longer as it was opened.
    pub fn changed_on_disk(&self, repo: &Path) -> bool {
        crate::git::worktree_file(&repo.join(&self.path))
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .as_ref()
            != Some(&self.opened)
    }

    /// Takes `text`, the file with some of its conflicts solved elsewhere (by Mergiraf): the
    /// conflicts left are offered again, the solved ones become text. Returns how many are left.
    pub fn take_solved(&mut self, text: String) -> usize {
        let (segments, ours_label, theirs_label) = parse(&text);
        let left = segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .count();
        if left > 0 {
            self.ours_label = ours_label;
            self.theirs_label = theirs_label;
        }
        self.choices = vec![None; left];
        self.segments = segments;
        self.result = text;
        self.manual = false;
        left
    }

    fn choose(&mut self, k: usize, choice: Choice) {
        if let Some(c) = self.choices.get_mut(k) {
            *c = if *c == Some(choice) {
                None
            } else {
                Some(choice)
            };
        }
        // Edited by hand: the choice waits for "Back to choices" rather than undo the edits.
        if !self.manual {
            self.result = render(
                &self.segments,
                &self.choices,
                (&self.ours_label, &self.theirs_label),
            );
        }
    }

    /// The conflicts left in the result, by its markers (also after editing it by hand).
    fn unresolved(&self) -> usize {
        unresolved_in(&self.result, &self.segments)
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
            opened: text.into(),
            result: text.into(),
            manual: false,
            ours_label: o,
            theirs_label: t,
            rebase: false,
            solving: false,
            on_disk: 1,
        };
        assert_eq!(ed.unresolved(), 1);
        ed.choose(0, Choice::Ours);
        assert_eq!(ed.unresolved(), 0);
    }

    fn editor(text: &str) -> ConflictEditor {
        let (segments, ours_label, theirs_label) = parse(text);
        let conflicts = segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .count();
        ConflictEditor {
            path: "f".into(),
            choices: vec![None; conflicts],
            segments,
            opened: text.into(),
            result: text.into(),
            manual: false,
            ours_label,
            theirs_label,
            rebase: false,
            solving: false,
            on_disk: conflicts,
        }
    }

    /// Markers left by a hand edit keep the file from being saved as resolved, also when the
    /// edit removed a start marker the file had as text.
    #[test]
    fn hand_edits_that_leave_markers_are_unresolved() {
        let text = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\nc\n<<<<<<< not a conflict\nd\n";
        let mut ed = editor(text);
        assert_eq!(ed.unresolved(), 1);
        // The stray line deleted by hand, the real conflict still there.
        ed.manual = true;
        ed.result = text.replacen("<<<<<<< not a conflict\n", "", 1);
        assert_eq!(ed.unresolved(), 1);
        assert!(
            ed.save(Path::new("/nonexistent"))
                .is_err_and(|e| e.contains("unresolved"))
        );
        // A conflict taken apart: its end marker deleted, the rest kept.
        let real = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\nc\n";
        let mut ed = editor(real);
        ed.manual = true;
        ed.result = "a\n<<<<<<< HEAD\nx\n=======\ny\nc\n".into();
        assert_eq!(ed.unresolved(), 1);
        ed.result = "a\nx\n=======\ny\nc\n".into();
        assert_eq!(ed.unresolved(), 1);
        assert!(
            ed.save(Path::new("/nonexistent"))
                .is_err_and(|e| e.contains("unresolved"))
        );
        ed.result = "a\nxy\nc\n".into();
        assert_eq!(ed.unresolved(), 0);
        // A side that has a marker-like line as text (a heading's underline) can be chosen.
        let heading = "<<<<<<< HEAD\nx\n=======\nTitles\n=======\n>>>>>>> b\n";
        let mut ed = editor(heading);
        ed.choose(0, Choice::Theirs);
        assert_eq!(ed.result, "Titles\n=======\n");
        assert_eq!(ed.unresolved(), 0);
    }

    /// Choosing after editing the result by hand keeps the edits, as "Back to choices" says.
    #[test]
    fn choices_wait_while_edited_by_hand() {
        let mut ed = editor(FILE);
        ed.choose(0, Choice::Ours);
        ed.manual = true;
        ed.result.push_str("my edit\n");
        let edited = ed.result.clone();
        ed.choose(1, Choice::Theirs);
        assert_eq!(ed.result, edited);
        assert_eq!(ed.choices, vec![Some(Choice::Ours), Some(Choice::Theirs)]);
    }

    #[test]
    fn solved_elsewhere_leaves_the_rest_to_choose() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-solved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let two =
            "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\nmid\n<<<<<<< HEAD\nc\n=======\nd\n>>>>>>> x\n";
        std::fs::write(dir.join("f"), two).map_err(|e| e.to_string())?;
        let mut ed = ConflictEditor::open(&dir, "f", None).ok_or("no conflicts")?;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ed.on_disk, 2);
        ed.choose(0, Choice::Ours);

        // One solved: the other is offered again, the earlier choice no longer applies.
        let one = "ab\nmid\n<<<<<<< HEAD\nc\n=======\nd\n>>>>>>> x\n";
        assert_eq!(ed.take_solved(one.into()), 1);
        assert_eq!(ed.choices, vec![None]);
        assert_eq!(ed.result, one);
        assert_eq!(ed.unresolved(), 1);
        ed.choose(0, Choice::Theirs);
        assert_eq!(ed.result, "ab\nmid\nd\n");

        // All solved: nothing left to choose, ready to save.
        assert_eq!(ed.take_solved("ab\nmid\ncd\n".into()), 0);
        assert_eq!(ed.unresolved(), 0);
        Ok(())
    }

    /// A merge leaves the other side's link where it conflicts with a file: the editor doesn't
    /// read (or later write) what it points to, which may never end (`/dev/zero`) or have
    /// conflict markers of its own.
    #[cfg(unix)]
    #[test]
    fn a_conflicted_link_is_not_followed() -> Result<(), String> {
        let dir =
            std::env::temp_dir().join(format!("gitcraft-conflict-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let marked = "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n";
        std::fs::write(dir.join("elsewhere"), marked).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("f"))
            .map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink("/dev/zero", dir.join("z")).map_err(|e| e.to_string())?;
        let (link, zero) = (
            ConflictEditor::open(&dir, "f", None),
            ConflictEditor::open(&dir, "z", None),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(link.is_none() && zero.is_none());
        Ok(())
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
