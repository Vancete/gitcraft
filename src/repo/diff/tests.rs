//! Tests for diff parsing, line pairing, partial patches, images, selection and the change map.

use std::collections::BTreeSet;

use egui::Color32;

use super::images::image_id;
use super::jump::{change_at, changes, offset_of, step};
use super::map::change_runs;
use super::paint::line_job;
use super::parse::{changed_ranges, hunk_start, meta};
use super::select::Side;
use super::*;
use crate::theme;

#[test]
fn changes_group_into_runs_per_kind() {
    let (n, d, a, both) = ((false, false), (true, false), (false, true), (true, true));
    let runs = change_runs([n, d, d, a, n, both, a].into_iter());
    assert_eq!(runs.removed, vec![1..3, 5..6]);
    assert_eq!(runs.added, vec![3..4, 5..7]);
}

#[test]
fn removals_and_additions_next_to_each_other_are_one_change() {
    let (n, d, a, both) = ((false, false), (true, false), (false, true), (true, true));
    let runs = change_runs([n, d, d, a, n, both, a, n, n, a].into_iter());
    assert_eq!(changes(&runs), vec![1..4, 5..7, 9..10]);
    assert!(changes(&change_runs([n, n].into_iter())).is_empty());
}

#[test]
fn stepping_goes_from_the_change_jumped_to_or_from_what_is_in_view() {
    let list = vec![1..4, 20..22, 40..41];
    // At the top: the first change is in view already, so the next one is the second.
    assert_eq!(step(&list, 0.0, None, 1), Some(1));
    assert_eq!(step(&list, 0.0, None, -1), Some(0));
    // Scrolled to the second change: on to the third, back to the first.
    let at = offset_of(20);
    assert_eq!(step(&list, at, None, 1), Some(2));
    assert_eq!(step(&list, at, None, -1), Some(0));
    // The view couldn't scroll as far as the last change jumped to: still steps from it.
    assert_eq!(step(&list, 300.0, Some((1, 300.0)), 1), Some(2));
    assert_eq!(step(&list, 300.0, Some((2, 300.0)), 1), None);
    assert_eq!(step(&list, 300.0, Some((0, 300.0)), -1), None);
    // Scrolled by hand since: from what is in view again.
    assert_eq!(step(&list, 0.0, Some((2, 300.0)), -1), Some(0));
    assert_eq!(step(&[], 0.0, None, 1), None);
}

#[test]
fn a_click_on_a_change_mark_finds_its_change() {
    let list = vec![1..4, 20..22];
    assert_eq!(change_at(&list, 2.0, 0.5), Some(0));
    assert_eq!(change_at(&list, 22.3, 0.5), Some(1));
    assert_eq!(change_at(&list, 10.0, 0.5), None);
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
    assert_eq!(unified.text(&lines, &Default::default()), "\nlet x");
    // On the new side the removals are not there.
    let new = sel(Side::New, (1, 0), (5, 1));
    assert_eq!(new.text(&lines, &Default::default()), "a\nlet x = 2;\nb");
    assert_eq!(new.chars_of(4, Side::New), Some(0..usize::MAX));
    assert_eq!(new.chars_of(4, Side::Old), None);
    assert_eq!(new.chars_of(5, Side::New), Some(0..1));
    // Pressed and not moved: nothing.
    assert_eq!(sel(Side::Old, (2, 3), (2, 3)).chars_of(2, Side::Old), None);
}

