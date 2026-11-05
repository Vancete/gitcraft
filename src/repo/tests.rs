//! Tests of the tab's git work: undo of what operations did, the watched folders, checkout
//! names, diff answers, the toolbar's error line.

use std::path::{Path, PathBuf};

use super::commands::{local_name_of, refuse_option_like};
use super::operations::{run_steps, stash_push_args};
use super::toolbar::headline;
use super::watch::{FsChange, Reread, fs_change, linked_git_dirs};
use super::{DiffKey, DiffView, OpKind, Undo, ops};
use crate::git;

/// A repository with one commit, in a fresh folder named after `name`.
fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gitcraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@t"],
        &["commit", "-q", "--allow-empty", "-m", "base"],
    ] {
        assert!(git::run(Some(&dir), args).is_ok());
    }
    dir
}

#[test]
fn stashing_nothing_is_not_undoable() {
    let dir = repo("stash-nothing");
    let stash = || ops(&[&stash_push_args()]);
    let custom = || {
        OpKind::Normal(
            Undo::Custom {
                undo: ops(&[&["stash", "pop", "--index"]]),
                redo: stash(),
            },
            String::new(),
        )
    };
    // Nothing to save: undoing would pop some older stash.
    let (result, kind) = run_steps(&dir, &stash(), custom(), None);
    assert!(result.is_ok());
    assert!(matches!(kind, OpKind::Normal(Undo::None, _)));
    // Something to save: undoable.
    let _ = std::fs::write(dir.join("f.txt"), "x\n");
    let (result, kind) = run_steps(&dir, &stash(), custom(), None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_ok());
    assert!(matches!(kind, OpKind::Normal(Undo::Custom { .. }, _)));
}

#[test]
fn undoing_a_head_move_goes_back_to_where_head_really_was() {
    let dir = repo("head-before");
    let head = |dir: &Path| git::head(dir);
    let seen = head(&dir);
    // Committed elsewhere (the terminal) since the view last read HEAD.
    assert!(git::run(Some(&dir), &["commit", "-q", "--allow-empty", "-m", "two"]).is_ok());
    let before = head(&dir);
    let kind = OpKind::Normal(Undo::HeadMove { mode: "--soft" }, seen.clone());
    let commit = ops(&[&["commit", "-q", "--allow-empty", "-m", "three"]]);
    let (result, kind) = run_steps(&dir, &commit, kind, None);
    let after = head(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_ok());
    assert_ne!(seen, before);
    assert!(matches!(
        kind,
        OpKind::Normal(Undo::HeadMove { .. }, heads) if heads == format!("{before}\u{0}{after}")
    ));
}

