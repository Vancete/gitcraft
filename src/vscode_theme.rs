//! The colour theme VS Code is using, read from its settings and extensions and turned into a
//! syntect theme, so diffs and blame colour code like the editor does.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::Value;
use syntect::highlighting::{
    Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

/// A VS Code theme ready for syntect.
pub struct EditorTheme {
    pub name: String,
    pub dark: bool,
    pub theme: Theme,
}

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Follow VS Code's theme or not (the "Code colours" setting).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The theme selected in VS Code; `None` when VS Code isn't installed or its theme can't be
/// found. Reloaded when VS Code's settings file changes.
pub fn current() -> Option<Arc<EditorTheme>> {
    type Cache = Option<(PathBuf, Option<SystemTime>, Option<Arc<EditorTheme>>)>;
    static CACHE: Mutex<Cache> = Mutex::new(None);

    let editor = installed_editors()
        .into_iter()
        .find(|e| e.settings.is_file())?;
    let modified = std::fs::metadata(&editor.settings)
        .and_then(|m| m.modified())
        .ok();
    let mut cache = CACHE.lock().ok()?;
    if let Some((path, when, theme)) = cache.as_ref()
        && *path == editor.settings
        && *when == modified
    {
        return theme.clone();
    }
    let theme = load(&editor).map(Arc::new);
    *cache = Some((editor.settings.clone(), modified, theme.clone()));
    theme
}

/// One VS Code flavour: its user settings file and where its extensions live.
struct Editor {
    settings: PathBuf,
    extension_dirs: Vec<PathBuf>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    }
}

/// Folders a VS Code install keeps its built-in extensions in.
fn builtin_dirs(install: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        install.join("extensions"),
        install.join("resources/app/extensions"),
    ];
    // Newer Windows installs put each version in a folder of its own.
    if let Ok(entries) = std::fs::read_dir(install) {
        for e in entries.flatten() {
            dirs.push(e.path().join("resources/app/extensions"));
        }
    }
    dirs.into_iter().filter(|d| d.is_dir()).collect()
}

fn installed_editors() -> Vec<Editor> {
    let (Some(config), Some(home)) = (config_dir(), home()) else {
        return Vec::new();
    };
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    // (settings folder, user extensions folder, install folder names)
    let flavours: [(&str, &str, &[&str]); 3] = [
        (
            "Code",
            ".vscode",
            &[
                "Microsoft VS Code",
                "Visual Studio Code.app/Contents/Resources/app",
                "code",
                "visual-studio-code",
            ],
        ),
        (
            "Code - Insiders",
            ".vscode-insiders",
            &[
                "Microsoft VS Code Insiders",
                "Visual Studio Code - Insiders.app/Contents/Resources/app",
                "code-insiders",
            ],
        ),
        (
            "VSCodium",
            ".vscode-oss",
            &[
                "VSCodium",
                "VSCodium.app/Contents/Resources/app",
                "codium",
                "vscodium",
            ],
        ),
    ];
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(l) = &local {
        roots.push(l.join("Programs"));
    }
    if let Some(p) = &program_files {
        roots.push(p.clone());
    }
    roots.extend(["/Applications", "/usr/share", "/usr/lib", "/opt"].map(PathBuf::from));

    flavours
        .iter()
        .map(|(settings, extensions, installs)| {
            let mut extension_dirs = vec![home.join(extensions).join("extensions")];
            for root in &roots {
                for name in *installs {
                    extension_dirs.extend(builtin_dirs(&root.join(name)));
                }
            }
            Editor {
                settings: config.join(settings).join("User/settings.json"),
                extension_dirs,
            }
        })
        .collect()
}

fn load(editor: &Editor) -> Option<EditorTheme> {
    let settings = read_jsonc(&editor.settings)?;
    let name = settings.get("workbench.colorTheme")?.as_str()?.to_owned();
    let (path, ui_dark) = find_theme(&editor.extension_dirs, &name)?;
    let (colors, rules, kind) = read_theme(&path, 0)?;
    let dark = match kind.as_deref() {
        Some("light" | "hcLight") => false,
        Some(_) => true,
        None => ui_dark,
    };
    let mut theme = build(&name, &colors, &rules);
    default_foreground(&mut theme, dark);
    Some(EditorTheme { theme, name, dark })
}

/// VS Code's own `editor.foreground` for a theme that sets none; syntect would otherwise
/// paint all plain text black, unreadable on a dark background.
fn default_foreground(theme: &mut Theme, dark: bool) {
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
fn find_theme(dirs: &[PathBuf], name: &str) -> Option<(PathBuf, bool)> {
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        // Newest version first when an extension is installed more than once.
        let mut exts: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        exts.sort();
        exts.reverse();
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
                let mut label = t.get("label").and_then(Value::as_str).map(str::to_owned);
                if let Some(key) = label
                    .as_deref()
                    .and_then(|l| l.strip_prefix('%'))
                    .and_then(|l| l.strip_suffix('%'))
                {
                    let nls = nls.get_or_insert_with(|| read_jsonc(&ext.join("package.nls.json")));
                    label = nls
                        .as_ref()
                        .and_then(|n| n.get(key))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
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

fn build(name: &str, colors: &serde_json::Map<String, Value>, rules: &[Value]) -> Theme {
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

fn read_jsonc(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&strip_jsonc(text.trim_start_matches('\u{feff}'))).ok()
}

/// Removes the comments and trailing commas VS Code allows in its JSON files.
fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                out.push(' ');
            }
            (']' | '}', _) => {
                // Drop a comma left right before the closing bracket.
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.truncate(kept - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let src = r#"{
            // a comment
            "a": "http://x", /* block */ "b": [1, 2,],
            "c": "quote \" // not a comment",
        }"#;
        let v: Value = serde_json::from_str(&strip_jsonc(src)).unwrap_or_default();
        assert_eq!(v["a"], "http://x");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"], "quote \" // not a comment");
    }

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
                { "label": "%label%", "uiTheme": "vs-dark", "path": "./themes/nice.json" } ] } }"#,
        );
        write("package.nls.json", r#"{ "label": "Nice Dark" }"#);
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
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found.is_some_and(|(_, dark)| dark));
        assert!(read.is_some());
        let (colors, rules, _) = read.unwrap_or_default();
        let theme = build("Nice Dark", &colors, &rules);
        assert_eq!(theme.scopes.len(), 2);
        assert_eq!(theme.settings.foreground, parse_color("#aaaaaa"));
    }
}
