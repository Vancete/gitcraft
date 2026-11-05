//! Tests of the tab labels, branch shortening, parking and the switcher's list.

use super::*;
use std::path::PathBuf;

fn main_tree(path: &str) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        branch: Some("main".to_owned()),
        head: "aaaaaaaaaaaa".to_owned(),
        main: true,
        ..Worktree::default()
    }
}

fn linked(path: &str, branch: Option<&str>) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        branch: branch.map(str::to_owned),
        head: "0123456789abcdef".to_owned(),
        current: true,
        ..Worktree::default()
    }
}

#[test]
fn only_other_worktrees_with_changes_are_marked() {
    let dirty = |current, changes| Worktree {
        current,
        changes,
        ..main_tree("/code/acme")
    };
    assert_eq!(changes_elsewhere(&dirty(false, Some(3))), Some(3));
    assert_eq!(changes_elsewhere(&dirty(true, Some(3))), None);
    assert_eq!(changes_elsewhere(&dirty(false, Some(0))), None);
    assert_eq!(changes_elsewhere(&dirty(false, None)), None);
    assert_eq!(
        changes_line(&dirty(false, Some(1))).as_deref(),
        Some("1 file with uncommitted changes")
    );
}

#[test]
fn a_linked_worktree_shows_the_repository_and_its_branch() {
    let list = [
        main_tree("/code/acme"),
        linked("/code/wt/t3-0219", Some("feature/search")),
    ];
    assert_eq!(
        tab_label(&list, "t3-0219"),
        TabLabel {
            repo: "acme".to_owned(),
            branch: Some("feature/search".to_owned()),
        }
    );
}

#[test]
fn the_main_worktree_shows_no_branch() {
    let main = Worktree {
        current: true,
        ..main_tree("/code/acme")
    };
    let other = Worktree {
        current: false,
        ..linked("/code/wt/a", Some("a"))
    };
    assert_eq!(
        tab_label(&[main, other], "acme"),
        TabLabel {
            repo: "acme".to_owned(),
            branch: None,
        }
    );
}

#[test]
fn a_detached_worktree_shows_its_short_hash_as_the_branch() {
    let list = [main_tree("/code/acme"), linked("/code/wt/check", None)];
    assert_eq!(tab_label(&list, "check").branch.as_deref(), Some("0123456"));
}

#[test]
fn a_bare_repository_is_named_without_its_suffix() {
    let list = [main_tree("/code/acme.git"), linked("/code/wt/a", Some("a"))];
    assert_eq!(tab_label(&list, "a").repo, "acme");
}

#[test]
fn the_plain_name_stands_in_until_the_worktrees_are_known() {
    assert_eq!(tab_label(&[], "t3-0219").repo, "t3-0219");
}

#[test]
fn closing_a_worktree_goes_back_to_the_main_one() {
    let list = [main_tree("/code/acme"), linked("/code/wt/a", Some("a"))];
    assert_eq!(main_to_return_to(&list), Some(Path::new("/code/acme")));
}

#[test]
fn a_bare_repository_has_no_main_worktree_to_go_back_to() {
    let bare = Worktree {
        bare: true,
        ..main_tree("/code/acme.git")
    };
    let list = [bare, linked("/code/wt/a", Some("a"))];
    assert_eq!(main_to_return_to(&list), None);
}

#[test]
fn a_main_worktree_whose_folder_is_gone_cannot_be_gone_back_to() {
    let gone = Worktree {
        prunable: Some("gone".to_owned()),
        ..main_tree("/code/acme")
    };
    assert_eq!(main_to_return_to(&[gone]), None);
}

#[test]
fn a_short_branch_stays_as_it_is() {
    assert_eq!(shorten_middle("docs/tutorial", 20), "docs/tutorial");
}

#[test]
fn a_long_branch_keeps_its_first_folder_and_its_end() {
    assert_eq!(
        shorten_middle("feature/plugins/marketplace/search-ui", 24),
        "feature/…/search-ui"
    );
}

#[test]
fn a_long_name_without_folders_keeps_more_of_its_end() {
    let short = shorten_middle("worktree-agent-a5b4669dd1a48230", 20);
    assert_eq!(short.chars().count(), 20);
    assert!(short.starts_with("worktre"), "{short}");
    assert!(short.ends_with("1a48230"), "{short}");
    assert!(short.contains('…'), "{short}");
}

#[test]
fn folders_that_would_not_fit_are_cut_like_any_long_name() {
    let short = shorten_middle("a-long-folder-name/another-very-long-branch-name", 16);
    assert_eq!(short.chars().count(), 16);
    assert!(short.ends_with("anch-name"), "{short}");
}

#[test]
fn shortening_counts_characters_not_bytes() {
    let short = shorten_middle("äöüäöüäöüäöüäöüäöü", 10);
    assert_eq!(short.chars().count(), 10);
}

