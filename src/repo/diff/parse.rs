//! Reading git's diff text into lines, and pairing removed lines with added ones.

use super::{DiffLine, Kind, MAX_LINES};

type Split = Vec<(Option<usize>, Option<usize>)>;

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
        out.push(body_line(line, prefix, &mut old, &mut new));
    }
    out
}

/// A context, added or removed line inside a hunk, its `prefix` mark columns cut off; counts
/// it on the side(s) it belongs to.
fn body_line(line: &str, prefix: usize, old: &mut u32, new: &mut u32) -> DiffLine {
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
            *new += 1;
            (None, Some(*new))
        }
        Kind::Del => {
            *old += 1;
            (Some(*old), None)
        }
        _ => {
            *old += 1;
            *new += 1;
            (Some(*old), Some(*new))
        }
    };
    DiffLine {
        kind,
        old: if combined { None } else { o },
        new: if combined { None } else { n },
        text: body.trim_end_matches('\r').replace('\t', "    "),
        raw: line.to_owned(),
    }
}

pub(super) fn meta(text: &str) -> DiffLine {
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
pub(super) fn patch_header(text: &str) -> String {
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

/// `@@ -12,5 +14,7 @@` → (11, 13): the numbers before the first line of the hunk.
pub(super) fn hunk_start(header: &str) -> (u32, u32) {
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

/// Pairs each run of removed lines with the added lines after it: marks what changed inside
/// each pair, and lays out side-by-side rows.
pub(super) fn pair_lines(lines: &[DiffLine]) -> (Vec<Option<(usize, usize)>>, Split) {
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
pub(super) fn changed_ranges(old: &str, new: &str) -> Option<((usize, usize), (usize, usize))> {
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
