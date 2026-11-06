//! Finding a theme among the extensions, reading its file, and turning it into a syntect theme.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::Value;
use syntect::highlighting::{
    Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

use super::jsonc::read_jsonc;

/// VS Code's own `editor.foreground` for a theme that sets none; syntect would otherwise
/// paint all plain text black, unreadable on a dark background.
pub(super) fn default_foreground(theme: &mut Theme, dark: bool) {
    if theme.settings.foreground.is_none() {
        let v = if dark { 0xbb } else { 0x33 };
        theme.settings.foreground = Some(Color {
            r: v,
            g: v,
            b: v,
            a: 255,
        });
    }
}

/// Finds the theme file contributed under `name` (its label or id), and whether VS Code
/// treats it as dark.
pub(super) fn find_theme(dirs: &[PathBuf], name: &str) -> Option<(PathBuf, bool)> {
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        // Newest version first when an extension is installed more than once.
        let mut exts: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        exts.sort_by(|a, b| natural_cmp(&b.to_string_lossy(), &a.to_string_lossy()));
        for ext in exts {
            let Some(pkg) = read_jsonc(&ext.join("package.json")) else {
                continue;
            };
            let Some(themes) = pkg.pointer("/contributes/themes").and_then(Value::as_array) else {
                continue;
            };
            let mut nls = None;
            for t in themes {
                let id = t.get("id").and_then(Value::as_str);
                let label = theme_label(t, &ext, &mut nls);
                if id != Some(name) && label.as_deref() != Some(name) {
                    continue;
                }
                let path = t.get("path").and_then(Value::as_str)?;
                let ui = t
                    .get("uiTheme")
                    .and_then(Value::as_str)
                    .unwrap_or("vs-dark");
                return Some((ext.join(path), !matches!(ui, "vs" | "hc-light")));
            }
        }
    }
    None
}

/// Orders names with their numbers by value, so `theme-1.10.0` comes after `theme-1.9.0`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a, b);
    loop {
        let digits = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let (da, db) = (digits(a), digits(b));
        let order = if da > 0 && db > 0 {
            let (na, nb) = (
                a[..da].trim_start_matches('0'),
                b[..db].trim_start_matches('0'),
            );
            na.len().cmp(&nb.len()).then_with(|| na.cmp(nb))
        } else {
            let (Some(ca), Some(cb)) = (a.chars().next(), b.chars().next()) else {
                return a.len().cmp(&b.len());
            };
            ca.cmp(&cb)
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
        let step = |s: &str, d: usize| d.max(s.chars().next().map_or(0, char::len_utf8));
        a = &a[step(a, da)..];
        b = &b[step(b, db)..];
    }
}

/// A contributed theme's label, looked up in the extension's `package.nls.json` (read once, into
/// `nls`) when it is a `%key%`.
fn theme_label(theme: &Value, ext: &Path, nls: &mut Option<Option<Value>>) -> Option<String> {
    let label = theme.get("label").and_then(Value::as_str)?;
    let Some(key) = label.strip_prefix('%').and_then(|l| l.strip_suffix('%')) else {
        return Some(label.to_owned());
    };
    let nls = nls.get_or_insert_with(|| read_jsonc(&ext.join("package.nls.json")));
    // A string, or `{ "message": …, "comment": … }`.
    let value = nls.as_ref()?.get(key)?;
    value
        .as_str()
        .or_else(|| value.get("message")?.as_str())
        .map(str::to_owned)
}

/// Workbench colours, token rules and type (`dark`, `light`, ...) of a theme file.
pub(super) type ThemeFile = (serde_json::Map<String, Value>, Vec<Value>, Option<String>);

