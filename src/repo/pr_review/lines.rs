//! Which lines of a pull request's diff take comments, and which have them.

use std::collections::HashMap;

use crate::pr_threads::Thread;
use crate::pull_request::DiffSide;
use crate::repo::diff::Kind;

use super::LineTarget;

/// The side and number of a diff line a comment can go on, if it can: GitHub takes them on the
/// lines of the diff, in `column` (the old or new half of the side-by-side view, `None` for the
/// unified one).
pub(in crate::repo) fn target_of(
    kind: Kind,
    old: Option<u32>,
    new: Option<u32>,
    column: Option<DiffSide>,
) -> Option<(DiffSide, u64)> {
    let side = match (kind, column) {
        (Kind::Add, None | Some(DiffSide::Right)) | (Kind::Context, None) => DiffSide::Right,
        (Kind::Del, None | Some(DiffSide::Left)) => DiffSide::Left,
        (Kind::Context, Some(side)) => side,
        _ => return None,
    };
    let number = match side {
        DiffSide::Left => old,
        DiffSide::Right => new,
    };
    Some((side, u64::from(number?)))
}

/// Whether a comment's suggestion can be committed: GitHub takes it only for lines of the file as
/// it is now, on a pull request that's still going.
pub(super) fn suggestion_applies(thread: &Thread, comment_pending: bool, open: bool) -> bool {
    open && !comment_pending
        && !thread.outdated
        && thread.side == DiffSide::Right
        && !thread.starts_left
}

/// Which lines a suggestion replaces.
pub(super) fn suggestion_scope(start: u64, end: u64) -> String {
    if start == end {
        format!("Replaces line {end}")
    } else {
        format!("Replaces lines {start} to {end}")
    }
}

/// Where a line of the unified diff opens: the thread it has, or else the file as it is now.
pub(in crate::repo) fn unified_spot(
    kind: Kind,
    old: Option<u32>,
    new: Option<u32>,
    marks: &HashMap<(DiffSide, u64), usize>,
) -> Option<(DiffSide, u64)> {
    let right = target_of(kind, old, new, None);
    if kind == Kind::Context
        && let (Some(spot), Some(old)) = (right, old)
        && !marks.contains_key(&spot)
        && marks.contains_key(&(DiffSide::Left, u64::from(old)))
    {
        return Some((DiffSide::Left, u64::from(old)));
    }
    right
}

/// Whether `thread` is on that line now: an outdated one isn't on any.
pub(super) fn thread_is_on(thread: &Thread, path: &str, side: DiffSide, line: u64) -> bool {
    !thread.outdated && thread.path == path && thread.side == side && thread.line == Some(line)
}

/// How many comments each line of `path` has, the pending ones too.
pub(super) fn line_marks(threads: &[Thread], path: &str) -> HashMap<(DiffSide, u64), usize> {
    let mut marks = HashMap::new();
    for thread in threads
        .iter()
        .filter(|t| t.path == path && !t.outdated && !t.comments.is_empty())
    {
        if let Some(line) = thread.line {
            *marks.entry((thread.side, line)).or_insert(0) += thread.comments.len();
        }
    }
    marks
}

/// What a diff needs to offer comments: it shows a file of the pull request shown.
pub(in crate::repo) struct DiffReview {
    pub marks: HashMap<(DiffSide, u64), usize>,
    pub target: Option<LineTarget>,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_threads::ThreadComment;

    #[test]
    fn in_the_unified_diff_a_line_has_one_place_to_comment() {
        assert_eq!(
            target_of(Kind::Add, None, Some(7), None),
            Some((DiffSide::Right, 7))
        );
        assert_eq!(
            target_of(Kind::Del, Some(5), None, None),
            Some((DiffSide::Left, 5))
        );
        assert_eq!(
            target_of(Kind::Context, Some(5), Some(7), None),
            Some((DiffSide::Right, 7))
        );
    }

    #[test]
    fn side_by_side_a_line_that_stayed_can_be_commented_on_either_side() {
        assert_eq!(
            target_of(Kind::Context, Some(5), Some(7), Some(DiffSide::Left)),
            Some((DiffSide::Left, 5))
        );
        assert_eq!(
            target_of(Kind::Context, Some(5), Some(7), Some(DiffSide::Right)),
            Some((DiffSide::Right, 7))
        );
    }

