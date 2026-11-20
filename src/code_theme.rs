//! The colours code is shown with in diffs and blame: one of gitcraft's own styles (the same as
//! CodeCraft's), or a VS Code colour theme imported in the settings, which also colours the rest
//! of the window.
//!
//! Diffs are coloured on background threads, so the choice lives here, behind a lock.

use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use syntect::highlighting::{Color, StyleModifier, Theme, ThemeItem, ThemeSettings};

use crate::vscode::theme::EditorTheme;

/// What code is coloured with now.
#[derive(Clone)]
pub enum CodeColors {
    Builtin(SyntaxStyle),
    VsCode(Arc<EditorTheme>),
}

static CURRENT: RwLock<Option<CodeColors>> = RwLock::new(None);

pub fn set_current(colors: CodeColors) {
    if let Ok(mut current) = CURRENT.write() {
        *current = Some(colors);
    }
}

/// The syntect theme for a dark or light background. A VS Code theme is used as it is: it
/// colours the window too, so the background is its own.
pub fn current_theme(dark: bool) -> Theme {
    let current = CURRENT.read().ok().and_then(|c| c.clone());
    match current {
        Some(CodeColors::VsCode(vs)) => vs.syntax.clone(),
        Some(CodeColors::Builtin(style)) => style.theme(dark).clone(),
        None => SyntaxStyle::default().theme(dark).clone(),
    }
}

/// gitcraft's own code colours (CodeCraft's), for when a VS Code theme isn't chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntaxStyle {
    /// Warm: orange keywords, olive strings, gold functions.
    Ember,
    /// Cool: blue keywords, mint strings, violet types.
    Tide,
    /// Bright and balanced: purple keywords, green strings, blue functions, yellow types.
    #[default]
    Orchard,
    /// Mostly plain text: muted keywords and strings, dim comments.
    Quiet,
}

/// A style's colours (0xRRGGBB) for each kind of token; `None` leaves it as plain text.
struct Palette {
    comment: u32,
    keyword: u32,
    operator: Option<u32>,
    string: u32,
    number: u32,
    function: Option<u32>,
    kind: u32,
    macros: u32,
    this: u32,
    namespace: Option<u32>,
}

