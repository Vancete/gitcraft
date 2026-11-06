//! Partial patches: only some hunks or lines of a diff, for `git apply` to stage, unstage or
//! discard.

use std::collections::BTreeSet;

use super::{DiffLine, Kind, Pick};

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

/// One line of a partial hunk: its mark (` `, `+`, `-`), its text, and the "\ No newline at
/// end of file" note after it, if it has one.
struct OutLine<'a> {
    mark: char,
    text: &'a str,
    note: Option<&'a str>,
}

/// One hunk of a partial patch being written.
#[derive(Default)]
struct HunkOut<'a> {
    out: Vec<OutLine<'a>>,
    old_n: i64,
    new_n: i64,
    changed: bool,
}

impl<'a> HunkOut<'a> {
    /// Writes line `i` as the patch needs it (or not at all), and the "\ No newline at end of
    /// file" note right after it if it has one.
    fn line(&mut self, lines: &'a [DiffLine], i: usize, picked: bool, reverse: bool) {
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
        let mark = if as_context {
            self.old_n += 1;
            self.new_n += 1;
            ' '
        } else {
            self.changed = true;
            if line.kind == Kind::Add {
                self.new_n += 1;
                '+'
            } else {
                self.old_n += 1;
                '-'
            }
        };
        let note = lines
            .get(i + 1)
            .filter(|l| l.kind == Kind::Meta)
            .map(|l| l.raw.as_str());
        self.out.push(OutLine {
            mark,
            text: line.raw.get(1..).unwrap_or_default(),
            note,
        });
    }

    /// The hunk's lines as patch text. A line marked as having no newline at the end of the
    /// file has to be the last of its side: when a line left out (or kept as context) puts
    /// others after it on a side, that side gets the newline (`git apply` would otherwise join
    /// the lines into one). Context is split into a removal and an addition for that, so the
    /// side it does end keeps the note.
    fn body(&self) -> String {
        let on_old = |m: char| m != '+';
        let on_new = |m: char| m != '-';
        let mut body = String::new();
        let mut push = |mark: char, text: &str, note: Option<&str>| {
            body.push(mark);
            body.push_str(text);
            body.push('\n');
            if let Some(note) = note {
                body.push_str(note);
                body.push('\n');
            }
        };
        for (k, l) in self.out.iter().enumerate() {
            let Some(note) = l.note else {
                push(l.mark, l.text, None);
                continue;
            };
            let rest = &self.out[k + 1..];
            let old_goes_on = on_old(l.mark) && rest.iter().any(|r| on_old(r.mark));
            let new_goes_on = on_new(l.mark) && rest.iter().any(|r| on_new(r.mark));
            match (l.mark, old_goes_on, new_goes_on) {
                (' ', false, true) => {
                    push('-', l.text, Some(note));
                    push('+', l.text, None);
                }
                (' ', true, false) => {
                    push('-', l.text, None);
                    push('+', l.text, Some(note));
                }
                (_, false, false) => push(l.mark, l.text, Some(note)),
                _ => push(l.mark, l.text, None),
            }
        }
        body
    }
}

/// The body of the hunk whose header is at `h`, with only the `chosen` changes.
fn write_hunk<'a>(
    lines: &'a [DiffLine],
    h: usize,
    chosen: &BTreeSet<usize>,
    reverse: bool,
) -> HunkOut<'a> {
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
                    while i < body.end && matches!(lines[i].kind, k if k == kind || k == Kind::Meta)
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
    out
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
        let hunk = write_hunk(lines, h, chosen, reverse);
        let (old_n, new_n) = (hunk.old_n, hunk.new_n);
        if !hunk.changed {
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
        patch.push_str(&hunk.body());
    }
    any.then_some(patch)
}
