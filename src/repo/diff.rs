//! Central pane while a file is open: its diff, unified or side by side, with syntax colours,
//! the changed part of each line marked, and images shown before / after. Uncommitted changes
//! can be staged, unstaged or discarded a hunk or a few lines at a time.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use egui::{
    Align2, Color32, Pos2, Rect, Sense, Stroke, Ui, load::Bytes, pos2, text::CCursor,
    text::LayoutJob, text::TextFormat, vec2,
};

use super::highlight::{self, Spans};
use super::pr_review::{LineTarget, PrAct, paint_gutter_mark, target_of, unified_spot};
use super::widgets;
use super::{Action, DiffKey, Refresh, RepoTab, ops, short};
use crate::git::{self, Area, DiffOptions};
use crate::icons;
use crate::pull_request::DiffSide;
use crate::settings::Settings;
use crate::theme::{self, Tokens};

const LINE_H: f32 = 19.0;
const NUM_W: f32 = 46.0;
/// Width of the strip of changes beside the scrollbar.
const MAP_W: f32 = 12.0;
/// Very long diffs are cut here; the UI stays responsive and git is one click away.
const MAX_LINES: usize = 50_000;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Add,
    Del,
    Hunk,
    /// Binary notices, "No newline at end of file".
    Meta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// What is shown: the line without its `+`/`-` column, tabs expanded.
    pub text: String,
    /// The line exactly as git printed it, to build patches from.
    pub raw: String,
}

/// Everything the view needs, prepared on a background thread.
pub struct DiffData {
    pub lines: Vec<DiffLine>,
    /// Syntax colours per line (`None` when the file type isn't known).
    pub spans: Vec<Option<Spans>>,
    /// The changed part of a removed / added line paired with its counterpart (char range).
    pub changed: Vec<Option<(usize, usize)>>,
    /// Side-by-side rows: (left line, right line).
    pub split: Vec<(Option<usize>, Option<usize>)>,
    pub image: Option<ImagePair>,
    /// The patch's file header (`diff --git`, `index`, `---`, `+++`), for partial patches.
    pub header: String,
    /// Hunks and lines can be staged, unstaged or discarded on their own.
    pub partial: bool,
    /// Characters in the longest line, for the width to scroll (counted once, not every frame).
    longest: usize,
}

/// What to do with some hunks or lines of an uncommitted file's diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Unstaged lines into the index.
    Stage,
    /// Staged lines out of the index.
    Unstage,
    /// Unstaged lines out of the working tree.
    Discard,
}

impl Pick {
    /// The patch is applied with `--reverse`.
    fn reverse(self) -> bool {
        self != Pick::Stage
    }

    /// `git apply` arguments before the patch file.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Pick::Stage => &["apply", "--cached"],
            Pick::Unstage => &["apply", "--cached", "--reverse"],
            Pick::Discard => &["apply", "--reverse"],
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            Pick::Stage => "Stage",
            Pick::Unstage => "Unstage",
            Pick::Discard => "Discard",
        }
    }
}

pub struct ImagePair {
    pub old: Option<Arc<[u8]>>,
    pub new: Option<Arc<[u8]>>,
    /// Makes the image URIs unique to this diff.
    pub id: String,
}

/// Reads and prepares the diff for `key`.
pub fn load(repo: &Path, key: &DiffKey, opts: DiffOptions, dark: bool) -> Result<DiffData, String> {
    let (text, path) = match key {
        DiffKey::Commit { hash, path } => (git::file_diff(repo, hash, path, opts)?, path.clone()),
        DiffKey::Work(file) => (git::work_diff(repo, file, opts)?, file.path.clone()),
        DiffKey::Range { from, to, path } => (
            git::range_file_diff(repo, from, to.as_deref(), path, opts)?,
            path.clone(),
        ),
    };
    let lines = parse(&text);
    let header = patch_header(&text);
    // A patch made from a `-w` diff wouldn't apply; new, deleted, renamed and binary files, and
    // truncated diffs, only go whole.
    let partial = matches!(key, DiffKey::Work(f) if matches!(f.area, Area::Staged | Area::Unstaged))
        && !opts.ignore_ws
        && !header.is_empty()
        && ![
            "new file mode",
            "deleted file mode",
            "rename from",
            "copy from",
            "Binary files",
        ]
        .iter()
        .any(|h| header.contains(h))
        && lines
            .iter()
            .all(|l| l.kind != Kind::Meta || l.raw.starts_with('\\'));
    let image = if is_image(&path) {
        Some(load_images(repo, key, &path, renamed_from(&text)))
    } else {
        None
    };

    // Colour the old and the new side each as one stream, so multi-line constructs work.
    let mut spans = vec![None; lines.len()];
    let old_side: Vec<usize> = (0..lines.len())
        .filter(|&i| matches!(lines[i].kind, Kind::Context | Kind::Del))
        .collect();
    let new_side: Vec<usize> = (0..lines.len())
        .filter(|&i| matches!(lines[i].kind, Kind::Context | Kind::Add))
        .collect();
    if let Some(old) = highlight::highlight(
        &path,
        old_side.iter().map(|&i| lines[i].text.as_str()),
        dark,
    ) {
        for (k, &i) in old_side.iter().enumerate() {
            if lines[i].kind == Kind::Del {
                spans[i] = old.get(k).cloned();
            }
        }
    }
    if let Some(new) = highlight::highlight(
        &path,
        new_side.iter().map(|&i| lines[i].text.as_str()),
        dark,
    ) {
        for (k, &i) in new_side.iter().enumerate() {
            spans[i] = new.get(k).cloned();
        }
    }

    let (changed, split) = pair_lines(&lines);
    let longest = lines
        .iter()
        .map(|l| l.text.chars().count())
        .max()
        .unwrap_or(0);
    Ok(DiffData {
        lines,
        spans,
        changed,
        split,
        image,
        header,
        partial,
        longest,
    })
}

fn is_image(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()))
}

/// The old name of a renamed file, from its patch's header (unless git had to quote it).
fn renamed_from(header: &str) -> Option<&str> {
    header
        .lines()
        .find_map(|l| l.strip_prefix("rename from "))
        .filter(|p| !p.starts_with('"'))
}

/// The pictures before and after; the one before under its old name when the file was renamed.
fn load_images(repo: &Path, key: &DiffKey, path: &str, renamed_from: Option<&str>) -> ImagePair {
    let get = |rev: Option<&str>, path: &str| {
        git::file_at(repo, rev, path)
            .ok()
            .map(|b| Arc::from(b.into_boxed_slice()))
    };
    let old_path = match key {
        DiffKey::Work(file) => file.old_path.as_deref(),
        _ => None,
    }
    .or(renamed_from)
    .unwrap_or(path);
    let old = |rev: &str| get(Some(rev), old_path);
    let (old, new) = match key {
        DiffKey::Commit { hash, .. } => (old(&format!("{hash}^")), get(Some(hash), path)),
        DiffKey::Work(file) => match file.area {
            Area::Staged => (old("HEAD"), get(Some(""), path)),
            Area::Unstaged => (get(Some(""), path), get(None, path)),
            Area::Untracked | Area::Conflicted => (None, get(None, path)),
        },
        DiffKey::Range { from, to, .. } => (old(from), get(to.as_deref(), path)),
    };
    let id = image_id(old.as_deref(), new.as_deref());
    ImagePair { old, new, id }
}

/// Names the pictures by what they hold: egui keeps an image by its URI for good, so the same
/// name for new contents would show the old picture, and a new name for each read of the same
/// one would keep another copy every time the diff is read again.
fn image_id(old: Option<&[u8]>, new: Option<&[u8]>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (old, new).hash(&mut h);
    format!("{:016x}", h.finish())
}

type Split = Vec<(Option<usize>, Option<usize>)>;

