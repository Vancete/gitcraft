//! VS Code's colour theme: its token colours as a syntect theme for the editor, and its
//! workbench colours for the rest of the window (see `theme::Tokens::with_vscode`).

use std::path::Path;
use std::str::FromStr;

use egui::Color32;
use serde_json::Value;
use syntect::highlighting::{
    Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

use super::{parse_color, parse_color32, read_jsonc};

/// A VS Code colour theme, ready for syntect and the window.
pub struct EditorTheme {
    pub dark: bool,
    pub syntax: Theme,
    /// Workbench colours (`editor.background`, `sideBar.background`, ...).
    colors: serde_json::Map<String, Value>,
}

impl EditorTheme {
    /// A workbench colour, if the theme sets it.
    pub fn color(&self, key: &str) -> Option<Color32> {
        self.colors
            .get(key)
            .and_then(Value::as_str)
            .and_then(parse_color32)
    }
}

/// Whether a theme is dark by its extension's `uiTheme`.
pub fn ui_dark(ui_theme: Option<&str>) -> bool {
    !matches!(ui_theme, Some("vs" | "hc-light"))
}

/// Reads the theme `name` from its file; `ui_dark` is what its extension says it is, in case
/// the file doesn't say.
pub fn load_file(name: &str, path: &Path, ui_dark: bool) -> Option<EditorTheme> {
    let (colors, rules, kind) = read_theme(path, 0)?;
    let dark = match kind.as_deref() {
        Some("light" | "hcLight") => false,
        Some(_) => true,
        None => ui_dark,
    };
    Some(EditorTheme {
        syntax: build(name, &colors, &rules),
        dark,
        colors,
    })
}

/// Workbench colours, token rules and type (`dark`, `light`, ...) of a theme file.
type ThemeFile = (serde_json::Map<String, Value>, Vec<Value>, Option<String>);

/// Reads a theme file, following its `include`s.
fn read_theme(path: &Path, depth: u32) -> Option<ThemeFile> {
    if depth > 8 {
        return None;
    }
    let json = read_jsonc(path)?;
    let (mut colors, mut rules, mut kind) = match json.get("include").and_then(Value::as_str) {
        Some(inc) => read_theme(&path.parent()?.join(inc), depth + 1)?,
        None => Default::default(),
    };
    if let Some(c) = json.get("colors").and_then(Value::as_object) {
        colors.extend(c.clone());
    }
    // A theme may point at a .tmTheme file instead; only inline rules are read.
    if let Some(r) = json.get("tokenColors").and_then(Value::as_array) {
        rules.extend(r.iter().cloned());
    }
    if let Some(t) = json.get("type").and_then(Value::as_str) {
        kind = Some(t.to_owned());
    }
    Some((colors, rules, kind))
}

fn syntect_color(s: &str) -> Option<Color> {
    let [r, g, b, a] = parse_color(s)?;
    Some(Color { r, g, b, a })
}

fn build(name: &str, colors: &serde_json::Map<String, Value>, rules: &[Value]) -> Theme {
    let color = |key: &str| {
        colors
            .get(key)
            .and_then(Value::as_str)
            .and_then(syntect_color)
    };
    let mut settings = ThemeSettings {
        foreground: color("editor.foreground"),
        background: color("editor.background"),
        ..Default::default()
    };
    let mut scopes = Vec::new();
    for rule in rules {
        let Some(s) = rule.get("settings") else {
            continue;
        };
        let foreground = s
            .get("foreground")
            .and_then(Value::as_str)
            .and_then(syntect_color);
        let selector = match rule.get("scope") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            // A rule without a scope sets the defaults.
            _ => {
                if settings.foreground.is_none() {
                    settings.foreground = foreground;
                }
                continue;
            }
        };
        let Some(foreground) = foreground else {
            continue;
        };
        let Ok(scope) = ScopeSelectors::from_str(selector.trim()) else {
            continue;
        };
        scopes.push(ThemeItem {
            scope,
            style: StyleModifier {
                foreground: Some(foreground),
                background: None,
                font_style: None,
            },
        });
    }
    Theme {
        name: Some(name.to_owned()),
        author: None,
        settings,
        scopes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_extension_theme_with_include() {
        let dir = std::env::temp_dir().join(format!("gitcraft-vstheme-{}", std::process::id()));
        let ext = dir.join("someone.nice-theme-1.0.0");
        let _ = std::fs::create_dir_all(ext.join("themes"));
        let write = |p: &str, s: &str| {
            let _ = std::fs::write(ext.join(p), s);
        };
        write(
            "package.json",
            r#"{ "contributes": { "themes": [
                { "label": "%label%", "uiTheme": "vs-dark", "path": "./themes/nice.json" } ] } }"#,
        );
        write("package.nls.json", r#"{ "label": "Nice Dark" }"#);
        write(
            "themes/base.json",
            r##"{ "colors": { "editor.foreground": "#aaaaaa", "sideBar.background": "#202020" },
                 "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#777777" } } ] }"##,
        );
        write(
            "themes/nice.json",
            r##"{ "include": "./base.json", // comment
                 "tokenColors": [ { "scope": ["keyword", "storage"], "settings": { "foreground": "#ff6188" } }, ] }"##,
        );
        let extensions = super::super::scan(std::slice::from_ref(&dir));
        let found = super::super::color_themes_in(&extensions);
        let theme = found
            .first()
            .and_then(|t| load_file(&t.name, &t.path, t.dark));
        assert_eq!(found.first().map(|t| t.name.as_str()), Some("Nice Dark"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(theme.is_some(), "theme not found");
        let Some(theme) = theme else { return };
        assert!(theme.dark);
        assert_eq!(theme.syntax.scopes.len(), 2);
        assert_eq!(theme.syntax.settings.foreground, syntect_color("#aaaaaa"));
        assert_eq!(
            theme.color("sideBar.background"),
            Some(Color32::from_rgb(0x20, 0x20, 0x20))
        );
    }
}
