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

/// The body of the hunk whose header is at `h`, with only the `chosen` changes.
fn write_hunk(lines: &[DiffLine], h: usize, chosen: &BTreeSet<usize>, reverse: bool) -> HunkOut {
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
        let HunkOut {
            body,
            old_n,
            new_n,
            changed,
        } = write_hunk(lines, h, chosen, reverse);
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
