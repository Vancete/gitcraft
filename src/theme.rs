//! Design tokens, modelled on PhotoCraft's "Pro" themes: flat grey panels, a darker tab strip,
//! a blue accent, Inter for the UI and JetBrains Mono for hashes and diffs.
//!
//! Widgets read [`Tokens::get`] instead of hard-coding colours, so every theme applies everywhere.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Visuals,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    /// Charcoal panels (PhotoCraft's "Pro (Dark)").
    #[default]
    Dark,
    /// Mid-grey panels (PhotoCraft's default "Pro (Medium Gray)").
    MediumGray,
    /// Light panels (PhotoCraft's "Pro (Light)").
    Light,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 3] = [ThemeKind::Dark, ThemeKind::MediumGray, ThemeKind::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "Dark",
            ThemeKind::MediumGray => "Medium Gray",
            ThemeKind::Light => "Light",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Toolbars and the active tab.
    pub chrome: Color32,
    /// Behind panels (the graph area).
    pub dock: Color32,
    /// Side panels.
    pub card: Color32,
    /// Inputs and code blocks.
    pub field: Color32,
    pub field_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub separator: Color32,
    pub shadow: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub success: Color32,
    /// Window tab strip and panel headers.
    pub tab_strip: Color32,
    pub row_selected: Color32,
    pub diff_add_bg: Color32,
    pub diff_del_bg: Color32,
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Dark => Tokens {
                kind,
                chrome: Color32::from_rgb(50, 50, 50),
                dock: Color32::from_rgb(40, 40, 40),
                card: Color32::from_rgb(50, 50, 50),
                field: Color32::from_rgb(36, 36, 36),
                field_border: Color32::from_rgb(74, 74, 74),
                hover: Color32::from_rgb(66, 66, 66),
                pressed: Color32::from_rgb(78, 78, 78),
                text: Color32::from_rgb(222, 222, 222),
                text_dim: Color32::from_rgb(178, 178, 178),
                text_faint: Color32::from_rgb(128, 128, 128),
                icon: Color32::from_rgb(200, 200, 200),
                accent: Color32::from_rgb(55, 142, 240),
                accent_text: Color32::WHITE,
                separator: Color32::from_rgb(30, 30, 30),
                shadow: Color32::from_black_alpha(150),
                danger: Color32::from_rgb(236, 91, 98),
                warning: Color32::from_rgb(232, 176, 70),
                success: Color32::from_rgb(92, 196, 122),
                tab_strip: Color32::from_rgb(38, 38, 38),
                row_selected: Color32::from_rgb(38, 76, 122),
                diff_add_bg: Color32::from_rgba_unmultiplied(92, 196, 122, 34),
                diff_del_bg: Color32::from_rgba_unmultiplied(236, 91, 98, 34),
                radius_sm: 3.0,
                radius: 4.0,
                radius_lg: 6.0,
            },
            ThemeKind::MediumGray => Tokens {
                kind,
                chrome: Color32::from_rgb(83, 83, 83),
                dock: Color32::from_rgb(66, 66, 66),
                card: Color32::from_rgb(83, 83, 83),
                field: Color32::from_rgb(69, 69, 69),
                field_border: Color32::from_rgb(104, 104, 104),
                hover: Color32::from_rgb(98, 98, 98),
                pressed: Color32::from_rgb(112, 112, 112),
                text: Color32::from_rgb(238, 238, 238),
                text_dim: Color32::from_rgb(212, 212, 212),
                text_faint: Color32::from_rgb(160, 160, 160),
                icon: Color32::from_rgb(226, 226, 226),
                separator: Color32::from_rgb(62, 62, 62),
                tab_strip: Color32::from_rgb(74, 74, 74),
                row_selected: Color32::from_rgb(52, 98, 150),
                ..Tokens::for_kind(ThemeKind::Dark)
            },
            ThemeKind::Light => Tokens {
                kind,
                chrome: Color32::from_rgb(240, 240, 240),
                dock: Color32::from_rgb(250, 250, 250),
                card: Color32::from_rgb(240, 240, 240),
                field: Color32::WHITE,
                field_border: Color32::from_rgb(186, 186, 186),
                hover: Color32::from_rgb(226, 226, 226),
                pressed: Color32::from_rgb(206, 206, 206),
                text: Color32::from_rgb(20, 20, 20),
                text_dim: Color32::from_rgb(64, 64, 64),
                text_faint: Color32::from_rgb(120, 120, 120),
                icon: Color32::from_rgb(46, 46, 46),
                accent: Color32::from_rgb(16, 110, 220),
                accent_text: Color32::WHITE,
                separator: Color32::from_rgb(214, 214, 214),
                shadow: Color32::from_black_alpha(50),
                danger: Color32::from_rgb(200, 50, 60),
                warning: Color32::from_rgb(180, 120, 10),
                success: Color32::from_rgb(30, 140, 70),
                tab_strip: Color32::from_rgb(226, 226, 226),
                row_selected: Color32::from_rgb(198, 220, 246),
                diff_add_bg: Color32::from_rgba_unmultiplied(30, 160, 70, 40),
                diff_del_bg: Color32::from_rgba_unmultiplied(220, 50, 60, 36),
                ..Tokens::for_kind(ThemeKind::Dark)
            },
        }
    }

    pub fn dark(&self) -> bool {
        self.kind != ThemeKind::Light
    }

    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(tokens_id()))
            .unwrap_or_else(|| Tokens::for_kind(ThemeKind::default()))
    }

    /// Colour of a graph lane; lanes keep their colour for their whole life.
    pub fn lane(&self, lane: usize) -> Color32 {
        const DARK: [Color32; 10] = [
            Color32::from_rgb(64, 156, 255),
            Color32::from_rgb(255, 120, 80),
            Color32::from_rgb(90, 200, 120),
            Color32::from_rgb(200, 110, 230),
            Color32::from_rgb(250, 200, 60),
            Color32::from_rgb(60, 205, 210),
            Color32::from_rgb(240, 90, 150),
            Color32::from_rgb(160, 200, 70),
            Color32::from_rgb(150, 140, 255),
            Color32::from_rgb(230, 150, 70),
        ];
        const LIGHT: [Color32; 10] = [
            Color32::from_rgb(20, 110, 220),
            Color32::from_rgb(220, 80, 40),
            Color32::from_rgb(30, 150, 70),
            Color32::from_rgb(150, 60, 190),
            Color32::from_rgb(200, 140, 0),
            Color32::from_rgb(0, 150, 160),
            Color32::from_rgb(200, 40, 110),
            Color32::from_rgb(110, 150, 20),
            Color32::from_rgb(90, 80, 220),
            Color32::from_rgb(190, 100, 20),
        ];
        let set = if self.dark() { &DARK } else { &LIGHT };
        set[lane % set.len()]
    }
}

