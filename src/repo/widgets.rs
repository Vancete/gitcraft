//! Small painted widgets shared by the repository panes.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Ui, pos2, vec2};

use crate::icons;
use crate::theme::{self, Tokens};

pub const ROW_H: f32 = 24.0;

pub fn toolbar_separator(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t.separator);
    ui.add_space(4.0);
}

/// Width of a folded side panel: its footer's toggle and the footer's margins.
pub const FOLDED_W: f32 = 36.0;
/// Height of [`panel_header`].
pub const PANEL_HEADER_H: f32 = 30.0;

/// How far a side panel whose outer edge belongs at `outer_edge` (the right one for a panel
/// on the right) is slid out of place. A panel folding or opening slides toward its outer
/// edge, where the part still in view is: what is moved back by this stays put there instead
/// of sliding out of sight and back.
pub fn slide_offset(ui: &Ui, outer_edge: f32, right: bool) -> f32 {
    let panel = ui.max_rect();
    if right {
        outer_edge - panel.right()
    } else {
        outer_edge - panel.left()
    }
}

/// The row of buttons along the bottom of a side panel, starting from its outer edge (the
/// right one for a panel on the right), which lies at `outer_edge`. Call it before the
/// panel's other contents.
pub fn panel_footer(
    ui: &mut Ui,
    id: &'static str,
    outer_edge: f32,
    right: bool,
    add_buttons: impl FnOnce(&mut Ui),
) {
    let t = Tokens::get(ui.ctx());
    let slid = slide_offset(ui, outer_edge, right);
    egui::Panel::bottom(id)
        .frame(
            egui::Frame::new()
                .fill(t.tab_strip)
                .inner_margin(egui::Margin::same(4)),
        )
        .show(ui, |ui| {
            let layout = if right {
                egui::Layout::right_to_left(egui::Align::Center)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            };
            // A child the parent doesn't grow by: the slid rect lies partly outside the panel.
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(ui.max_rect().translate(vec2(slid, 0.0)))
                    .layout(layout),
            );
            row.spacing_mut().item_spacing.x = 2.0;
            add_buttons(&mut row);
            ui.allocate_exact_size(vec2(0.0, row.min_rect().height()), Sense::hover());
        });
}