/// Pairs each run of removed lines with the added lines after it: marks what changed inside
/// each pair, and lays out side-by-side rows.
fn pair_lines(lines: &[DiffLine]) -> (Vec<Option<(usize, usize)>>, Split) {
    let mut changed = vec![None; lines.len()];
    let mut split = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            Kind::Del | Kind::Add => {
                // A "\ No newline at end of file" note inside the run doesn't end it (the last
                // line changed): it gets its own row after the pairs.
                let mut notes = Vec::new();
                let mut run = |kind: Kind| {
                    let mut found = Vec::new();
                    while i < lines.len()
                        && matches!(lines[i].kind, k if k == kind || k == Kind::Meta)
                    {
                        if lines[i].kind == kind {
                            found.push(i);
                        } else {
                            notes.push(i);
                        }
                        i += 1;
                    }
                    found
                };
                let dels = run(Kind::Del);
                let adds = run(Kind::Add);
                for k in 0..dels.len().max(adds.len()) {
                    let (d, a) = (dels.get(k).copied(), adds.get(k).copied());
                    if let (Some(d), Some(a)) = (d, a)
                        && let Some((dr, ar)) = changed_ranges(&lines[d].text, &lines[a].text)
                    {
                        changed[d] = Some(dr);
                        changed[a] = Some(ar);
                    }
                    split.push((d, a));
                }
                split.extend(notes.into_iter().map(|n| (Some(n), Some(n))));
            }
            _ => {
                split.push((Some(i), Some(i)));
                i += 1;
            }
        }
    }
    (changed, split)
}

/// The differing middle of two lines (common prefix and suffix removed), as char ranges;
/// `None` when the lines have little in common (a rewrite, not an edit).
fn changed_ranges(old: &str, new: &str) -> Option<((usize, usize), (usize, usize))> {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let common = prefix + suffix;
    if common * 3 < a.len().max(b.len()) {
        return None;
    }
    Some(((prefix, a.len() - suffix), (prefix, b.len() - suffix)))
}

/// Parses a unified (or combined, for conflicts) diff of one file.
pub fn parse(text: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    // Combined diffs (`@@@`) prefix every line with one column per parent.
    let mut prefix = 1;
    // Not `lines()`: a `\r` before the newline belongs to the file's line.
    for line in text.split_terminator('\n') {
        if out.len() >= MAX_LINES {
            out.push(meta("… diff truncated"));
            break;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            let ats = line.chars().take_while(|c| *c == '@').count();
            prefix = ats.saturating_sub(1).max(1);
            (old, new) = hunk_start(line);
            out.push(DiffLine {
                kind: Kind::Hunk,
                old: None,
                new: None,
                text: line.trim_end_matches('\r').to_owned(),
                raw: line.to_owned(),
            });
            continue;
        }
        if !in_hunk {
            if line.starts_with("Binary files") {
                out.push(meta(line));
            }
            continue;
        }
        if line.starts_with('\\') {
            out.push(meta(line));
            continue;
        }
        if line.starts_with("diff ") {
            // Next file (shouldn't happen for a single path): back to the preamble.
            in_hunk = false;
            continue;
        }
        let cut = line
            .char_indices()
            .nth(prefix)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let (marks, body) = line.split_at(cut);
        let kind = if marks.contains('+') {
            Kind::Add
        } else if marks.contains('-') {
            Kind::Del
        } else {
            Kind::Context
        };
        let combined = prefix > 1;
        let (o, n) = match kind {
            Kind::Add => {
                new += 1;
                (None, Some(new))
            }
            Kind::Del => {
                old += 1;
                (Some(old), None)
            }
            _ => {
                old += 1;
                new += 1;
                (Some(old), Some(new))
            }
        };
        out.push(DiffLine {
            kind,
            old: if combined { None } else { o },
            new: if combined { None } else { n },
            text: body.trim_end_matches('\r').replace('\t', "    "),
            raw: line.to_owned(),
        });
    }
    out
}

fn meta(text: &str) -> DiffLine {
    DiffLine {
        kind: Kind::Meta,
        old: None,
        new: None,
        text: text.trim_end_matches('\r').to_owned(),
        raw: text.to_owned(),
    }
}

/// Everything before the first hunk of a one-file patch, each line ending in `\n`. A mode
/// change is left out: picking some lines shouldn't also stage, unstage or throw away a
/// `chmod`.
fn patch_header(text: &str) -> String {
    let mut header = String::new();
    for line in text.split_terminator('\n') {
        if line.starts_with("@@") {
            return header;
        }
        if line.starts_with("old mode ") || line.starts_with("new mode ") {
            continue;
        }
        header.push_str(line);
        header.push('\n');
    }
    // No hunk at all (binary, mode change only): nothing to pick from.
    String::new()
}

/// The lines of the hunk whose header is at `h`: up to the next hunk.
fn hunk_body(lines: &[DiffLine], h: usize) -> std::ops::Range<usize> {
    let end = lines[h + 1..]
        .iter()
        .position(|l| l.kind == Kind::Hunk)
        .map_or(lines.len(), |n| h + 1 + n);
    h + 1..end
}

/// The added and removed lines of the hunk whose header is at `h`.
pub fn hunk_changes(lines: &[DiffLine], h: usize) -> BTreeSet<usize> {
    hunk_body(lines, h)
        .filter(|&i| matches!(lines[i].kind, Kind::Add | Kind::Del))
        .collect()
}

/// `@@ -12,5 +14,7 @@ fn x` → (12, 14, " fn x").
fn hunk_numbers(header: &str) -> Option<(u32, u32, &str)> {
    let rest = header.strip_prefix("@@ -")?;
    let (ranges, tail) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let start = |r: &str| r.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?, tail))
}

/// One hunk of a partial patch being written.
#[derive(Default)]
struct HunkOut {
    body: String,
    old_n: i64,
    new_n: i64,
    changed: bool,
}

impl HunkOut {
    /// Writes line `i` as the patch needs it (or not at all), and the "\ No newline at end of
    /// file" note right after it if it has one.
    fn line(&mut self, lines: &[DiffLine], i: usize, picked: bool, reverse: bool) {
        let line = &lines[i];
        let as_context = match line.kind {
            Kind::Context => true,
            Kind::Add | Kind::Del if picked => false,
            // Left out: an addition only exists on the new side, a removal on the old one, so
            // each stays as context on its side and goes from the other.
            Kind::Add if reverse => true,
            Kind::Del if !reverse => true,
            _ => return,
        };
        if as_context {
            self.body.push(' ');
            self.body.push_str(line.raw.get(1..).unwrap_or_default());
            self.old_n += 1;
            self.new_n += 1;
        } else {
            self.body.push_str(&line.raw);
            self.changed = true;
            if line.kind == Kind::Add {
                self.new_n += 1;
            } else {
                self.old_n += 1;
            }
        }
        self.body.push('\n');
        if let Some(note) = lines.get(i + 1).filter(|l| l.kind == Kind::Meta) {
            self.body.push_str(&note.raw);
            self.body.push('\n');
        }
    }
}

/// A patch of `header` with only the `chosen` added / removed lines (indices into `lines`), for
/// `git apply` as `pick` runs it. What isn't chosen stays as it is: a removal left out becomes
/// context where the patch is checked against the old side, an addition left out where it is
/// checked against the new side (applied in reverse). `None` when no change is chosen.
pub fn partial_patch(
    header: &str,
    lines: &[DiffLine],
    chosen: &BTreeSet<usize>,
    pick: Pick,
) -> Option<String> {
    let reverse = pick.reverse();
    let mut patch = header.to_owned();
    // How far the hunks written so far moved the other side's line numbers.
    let mut shift: i64 = 0;
    let mut any = false;
    for h in (0..lines.len()).filter(|&i| lines[i].kind == Kind::Hunk) {
        let (old_start, new_start, tail) = hunk_numbers(&lines[h].raw)?;
        let mut out = HunkOut::default();
        let body = hunk_body(lines, h);
        let mut i = body.start;
        while i < body.end {
            match lines[i].kind {
                Kind::Del | Kind::Add => {
                    // A run of removals and the additions after it, written in pairs (like the
                    // side-by-side view pairs them): replacing the 2nd line of three keeps its
                    // place even when the others are left out.
                    let mut run = |kind: Kind| {
                        let mut found = Vec::new();
                        while i < body.end
                            && matches!(lines[i].kind, k if k == kind || k == Kind::Meta)
                        {
                            if lines[i].kind == kind {
                                found.push(i);
                            }
                            i += 1;
                        }
                        found
                    };
                    let dels = run(Kind::Del);
                    let adds = run(Kind::Add);
                    for k in 0..dels.len().max(adds.len()) {
                        for &j in dels.get(k).into_iter().chain(adds.get(k)) {
                            out.line(lines, j, chosen.contains(&j), reverse);
                        }
                    }
                }
                Kind::Context => {
                    out.line(lines, i, false, reverse);
                    i += 1;
                }
                // Notes go with their line.
                _ => i += 1,
            }
        }
        let HunkOut {
            body,
            old_n,
            new_n,
            changed,
        } = out;
        if !changed {
            continue;
        }
        any = true;
        // `git apply` places a hunk by the side it checks: the old one, the new one in reverse.
        let (old_start, new_start) = (i64::from(old_start), i64::from(new_start));
        let (o, n) = if reverse {
            (new_start - shift, new_start)
        } else {
            (old_start, old_start + shift)
        };
        shift += new_n - old_n;
        patch.push_str(&format!(
            "@@ -{},{old_n} +{},{new_n} @@{tail}\n",
            o.max(0),
            n.max(0)
        ));
        patch.push_str(&body);
    }
    any.then_some(patch)
}

