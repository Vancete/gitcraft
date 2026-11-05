//! The right-click menu of a file, and the `.gitignore` lines it offers.

use egui::Response;

use super::file_row::split_dir;
use crate::repo::Action;
use crate::repo::file_view::FileTab;

/// Right-click menu for a file of the working tree or a commit: open it, its folder, copy path.
/// Right-click menu of a file. `rev`: the commit it is shown in (history and blame start there);
/// `ignorable`: a working-tree file that can be added to `.gitignore`.
pub fn file_menu(
    resp: &Response,
    path: &str,
    rev: Option<&str>,
    ignorable: bool,
    acts: &mut Vec<Action>,
) {
    egui::Popup::context_menu(resp)
        .style(crate::theme::menu_style)
        .show(|ui| {
            let rel = std::path::PathBuf::from(path);
            if ui.button("Open file").clicked() {
                acts.push(Action::OpenPath(rel.clone()));
            }
            if ui.button("Open in editor").clicked() {
                acts.push(Action::OpenInEditor(rel.clone()));
            }
            if ui.button(crate::repo::reveal_label()).clicked() {
                acts.push(Action::OpenPath(
                    rel.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
                ));
            }
            ui.separator();
            if ui.button("File history").clicked() {
                acts.push(Action::FileView {
                    path: path.to_owned(),
                    rev: rev.map(str::to_owned),
                    tab: FileTab::History,
                });
            }
            if ui.button("Blame").clicked() {
                acts.push(Action::FileView {
                    path: path.to_owned(),
                    rev: rev.map(str::to_owned),
                    tab: FileTab::Blame,
                });
            }
            if ignorable {
                ui.menu_button("Ignore", |ui| {
                    for (label, pattern, untrack) in ignore_patterns(path) {
                        if ui.button(label).clicked() {
                            acts.push(Action::Ignore {
                                pattern,
                                path: untrack,
                            });
                        }
                    }
                });
            }
            ui.separator();
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(path.to_owned());
            }
        });
}

/// The ways to ignore `path`: (menu label, `.gitignore` line, what to stop tracking).
fn ignore_patterns(path: &str) -> Vec<(String, String, String)> {
    let p = std::path::Path::new(path);
    let mut out = Vec::new();
    if let Some(literal) = ignore_literal(path) {
        out.push((
            "This file".to_owned(),
            format!("/{literal}"),
            path.to_owned(),
        ));
    }
    if let Some(ext) = p.extension().and_then(|e| e.to_str())
        && let Some(literal) = ignore_literal(ext)
    {
        out.push((
            format!("All .{ext} files"),
            format!("*.{literal}"),
            path.to_owned(),
        ));
    }
    let (dir, _) = split_dir(path);
    if !dir.is_empty()
        && let Some(literal) = ignore_literal(dir)
    {
        out.push((
            format!("Folder {dir}/"),
            format!("/{literal}/"),
            dir.to_owned(),
        ));
    }
    out
}

/// `name` as a `.gitignore` pattern matching just it: `*`, `?`, `[` and `\` stand for
/// themselves, and so do spaces at the end (which git would drop). `None` for a name with a
/// line break, which a `.gitignore` line can't hold.
fn ignore_literal(name: &str) -> Option<String> {
    if name.contains(['\n', '\r']) {
        return None;
    }
    let body = name.trim_end_matches(' ');
    let mut out = String::with_capacity(name.len() + 4);
    for c in body.chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    for _ in body.len()..name.len() {
        out.push_str("\\ ");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(path: &str) -> Vec<String> {
        ignore_patterns(path)
            .into_iter()
            .map(|(_, p, _)| p)
            .collect()
    }

    #[test]
    fn plain_paths_ignore_as_before() {
        assert_eq!(patterns("src/a.log"), ["/src/a.log", "*.log", "/src/"]);
        assert_eq!(patterns("README"), ["/README"]);
    }

    #[test]
    fn glob_characters_in_a_name_match_only_themselves() {
        // Unescaped, `/[ab].txt` would ignore `a.txt` and `b.txt` but not the file itself.
        assert_eq!(patterns("[ab].txt"), ["/\\[ab].txt", "*.txt"]);
        assert_eq!(patterns("x*?/y"), ["/x\\*\\?/y", "/x\\*\\?/"]);
    }

    #[test]
    fn trailing_spaces_are_kept() {
        assert_eq!(patterns("notes  "), ["/notes\\ \\ "]);
    }

    #[test]
    fn a_name_with_a_line_break_is_never_written_to_gitignore() {
        // `a\n*` would add a second line ignoring everything.
        assert!(patterns("a\n*").is_empty());
        assert_eq!(patterns("d\nx/f.rs"), ["*.rs"]);
    }

    #[test]
    fn a_nested_repository_is_ignored_as_a_folder() {
        assert_eq!(patterns("lib/sub/"), ["/lib/sub/", "/lib/"]);
    }
}