    #[test]
    fn headers_and_notes_take_no_comments() {
        assert_eq!(target_of(Kind::Hunk, None, None, None), None);
        assert_eq!(target_of(Kind::Meta, Some(1), Some(1), None), None);
    }

    fn thread(path: &str, line: u64, side: DiffSide, outdated: bool, comments: usize) -> Thread {
        Thread {
            id: format!("{path}:{line}"),
            path: path.to_owned(),
            line: Some(line),
            start_line: None,
            side,
            outdated,
            resolved: false,
            can_resolve: true,
            can_unresolve: false,
            can_reply: true,
            comments: vec![ThreadComment::default(); comments],
            starts_left: false,
            more_comments: false,
        }
    }

    #[test]
    fn marks_count_the_comments_of_this_file() {
        let threads = [
            thread("a.rs", 3, DiffSide::Right, false, 2),
            thread("a.rs", 3, DiffSide::Left, false, 1),
            thread("b.rs", 3, DiffSide::Right, false, 5),
        ];
        let marks = line_marks(&threads, "a.rs");
        assert_eq!(marks.get(&(DiffSide::Right, 3)), Some(&2));
        assert_eq!(marks.get(&(DiffSide::Left, 3)), Some(&1));
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn outdated_threads_mark_no_line() {
        let threads = [thread("a.rs", 3, DiffSide::Right, true, 1)];
        assert!(line_marks(&threads, "a.rs").is_empty());
        assert!(!thread_is_on(&threads[0], "a.rs", DiffSide::Right, 3));
    }

    #[test]
    fn a_thread_is_on_its_own_line_and_side_only() {
        let on = thread("a.rs", 3, DiffSide::Right, false, 1);
        assert!(thread_is_on(&on, "a.rs", DiffSide::Right, 3));
        assert!(!thread_is_on(&on, "a.rs", DiffSide::Left, 3));
        assert!(!thread_is_on(&on, "a.rs", DiffSide::Right, 4));
        assert!(!thread_is_on(&on, "b.rs", DiffSide::Right, 3));
    }

    #[test]
    fn a_suggestion_applies_only_while_it_can_land() {
        let plain = thread("a.rs", 3, DiffSide::Right, false, 1);
        assert!(suggestion_applies(&plain, false, true));
        assert!(!suggestion_applies(&plain, true, true), "still pending");
        assert!(
            !suggestion_applies(&plain, false, false),
            "merged or closed"
        );
        let old = thread("a.rs", 3, DiffSide::Left, false, 1);
        assert!(!suggestion_applies(&old, false, true), "on the old file");
        let mixed = Thread {
            starts_left: true,
            ..thread("a.rs", 3, DiffSide::Right, false, 1)
        };
        assert!(
            !suggestion_applies(&mixed, false, true),
            "starts on the old file"
        );
        let outdated = Thread {
            outdated: true,
            ..thread("a.rs", 3, DiffSide::Right, false, 1)
        };
        assert!(
            !suggestion_applies(&outdated, false, true),
            "its line number is from an older commit"
        );
    }

    #[test]
    fn says_which_lines_a_suggestion_replaces() {
        assert_eq!(suggestion_scope(3, 3), "Replaces line 3");
        assert_eq!(suggestion_scope(2, 4), "Replaces lines 2 to 4");
    }

    #[test]
    fn a_line_that_stayed_opens_the_thread_it_has() {
        let mut marks = HashMap::new();
        assert_eq!(
            unified_spot(Kind::Context, Some(5), Some(7), &marks),
            Some((DiffSide::Right, 7))
        );
        marks.insert((DiffSide::Left, 5), 1);
        assert_eq!(
            unified_spot(Kind::Context, Some(5), Some(7), &marks),
            Some((DiffSide::Left, 5))
        );
        marks.insert((DiffSide::Right, 7), 1);
        assert_eq!(
            unified_spot(Kind::Context, Some(5), Some(7), &marks),
            Some((DiffSide::Right, 7))
        );
    }
}