/// `@@ -12,5 +14,7 @@` → (11, 13): the numbers before the first line of the hunk.
fn hunk_start(header: &str) -> (u32, u32) {
    let mut old = 0;
    let mut new = 0;
    // Only the ranges between the `@@`s: the function name after them can hold `-x` or `+x`.
    let ats = header.chars().take_while(|c| *c == '@').count();
    let rest = header.get(ats..).unwrap_or_default();
    let ranges = rest.split_once(" @@").map_or(rest, |(ranges, _)| ranges);
    for part in ranges.split_whitespace() {
        let num = |s: &str| {
            s.split(',')
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(1)
                .saturating_sub(1)
        };
        if let Some(rest) = part.strip_prefix('-') {
            old = num(rest);
        } else if let Some(rest) = part.strip_prefix('+') {
            new = num(rest);
        }
    }
    (old, new)
}

impl RepoTab {
    pub(super) fn diff_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(view) = &self.diff else { return };
        let idle = self.busy_op.is_none();

        // Header: file, where it comes from, options, actions, close.
        let (path, origin) = match &view.key {
            DiffKey::Commit { hash, path } => (
                path.clone(),
                format!("Commit {}", hash.get(..8).unwrap_or(hash)),
            ),
            DiffKey::Work(file) => (
                file.path.clone(),
                match file.area {
                    Area::Staged => "Staged changes".to_owned(),
                    Area::Unstaged => "Unstaged changes".to_owned(),
                    Area::Untracked => "New file".to_owned(),
                    Area::Conflicted => "Conflicted".to_owned(),
                },
            ),
            DiffKey::Range { from, to, path } => (path.clone(), self.range_label(from, to)),
        };
        let partial = matches!(&view.data, Some(Ok(d)) if d.partial) && idle;
        let chosen = view.chosen.len();
        let settings = Settings::get(ui.ctx());
        let (split, ignore_ws, whole_file) = (
            settings.diff_split,
            self.diff_ignore_ws,
            settings.diff_whole_file,
        );
        let mut clear = false;
        egui::Frame::new()
            .fill(t.chrome)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .stroke(Stroke::new(1.0, t.separator))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::inline(ui, "file-diff", 16.0, t.icon);
                    let title = ui.add(
                        egui::Label::new(egui::RichText::new(&path).font(theme::medium(13.0)))
                            .sense(Sense::click()),
                    );
                    let (rev, ignorable) = match &view.key {
                        DiffKey::Commit { hash, .. } => (Some(hash.as_str()), false),
                        DiffKey::Range { to: Some(to), .. } => (Some(to.as_str()), false),
                        DiffKey::Range { to: None, .. } => (None, false),
                        DiffKey::Work(_) => (None, true),
                    };
                    widgets::file_menu(&title, &path, rev, ignorable, acts);
                    widgets::chip(ui, &origin, t.text_dim);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                            acts.push(Action::CloseDiff);
                        }
                        if let DiffKey::Work(file) = &view.key {
                            let (label, args): (&str, Vec<&str>) = match file.area {
                                Area::Staged => {
                                    ("Unstage file", unstage_args(&self.refs.head, &file.path))
                                }
                                Area::Conflicted => {
                                    ("Mark resolved", vec!["add", "--", &file.path])
                                }
                                _ => ("Stage file", vec!["add", "--", &file.path]),
                            };
                            if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                                acts.push(Action::Ops {
                                    label: label.to_owned(),
                                    ops: ops(&[&args]),
                                    refresh: Refresh::Status,
                                });
                            }
                            // The lines clicked in the diff.
                            if partial && chosen > 0 {
                                ui.add_space(8.0);
                                let lines = if chosen == 1 { "line" } else { "lines" };
                                let picks: &[Pick] = if file.area == Area::Staged {
                                    &[Pick::Unstage]
                                } else {
                                    &[Pick::Stage, Pick::Discard]
                                };
                                for &pick in picks {
                                    let b = egui::Button::new(format!(
                                        "{} {chosen} {lines}",
                                        pick.verb()
                                    ));
                                    if ui.add(b).clicked() {
                                        acts.push(Action::PickLines {
                                            pick,
                                            lines: view.chosen.clone(),
                                        });
                                    }
                                }
                                if ui
                                    .button("Clear")
                                    .on_hover_text("Unselect the lines")
                                    .clicked()
                                {
                                    clear = true;
                                }
                            }
                        }
                        ui.add_space(8.0);
                        let ws = icons::small_button(
                            ui,
                            "pilcrow",
                            if ignore_ws {
                                "Show whitespace changes"
                            } else {
                                "Ignore whitespace changes"
                            },
                        );
                        if ignore_ws {
                            widgets::active_underline(ui, ws.rect, t.accent);
                        }
                        if ws.clicked() {
                            acts.push(Action::SetDiffOptions {
                                split,
                                ignore_ws: !ignore_ws,
                                whole_file,
                            });
                        }
                        let whole = icons::small_button(
                            ui,
                            "unfold-vertical",
                            if whole_file {
                                "Show only the changes"
                            } else {
                                "Show the whole file"
                            },
                        );
                        if whole_file {
                            widgets::active_underline(ui, whole.rect, t.accent);
                        }
                        if whole.clicked() {
                            acts.push(Action::SetDiffOptions {
                                split,
                                ignore_ws,
                                whole_file: !whole_file,
                            });
                        }
                        let s = icons::small_button(ui, "columns-2", "Side by side");
                        if split {
                            widgets::active_underline(ui, s.rect, t.accent);
                        }
                        if s.clicked() {
                            acts.push(Action::SetDiffOptions {
                                split: true,
                                ignore_ws,
                                whole_file,
                            });
                        }
                        let u = icons::small_button(ui, "rows-2", "Unified");
                        if !split {
                            widgets::active_underline(ui, u.rect, t.accent);
                        }
                        if u.clicked() {
                            acts.push(Action::SetDiffOptions {
                                split: false,
                                ignore_ws,
                                whole_file,
                            });
                        }
                    });
                });
            });

        let data = match &view.data {
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
        if let Some(images) = &data.image {
            image_view(ui, images);
            return;
        }
        if data.lines.is_empty() {
            ui.centered_and_justified(|ui| {
                let msg = if ignore_ws {
                    "No changes besides whitespace"
                } else {
                    "No textual changes"
                };
                ui.label(egui::RichText::new(msg).color(t.text_dim));
            });
            return;
        }

        // A file of the pull request shown: its lines take comments.
        let review = self.diff_review(&view.key);
        if review.as_ref().is_some_and(|r| r.target.is_some())
            && let Some(pr) = &mut self.pr_view
        {
            let draft = self.pr_drafts.entry(pr.number).or_default();
            egui::Panel::bottom("pr_line_comment")
                .frame(
                    egui::Frame::new()
                        .fill(t.card)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .stroke(Stroke::new(1.0, t.separator)),
                )
                .show(ui, |ui| {
                    // The line as it is in the pull request, to suggest a change to.
                    let current = draft
                        .target
                        .as_ref()
                        .filter(|t| t.side == DiffSide::Right)
                        .and_then(|t| {
                            data.lines
                                .iter()
                                .find(|l| {
                                    l.kind != Kind::Del && l.new.map(u64::from) == Some(t.line)
                                })
                                .and_then(|l| l.raw.get(1..))
                        });
                    super::pr_review::line_panel(ui, pr, draft, current, acts);
                    super::pr_view::only_web_links(ui.ctx());
                });
        }

        let font = theme::mono(12.0);
        let char_w = ui
            .painter()
            .layout_no_wrap("M".to_owned(), font.clone(), t.text)
            .size()
            .x;
        let longest = data.longest as f32;

        ui.spacing_mut().item_spacing.y = 0.0;
        let rows = if split {
            data.split.len()
        } else {
            data.lines.len()
        };
        // Which hunk actions the headers offer.
        let hunk_picks: &[Pick] = match &view.key {
            DiffKey::Work(f) if partial && f.area == Area::Staged => &[Pick::Unstage],
            DiffKey::Work(_) if partial => &[Pick::Stage, Pick::Discard],
            _ => &[],
        };
        let shift = ui.input(|i| i.modifiers.shift);
        // A changed line clicked: (line, extend the selection).
        let mut clicked: Option<usize> = None;
        // Text selection by dragging: where the press and the pointer are in the lines drawn.
        // Only a selection made in the layout shown: one made side by side isn't on screen
        // unified, and would be what Ctrl+C copies.
        let text_sel = view.text_sel.filter(|s| s.made_in(split));
        let (press, pointer) = ui.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos()));
        let mut press_hit: Option<(usize, Side, usize)> = None;
        // Each with whether the pointer is over that line's half, not only at its height.
        let mut pointer_hits: Vec<((usize, Side, usize), bool)> = Vec::new();
        let mut text_hover = false;
        let mut paint = |ui: &mut Ui, rect: Rect, i: usize, side: Side, scroll_x: f32| {
            let kind = data.lines.get(i).map(|l| l.kind);
            let pickable = partial && matches!(kind, Some(Kind::Add | Kind::Del));
            let column = match side {
                Side::Old => Some(DiffSide::Left),
                Side::New => Some(DiffSide::Right),
                Side::Unified | Side::Both => None,
            };
            let spot = data
                .lines
                .get(i)
                .filter(|_| side != Side::Both)
                .and_then(|l| match (&review, column) {
                    (Some(review), None) => unified_spot(l.kind, l.old, l.new, &review.marks),
                    _ => target_of(l.kind, l.old, l.new, column),
                });
            let active = spot.is_some_and(|(s, n)| {
                review
                    .as_ref()
                    .and_then(|r| r.target.as_ref())
                    .is_some_and(|t| t.side == s && t.line == n)
            });
            let picked = view.chosen.contains(&i) || active;
            // Hunk headers aren't copied, so they don't look selected either.
            let sel_chars = text_sel
                .and_then(|s| s.chars_of(i, side))
                .filter(|_| kind.is_some_and(|k| side.shows(k)));
            let placed = paint_line(
                ui, rect, data, i, side, scroll_x, &font, &t, picked, sel_chars,
            );
            // Where the drag started and where the pointer is, in this line's text.
            if let Some((origin, galley)) = placed.filter(|_| kind.is_some_and(|k| side.shows(k))) {
                let hit = |p: Pos2| (i, side, galley.cursor_from_pos(p - origin).index.0);
                if let Some(p) = press.filter(|p| rect.contains(*p)) {
                    press_hit = Some(hit(p));
                }
                if let Some(p) = pointer.filter(|p| rect.y_range().contains(p.y)) {
                    let over = rect.x_range().contains(p.x);
                    pointer_hits.push((hit(p), over));
                    text_hover |= over && !pickable;
                }
            }
            if let (Some(review), Some((spot_side, number))) = (&review, spot) {
                let gutter = if side == Side::Unified {
                    NUM_W * 2.0
                } else {
                    NUM_W
                };
                let area = Rect::from_min_size(rect.min, vec2(gutter, LINE_H));
                let resp = ui
                    .interact(
                        area,
                        ui.id().with(("diff_gutter", i, column)),
                        Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Comment on this line");
                let count = review.marks.get(&(spot_side, number)).copied();
                paint_gutter_mark(ui, area, count, resp.hovered(), active);
                if resp.clicked() {
                    acts.push(Action::Pr(PrAct::OpenLine(LineTarget {
                        path: review.path.clone(),
                        line: number,
                        side: spot_side,
                    })));
                }
            }
            if pickable {
                let resp = ui
                    .interact(rect, ui.id().with(("diff_line", i)), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
            if kind == Some(Kind::Hunk) && !hunk_picks.is_empty() {
                // At the visible right end, however wide the line is.
                let mut right = rect.right().min(ui.clip_rect().right()) - 6.0;
                for &pick in hunk_picks.iter().rev() {
                    let (resp, left) = hunk_button(ui, right, rect.center().y, pick, i);
                    if resp.clicked() {
                        acts.push(Action::PickLines {
                            pick,
                            lines: hunk_changes(&data.lines, i),
                        });
                    }
                    right = left - 4.0;
                }
            }
        };
        // Every change of the file in a strip beside the scrollbar, to see where to scroll to
        // (above all when the whole file is shown). Clicking or dragging it scrolls there.
        let runs = if split {
            change_runs(data.split.iter().map(|&(l, r)| {
                let kind = |i: Option<usize>| i.and_then(|i| data.lines.get(i)).map(|x| x.kind);
                (kind(l) == Some(Kind::Del), kind(r) == Some(Kind::Add))
            }))
        } else {
            change_runs(
                data.lines
                    .iter()
                    .map(|l| (l.kind == Kind::Del, l.kind == Kind::Add)),
            )
        };
        let area = ui.available_rect_before_wrap();
        let strip = Rect::from_min_max(pos2(area.right() - MAP_W, area.top()), area.max);
        let body = Rect::from_min_max(area.min, pos2(strip.left() - 1.0, area.bottom()));
        let total_h = rows as f32 * LINE_H;
        let map = ui
            .interact(
                strip,
                ui.id().with(("diff_map", &path)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        // Under the lines, so dragging over them selects text (a click still picks a line).
        let text_drag = ui.interact(
            body,
            ui.id().with(("diff_text", &path)),
            Sense::click_and_drag(),
        );
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("diff_scroll", &path, split))
            .auto_shrink(false);
        if (map.clicked() || map.dragged())
            && let Some(p) = map.interact_pointer_pos()
        {
            let at = ((p.y - strip.top()) / strip.height()).clamp(0.0, 1.0) * total_h;
            scroll = scroll.vertical_scroll_offset((at - body.height() / 2.0).max(0.0));
        }
        let out = ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            scroll.show_rows(ui, LINE_H, rows, |ui, range| {
                if split {
                    // Each half is half of what's visible, however long the lines: scrolling
                    // sideways moves the text in both, the line numbers stay.
                    let view = ui.clip_rect();
                    let half = (view.width() / 2.0).floor();
                    let overflow = (NUM_W + 24.0 + longest * char_w + 16.0 - half).max(0.0);
                    for row in range {
                        let Some(&(l, r)) = data.split.get(row) else {
                            continue;
                        };
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(half * 2.0 + overflow, LINE_H),
                            Sense::hover(),
                        );
                        let scroll_x = (view.left() - rect.left()).clamp(0.0, overflow);
                        let left = Rect::from_min_size(
                            pos2(rect.left() + scroll_x, rect.top()),
                            vec2(half, LINE_H),
                        );
                        let right =
                            Rect::from_min_size(pos2(left.right(), rect.top()), vec2(half, LINE_H));
                        // Hunk headers and notes span both halves.
                        if l == r
                            && let Some(i) = l
                            && matches!(
                                data.lines.get(i).map(|x| x.kind),
                                Some(Kind::Hunk | Kind::Meta)
                            )
                        {
                            paint(ui, left.union(right), i, Side::Both, 0.0);
                            continue;
                        }
                        match l {
                            Some(i) => paint(ui, left, i, Side::Old, scroll_x),
                            None => filler(ui, left, &t),
                        }
                        match r {
                            Some(i) => paint(ui, right, i, Side::New, scroll_x),
                            None => filler(ui, right, &t),
                        }
                        ui.painter().line_segment(
                            [right.left_top(), right.left_bottom()],
                            Stroke::new(1.0, t.separator),
                        );
                    }
                } else {
                    let width =
                        (NUM_W * 2.0 + 24.0 + longest * char_w + 24.0).max(ui.available_width());
                    for i in range {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, LINE_H), Sense::hover());
                        paint(ui, rect, i, Side::Unified, 0.0);
                    }
                }
            })
        });
        let shown = out.inner;
        change_map(
            ui,
            strip,
            &runs,
            split,
            (shown.state.offset.y, shown.inner_rect.height(), total_h),
            &t,
        );

        // Select from where the drag started to the pointer, on the side it started on.
        let mut sel = text_sel;
        if text_drag.drag_started() {
            sel = press_hit.map(|(i, side, c)| TextSel {
                side,
                anchor: (i, c),
                cursor: (i, c),
            });
        } else if text_drag.dragged()
            && let Some(s) = &mut sel
            && let Some(((i, _, c), _)) = pointer_hits
                .iter()
                .find(|((_, side, _), _)| *side == s.side)
        {
            s.cursor = (*i, *c);
        } else if text_drag.clicked() {
            sel = None;
        }
        if text_hover || text_drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        let selected_text = sel.map(|s| s.text(&data.lines)).filter(|s| !s.is_empty());
        if let Some(text) = &selected_text {
            let copy = !ui.ctx().egui_wants_keyboard_input()
                && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            if copy {
                ui.ctx().copy_text(text.clone());
            }
        }
        egui::Popup::context_menu(&text_drag)
            .style(crate::theme::menu_style)
            .show(|ui| {
                if ui
                    .add_enabled(selected_text.is_some(), egui::Button::new("Copy"))
                    .clicked()
                    && let Some(text) = &selected_text
                {
                    ui.ctx().copy_text(text.clone());
                }
            });

        if let Some(view) = &mut self.diff {
            view.text_sel = sel;
            if clear {
                view.chosen.clear();
                view.anchor = None;
            }
            if let Some(i) = clicked {
                view.pick_line(i, shift);
            }
        }
    }

    /// "abc1234 → def5678" for a diff between two commits, with the names of the comparison.
    fn range_label(&self, from: &str, to: &Option<String>) -> String {
        let name = |rev: &str| {
            self.compare
                .as_ref()
                .and_then(|c| c.name_of(rev))
                .or_else(|| self.pr_name_of(rev))
                .unwrap_or_else(|| short(rev).to_owned())
        };
        match to {
            Some(to) => format!("{} → {}", name(from), name(to)),
            None => format!("{} → working tree", name(from)),
        }
    }
}