/// A rounded label with a tinted background.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = theme::medium(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = vec2(galley.size().x + 12.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.18));
    ui.painter().galley(
        pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// The strip at the top of a side panel, like PhotoCraft's panel tab strips: a tab with the
/// title, and room on the right for buttons.
pub fn panel_header(ui: &mut Ui, title: &str, add_buttons: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), PANEL_HEADER_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, t.tab_strip);
    let font = theme::semibold(11.5);
    let galley = painter.layout_no_wrap(title.to_owned(), font, t.text);
    let tab = Rect::from_min_size(
        pos2(rect.left() + 4.0, rect.top() + 4.0),
        vec2(galley.size().x + 24.0, rect.height() - 4.0),
    );
    painter.rect_filled(
        tab,
        CornerRadius {
            nw: t.radius as u8,
            ne: t.radius as u8,
            sw: 0,
            se: 0,
        },
        t.card,
    );
    painter.galley(
        pos2(tab.left() + 12.0, tab.center().y - galley.size().y / 2.0),
        galley,
        t.text,
    );
    let buttons = Rect::from_min_max(
        pos2(tab.right() + 4.0, rect.top()),
        rect.max - vec2(6.0, 0.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(buttons)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    add_buttons(&mut child);
}

pub fn status_color(t: &Tokens, code: char) -> Color32 {
    match code {
        'A' | '?' => t.success,
        'D' => t.danger,
        'R' | 'C' => t.accent,
        'U' => t.danger,
        _ => t.warning,
    }
}

/// What a file row shows on its right edge.
pub enum RowExtra<'a> {
    /// `+added −removed` (`None` for binary files).
    Stats(Option<u32>, Option<u32>),
    /// Icon buttons shown while the row is hovered: `(icon, tooltip)`.
    Buttons(&'a [(&'a str, &'a str)]),
}

pub struct FileRowResponse {
    pub row: Response,
    /// Index of the clicked hover button.
    pub button: Option<usize>,
}

/// One changed file: status letter, file name, its folder dimmed, and `extra` on the right.
/// In a tree (`indent` is how far in) the folder is above the row instead.
pub fn file_row(
    ui: &mut Ui,
    code: char,
    path: &str,
    old_path: Option<&str>,
    indent: Option<f32>,
    selected: bool,
    extra: RowExtra<'_>,
) -> FileRowResponse {
    let shift = indent.unwrap_or(0.0);
    let t = Tokens::get(ui.ctx());
    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 0.0, t.row_selected);
    } else if hovered {
        painter.rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;

    // Status letter in a small rounded square.
    let color = status_color(&t, code);
    let badge = Rect::from_center_size(pos2(rect.left() + 18.0 + shift, y), vec2(16.0, 16.0));
    painter.rect_filled(badge, CornerRadius::same(3), color.gamma_multiply(0.22));
    let letter = if code == '?' { 'A' } else { code };
    painter.text(
        badge.center(),
        Align2::CENTER_CENTER,
        letter.to_string(),
        theme::semibold(10.5),
        color,
    );

    // Right edge first, so the name is clipped before it.
    let mut right = rect.right() - 8.0;
    let mut button = None;
    match extra {
        RowExtra::Stats(added, removed) => {
            let small = theme::mono(11.0);
            match (added, removed) {
                (Some(a), Some(d)) => {
                    let r = painter.text(
                        pos2(right, y),
                        Align2::RIGHT_CENTER,
                        format!("−{d}"),
                        small.clone(),
                        t.danger,
                    );
                    right = r.left() - 6.0;
                    let r = painter.text(
                        pos2(right, y),
                        Align2::RIGHT_CENTER,
                        format!("+{a}"),
                        small,
                        t.success,
                    );
                    right = r.left() - 8.0;
                }
                _ => {
                    let r = painter.text(
                        pos2(right, y),
                        Align2::RIGHT_CENTER,
                        "bin",
                        small,
                        t.text_faint,
                    );
                    right = r.left() - 8.0;
                }
            }
        }
        RowExtra::Buttons(buttons) => {
            if hovered || selected {
                for (i, (icon, tooltip)) in buttons.iter().enumerate().rev() {
                    let r = Rect::from_center_size(pos2(right - 10.0, y), vec2(20.0, 20.0));
                    let resp = ui
                        .interact(r, row.id.with(("btn", i)), Sense::click())
                        .on_hover_text(*tooltip);
                    if resp.hovered() {
                        painter.rect_filled(r, t.radius_sm, t.pressed);
                    }
                    icons::paint(
                        ui,
                        r,
                        icon,
                        14.0,
                        if resp.hovered() { t.text } else { t.icon },
                    );
                    if resp.clicked() {
                        button = Some(i);
                    }
                    right = r.left() - 2.0;
                }
            }
        }
    }

    let (dir, name) = split_dir(path);
    let text_clip = Rect::from_min_max(
        pos2(rect.left() + 32.0 + shift, rect.top()),
        pos2(right, rect.bottom()),
    );
    let tp = painter.with_clip_rect(text_clip);
    let name_rect = tp.text(
        pos2(text_clip.left(), y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        t.text,
    );
    let mut faint = String::new();
    if let Some(old) = old_path {
        faint.push_str("← ");
        faint.push_str(old);
        faint.push_str("  ");
    }
    if indent.is_none() {
        faint.push_str(dir);
    }
    if !faint.is_empty() {
        tp.text(
            pos2(name_rect.right() + 8.0, y),
            Align2::LEFT_CENTER,
            faint,
            egui::FontId::proportional(11.5),
            t.text_faint,
        );
    }
    // The full path only adds something when the row shows it cut short.
    let row = match old_path {
        Some(old) => row.on_hover_text(format!("{old} → {path}")),
        None if !dir.is_empty() => row.on_hover_text(path),
        None => row,
    };
    FileRowResponse { row, button }
}

/// `(folder, name)` of a path: `src/repo` and `mod.rs`. A slash at the end (git shows a nested
/// repository as `sub/`) stays with the name.
fn split_dir(path: &str) -> (&str, &str) {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    match trimmed.rsplit_once('/') {
        Some((dir, _)) => (dir, &path[dir.len() + 1..]),
        None => ("", path),
    }
}

/// A primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let fill = if enabled { t.accent } else { t.hover };
    let text_color = if enabled { t.accent_text } else { t.text_faint };
    ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(text)
                .color(text_color)
                .font(theme::medium(12.5)),
        )
        .fill(fill)
        .min_size(vec2(width, 30.0)),
    )
}

/// A round avatar: the author's picture when one is loaded (see `avatars`), their initials in
/// a colour of their own until then.
pub fn avatar(ui: &Ui, center: egui::Pos2, radius: f32, name: &str, email: &str) {
    let rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
    if crate::avatars::paint(ui, rect, email) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter();
    painter.circle_filled(center, radius, t.lane(name_hash(name)));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initials(name),
        theme::semibold(radius * 0.8),
        Color32::WHITE,
    );
}

fn initials(name: &str) -> String {
    let mut parts = name.split_whitespace().filter_map(|w| w.chars().next());
    let first = parts.next().unwrap_or('?');
    let last = parts.next_back();
    let mut s: String = first.to_uppercase().collect();
    if let Some(l) = last {
        s.extend(l.to_uppercase());
    }
    s
}

