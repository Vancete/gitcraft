//! A changed file's row in a file list or tree.

use egui::{Align2, CornerRadius, Painter, Rect, Response, Sense, Ui, pos2, vec2};

use super::{ROW_H, status_color};
use crate::icons;
use crate::theme::{self, Tokens};

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
            right = stats(&painter, &t, right, y, added, removed);
        }
        RowExtra::Buttons(buttons) => {
            if hovered || selected {
                (right, button) = hover_buttons(ui, &painter, &t, &row, right, buttons);
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

/// `+added −removed` (or `bin`) right-aligned at `right`; returns where the name must end.
fn stats(
    painter: &Painter,
    t: &Tokens,
    right: f32,
    y: f32,
    added: Option<u32>,
    removed: Option<u32>,
) -> f32 {
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
            let r = painter.text(
                pos2(r.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                format!("+{a}"),
                small,
                t.success,
            );
            r.left() - 8.0
        }
        _ => {
            let r = painter.text(
                pos2(right, y),
                Align2::RIGHT_CENTER,
                "bin",
                small,
                t.text_faint,
            );
            r.left() - 8.0
        }
    }
}

/// The row's icon buttons, right-aligned at `right`; returns where the name must end and the
/// clicked button.
fn hover_buttons(
    ui: &Ui,
    painter: &Painter,
    t: &Tokens,
    row: &Response,
    mut right: f32,
    buttons: &[(&str, &str)],
) -> (f32, Option<usize>) {
    let y = row.rect.center().y;
    let mut clicked = None;
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
            clicked = Some(i);
        }
        right = r.left() - 2.0;
    }
    (right, clicked)
}

/// `(folder, name)` of a path: `src/repo` and `mod.rs`. A slash at the end (git shows a nested
/// repository as `sub/`) stays with the name.
pub(super) fn split_dir(path: &str) -> (&str, &str) {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    match trimmed.rsplit_once('/') {
        Some((dir, _)) => (dir, &path[dir.len() + 1..]),
        None => ("", path),
    }
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
}