impl super::DiffView {
    /// Click on a changed line: toggles it, or with Shift selects every changed line from the
    /// last one clicked.
    fn pick_line(&mut self, i: usize, extend: bool) {
        let Some(Ok(data)) = &self.data else { return };
        match self.anchor {
            Some(a) if extend => {
                let (lo, hi) = (a.min(i), a.max(i));
                self.chosen.extend(
                    (lo..=hi).filter(|&k| matches!(data.lines[k].kind, Kind::Add | Kind::Del)),
                );
            }
            _ => {
                if !self.chosen.remove(&i) {
                    self.chosen.insert(i);
                }
                self.anchor = Some(i);
            }
        }
    }
}

/// A small text button in a hunk header, its right edge at `right`; returns it and its left edge.
fn hunk_button(ui: &mut Ui, right: f32, y: f32, pick: Pick, hunk: usize) -> (egui::Response, f32) {
    let t = Tokens::get(ui.ctx());
    let label = format!("{} hunk", pick.verb());
    let color = if pick == Pick::Discard {
        t.text_dim
    } else {
        t.accent
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label, theme::medium(11.0), color);
    let r = Rect::from_min_max(
        pos2(right - galley.size().x - 14.0, y - 9.0),
        pos2(right, y + 9.0),
    );
    let resp = ui.interact(
        r,
        ui.id().with(("hunk_btn", hunk, pick.verb())),
        Sense::click(),
    );
    let fill = if resp.hovered() { t.hover } else { t.dock };
    ui.painter().rect_filled(r, t.radius_sm, fill);
    ui.painter().rect_stroke(
        r,
        t.radius_sm,
        Stroke::new(1.0, t.separator),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(r.left() + 7.0, y - galley.size().y / 2.0),
        galley,
        color,
    );
    let tip = match pick {
        Pick::Stage => {
            "Stage this hunk. Click lines to stage only some of them (Shift+click for a range)."
        }
        Pick::Unstage => {
            "Unstage this hunk. Click lines to unstage only some of them (Shift+click for a range)."
        }
        Pick::Discard => "Throw away this hunk's changes (Ctrl+Z brings them back).",
    };
    (resp.on_hover_text(tip), r.left())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Unified,
    Old,
    New,
    /// A hunk header across both halves of the split view.
    Both,
}

