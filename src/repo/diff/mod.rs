//! Central pane while a file is open: its diff, unified or side by side, with syntax colours,
//! the changed part of each line marked, images shown before / after and files in Git LFS as
//! their objects. Uncommitted changes can be staged, unstaged or discarded a hunk or a few
//! lines at a time.

mod images;
mod jump;
mod lfs;
mod map;
mod paint;
mod parse;
mod patch;
mod rows;
mod select;
#[cfg(test)]
mod tests;
mod view;

use std::path::Path;

use super::DiffKey;
use super::highlight::{self, MAX_LINE_CHARS, Spans, cap_line};
use crate::git::{self, Area, DiffOptions};

pub use images::ImagePair;
use images::{is_image, load_images, renamed_from};
pub use lfs::LfsPair;
use lfs::lfs_pair;
pub use parse::parse;
use parse::{pair_lines, patch_header};
pub use patch::{hunk_changes, partial_patch};
pub(super) use select::TextSel;

const LINE_H: f32 = 19.0;
const NUM_W: f32 = 46.0;
/// Width of the strip of changes beside the scrollbar.
const MAP_W: f32 = 12.0;
/// Very long diffs are cut here; the UI stays responsive and git is one click away.
const MAX_LINES: usize = 50_000;

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
    /// The file is kept in Git LFS: the objects before and after, shown instead of the lines.
    pub lfs: Option<LfsPair>,
    /// The patch's file header (`diff --git`, `index`, `---`, `+++`), for partial patches.
    pub header: String,
    /// Hunks and lines can be staged, unstaged or discarded on their own.
    pub partial: bool,
    /// Characters in the longest line, for the width to scroll (counted once, not every frame).
    longest: usize,
    /// The lines cut to be shown, as they were: what copying them copies.
    pub(super) uncut: std::collections::HashMap<usize, String>,
    /// The runs of changed rows and the changes they make, unified and side by side: worked
    /// out the first time they are needed, not every frame.
    map: [std::sync::OnceLock<(map::ChangeRuns, Vec<std::ops::Range<usize>>)>; 2],
}

impl DiffData {
    /// The runs of changed rows shown unified or side by side (`split`), and the changes they
    /// make (see [`jump::changes`]).
    pub(in crate::repo::diff) fn runs(
        &self,
        split: bool,
    ) -> &(map::ChangeRuns, Vec<std::ops::Range<usize>>) {
        self.map[usize::from(split)].get_or_init(|| {
            let runs = map::runs_of(self, split);
            let changes = jump::changes(&runs);
            (runs, changes)
        })
    }
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
    let partial = can_pick(key, opts, &header, &lines);
    let lfs = lfs_pair(&lines);
    // An image in LFS: git has only its pointer.
    let image = if is_image(&path) && lfs.is_none() {
        Some(load_images(repo, key, &path, renamed_from(&text)))
    } else {
        None
    };
    let mut spans = highlight_sides(&path, &lines, dark);
    let (mut changed, split) = pair_lines(&lines);
    let mut lines = lines;
    let mut uncut = std::collections::HashMap::new();
    for (i, (line, spans)) in lines.iter_mut().zip(spans.iter_mut()).enumerate() {
        if let Some(whole) = cap_line(&mut line.text, spans.as_mut()) {
            uncut.insert(i, whole);
        }
    }
    for range in changed.iter_mut().flatten() {
        *range = (range.0.min(MAX_LINE_CHARS), range.1.min(MAX_LINE_CHARS));
    }
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
        lfs,
        header,
        partial,
        longest,
        uncut,
        map: Default::default(),
    })
}

/// Whether hunks and lines of this diff can be picked on their own.
fn can_pick(key: &DiffKey, opts: DiffOptions, header: &str, lines: &[DiffLine]) -> bool {
    // A patch made from a `-w` diff wouldn't apply; new, deleted, renamed and binary files, and
    // truncated diffs, only go whole.
    matches!(key, DiffKey::Work(f) if matches!(f.area, Area::Staged | Area::Unstaged))
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
            .all(|l| l.kind != Kind::Meta || l.raw.starts_with('\\'))
}

/// Syntax colours per line. The old and the new side are each coloured as one stream, so
/// multi-line constructs work.
fn highlight_sides(path: &str, lines: &[DiffLine], dark: bool) -> Vec<Option<Spans>> {
    let mut spans = vec![None; lines.len()];
    let old_side: Vec<usize> = (0..lines.len())
        .filter(|&i| matches!(lines[i].kind, Kind::Context | Kind::Del))
        .collect();
    let new_side: Vec<usize> = (0..lines.len())
        .filter(|&i| matches!(lines[i].kind, Kind::Context | Kind::Add))
        .collect();
    if let Some(old) =
        highlight::highlight(path, old_side.iter().map(|&i| lines[i].text.as_str()), dark)
    {
        for (k, &i) in old_side.iter().enumerate() {
            if lines[i].kind == Kind::Del {
                spans[i] = old.get(k).cloned();
            }
        }
    }
    if let Some(new) =
        highlight::highlight(path, new_side.iter().map(|&i| lines[i].text.as_str()), dark)
    {
        for (k, &i) in new_side.iter().enumerate() {
            spans[i] = new.get(k).cloned();
        }
    }
    spans
}

/// `git restore --staged`, or `git rm --cached` before the first commit (no HEAD to restore from).
pub(super) fn unstage_args<'a>(head: &str, path: &'a str) -> Vec<&'a str> {
    if head.is_empty() {
        vec!["rm", "--cached", "-q", "--", path]
    } else {
        vec!["restore", "--staged", "--", path]
    }
}