fn tokens_id() -> egui::Id {
    egui::Id::new("gitcraft-theme")
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Inter and JetBrains Mono first; egui's default fonts stay last for symbols Inter lacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("Inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(
        "Inter-Medium",
        include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    );
    add(
        "Inter-SemiBold",
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    );
    add(
        "JetBrainsMono",
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    // The default fonts have only a part of the emoji people write: the rest comes from here.
    add(
        "NotoEmoji",
        include_bytes!("../assets/fonts/NotoEmoji-Regular.ttf"),
    );
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "Inter".to_owned());
    proportional.push("NotoEmoji".to_owned());
    let fallback = proportional.clone();
    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.insert(0, "JetBrainsMono".to_owned());
    monospace.push("NotoEmoji".to_owned());
    for (family, primary) in [("medium", "Inter-Medium"), ("semibold", "Inter-SemiBold")] {
        let mut stack = vec![primary.to_owned()];
        stack.extend(fallback.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(family.into()), stack);
    }
    ctx.set_fonts(fonts);
}

/// Menus and context menus: no border, rows highlighted from edge to edge with room for the
/// text, a margin only above and below (where the menu's corners are rounded).
pub fn menu_style(s: &mut egui::Style) {
    s.spacing.menu_margin = egui::Margin::symmetric(0, 6);
    s.spacing.button_padding = egui::vec2(16.0, 7.0);
    s.spacing.item_spacing.y = 0.0;
    s.visuals.window_stroke = Stroke::NONE;
    // A shade darker than cards on dark themes, with lighter separators.
    let fill = s.visuals.window_fill;
    let (fill, line) = if s.visuals.dark_mode {
        let fill = fill.lerp_to_gamma(Color32::BLACK, 0.3);
        (fill, fill.lerp_to_gamma(Color32::WHITE, 0.2))
    } else {
        (fill, fill.lerp_to_gamma(Color32::BLACK, 0.15))
    };
    s.visuals.window_fill = fill;
    s.visuals.popup_shadow.color = s.visuals.popup_shadow.color.gamma_multiply(0.6);
    s.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, line);
    let w = &mut s.visuals.widgets;
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.bg_stroke = Stroke::NONE;
        wv.corner_radius = CornerRadius::ZERO;
    }
}

pub fn apply(ctx: &egui::Context, kind: ThemeKind) {
    let t = Tokens::for_kind(kind);
    ctx.data_mut(|d| d.insert_temp(tokens_id(), t));
    let mut v = if t.dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = t.chrome;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.field_border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.card;
    v.code_bg_color = t.field;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.danger;
    v.window_corner_radius = CornerRadius::same(t.radius_lg as u8);
    v.menu_corner_radius = CornerRadius::same(t.radius_lg as u8);
    v.window_shadow = egui::Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 0,
        color: t.shadow,
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: t.shadow,
    };
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, t.accent_text);
    v.striped = false;
    let r = CornerRadius::same(t.radius_sm as u8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.card;
    w.noninteractive.weak_bg_fill = t.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_dim);
    w.noninteractive.corner_radius = r;
    for (wv, bg, stroke) in [
        (&mut w.inactive, t.field, t.field_border),
        (&mut w.hovered, t.hover, t.field_border),
        (&mut w.active, t.pressed, t.accent),
        (&mut w.open, t.hover, t.field_border),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0, stroke);
        wv.fg_stroke = Stroke::new(1.0, t.text);
        wv.corner_radius = r;
        wv.expansion = 0.0;
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(10.5)),
            (TextStyle::Body, FontId::proportional(12.5)),
            (TextStyle::Button, FontId::proportional(12.5)),
            (TextStyle::Heading, semibold(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size = egui::vec2(24.0, 24.0);
        // Tooltips and other popups (menus set their own, see `menu_style`).
        s.spacing.menu_margin = egui::Margin::symmetric(12, 7);
        s.spacing.window_margin = egui::Margin::same(16);
        s.spacing.icon_width = 14.0;
        s.visuals.indent_has_left_vline = false;
        let mut scroll = egui::style::ScrollStyle::thin();
        scroll.interact_handle_opacity = 1.0;
        scroll.interact_background_opacity = 0.4;
        scroll.floating_width = 4.0;
        s.spacing.scroll = scroll;
        s.spacing.tooltip_width = 320.0;
    });
    // egui picks its style branch from the OS appearance; both branches get our palette.
    let style = ctx.global_style();
    ctx.set_style_of(egui::Theme::Light, Arc::clone(&style));
    ctx.set_style_of(egui::Theme::Dark, style);
}
