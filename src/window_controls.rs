//! GNOME on Wayland leaves window decorations to each app, and winit's fallback is a bare bar
//! of its own above the tabs, with its buttons always on the right. There, gitcraft drops it
//! and puts the window buttons in the tab strip, on the side GNOME's `button-layout` setting
//! (GNOME Tweaks › Window Titlebars) puts them; the strip's empty space moves the window and
//! the window's edges resize it. Everywhere else the system's title bar stays.
//! `GITCRAFT_SYSTEM_TITLEBAR=1` keeps it on GNOME too.

use std::sync::OnceLock;

use egui::{
    Context, CursorIcon, PointerButton, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui,
    ViewportCommand, vec2,
};

use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

/// The window buttons at each end of the tab strip, left to right.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub left: Vec<Button>,
    pub right: Vec<Button>,
}

/// The buttons gitcraft draws itself, or `None` when the system's title bar is used.
pub fn layout() -> Option<&'static Layout> {
    static LAYOUT: OnceLock<Option<Layout>> = OnceLock::new();
    LAYOUT.get_or_init(detect).as_ref()
}

fn detect() -> Option<Layout> {
    if !cfg!(target_os = "linux") || std::env::var_os("GITCRAFT_SYSTEM_TITLEBAR").is_some() {
        return None;
    }
    let gnome = std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("gnome")));
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty());
    if !gnome || !wayland {
        return None;
    }
    let setting = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.wm.preferences", "button-layout"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
    // GNOME's own default.
    Some(parse(setting.as_deref().unwrap_or("appmenu:close")))
}

/// Reads GNOME's `button-layout`, such as `'close,minimize,maximize:appmenu'`: the buttons
/// left of the colon go on the left, the rest on the right. Other items (`appmenu`, `icon`,
/// `spacer`) are left out.
fn parse(setting: &str) -> Layout {
    let setting = setting.trim().trim_matches('\'');
    let (left, right) = setting.split_once(':').unwrap_or((setting, ""));
    let side = |items: &str| {
        items
            .split(',')
            .filter_map(|item| match item.trim() {
                "minimize" => Some(Button::Minimize),
                "maximize" => Some(Button::Maximize),
                "close" => Some(Button::Close),
                _ => None,
            })
            .collect()
    };
    Layout {
        left: side(left),
        right: side(right),
    }
}

/// Draws `buttons` left to right, round like GNOME's, and does what they say when clicked.
/// Each takes the bar's full `height`, so it sits level with whatever the row holds later.
pub fn buttons(ui: &mut Ui, buttons: &[Button], height: f32) {
    let t = Tokens::get(ui.ctx());
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    ui.spacing_mut().item_spacing.x = 8.0;
    for &button in buttons {
        let (rect, resp) = ui.allocate_exact_size(vec2(24.0, height), Sense::click());
        let fill = if resp.is_pointer_button_down_on() {
            t.pressed.lerp_to_gamma(t.text, 0.3)
        } else if resp.hovered() {
            t.pressed.lerp_to_gamma(t.text, 0.15)
        } else {
            t.pressed
        };
        let stroke = Stroke::new(1.5, if resp.hovered() { t.text } else { t.icon });
        let c = rect.center();
        let p = ui.painter();
        p.circle_filled(c, 12.0, fill);
        let tooltip = match button {
            Button::Close => {
                p.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], stroke);
                p.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], stroke);
                "Close"
            }
            Button::Minimize => {
                p.line_segment([c + vec2(-4.0, 3.0), c + vec2(4.0, 3.0)], stroke);
                "Minimize"
            }
            Button::Maximize => {
                let size = if maximized { 6.0 } else { 8.0 };
                let square = Rect::from_center_size(c, vec2(size, size));
                p.rect_stroke(square, 1.0, stroke, StrokeKind::Middle);
                if maximized { "Restore" } else { "Maximize" }
            }
        };
        if resp.on_hover_text(tooltip).clicked() {
            ui.ctx().send_viewport_cmd(match button {
                Button::Close => ViewportCommand::Close,
                Button::Minimize => ViewportCommand::Minimized(true),
                Button::Maximize => ViewportCommand::Maximized(!maximized),
            });
        }
    }
}

