//! Lucide icons (ISC, see `assets/icons/LICENSE-lucide.txt`), embedded as SVG, recoloured to
//! white, rasterised by egui_extras at the on-screen size and tinted per use. Same approach as
//! PhotoCraft: no Unicode glyphs, which half the time render as boxes.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use egui::{Color32, Rect, Response, Sense, Ui, Vec2, load::Bytes};

use crate::theme::Tokens;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../assets/icons/", $name, ".svg")))),*]
    };
}

const SOURCES: &[(&str, &str)] = icons![
    "archive",
    "arrow-down",
    "arrow-down-to-line",
    "arrow-up",
    "arrow-up-from-line",
    "book-open",
    "check",
    "chevron-down",
    "chevron-right",
    "chevron-up",
    "circle-alert",
    "cloud",
    "cloud-download",
    "code",
    "columns-2",
    "command",
    "copy",
    "database",
    "external-link",
    "file-diff",
    "flask-conical",
    "folder-git-2",
    "folder-open",
    "folder-plus",
    "folder-tree",
    "gamepad-2",
    "git-branch",
    "git-branch-plus",
    "git-merge",
    "git-pull-request",
    "globe",
    "heart",
    "history",
    "laptop",
    "log-in",
    "minus",
    "package",
    "palette",
    "pilcrow",
    "plus",
    "redo-2",
    "refresh-cw",
    "rocket",
    "rows-2",
    "search",
    "server",
    "settings",
    "smartphone",
    "square-terminal",
    "star",
    "tag",
    "undo-2",
    "user-round",
    "wrench",
    "x",
];

fn white_icons() -> &'static HashMap<&'static str, Arc<[u8]>> {
    static MAP: OnceLock<HashMap<&'static str, Arc<[u8]>>> = OnceLock::new();
    MAP.get_or_init(|| {
        SOURCES
            .iter()
            .map(|(name, svg)| {
                let white = svg.replace("currentColor", "#ffffff");
                (*name, Arc::from(white.into_bytes().into_boxed_slice()))
            })
            .collect()
    })
}

/// An egui image for an icon, tinted. Unknown names give an empty image.
pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    let bytes = white_icons()
        .get(name)
        .cloned()
        .unwrap_or_else(|| Arc::from(Vec::new().into_boxed_slice()));
    egui::Image::from_bytes(format!("bytes://icons/{name}.svg"), Bytes::Shared(bytes))
        .fit_to_exact_size(Vec2::splat(size))
        .tint(tint)
}

/// Paints an icon centred in `rect`.
pub fn paint(ui: &Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    let r = Rect::from_center_size(rect.center(), Vec2::splat(size));
    image(name, size, tint).paint_at(ui, r);
}

/// A flat toolbar button: icon plus optional label, background only on hover (PhotoCraft's
/// options-bar buttons).
pub fn tool_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> Response {
    tool_button_count(ui, icon, label, 0, enabled)
}

/// A toolbar button with `count` in a circle after its label (none when 0).
pub fn tool_button_count(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    count: u32,
    enabled: bool,
) -> Response {
    // Painted by hand at a fixed size: egui's button adds its border's width only while the
    // frame shows, so hovering one pushed the rest of the toolbar sideways.
    const PAD: f32 = 10.0;
    const ICON: f32 = 16.0;
    const GAP: f32 = 6.0;
    let t = Tokens::get(ui.ctx());
    let tint = if enabled { t.icon } else { t.text_faint };
    let color = if enabled { t.text } else { t.text_faint };
    let galley = (!label.is_empty()).then(|| {
        let font = egui::TextStyle::Button.resolve(ui.style());
        ui.painter().layout_no_wrap(label.to_owned(), font, color)
    });
    const BADGE: f32 = 16.0;
    let badge = (count > 0).then(|| {
        let text = if count > 99 {
            "99+".to_owned()
        } else {
            count.to_string()
        };
        let galley = ui
            .painter()
            .layout_no_wrap(text, crate::theme::semibold(10.0), t.accent_text);
        // A circle for one digit, stretched into a pill for more.
        let width = (galley.size().x + 8.0).max(BADGE);
        (galley, width)
    });
    let badge_width = badge.as_ref().map_or(0.0, |(_, w)| GAP + w);
    let width = match &galley {
        Some(g) => PAD + ICON + GAP + g.size().x + badge_width + PAD,
        None => 28.0,
    };
    ui.add_enabled(enabled, |ui: &mut Ui| {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
        let r = egui::CornerRadius::same(t.radius_sm as u8);
        if resp.is_pointer_button_down_on() {
            ui.painter().rect_filled(rect, r, t.pressed);
        } else if resp.hovered() || resp.has_focus() {
            ui.painter().rect_filled(rect, r, t.hover);
            ui.painter().rect_stroke(
                rect,
                r,
                egui::Stroke::new(1.0, t.field_border),
                egui::StrokeKind::Inside,
            );
        }
        let icon_left = if galley.is_some() {
            rect.left() + PAD
        } else {
            rect.center().x - ICON / 2.0
        };
        let icon_rect = Rect::from_min_size(
            egui::pos2(icon_left, rect.center().y - ICON / 2.0),
            Vec2::splat(ICON),
        );
        paint(ui, icon_rect, icon, ICON, tint);
        if let Some(g) = galley {
            let pos = egui::pos2(icon_rect.right() + GAP, rect.center().y - g.size().y / 2.0);
            let text_right = pos.x + g.size().x;
            ui.painter().galley(pos, g, color);
            if let Some((count, w)) = badge {
                let pill = Rect::from_min_size(
                    egui::pos2(text_right + GAP, rect.center().y - BADGE / 2.0),
                    Vec2::new(w, BADGE),
                );
                let fill = if enabled { t.accent } else { t.text_faint };
                ui.painter().rect_filled(pill, BADGE / 2.0, fill);
                ui.painter()
                    .galley(pill.center() - count.size() / 2.0, count, t.accent_text);
            }
        }
        resp
    })
}

/// A small square icon-only button (rows, panel headers).
pub fn small_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.pressed);
    }
    let tint = if !ui.is_enabled() {
        t.text_faint
    } else if resp.hovered() {
        t.text
    } else {
        t.icon
    };
    paint(ui, rect, icon, 14.0, tint);
    resp.on_hover_text(tooltip)
}

/// An icon-only button of the window's title bar, a size up from [`small_button`].
pub fn bar_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius, t.pressed);
    }
    let tint = if resp.hovered() { t.text } else { t.icon };
    paint(ui, rect, icon, 17.0, tint);
    resp.on_hover_text(tooltip)
}

/// An icon laid out inline, like a label.
pub fn inline(ui: &mut Ui, name: &str, size: f32, tint: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui, rect, name, size, tint);
}
