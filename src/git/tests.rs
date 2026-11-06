//! Tests of the helpers in `mod.rs`, and one that goes through most of the wrapper against a
//! real repository.

use super::*;

/// The user's config doesn't change what gitcraft reads, and a ref that looks like an option
/// (a tag fetched from somewhere) is read as a ref.
#[test]
fn hostile_config_and_option_like_refs() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("gitcraft-hostile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let git = |args: &[&str]| run(Some(&dir), args);
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@t"],
        &["config", "commit.gpgSign", "false"],
        &["config", "core.autocrlf", "false"],
        &["config", "color.ui", "always"],
        &["config", "diff.external", "false"],
        &["config", "diff.noprefix", "true"],
        &["config", "log.showSignature", "true"],
    ] {
        git(args)?;
    }
    // A name git quotes in its output. Windows allows no `"` in file names; a letter
    // outside ASCII is quoted too.
    let name = if cfg!(windows) { "qé.txt" } else { "q\"x.txt" };
    std::fs::write(dir.join(name), "a\n").map_err(|e| e.to_string())?;
    git(&["add", "-A"])?;
    git(&["commit", "-qm", "one"])?;
    std::fs::write(dir.join(name), "b\n").map_err(|e| e.to_string())?;
    git(&["commit", "-qam", "two"])?;
    let tag = "--output=written";
    git(&["update-ref", &format!("refs/tags/{tag}"), "HEAD~1"])?;
    let head = head(&dir);

    let commits = log(&dir, None, 0, 10)?;
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].hash, head);
    // From the option-like tag alone: only the commit it is on.
    let from_tag = log(&dir, Some(&[format!("refs/tags/{tag}")]), 0, 10)?;
    assert_eq!(from_tag.len(), 1);
    assert!(log(&dir, Some(&[]), 0, 10)?.is_empty());
    let tag_only = [format!("refs/tags/{tag}")];
    assert_eq!(search(&dir, None, "two")?, std::slice::from_ref(&head));
    // A query that names a commit (here a tag `2024`) and is in its message: found once.
    git(&["commit", "-q", "--allow-empty", "-m", "release 2024"])?;
    git(&["tag", "2024"])?;
    let found = search(&dir, None, "2024")?;
    let release = git(&["rev-parse", "HEAD"])?;
    git(&["tag", "-d", "2024"])?;
    git(&["reset", "-q", "--soft", "HEAD~1"])?;
    assert_eq!(found, [release.trim()]);
    assert!(search(&dir, Some(&tag_only), "two")?.is_empty());
    assert_eq!(search_code(&dir, Some(&tag_only), "a")?.len(), 1);
    let patch = file_diff(&dir, &head, name, DiffOptions::default())?;
    assert!(patch.starts_with("diff --git \"a/q"), "{patch}");
    assert!(!patch.contains('\x1b'), "{patch}");
    let range = range_file_diff(&dir, tag, Some(&head), name, DiffOptions::default())?;
    assert!(range.contains("+b"), "{range}");
    assert_eq!(diff_files(&dir, tag, None)?.len(), 1);
    assert_eq!(count_apart(&dir, tag, "HEAD")?, (0, 1));
    assert!(merge_base(&dir, tag, "HEAD").is_ok());
    assert_eq!(rebase_commits(&dir, tag)?.len(), 1);
    assert_eq!(file_at(&dir, Some(tag), name)?, b"a\n");
    assert_eq!(tree_files(&dir, tag)?, [name]);
    assert!(resolve(&dir, tag).is_err());
    assert!(blame(&dir, Some(tag), name).is_err());
    assert!(!dir.join("written").exists());
    std::fs::write(dir.join(name), "c\n").map_err(|e| e.to_string())?;
    let file = StatusFile {
        path: name.to_owned(),
        old_path: None,
        code: 'M',
        area: Area::Unstaged,
    };
    let work = work_diff(&dir, &file, DiffOptions::default())?;
    assert!(work.contains("+c") && !work.contains('\x1b'), "{work}");
    let history = file_history(&dir, None, name)?;
    assert_eq!(history.len(), 2);
    // A rename is shown as one, in a commit and staged.
    git(&["mv", name, "moved.txt"])?;
    let staged = StatusFile {
        path: "moved.txt".to_owned(),
        old_path: Some(name.to_owned()),
        code: 'R',
        area: Area::Staged,
    };
    let work = work_diff(&dir, &staged, DiffOptions::default())?;
    assert!(work.contains("rename from"), "{work}");
    git(&["commit", "-qm", "three"])?;
    let moved = run(Some(&dir), &["rev-parse", "HEAD"])?;
    let patch = file_diff(&dir, moved.trim(), "moved.txt", DiffOptions::default())?;
    assert!(patch.contains("rename from"), "{patch}");
    assert_eq!(head_message(&dir)?, "three");
    git(&["reset", "-q", "--hard", "HEAD~1"])?;
    assert!(history.iter().all(|c| c.path == name), "{history:?}");
    assert_eq!(head_message(&dir)?, "two");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn relative_times() {
    assert_eq!(relative_time(1000, 1030), "just now");
    assert_eq!(relative_time(0, 60), "1 minute ago");
    assert_eq!(relative_time(0, 2 * 3_600), "2 hours ago");
    assert_eq!(relative_time(0, 5 * 86_400), "5 days ago");
    assert_eq!(relative_time(0, 3 * 604_800), "3 weeks ago");
    assert_eq!(relative_time(0, 400 * 86_400), "1 year ago");
    assert_eq!(relative_time(100, 50), "just now");
}

