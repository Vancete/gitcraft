//! A repository tab's own icon and highlight colour, picked from the tab's right-click menu.

use egui::{Color32, Rect, RichText, Sense, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};

use crate::icons;
use crate::theme::{self, Tokens};

/// The icons a tab can wear: (icon, what it suggests). The first is the default.
pub const ICONS: &[(&str, &str)] = &[
    ("folder-git-2", "Repository"),
    ("code", "Code"),
    ("globe", "Web"),
    ("smartphone", "Mobile"),
    ("server", "Server"),
    ("database", "Database"),
    ("package", "Library"),
    ("square-terminal", "Tool"),
    ("rocket", "Launch"),
    ("gamepad-2", "Game"),
    ("flask-conical", "Experiment"),
    ("book-open", "Docs"),
    ("palette", "Design"),
    ("wrench", "Config"),
    ("star", "Favourite"),
    ("heart", "Personal"),
];

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabColor {
    #[default]
    Blue,
    Teal,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
    Gray,
}

impl TabColor {
    pub const ALL: [TabColor; 9] = [
        TabColor::Blue,
        TabColor::Teal,
        TabColor::Green,
        TabColor::Yellow,
        TabColor::Orange,
        TabColor::Red,
        TabColor::Pink,
        TabColor::Purple,
        TabColor::Gray,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TabColor::Blue => "Blue",
            TabColor::Teal => "Teal",
            TabColor::Green => "Green",
            TabColor::Yellow => "Yellow",
            TabColor::Orange => "Orange",
            TabColor::Red => "Red",
            TabColor::Pink => "Pink",
            TabColor::Purple => "Purple",
            TabColor::Gray => "Gray",
        }
    }

    /// Blue is the theme's accent; the others are the same in every theme.
    pub fn color(self, t: &Tokens) -> Color32 {
        match self {
            TabColor::Blue => t.accent,
            TabColor::Teal => Color32::from_rgb(20, 178, 160),
            TabColor::Green => Color32::from_rgb(64, 180, 96),
            TabColor::Yellow => Color32::from_rgb(230, 178, 20),
            TabColor::Orange => Color32::from_rgb(240, 128, 40),
            TabColor::Red => Color32::from_rgb(230, 72, 72),
            TabColor::Pink => Color32::from_rgb(232, 80, 156),
            TabColor::Purple => Color32::from_rgb(150, 104, 236),
            TabColor::Gray => Color32::from_rgb(140, 142, 152),
        }
    }
}

/// How a repository's tab looks.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabLook {
    /// One of [`ICONS`]; `None` for the default.
    pub icon: Option<String>,
    pub color: TabColor,
}

impl TabLook {
    pub fn icon(&self) -> &str {
        self.icon.as_deref().unwrap_or(ICONS[0].0)
    }
}

/// The menu's icon grid and colour swatches; returns the look picked, if any.
pub fn menu(ui: &mut Ui, look: &TabLook) -> Option<TabLook> {
    let t = Tokens::get(ui.ctx());
    let accent = look.color.color(&t);
    let mut picked = None;
    let heading = |ui: &mut Ui, text: &str| {
        ui.label(
            RichText::new(text)
                .font(theme::semibold(10.5))
                .color(t.text_faint),
        );
    };

    // Menus have no side margin of their own (their buttons pad themselves); the grids get one.
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            heading(ui, "ICON");
            for row in ICONS.chunks(8) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for &(name, what) in row {
                        let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                        let current = look.icon() == name;
                        if current {
                            ui.painter().rect_filled(
                                rect,
                                t.radius_sm,
                                accent.gamma_multiply(0.22),
                            );
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, t.radius_sm, t.hover);
                        }
                        let tint = if current { accent } else { t.text_dim };
                        icons::paint(
                            ui,
                            Rect::from_center_size(rect.center(), vec2(16.0, 16.0)),
                            name,
                            15.0,
                            tint,
                        );
                        if resp.on_hover_text(what).clicked() {
                            picked = Some(TabLook {
                                icon: (name != ICONS[0].0).then(|| name.to_owned()),
                                ..look.clone()
                            });
                        }
                    }
                });
            }

            ui.add_space(6.0);
            heading(ui, "HIGHLIGHT");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for color in TabColor::ALL {
                    let (rect, resp) = ui.allocate_exact_size(vec2(25.0, 25.0), Sense::click());
                    let c = rect.center();
                    if color == look.color {
                        ui.painter()
                            .circle_stroke(c, 10.0, Stroke::new(1.5, t.text));
                    } else if resp.hovered() {
                        ui.painter()
                            .circle_stroke(c, 10.0, Stroke::new(1.5, t.text_faint));
                    }
                    ui.painter().circle_filled(c, 7.0, color.color(&t));
                    if resp.on_hover_text(color.label()).clicked() {
                        picked = Some(TabLook {
                            color,
                            ..look.clone()
                        });
                    }
                }
            });
        });

    if *look != TabLook::default() {
        ui.separator();
        if ui.button("Reset to the default look").clicked() {
            picked = Some(TabLook::default());
        }
    }
    picked
}