impl SyntaxStyle {
    /// In the order the settings show them, two to a row.
    pub const ALL: [SyntaxStyle; 4] = [
        SyntaxStyle::Orchard,
        SyntaxStyle::Ember,
        SyntaxStyle::Quiet,
        SyntaxStyle::Tide,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SyntaxStyle::Ember => "Ember",
            SyntaxStyle::Tide => "Tide",
            SyntaxStyle::Orchard => "Orchard",
            SyntaxStyle::Quiet => "Quiet",
        }
    }

    /// The style as a syntect theme, for dark or light backgrounds. Plain text is left
    /// transparent: it takes the editor's text colour.
    pub fn theme(self, dark: bool) -> &'static Theme {
        static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
        let themes = THEMES.get_or_init(|| {
            Self::ALL
                .iter()
                .flat_map(|s| [s.build(true), s.build(false)])
                .collect()
        });
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0) * 2 + usize::from(!dark);
        &themes[i]
    }

    fn palette(self, dark: bool) -> Palette {
        match (self, dark) {
            (SyntaxStyle::Ember, true) => Palette {
                comment: 0x7d756c,
                keyword: 0xf28c5b,
                operator: Some(0xd9a385),
                string: 0xb5c77a,
                number: 0xd49be0,
                function: Some(0xf2c96b),
                kind: 0x7fc8c2,
                macros: 0xe6828f,
                this: 0xe6828f,
                namespace: Some(0xc9b79c),
            },
            (SyntaxStyle::Ember, false) => Palette {
                comment: 0x8c857c,
                keyword: 0xc2501b,
                operator: Some(0xa0622e),
                string: 0x5e7a1e,
                number: 0x8a3fa0,
                function: Some(0x9a6a00),
                kind: 0x1e7f78,
                macros: 0xb53a4c,
                this: 0xb53a4c,
                namespace: Some(0x7a6648),
            },
            (SyntaxStyle::Tide, true) => Palette {
                comment: 0x6a7a8c,
                keyword: 0x6fa8f7,
                operator: Some(0x8fb3d9),
                string: 0x7fd1b9,
                number: 0xf5b971,
                function: Some(0x82d3f0),
                kind: 0xb49cf5,
                macros: 0xf08fb5,
                this: 0xf08fb5,
                namespace: Some(0xa9bcd6),
            },
            (SyntaxStyle::Tide, false) => Palette {
                comment: 0x7d8a99,
                keyword: 0x1d5fc4,
                operator: Some(0x3f6a99),
                string: 0x0e7c64,
                number: 0xb0620c,
                function: Some(0x0b7aa3),
                kind: 0x6a45c9,
                macros: 0xb8336e,
                this: 0xb8336e,
                namespace: Some(0x4a5f7d),
            },
            (SyntaxStyle::Orchard, true) => Palette {
                comment: 0x6f7782,
                keyword: 0xc38ae6,
                operator: Some(0x5cc9c9),
                string: 0x98c97a,
                number: 0xe8a56a,
                function: Some(0x6eb4f0),
                kind: 0xe9c46a,
                macros: 0x5cc9c9,
                this: 0xe87a7a,
                namespace: Some(0xe9c46a),
            },
            (SyntaxStyle::Orchard, false) => Palette {
                comment: 0x8a9099,
                keyword: 0x8a3fb8,
                operator: Some(0x157f82),
                string: 0x3f7f2a,
                number: 0xb5600e,
                function: Some(0x1f64b8),
                kind: 0x946c00,
                macros: 0x157f82,
                this: 0xc03d3d,
                namespace: Some(0x946c00),
            },
            (SyntaxStyle::Quiet, true) => Palette {
                comment: 0x6b6b6b,
                keyword: 0x8fa8c8,
                operator: None,
                string: 0xa8c49a,
                number: 0xc8a88f,
                function: None,
                kind: 0xb8b0d0,
                macros: 0x8fa8c8,
                this: 0x8fa8c8,
                namespace: None,
            },
            (SyntaxStyle::Quiet, false) => Palette {
                comment: 0x9a9a9a,
                keyword: 0x3d5f8a,
                operator: None,
                string: 0x4e7240,
                number: 0x8a5a35,
                function: None,
                kind: 0x5a4e86,
                macros: 0x3d5f8a,
                this: 0x3d5f8a,
                namespace: None,
            },
        }
    }

    fn build(self, dark: bool) -> Theme {
        let p = self.palette(dark);
        let rules: [(&str, Option<u32>); 12] = [
            ("comment, punctuation.definition.comment", Some(p.comment)),
            (
                "keyword, storage, entity.name.tag, markup.heading",
                Some(p.keyword),
            ),
            ("keyword.operator", p.operator),
            ("string, punctuation.definition.string", Some(p.string)),
            (
                "constant.numeric, constant.language, constant.character, constant.other, markup.bold",
                Some(p.number),
            ),
            (
                "entity.name.function, support.function, variable.function, \
                 meta.function-call.identifier, entity.other.attribute-name",
                p.function,
            ),
            (
                "entity.name.type, entity.name.struct, entity.name.enum, entity.name.trait, \
                 entity.name.class, entity.name.impl, support.type, support.class, \
                 storage.type.primitive, storage.type.numeric",
                Some(p.kind),
            ),
            (
                "entity.name.macro, support.macro, entity.name.function.macro, \
                 meta.annotation, meta.attribute, punctuation.definition.annotation",
                Some(p.macros),
            ),
            ("variable.language", Some(p.this)),
            ("entity.name.namespace, entity.name.module", p.namespace),
            ("markup.inline.raw, markup.raw", Some(p.string)),
            ("invalid", Some(0xe05252)),
        ];
        let color = |rgb: u32| Color {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
            a: 0xff,
        };
        Theme {
            name: Some(format!(
                "gitcraft {} {}",
                self.label(),
                if dark { "Dark" } else { "Light" }
            )),
            settings: ThemeSettings {
                foreground: Some(Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                }),
                ..Default::default()
            },
            scopes: rules
                .iter()
                .filter_map(|(selector, rgb)| {
                    Some(ThemeItem {
                        scope: selector.parse().ok()?,
                        style: StyleModifier {
                            foreground: Some(color((*rgb)?)),
                            background: None,
                            font_style: None,
                        },
                    })
                })
                .collect(),
            ..Default::default()
        }
    }
}