#[test]
fn clone_folder_names() {
    assert_eq!(clone_dir_name("https://github.com/a/b.git"), "b");
    assert_eq!(clone_dir_name("git@github.com:a/b.git"), "b");
    assert_eq!(clone_dir_name("C:\\repos\\thing\\"), "thing");
    assert_eq!(clone_dir_name("git@host:proj"), "proj");
    assert_eq!(clone_dir_name("/home/me/proj/.git"), "proj");
    assert_eq!(clone_dir_name("C:\\src\\proj\\.git\\"), "proj");
    assert_eq!(clone_dir_name("https://host/a/.git"), "a");
}

/// Older history read page after page from one running `git log` is what `log` gives at once.
#[test]
fn history_read_in_pages_is_the_whole_log() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("gitcraft-log-pages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let git = |args: &[&str]| run(Some(&dir), args);
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@t"],
        &["config", "commit.gpgSign", "false"],
    ] {
        git(args)?;
    }
    for i in 0..23 {
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            &format!("c{i}\n\nbody {i}"),
        ])?;
        if i % 5 == 0 {
            git(&["branch", &format!("b{i}")])?;
        }
    }
    let whole = log(&dir, None, 0, 100)?;
    let mut stream = LogStream::start(&dir, None, 3)?;
    let mut paged = log(&dir, None, 0, 3)?;
    loop {
        let page = stream.next_page(7)?;
        let last = page.len() < 7;
        paged.extend(page);
        if last {
            break;
        }
    }
    // No revisions have no history, as with `log` (git alone would log HEAD).
    let none = LogStream::start(&dir, Some(&[]), 0)?.next_page(7)?;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(none.is_empty());
    let hashes = |c: &[Commit]| c.iter().map(|c| c.hash.clone()).collect::<Vec<_>>();
    assert_eq!(whole.len(), 23);
    assert_eq!(hashes(&paged), hashes(&whole));
    assert_eq!(paged[22].summary, whole[22].summary);
    assert_eq!(paged[22].body, "body 0");
    assert_eq!(paged[20].refs, whole[20].refs);
    Ok(())
}

/// A `git log` that fails (a revision gone) says why, rather than looking like the end of
/// history.
#[test]
fn history_read_in_pages_reports_failure() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("gitcraft-log-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    run(Some(&dir), &["init", "-q"])?;
    run(
        Some(&dir),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "a",
        ],
    )?;
    let gone = ["no-such-branch".to_owned()];
    let result = LogStream::start(&dir, Some(&gone), 0).and_then(|mut s| s.next_page(10));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err());
    Ok(())
}