#[test]
fn a_branch_that_fits_keeps_its_characters() {
    let short = fit_branch("feature/search", 200.0, |s| s.chars().count() as f32 * 7.0);
    assert_eq!(short, "feature/search");
}

#[test]
fn a_branch_of_wide_characters_is_cut_to_the_width() {
    let width = |s: &str| s.chars().count() as f32 * 12.0;
    let short = fit_branch("機能/検索画面の新しいデザインと実装の詳細", 120.0, width);
    assert!(width(&short) <= 120.0, "{short}");
    assert!(short.contains('…'), "{short}");
    assert!(short.ends_with("詳細"), "{short}");
}

#[test]
fn a_branch_is_cut_to_nothing_but_the_ellipsis_rather_than_overflow() {
    assert_eq!(fit_branch("abc", 0.0, |s| s.len() as f32), "…");
}

#[test]
fn parked_worktrees_with_unfinished_work_are_never_let_go_of() {
    assert_eq!(parked_to_drop(&[true, true, true], 0), Vec::<usize>::new());
}

#[test]
fn only_the_most_recent_idle_worktrees_are_kept() {
    // Oldest first: index 0, 1 and 3 are idle, and only the last 2 of those stay.
    assert_eq!(parked_to_drop(&[false, false, true, false], 2), vec![0]);
}

#[test]
fn idle_worktrees_within_the_limit_all_stay() {
    assert_eq!(parked_to_drop(&[false, false], 3), Vec::<usize>::new());
}

fn entries<'a>(
    list: &'a [Worktree],
    background: &[(PathBuf, &'static str)],
    query: &str,
) -> Vec<&'a str> {
    picker_entries(list, background, query)
        .iter()
        .map(|w| w.branch.as_deref().unwrap_or(""))
        .collect()
}

fn other(path: &str, branch: &str) -> Worktree {
    Worktree {
        current: false,
        ..linked(path, Some(branch))
    }
}

#[test]
fn the_switcher_lists_the_current_one_then_background_work_then_the_rest() {
    let list = [
        main_tree("/code/acme"),
        other("/wt/a", "a"),
        other("/wt/b", "b"),
        linked("/wt/c", Some("c")),
    ];
    let background = [(PathBuf::from("/wt/b"), "terminal")];
    assert_eq!(entries(&list, &background, ""), ["c", "b", "main", "a"]);
}

#[test]
fn the_switcher_finds_branches_and_folders_whatever_the_case() {
    let list = [
        main_tree("/code/acme"),
        other("/wt/t3-0219", "feature/Search"),
        other("/wt/t3-0abc", "docs/intro"),
    ];
    assert_eq!(entries(&list, &[], "SEARCH"), ["feature/Search"]);
    assert_eq!(entries(&list, &[], "0abc"), ["docs/intro"]);
}

#[test]
fn the_switcher_leaves_out_folders_that_are_gone() {
    let gone = Worktree {
        prunable: Some("gitdir file points to non-existent location".to_owned()),
        ..other("/wt/gone", "gone")
    };
    let list = [main_tree("/code/acme"), gone];
    assert_eq!(entries(&list, &[], ""), ["main"]);
}

#[test]
fn the_switcher_leaves_out_a_bare_repository() {
    let bare = Worktree {
        bare: true,
        branch: None,
        ..main_tree("/code/acme.git")
    };
    let list = [bare, linked("/wt/a", Some("a")), other("/wt/b", "b")];
    assert_eq!(entries(&list, &[], ""), ["a", "b"]);
}

#[test]
fn only_operations_and_terminals_run_on_by_themselves() {
    assert!(Unfinished::running(Unfinished {
        operation: true,
        ..Unfinished::default()
    }));
    assert!(Unfinished::running(Unfinished {
        terminal: true,
        ..Unfinished::default()
    }));
    assert!(!Unfinished::running(Unfinished {
        commit_message: true,
        editor: true,
        ..Unfinished::default()
    }));
}

#[test]
fn nothing_going_on_means_the_tab_can_move() {
    assert_eq!(Unfinished::default().label(), None);
}

#[test]
fn each_kind_of_unfinished_work_keeps_the_tab_as_it_is() {
    let cases = [
        Unfinished {
            operation: true,
            ..Unfinished::default()
        },
        Unfinished {
            terminal: true,
            ..Unfinished::default()
        },
        Unfinished {
            commit_message: true,
            ..Unfinished::default()
        },
        Unfinished {
            editor: true,
            ..Unfinished::default()
        },
        Unfinished {
            review: true,
            ..Unfinished::default()
        },
    ];
    for unfinished in cases {
        assert!(unfinished.label().is_some(), "{unfinished:?}");
    }
}