/// Reads a theme file, following its `include`s.
pub(super) fn read_theme(path: &Path, depth: u32) -> Option<ThemeFile> {
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

pub(super) fn build(name: &str, colors: &serde_json::Map<String, Value>, rules: &[Value]) -> Theme {
    let color = |key: &str| {
        colors
            .get(key)
            .and_then(Value::as_str)
            .and_then(parse_color)
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
            .and_then(parse_color);
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

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
fn parse_color(s: &str) -> Option<Color> {
    let hex = s.trim().strip_prefix('#')?;
    let digit =
        |i: usize, len: usize| u8::from_str_radix(hex.get(i * len..i * len + len)?, 16).ok();
    let (r, g, b, a) = match hex.len() {
        3 | 4 => {
            let d = |i| digit(i, 1).map(|v| v * 17);
            (
                d(0)?,
                d(1)?,
                d(2)?,
                if hex.len() == 4 { d(3)? } else { 255 },
            )
        }
        6 | 8 => (
            digit(0, 2)?,
            digit(1, 2)?,
            digit(2, 2)?,
            if hex.len() == 8 { digit(3, 2)? } else { 255 },
        ),
        _ => return None,
    };
    Some(Color { r, g, b, a })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours() {
        assert_eq!(
            parse_color("#fcfcfa"),
            Some(Color {
                r: 0xfc,
                g: 0xfc,
                b: 0xfa,
                a: 255
            })
        );
        assert_eq!(
            parse_color("#f0a8"),
            Some(Color {
                r: 255,
                g: 0,
                b: 0xaa,
                a: 0x88
            })
        );
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn versions_compare_by_value() {
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("a.theme-1.10.0", "a.theme-1.9.0"), Greater);
        assert_eq!(natural_cmp("a.theme-1.9.0", "a.theme-1.9.0"), Equal);
        assert_eq!(natural_cmp("a.theme-2.0.0", "a.theme-10.0.0"), Less);
        assert_eq!(natural_cmp("a.theme-1.0.0", "a.theme-1.0.0-x"), Less);
        assert_eq!(natural_cmp("b", "a"), Greater);
        assert_eq!(natural_cmp("é1", "é2"), Less);
    }

    #[test]
    fn a_theme_without_a_foreground_gets_a_readable_one() {
        let mut theme = build("Bare", &serde_json::Map::new(), &[]);
        default_foreground(&mut theme, true);
        assert_eq!(theme.settings.foreground, parse_color("#bbbbbb"));
        let mut theme = build("Bare", &serde_json::Map::new(), &[]);
        default_foreground(&mut theme, false);
        assert_eq!(theme.settings.foreground, parse_color("#333333"));
        // A theme's own foreground stays.
        let colors = serde_json::json!({ "editor.foreground": "#123456" });
        let mut theme = build(
            "Own",
            colors.as_object().unwrap_or(&serde_json::Map::new()),
            &[],
        );
        default_foreground(&mut theme, true);
        assert_eq!(theme.settings.foreground, parse_color("#123456"));
    }

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
                { "label": "%light%", "uiTheme": "vs", "path": "./themes/light.json" },
                { "label": "%label%", "uiTheme": "vs-dark", "path": "./themes/nice.json" } ] } }"#,
        );
        write(
            "package.nls.json",
            r#"{ "label": "Nice Dark", "light": { "message": "Nice Light", "comment": [] } }"#,
        );
        write(
            "themes/base.json",
            r##"{ "colors": { "editor.foreground": "#aaaaaa" },
                 "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#777777" } } ] }"##,
        );
        write(
            "themes/nice.json",
            r##"{ "include": "./base.json", // comment
                 "tokenColors": [ { "scope": ["keyword", "storage"], "settings": { "foreground": "#ff6188" } }, ] }"##,
        );
        let found = find_theme(std::slice::from_ref(&dir), "Nice Dark");
        let read = found.as_ref().and_then(|(path, _)| read_theme(path, 0));
        let light = find_theme(std::slice::from_ref(&dir), "Nice Light");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(light.is_some_and(|(path, dark)| !dark && path.ends_with("themes/light.json")));
        assert!(found.is_some_and(|(_, dark)| dark));
        assert!(read.is_some());
        let (colors, rules, _) = read.unwrap_or_default();
        let theme = build("Nice Dark", &colors, &rules);
        assert_eq!(theme.scopes.len(), 2);
        assert_eq!(theme.settings.foreground, parse_color("#aaaaaa"));
    }
}