#[test]
fn a_linked_worktree_is_watched_where_git_keeps_its_data() {
    let dir = repo("linked-watch");
    let linked = dir.with_file_name(format!("gitcraft-linked-watch-wt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&linked);
    let added = git::run(
        Some(&dir),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            &linked.to_string_lossy(),
        ],
    );
    let dirs = linked_git_dirs(&linked);
    let main = linked_git_dirs(&dir);
    let canonical = std::fs::canonicalize(dir.join(".git")).ok();
    let _ = std::fs::remove_dir_all(&linked);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(added.is_ok());
    assert!(main.is_none());
    assert!(dirs.is_some());
    let (own, common) = dirs.unwrap_or_default();
    assert_eq!(Some(common.clone()), canonical);
    assert!(own.starts_with(&common) && own != common);
    let watched = [own.clone(), common.clone()];
    // Its HEAD, the shared refs: reload; objects: nothing; its index: status and submodules;
    // its files: status.
    assert_eq!(fs_change(&own.join("HEAD"), &watched), Some(FsChange::Git));
    assert_eq!(
        fs_change(&own.join("index"), &watched),
        Some(FsChange::Index)
    );
    assert_eq!(
        fs_change(&common.join("refs/heads/side"), &watched),
        Some(FsChange::Git)
    );
    assert_eq!(fs_change(&common.join("objects/ab/cd"), &watched), None);
    assert_eq!(
        fs_change(&linked.join("src/a.rs"), &watched),
        Some(FsChange::Files)
    );
    // A main worktree as before.
    assert_eq!(
        fs_change(Path::new("/r/.git/index"), &[]),
        Some(FsChange::Index)
    );
    assert_eq!(
        fs_change(Path::new("/r/.git/MERGE_HEAD"), &[]),
        Some(FsChange::Git)
    );
    assert_eq!(fs_change(Path::new("/r/.git/FETCH_HEAD"), &[]), None);
    assert_eq!(fs_change(Path::new("/r/.git/ORIG_HEAD"), &[]), None);
    assert_eq!(fs_change(Path::new("/r/.git/objects/x"), &[]), None);
    assert_eq!(fs_change(Path::new("/r/a.rs"), &[]), Some(FsChange::Files));
    assert_eq!(
        fs_change(Path::new("/r/.gitmodules"), &[]),
        Some(FsChange::Index)
    );
    // A repository made with `--ref-format=reftable`.
    assert_eq!(
        fs_change(Path::new("/r/.git/reftable/tables.list"), &[]),
        Some(FsChange::Git)
    );
}

#[test]
fn a_change_on_disk_reads_again_what_it_can_have_changed() {
    // The index holds the commits submodules should be at: they are read again with the status.
    assert_eq!(
        FsChange::Index.rereads(),
        [Reread::Status, Reread::Submodules]
    );
    assert_eq!(FsChange::Files.rereads(), [Reread::Status]);
    assert_eq!(FsChange::Git.rereads(), [Reread::All]);
}

#[test]
fn a_remote_branch_is_checked_out_under_its_name_on_the_remote() {
    let remotes = ["origin", "my", "my/fork"];
    assert_eq!(local_name_of("origin/feature/x", &remotes), "feature/x");
    assert_eq!(local_name_of("my/fork/feat", &remotes), "feat");
    assert_eq!(local_name_of("my/feat", &remotes), "feat");
    assert_eq!(local_name_of("remotes/origin/main", &remotes), "main");
    // A remote no longer configured, whose branches are still there.
    assert_eq!(local_name_of("gone/main", &remotes), "main");
}

#[test]
fn names_git_would_read_as_options_are_not_checked_out() {
    let mut status = None;
    assert!(refuse_option_like("--orphan=x", &mut status));
    assert!(status.is_some_and(|(_, err)| err));
    let mut status = None;
    assert!(!refuse_option_like("feature/-x", &mut status));
    assert!(!refuse_option_like("refs/tags/v1", &mut status));
    assert!(status.is_none());
}

#[test]
fn an_older_diff_answer_does_not_replace_a_newer_one() {
    let key = DiffKey::Commit {
        hash: "abc".to_owned(),
        path: "a.rs".to_owned(),
    };
    let mut view = DiffView::new(key.clone());
    let older = view.next_request();
    let newer = view.next_request();
    view.arrived(newer, Err("newer".to_owned()));
    view.arrived(older, Err("older".to_owned()));
    assert!(matches!(&view.data, Some(Err(e)) if e == "newer"));
    // Nor an answer to a diff opened before, of the same file.
    let mut reopened = DiffView::new(key);
    let request = reopened.next_request();
    reopened.arrived(newer, Err("closed".to_owned()));
    assert!(reopened.data.is_none());
    reopened.arrived(request, Err("open".to_owned()));
    assert!(matches!(&reopened.data, Some(Err(e)) if e == "open"));
}

#[test]
fn an_older_read_of_the_working_tree_does_not_replace_a_newer_one() {
    use super::{Msg, RepoTab};
    let dir = repo("older-read");
    let ctx = egui::Context::default();
    let mut tab = RepoTab::new_deferred(dir.clone(), &ctx);
    let file = |path: &str| git::StatusFile {
        path: path.to_owned(),
        old_path: None,
        code: '?',
        area: git::Area::Untracked,
    };
    let work = |path: &str| git::WorkStatus {
        unstaged: vec![file(path)],
        ..Default::default()
    };
    let submodule = |path: &str| git::Submodule {
        path: path.to_owned(),
        ..Default::default()
    };
    // Read 2's answers come in first, then read 1's.
    for msg in [
        Msg::Status(2, Ok(work("newer"))),
        Msg::Submodules(2, vec![submodule("newer")]),
        Msg::Status(1, Ok(work("older"))),
        Msg::Submodules(1, vec![submodule("older")]),
    ] {
        let _ = tab.tx.send(msg);
    }
    tab.poll(&ctx);
    assert_eq!(tab.work.unstaged, vec![file("newer")]);
    assert_eq!(tab.submodules.len(), 1);
    assert_eq!(tab.submodules[0].path, "newer");

    // A full reload started before them (read 1) comes in last: the working tree, and this
    // worktree's count of changes, stay those of the newer status read.
    let here = |changes| git::Worktree {
        path: dir.clone(),
        current: true,
        changes: Some(changes),
        ..Default::default()
    };
    let _ = tab.tx.send(Msg::Loaded(Ok(Box::new(super::load::Loaded {
        refs: Default::default(),
        history: Vec::new(),
        work: git::WorkStatus::default(),
        worktrees: vec![here(0)],
        stashes: Vec::new(),
        remotes: Vec::new(),
        submodules: Vec::new(),
        lfs: false,
        identity: Default::default(),
        requested: git::LOG_PAGE,
        log_from: None,
        read: 1,
    }))));
    tab.poll(&ctx);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(tab.work.unstaged, vec![file("newer")]);
    assert_eq!(tab.worktrees, vec![here(1)]);
}

#[test]
fn headline_prefers_conflicts_and_errors() {
    let merge = "Merge x failed: Auto-merging a.rs
CONFLICT (content): Merge conflict in a.rs
Automatic merge failed";
    assert_eq!(
        headline(merge),
        "Merge x failed: conflicts in 1 file, resolve them in Changes"
    );
    let push = "Push failed: To origin
 ! [rejected] main
error: failed to push some refs";
    assert_eq!(
        headline(push),
        "Push failed: error: failed to push some refs"
    );
    assert_eq!(headline("Fetch: done"), "Fetch: done");
}