fn name_hash(name: &str) -> usize {
    name.bytes()
        .fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}

/// The accent line under an active toolbar toggle.
pub fn active_underline(ui: &Ui, button: Rect, color: Color32) {
    ui.painter().line_segment(
        [
            button.left_bottom() + vec2(6.0, -1.0),
            button.right_bottom() + vec2(-6.0, -1.0),
        ],
        egui::Stroke::new(2.0, color),
    );
}

/// The terminal panel's strip: one tab per shell, `+` with a shell menu, and close.
pub fn terminal_tabs(
    ui: &mut Ui,
    terminals: &[crate::terminal::Terminal],
    active: usize,
    acts: &mut Vec<super::Action>,
) {
    let t = Tokens::get(ui.ctx());
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(strip, 0.0, t.tab_strip);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(strip.shrink2(vec2(4.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Max)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 2.0;
    for (i, term) in terminals.iter().enumerate() {
        let selected = i == active;
        let label = format!("{} {}", i + 1, term.shell);
        let font = if selected {
            theme::semibold(11.5)
        } else {
            theme::medium(11.5)
        };
        let galley =
            ui.painter()
                .layout_no_wrap(label, font, if selected { t.text } else { t.text_dim });
        let (rect, resp) =
            ui.allocate_exact_size(vec2(galley.size().x + 44.0, 26.0), Sense::click());
        let shape = CornerRadius {
            nw: t.radius as u8,
            ne: t.radius as u8,
            sw: 0,
            se: 0,
        };
        if selected {
            ui.painter().rect_filled(rect, shape, t.card);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, shape, t.hover);
        }
        icons::paint(
            ui,
            Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(14.0, 14.0)),
            "square-terminal",
            12.0,
            t.text_faint,
        );
        ui.painter().galley(
            pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0),
            galley,
            t.text,
        );
        let x_rect =
            Rect::from_center_size(pos2(rect.right() - 11.0, rect.center().y), vec2(16.0, 16.0));
        let x = ui
            .interact(x_rect, resp.id.with("close"), Sense::click())
            .on_hover_text("Close (ends the shell)");
        if x.hovered() {
            ui.painter().rect_filled(x_rect, t.radius_sm, t.pressed);
        }
        icons::paint(ui, x_rect, "x", 11.0, t.text_faint);
        if x.clicked() {
            acts.push(super::Action::CloseTerminal(i));
        } else if resp.clicked() {
            acts.push(super::Action::SelectTerminal(i));
        }
    }
    ui.add_space(4.0);
    let plus = icons::small_button(ui, "plus", "New terminal");
    if plus.clicked() {
        acts.push(super::Action::NewTerminal {
            shell: None,
            cwd: None,
        });
    }
    egui::Popup::context_menu(&plus)
        .style(crate::theme::menu_style)
        .show(|ui| {
            for shell in crate::terminal::available_shells() {
                if ui.button(&shell.label).clicked() {
                    acts.push(super::Action::NewTerminal {
                        shell: Some(shell),
                        cwd: None,
                    });
                }
            }
        });
    let more = icons::small_button(ui, "chevron-down", "New terminal with…");
    egui::Popup::menu(&more)
        .style(crate::theme::menu_style)
        .show(|ui| {
            for shell in crate::terminal::available_shells() {
                if ui.button(&shell.label).clicked() {
                    acts.push(super::Action::NewTerminal {
                        shell: Some(shell),
                        cwd: None,
                    });
                }
            }
        });
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        if icons::small_button(ui, "x", "Hide the panel (shells keep running)").clicked() {
            acts.push(super::Action::ToggleTerminal);
        }
    });
}

/// Right-click menu for a file of the working tree or a commit: open it, its folder, copy path.
/// Right-click menu of a file. `rev`: the commit it is shown in (history and blame start there);
/// `ignorable`: a working-tree file that can be added to `.gitignore`.
pub fn file_menu(
    resp: &Response,
    path: &str,
    rev: Option<&str>,
    ignorable: bool,
    acts: &mut Vec<super::Action>,
) {
    use super::Action;
    use super::file_view::FileTab;
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
            if ui.button(super::reveal_label()).clicked() {
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

    #[test]
    fn a_path_splits_into_folder_and_name() {
        assert_eq!(split_dir("src/repo/mod.rs"), ("src/repo", "mod.rs"));
        assert_eq!(split_dir("Cargo.toml"), ("", "Cargo.toml"));
        assert_eq!(split_dir("dös/ñame.rs"), ("dös", "ñame.rs"));
    }

    #[test]
    fn a_nested_repository_keeps_its_name() {
        // `git status` shows a repository inside the working tree as `sub/`.
        assert_eq!(split_dir("sub/"), ("", "sub/"));
        assert_eq!(split_dir("lib/sub/"), ("lib", "sub/"));
    }

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