impl Side {
    /// Whether a line of `kind` shows on this side.
    fn shows(self, kind: Kind) -> bool {
        match self {
            Side::Unified => matches!(kind, Kind::Context | Kind::Add | Kind::Del),
            Side::Old => matches!(kind, Kind::Context | Kind::Del),
            Side::New => matches!(kind, Kind::Context | Kind::Add),
            Side::Both => false,
        }
    }
}

/// Text selected in a diff by dragging, on one side of it: from where the drag started to
/// where the pointer is, as (line, character) positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TextSel {
    side: Side,
    anchor: (usize, usize),
    cursor: (usize, usize),
}

impl TextSel {
    /// Whether it was made in the side-by-side view (`split`) or the unified one.
    fn made_in(&self, split: bool) -> bool {
        (self.side == Side::Unified) != split
    }

    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    /// The characters of line `i` selected where it shows on `side` (`usize::MAX`: through
    /// the end of the line, line break included).
    fn chars_of(&self, i: usize, side: Side) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if side != self.side || i < start.0 || i > end.0 {
            return None;
        }
        let from = if i == start.0 { start.1 } else { 0 };
        let to = if i == end.0 { end.1 } else { usize::MAX };
        (from < to).then_some(from..to)
    }

    /// The selected text, one line of the side per line.
    fn text(&self, lines: &[DiffLine]) -> String {
        let (start, end) = self.ordered();
        let mut out: Vec<String> = Vec::new();
        for i in start.0..=end.0 {
            let Some(line) = lines.get(i).filter(|l| self.side.shows(l.kind)) else {
                continue;
            };
            let from = if i == start.0 { start.1 } else { 0 };
            let to = if i == end.0 { end.1 } else { usize::MAX };
            out.push(
                line.text
                    .chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect(),
            );
        }
        out.join("\n")
    }
}

fn filler(ui: &Ui, rect: Rect, t: &Tokens) {
    ui.painter()
        .rect_filled(rect, 0.0, t.card.gamma_multiply(0.35));
}

/// The rows of removed and of added lines, in runs: from flags (removed, added) per row.
#[derive(Debug, Default, PartialEq)]
struct ChangeRuns {
    removed: Vec<Range<usize>>,
    added: Vec<Range<usize>>,
}

fn change_runs(rows: impl Iterator<Item = (bool, bool)>) -> ChangeRuns {
    let mut runs = ChangeRuns::default();
    let extend = |list: &mut Vec<Range<usize>>, row: usize| match list.last_mut() {
        Some(last) if last.end == row => last.end = row + 1,
        _ => list.push(row..row + 1),
    };
    for (row, (removed, added)) in rows.enumerate() {
        if removed {
            extend(&mut runs.removed, row);
        }
        if added {
            extend(&mut runs.added, row);
        }
    }
    runs
}