#[test]
fn a_line_cut_to_be_shown_is_copied_whole() {
    use crate::repo::highlight::{MAX_LINE_CHARS, cap_line};
    let whole = format!("let x = \"{}\";", "y".repeat(MAX_LINE_CHARS));
    let mut lines = parse(&format!("@@ -1,2 +1,2 @@\n a\n+{whole}\n"));
    let uncut: std::collections::HashMap<usize, String> = lines
        .iter_mut()
        .enumerate()
        .filter_map(|(i, l)| Some((i, cap_line(&mut l.text, None)?)))
        .collect();
    assert_eq!(uncut.get(&2), Some(&whole));
    let shown = lines[2].text.chars().count();
    let sel = |anchor, cursor| TextSel {
        side: Side::Unified,
        anchor,
        cursor,
    };
    // Through the end of what is shown (the note included): the whole line.
    assert_eq!(
        sel((1, 0), (2, shown)).text(&lines, &uncut),
        format!("a\n{whole}")
    );
    // From inside the note: from the cut on, to the end.
    let rest: String = whole.chars().skip(MAX_LINE_CHARS).collect();
    assert_eq!(
        sel((2, MAX_LINE_CHARS + 3), (2, shown)).text(&lines, &uncut),
        rest
    );
    // Within what is shown: just that.
    assert_eq!(sel((2, 0), (2, 3)).text(&lines, &uncut), "let");
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

/// A line without a final newline that a partial patch puts others after: the side that goes
/// on gets the newline, or `git apply` would join the two lines ("bc", "a0d1").
#[test]
fn no_newline_note_only_ends_its_side() {
    let hunks_of = |text: &str, chosen: &[usize], pick| {
        let header = "diff --git a/f b/f\n--- a/f\n+++ b/f\n";
        let lines = parse(&format!("{header}{text}"));
        let chosen: BTreeSet<usize> = chosen.iter().copied().collect();
        partial_patch(header, &lines, &chosen, pick)
            .and_then(|p| p.strip_prefix(header).map(str::to_owned))
    };
    let note = "\\ No newline at end of file\n";
    // "a\nb" → "a\nb\nc\n", staging only "+c": b keeps its place but gets a newline.
    let grown = format!("@@ -1,2 +1,3 @@\n a\n-b\n{note}+b\n+c\n");
    assert_eq!(
        hunks_of(&grown, &[5], Pick::Stage),
        Some(format!("@@ -1,2 +1,3 @@\n a\n-b\n{note}+b\n+c\n"))
    );
    // "x\nd0\nd1\n" → "x\na0", staging only "+a0": d1 stays after it.
    let cut = format!("@@ -1,3 +1,2 @@\n x\n-d0\n-d1\n+a0\n{note}");
    assert_eq!(
        hunks_of(&cut, &[4], Pick::Stage),
        Some("@@ -1,3 +1,4 @@\n x\n d0\n+a0\n d1\n".to_owned())
    );
    // "x\nd0" → "x\na0\na1\n", discarding only "-d0": the lines kept come after it.
    let added = format!("@@ -1,2 +1,3 @@\n x\n-d0\n{note}+a0\n+a1\n");
    assert_eq!(
        hunks_of(&added, &[2], Pick::Discard),
        Some("@@ -1,4 +1,3 @@\n x\n-d0\n a0\n a1\n".to_owned())
    );
    // "d0\nd1\n" → "a0", discarding only "-d1": a0, kept, is no longer last on the old side.
    let shrunk = format!("@@ -1,2 +1 @@\n-d0\n-d1\n+a0\n{note}");
    assert_eq!(
        hunks_of(&shrunk, &[2], Pick::Discard),
        Some(format!("@@ -1,2 +1,1 @@\n-a0\n+a0\n{note}-d1\n"))
    );
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

/// A Latin-1 file is read with its odd bytes replaced: a patch of its lines would write the
/// replacements into the index, so its lines only go whole.
#[test]
fn text_that_is_not_utf8_is_not_picked_from() {
    let dir = std::env::temp_dir().join(format!("gitcraft-partial-latin1-{}", std::process::id()));
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
    let file = dir.join("f.txt");
    let _ = std::fs::write(&file, b"a\nb\n");
    assert!(git(&["add", "."]).is_ok());
    assert!(git(&["commit", "-q", "-m", "base"]).is_ok());
    let _ = std::fs::write(&file, b"a\ncaf\xe9\nb\n");
    let key = DiffKey::Work(git::StatusFile {
        path: "f.txt".to_owned(),
        old_path: None,
        code: 'M',
        area: Area::Unstaged,
    });
    let data = load(&dir, &key, DiffOptions::default(), true);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(data.map(|d| (d.lines.len(), d.partial)), Ok((4, false)));
}

/// A file made executable and edited: picking lines leaves the mode change where it is.
#[cfg(unix)]
#[test]
fn partial_patches_leave_a_mode_change_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("gitcraft-partial-mode-{}", std::process::id()));
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

/// Unstaging `[ab].txt` leaves `a.txt` staged: write commands take file names literally.
#[test]
fn unstaging_a_file_does_not_treat_its_name_as_a_pattern() {
    let dir = std::env::temp_dir().join(format!("gitcraft-unstage-literal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let git = |args: &[&str]| git::run(Some(&dir), args);
    let staged = || git(&["diff", "--cached", "--name-only"]);
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@t"],
    ] {
        assert!(git(args).is_ok());
    }
    for name in ["a.txt", "[ab].txt"] {
        let _ = std::fs::write(dir.join(name), b"x\n");
    }
    // Before the first commit (`rm --cached`), then after it (`restore --staged`).
    assert!(git(&["add", "-A"]).is_ok());
    let unstage = |head| {
        let args = unstage_args(head, "[ab].txt");
        git(&args.iter().map(String::as_str).collect::<Vec<_>>())
    };
    assert!(unstage("").is_ok());
    let before_commit = staged();
    assert!(git(&["commit", "-q", "-m", "base"]).is_ok());
    for name in ["a.txt", "[ab].txt"] {
        let _ = std::fs::write(dir.join(name), b"y\n");
    }
    assert!(git(&["add", "-A"]).is_ok());
    assert!(unstage("HEAD").is_ok());
    let after_commit = staged();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(before_commit.as_deref().map(str::trim), Ok("a.txt"));
    assert_eq!(after_commit.as_deref().map(str::trim), Ok("a.txt"));
}