/// The right-hand buttons, from inside a right-to-left layout: placed last first, rightmost.
pub fn right_buttons(ui: &mut Ui, layout: &Layout, height: f32) {
    let reversed: Vec<_> = layout.right.iter().rev().copied().collect();
    buttons(ui, &reversed, height);
}

/// A title bar with only the buttons and room to move the window, for the screen shown
/// without tabs (git missing).
pub fn bare_bar(ui: &mut Ui, layout: &Layout) {
    ui.horizontal(|ui| {
        buttons(ui, &layout.left, 24.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right_buttons(ui, layout, 24.0);
            drag_area(ui, ui.available_rect_before_wrap());
        });
    });
}

/// Makes `rect`, an empty part of the tab strip, move the window when dragged and maximize
/// or restore it on a double-click, as a title bar does.
pub fn drag_area(ui: &Ui, rect: Rect) {
    let resp = ui.interact(rect, ui.id().with("window-drag"), Sense::click_and_drag());
    let ctx = ui.ctx();
    if resp.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
}

/// Resizes the window from its edges and corners, which have no frame to grab otherwise.
/// Called after the rest of the frame, so its cursor wins.
pub fn resize_edges(ctx: &Context) {
    const EDGE: f32 = 5.0;
    const CORNER: f32 = 16.0;
    let (pos, fixed, pressed) = ctx.input(|i| {
        let v = i.viewport();
        (
            i.pointer.hover_pos(),
            v.maximized == Some(true) || v.fullscreen == Some(true),
            i.pointer.primary_pressed(),
        )
    });
    let Some(pos) = pos.filter(|_| !fixed) else {
        return;
    };
    let r = ctx.viewport_rect();
    let (left, right) = (pos.x - r.left(), r.right() - pos.x);
    let (top, bottom) = (pos.y - r.top(), r.bottom() - pos.y);
    let near = |d: f32, within: f32| d < within;
    let (dir, cursor) = if (near(top, EDGE) && near(left, CORNER))
        || (near(left, EDGE) && near(top, CORNER))
    {
        (ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)
    } else if (near(top, EDGE) && near(right, CORNER)) || (near(right, EDGE) && near(top, CORNER)) {
        (ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)
    } else if (near(bottom, EDGE) && near(left, CORNER))
        || (near(left, EDGE) && near(bottom, CORNER))
    {
        (ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)
    } else if (near(bottom, EDGE) && near(right, CORNER))
        || (near(right, EDGE) && near(bottom, CORNER))
    {
        (ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)
    } else if near(top, EDGE) {
        (ResizeDirection::North, CursorIcon::ResizeNorth)
    } else if near(bottom, EDGE) {
        (ResizeDirection::South, CursorIcon::ResizeSouth)
    } else if near(left, EDGE) {
        (ResizeDirection::West, CursorIcon::ResizeWest)
    } else if near(right, EDGE) {
        (ResizeDirection::East, CursorIcon::ResizeEast)
    } else {
        return;
    };
    ctx.set_cursor_icon(cursor);
    if pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_layouts() {
        use Button::*;
        let layout = |left: &[Button], right: &[Button]| Layout {
            left: left.to_vec(),
            right: right.to_vec(),
        };
        assert_eq!(parse("'appmenu:close'\n"), layout(&[], &[Close]));
        assert_eq!(
            parse("'close,minimize,maximize:appmenu'"),
            layout(&[Close, Minimize, Maximize], &[])
        );
        assert_eq!(
            parse("appmenu:minimize,spacer,maximize,close"),
            layout(&[], &[Minimize, Maximize, Close])
        );
        assert_eq!(parse("close"), layout(&[Close], &[]));
        assert_eq!(parse(""), layout(&[], &[]));
    }
}