/// The strip beside the diff's scrollbar: a mark for each run of changes where it sits in the
/// file (side by side: removals on the left half, additions on the right), and the part on
/// screen. `view`: (scroll offset, visible height, content height).
fn change_map(
    ui: &Ui,
    strip: Rect,
    runs: &ChangeRuns,
    split: bool,
    view: (f32, f32, f32),
    t: &Tokens,
) {
    let p = ui.painter_at(strip);
    p.rect_filled(strip, 0.0, t.chrome);
    let (offset, visible, total) = view;
    if total <= 0.0 {
        return;
    }
    let y = |px: f32| strip.top() + px / total * strip.height();
    let rows = total / LINE_H;
    let span = |run: &Range<usize>, left: f32, right: f32| {
        let top = y(run.start as f32 * LINE_H);
        // Thin runs stay visible however long the file.
        let bottom = y(run.end as f32 * LINE_H).max(top + 2.0);
        Rect::from_min_max(pos2(left, top), pos2(right, bottom))
    };
    let mid = if split {
        strip.center().x
    } else {
        strip.right()
    };
    let added_left = if split { mid } else { strip.left() };
    for run in &runs.removed {
        p.rect_filled(span(run, strip.left(), mid), 0.0, t.danger);
    }
    for run in &runs.added {
        p.rect_filled(span(run, added_left, strip.right()), 0.0, t.success);
    }
    // The part on screen, when there is more than fits.
    if visible < total && rows > 0.0 {
        let seen = Rect::from_min_max(
            pos2(strip.left(), y(offset)),
            pos2(strip.right(), y(offset + visible).min(strip.bottom())),
        );
        p.rect_filled(seen, 0.0, t.text.gamma_multiply(0.12));
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_line(
    ui: &Ui,
    rect: Rect,
    data: &DiffData,
    i: usize,
    side: Side,
    // How far the text is scrolled sideways inside `rect` (the numbers stay).
    scroll_x: f32,
    font: &egui::FontId,
    t: &Tokens,
    selected: bool,
    // Characters of the text selected by dragging.
    text_sel: Option<Range<usize>>,
) -> Option<(Pos2, Arc<egui::Galley>)> {
    let line = data.lines.get(i)?;
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let (bg, fg) = match line.kind {
        Kind::Add => (Some(t.diff_add_bg), t.text),
        Kind::Del => (Some(t.diff_del_bg), t.text),
        Kind::Hunk => (Some(t.accent.gamma_multiply(0.12)), t.accent),
        Kind::Meta => (None, t.text_faint),
        Kind::Context => (None, t.text_dim),
    };
    if let Some(bg) = bg {
        p.rect_filled(rect, 0.0, bg);
    }
    // Picked to stage / unstage / discard: tinted, with a bar on the left.
    if selected {
        p.rect_filled(rect, 0.0, t.accent.gamma_multiply(0.22));
    }
    let y = rect.center().y;
    let numbers: &[(Option<u32>, f32)] = match side {
        Side::Unified => &[(line.old, NUM_W - 8.0), (line.new, NUM_W * 2.0 - 8.0)],
        Side::Old => &[(line.old, NUM_W - 8.0)],
        Side::New => &[(line.new, NUM_W - 8.0)],
        Side::Both => &[],
    };
    let gutter = match side {
        Side::Unified => NUM_W * 2.0,
        Side::Both => 0.0,
        _ => NUM_W,
    };
    if gutter > 0.0 {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(gutter, LINE_H)),
            0.0,
            t.card.gamma_multiply(0.6),
        );
    }
    if selected {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(3.0, LINE_H)),
            0.0,
            t.accent,
        );
    }
    for &(n, x) in numbers {
        if let Some(n) = n {
            p.text(
                pos2(rect.left() + x, y),
                Align2::RIGHT_CENTER,
                n.to_string(),
                theme::mono(11.0),
                t.text_faint,
            );
        }
    }
    let marker = match line.kind {
        Kind::Add => "+",
        Kind::Del => "−",
        _ => "",
    };
    let mark_color = if line.kind == Kind::Add {
        t.success
    } else {
        t.danger
    };
    p.text(
        pos2(rect.left() + gutter + 8.0, y),
        Align2::LEFT_CENTER,
        marker,
        font.clone(),
        mark_color,
    );

    // Text: syntax colours when known, the changed part on a stronger background.
    let strong = match line.kind {
        Kind::Add => t.success.gamma_multiply(0.35),
        Kind::Del => t.danger.gamma_multiply(0.35),
        _ => Color32::TRANSPARENT,
    };
    let changed = data.changed.get(i).copied().flatten();
    let spans: Vec<(Color32, &str)> = match (line.kind, data.spans.get(i).and_then(|s| s.as_ref()))
    {
        (Kind::Add | Kind::Del | Kind::Context, Some(spans)) => {
            spans.iter().map(|(c, s)| (*c, s.as_str())).collect()
        }
        _ => vec![(fg, line.text.as_str())],
    };
    let job = line_job(&spans, changed, font, strong);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let x = rect.left() + gutter + 24.0;
    // Scrolled text goes under the marker column, not over the numbers.
    let p = if scroll_x > 0.0 {
        p.with_clip_rect(p.clip_rect().intersect(rect.with_min_x(x - 4.0)))
    } else {
        p
    };
    let origin = pos2(x - scroll_x, y - galley.size().y / 2.0);
    if let Some(chars) = text_sel {
        let at = |c: usize| origin.x + galley.pos_from_cursor(CCursor::new(c)).min.x;
        let left = at(chars.start);
        // Selected through the end: a little past the text, for the line break.
        let right = if chars.end == usize::MAX {
            origin.x + galley.size().x + 6.0
        } else {
            at(chars.end)
        };
        p.rect_filled(
            Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
            0.0,
            t.accent.gamma_multiply(0.35),
        );
    }
    p.galley(origin, galley.clone(), fg);
    Some((origin, galley))
}

/// A layout job for `spans`, with the chars in `changed` on `strong`.
fn line_job(
    spans: &[(Color32, &str)],
    changed: Option<(usize, usize)>,
    font: &egui::FontId,
    strong: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut pos = 0usize;
    for (color, text) in spans {
        let n = text.chars().count();
        let (start, end) = (pos, pos + n);
        // Split the span at the changed range's edges.
        let cuts: Vec<usize> = match changed {
            Some((a, b)) => vec![start, a.clamp(start, end), b.clamp(start, end), end],
            None => vec![start, end],
        };
        for w in cuts.windows(2) {
            let (from, to) = (w[0], w[1]);
            if to <= from {
                continue;
            }
            let piece: String = text.chars().skip(from - start).take(to - from).collect();
            let inside = changed.is_some_and(|(a, b)| from >= a && to <= b);
            job.append(
                &piece,
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: *color,
                    background: if inside { strong } else { Color32::TRANSPARENT },
                    ..Default::default()
                },
            );
        }
        pos = end;
    }
    job
}

fn image_view(ui: &mut Ui, images: &ImagePair) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(12.0);
    ui.columns(2, |cols| {
        for (col, (label, bytes, side)) in cols.iter_mut().zip([
            ("Before", &images.old, "old"),
            ("After", &images.new, "new"),
        ]) {
            col.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(label)
                        .font(theme::semibold(12.0))
                        .color(t.text_dim),
                );
                ui.add_space(6.0);
                match bytes {
                    Some(bytes) => {
                        let uri = format!("bytes://diff/{}/{side}.png", images.id);
                        let max = vec2(ui.available_width() - 16.0, ui.available_height() - 40.0);
                        ui.add(
                            egui::Image::from_bytes(uri, Bytes::Shared(bytes.clone()))
                                .max_size(max)
                                .fit_to_original_size(1.0)
                                .bg_fill(t.field),
                        );
                        ui.label(
                            egui::RichText::new(format!("{} KB", bytes.len().div_ceil(1024)))
                                .color(t.text_faint)
                                .small(),
                        );
                    }
                    None => {
                        let note = if side == "old" {
                            "(new file)"
                        } else {
                            "(deleted)"
                        };
                        ui.label(egui::RichText::new(note).color(t.text_faint));
                    }
                }
            });
        }
    });
}

/// `git restore --staged`, or `git rm --cached` before the first commit (no HEAD to restore from).
pub(super) fn unstage_args<'a>(head: &str, path: &'a str) -> Vec<&'a str> {
    if head.is_empty() {
        vec!["rm", "--cached", "-q", "--", path]
    } else {
        vec!["restore", "--staged", "--", path]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_group_into_runs_per_kind() {
        let (n, d, a, both) = ((false, false), (true, false), (false, true), (true, true));
        let runs = change_runs([n, d, d, a, n, both, a].into_iter());
        assert_eq!(runs.removed, vec![1..3, 5..6]);
        assert_eq!(runs.added, vec![3..4, 5..7]);
    }

    #[test]
    fn selected_text_follows_the_side_it_was_made_on() {
        // 0 hunk, 1 " a", 2 "-let x = 1;", 3 "-gone", 4 "+let x = 2;", 5 " b"
        let lines = parse("@@ -1,3 +1,3 @@\n a\n-let x = 1;\n-gone\n+let x = 2;\n b\n");
        let sel = |side, anchor, cursor| TextSel {
            side,
            anchor,
            cursor,
        };
        // Dragged backwards, from "let x" in the removal to the end of "a".
        let unified = sel(Side::Unified, (2, 5), (1, 1));
        assert_eq!(unified.text(&lines), "\nlet x");
        // On the new side the removals are not there.
        let new = sel(Side::New, (1, 0), (5, 1));
        assert_eq!(new.text(&lines), "a\nlet x = 2;\nb");
        assert_eq!(new.chars_of(4, Side::New), Some(0..usize::MAX));
        assert_eq!(new.chars_of(4, Side::Old), None);
        assert_eq!(new.chars_of(5, Side::New), Some(0..1));
        // Pressed and not moved: nothing.
        assert_eq!(sel(Side::Old, (2, 3), (2, 3)).chars_of(2, Side::Old), None);
    }

    #[test]
    fn numbers_follow_hunk_headers() {
        let text = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -3,3 +3,4 @@ fn x\n a\n-b\n+c\n+d\n e\n\\ No newline at end of file\n";
        let lines = parse(text);
        let summary: Vec<_> = lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            summary,
            vec![
                (Kind::Hunk, None, None),
                (Kind::Context, Some(3), Some(3)),
                (Kind::Del, Some(4), None),
                (Kind::Add, None, Some(4)),
                (Kind::Add, None, Some(5)),
                (Kind::Context, Some(5), Some(6)),
                (Kind::Meta, None, None),
            ]
        );
        assert_eq!(lines[3].text, "c");
    }

    /// The function name git prints after a hunk's ranges can hold `-` and `+` words.
    #[test]
    fn numbers_ignore_the_hunk_headers_function_name() {
        assert_eq!(hunk_start("@@ -13,7 +14,7 @@ Hello - world +x"), (12, 13));
        assert_eq!(hunk_start("@@ -1 +1 @@"), (0, 0));
        assert_eq!(hunk_start("@@@ -1,1 -2,1 +3,5 @@@ a -9"), (1, 2));
        let lines = parse("@@ -13,2 +13,2 @@ int f(int a, -1)\n a\n-b\n+c\n");
        assert_eq!(lines[1].old, Some(13));
        assert_eq!(lines[2].old, Some(14));
        assert_eq!(lines[3].new, Some(14));
    }

    /// The last line changed in a file without a final newline: its old and new versions are
    /// still side by side, the note after them.
    #[test]
    fn no_newline_note_keeps_the_last_lines_paired() {
        let lines = parse(
            "@@ -1,2 +1,2 @@\n a\n-let x = 1;\n\\ No newline at end of file\n+let x = 2;\n\\ No newline at end of file\n",
        );
        let (changed, split) = pair_lines(&lines);
        assert_eq!(
            split,
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(1)),
                (Some(2), Some(4)),
                (Some(3), Some(3)),
                (Some(5), Some(5)),
            ]
        );
        assert_eq!(changed[2], Some((8, 9)));
        assert_eq!(changed[4], Some((8, 9)));
    }

    #[test]
    fn selection_belongs_to_the_layout_it_was_made_in() {
        let sel = |side| TextSel {
            side,
            anchor: (1, 0),
            cursor: (2, 1),
        };
        assert!(sel(Side::Unified).made_in(false));
        assert!(!sel(Side::Unified).made_in(true));
        assert!(sel(Side::Old).made_in(true));
        assert!(!sel(Side::New).made_in(false));
    }

    #[test]
    fn images_are_named_by_their_contents() {
        let (a, b): (&[u8], &[u8]) = (b"one", b"two");
        assert_eq!(image_id(Some(a), Some(b)), image_id(Some(a), Some(b)));
        assert_ne!(image_id(Some(a), Some(b)), image_id(Some(a), Some(a)));
        assert_ne!(image_id(None, Some(a)), image_id(Some(a), None));
    }

    #[test]
    fn combined_conflict_diff() {
        let text = "@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<< HEAD\n +ours\n  same\n";
        let lines = parse(text);
        assert_eq!(lines[1].kind, Kind::Add);
        assert_eq!(lines[1].text, "<<<<<<< HEAD");
        assert_eq!(lines[3].kind, Kind::Context);
        assert_eq!(lines[3].old, None);
    }

    #[test]
    fn binary_notice() {
        let lines = parse("diff --git a/x b/x\nBinary files a/x and b/x differ\n");
        assert_eq!(lines, vec![meta("Binary files a/x and b/x differ")]);
    }

    #[test]
    fn side_by_side_pairs_and_inline_changes() {
        let lines = parse("@@ -1,3 +1,3 @@\n a\n-let x = 1;\n-gone\n+let x = 2;\n b\n");
        let (changed, split) = pair_lines(&lines);
        // hunk, a, (del x1, add x2), (del gone, —), b
        assert_eq!(
            split,
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(1)),
                (Some(2), Some(4)),
                (Some(3), None),
                (Some(5), Some(5))
            ]
        );
        assert_eq!(changed[2], Some((8, 9)));
        assert_eq!(changed[4], Some((8, 9)));
        assert_eq!(changed[3], None);
    }

    #[test]
    fn rewrites_are_not_marked_inline() {
        assert_eq!(changed_ranges("completely", "different words"), None);
        assert_eq!(changed_ranges("abc", "abXc"), Some(((2, 2), (2, 3))));
    }

    const PATCH: &str = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n\
        @@ -1,4 +1,4 @@ top\n a\n-b\n-c\n+B\n+C\n d\n\
        @@ -10,2 +10,3 @@\n x\n+y\n z\n";

    /// The hunks of a partial patch, without the file header.
    fn hunks(lines: &[DiffLine], chosen: &[usize], pick: Pick) -> Option<String> {
        let header = patch_header(PATCH);
        let chosen: BTreeSet<usize> = chosen.iter().copied().collect();
        partial_patch(&header, lines, &chosen, pick)
            .and_then(|p| p.strip_prefix(&header).map(str::to_owned))
    }

    #[test]
    fn partial_patch_keeps_only_the_chosen_lines() {
        let lines = parse(PATCH);
        assert_eq!(patch_header(PATCH).lines().count(), 4);
        // Indices: 0 hunk, 1 a, 2 -b, 3 -c, 4 +B, 5 +C, 6 d, 7 hunk, 8 x, 9 +y, 10 z.
        // b → B only: c stays where it was, after B.
        assert_eq!(
            hunks(&lines, &[2, 4], Pick::Stage).as_deref(),
            Some("@@ -1,4 +1,4 @@ top\n a\n-b\n+B\n c\n d\n")
        );
        // In reverse an addition left out stays as context and a removal left out goes.
        assert_eq!(
            hunks(&lines, &[2, 4], Pick::Discard).as_deref(),
            Some("@@ -1,4 +1,4 @@ top\n a\n-b\n+B\n C\n d\n")
        );
        // The second hunk alone, after the first moved nothing.
        let second: Vec<usize> = hunk_changes(&lines, 7).into_iter().collect();
        assert_eq!(
            hunks(&lines, &second, Pick::Stage).as_deref(),
            Some("@@ -10,2 +10,3 @@\n x\n+y\n z\n")
        );
        assert_eq!(hunks(&lines, &[], Pick::Stage), None);
    }

    #[test]
    fn later_hunks_follow_the_lines_earlier_ones_add() {
        let lines = parse(PATCH);
        // Only the additions of the first hunk (+2 lines), and the second hunk.
        let patch = hunks(&lines, &[4, 5, 9], Pick::Stage).unwrap_or_default();
        assert!(patch.starts_with("@@ -1,4 +1,6 @@ top\n a\n b\n+B\n c\n+C\n d\n"));
        assert!(patch.contains("@@ -10,2 +12,3 @@\n"));
    }

    #[test]
    fn no_newline_note_follows_its_line() {
        let text = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n";
        let lines = parse(text);
        let header = patch_header(text);
        // Only the removal of the last line: its note goes with it, the addition is left out.
        let chosen: BTreeSet<usize> = [2].into();
        let patch = partial_patch(&header, &lines, &chosen, Pick::Stage).unwrap_or_default();
        assert!(patch.ends_with("@@ -1,2 +1,1 @@\n a\n-b\n\\ No newline at end of file\n"));
    }

    #[test]
    fn crlf_lines_keep_their_carriage_return() {
        let lines = parse("@@ -1 +1 @@\n-a\r\n+b\r\n");
        assert_eq!(lines[1].text, "a");
        assert_eq!(lines[2].raw, "+b\r");
    }

    #[test]
    fn partial_patches_apply_with_git() {
        partial_patches_apply(false);
    }

    #[test]
    fn partial_patches_of_the_whole_file_apply_with_git() {
        partial_patches_apply(true);
    }

    /// Stages, unstages and discards a few lines in a real repository.
    fn partial_patches_apply(whole_file: bool) {
        let dir = std::env::temp_dir().join(format!(
            "gitcraft-partial-{}-{whole_file}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["config", "core.autocrlf", "false"],
        ] {
            assert!(git(args).is_ok());
        }
        let file = dir.join("f.txt");
        let base: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let _ = std::fs::write(&file, &base);
        assert!(git(&["add", "."]).is_ok());
        assert!(git(&["commit", "-q", "-m", "base"]).is_ok());
        let edited = base
            .replace("line 2\n", "line two\n")
            .replace("line 3\n", "line three\n")
            .replace("line 18\n", "line 18\nline 18b\n");
        let _ = std::fs::write(&file, &edited);

        let work = |area| git::StatusFile {
            path: "f.txt".to_owned(),
            old_path: None,
            code: 'M',
            area,
        };
        let pick = |area, which: &dyn Fn(&DiffLine) -> bool, how: Pick| -> Result<(), String> {
            let opts = DiffOptions {
                whole_file,
                ..DiffOptions::default()
            };
            let data = load(&dir, &DiffKey::Work(work(area)), opts, true)?;
            assert!(data.partial);
            let chosen = (0..data.lines.len())
                .filter(|&i| which(&data.lines[i]))
                .collect();
            let patch =
                partial_patch(&data.header, &data.lines, &chosen, how).ok_or("nothing picked")?;
            let path = git::save_patch(&patch)?.to_string_lossy().into_owned();
            let mut args = how.args().to_vec();
            args.push(&path);
            git(&args).map(|_| ())
        };
        let index = || git(&["show", ":f.txt"]).unwrap_or_default();
        // Stage line 2 → "line two" only.
        let second = |l: &DiffLine| l.text == "line 2" || l.text == "line two";
        assert_eq!(pick(Area::Unstaged, &second, Pick::Stage), Ok(()));
        assert_eq!(index(), base.replace("line 2\n", "line two\n"));
        // Discard the line added near the end.
        let added = |l: &DiffLine| l.text == "line 18b";
        assert_eq!(pick(Area::Unstaged, &added, Pick::Discard), Ok(()));
        let on_disk = std::fs::read_to_string(&file).unwrap_or_default();
        assert_eq!(
            on_disk,
            base.replace("line 2\n", "line two\n")
                .replace("line 3\n", "line three\n")
        );
        // Unstage the addition but keep the removal staged.
        let two = |l: &DiffLine| l.text == "line two";
        assert_eq!(pick(Area::Staged, &two, Pick::Unstage), Ok(()));
        assert_eq!(index(), base.replace("line 2\n", ""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file made executable and edited: picking lines leaves the mode change where it is.
    #[cfg(unix)]
    #[test]
    fn partial_patches_leave_a_mode_change_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("gitcraft-partial-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["config", "core.fileMode", "true"],
        ] {
            assert!(git(args).is_ok());
        }
        let file = dir.join("f.txt");
        let base: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let _ = std::fs::write(&file, &base);
        assert!(git(&["add", "."]).is_ok());
        assert!(git(&["commit", "-q", "-m", "base"]).is_ok());
        let _ = std::fs::write(&file, base.replace("line 2\n", "line two\n"));
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755));
        let mode = || {
            std::fs::metadata(&file)
                .map(|m| m.permissions().mode() & 0o111)
                .unwrap_or(0)
        };
        let key = DiffKey::Work(git::StatusFile {
            path: "f.txt".to_owned(),
            old_path: None,
            code: 'M',
            area: Area::Unstaged,
        });
        let apply = |pick: Pick| -> Result<(), String> {
            let data = load(&dir, &key, DiffOptions::default(), true)?;
            assert!(data.partial);
            let chosen = hunk_changes(&data.lines, 0);
            let patch =
                partial_patch(&data.header, &data.lines, &chosen, pick).ok_or("nothing picked")?;
            let path = git::save_patch(&patch)?.to_string_lossy().into_owned();
            let mut args = pick.args().to_vec();
            args.push(&path);
            git(&args).map(|_| ())
        };
        assert_eq!(apply(Pick::Stage), Ok(()));
        assert_eq!(
            git(&["ls-files", "-s", "f.txt"]).map(|s| s.starts_with("100644")),
            Ok(true)
        );
        assert!(git(&["reset", "-q"]).is_ok());
        assert_eq!(apply(Pick::Discard), Ok(()));
        assert_eq!(std::fs::read_to_string(&file).ok(), Some(base));
        assert_ne!(mode(), 0, "the file is still executable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A picture renamed between two commits shows what it was before, not "(new file)".
    #[test]
    fn renamed_image_shows_its_old_picture() {
        let dir = std::env::temp_dir().join(format!("gitcraft-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let git = |args: &[&str]| git::run(Some(&dir), args);
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
        ] {
            assert!(git(args).is_ok());
        }
        let _ = std::fs::write(dir.join("a.png"), b"\x89PNG not really\0\x01");
        assert!(git(&["add", "."]).is_ok());
        assert!(git(&["commit", "-q", "-m", "a"]).is_ok());
        assert!(git(&["mv", "a.png", "b.png"]).is_ok());
        assert!(git(&["commit", "-q", "-m", "b"]).is_ok());
        let key = DiffKey::Range {
            from: "HEAD~1".to_owned(),
            to: Some("HEAD".to_owned()),
            path: "b.png".to_owned(),
        };
        let data = load(&dir, &key, DiffOptions::default(), true);
        let image = data.ok().and_then(|d| d.image);
        assert!(
            image
                .as_ref()
                .is_some_and(|i| i.old.is_some() && i.new.is_some())
        );
        assert!(image.is_some_and(|i| i.old == i.new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_part_gets_its_own_section() {
        let font = theme::mono(12.0);
        let job = line_job(
            &[(Color32::RED, "let x = 2;")],
            Some((8, 9)),
            &font,
            Color32::GREEN,
        );
        let pieces: Vec<&str> = job
            .sections
            .iter()
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(pieces, vec!["let x = ", "2", ";"]);
        assert_eq!(job.sections[1].format.background, Color32::GREEN);
    }
}
