//! VS Code's colour themes ([`theme`]), to import into gitcraft.
//!
//! Listing them means scanning every extension's `package.json` (built-in ones included), so
//! that is only done when the list is asked for, and kept. An imported theme remembers its
//! file, so loading it later reads just that file.

pub mod theme;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::Color32;
use serde_json::Value;

/// A colour theme installed in VS Code.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThemeInfo {
    pub name: String,
    pub dark: bool,
    /// The theme file.
    pub path: PathBuf,
}

/// What the extensions of every installed VS Code flavour contribute, scanned once.
fn extensions() -> Arc<Extensions> {
    static SCANNED: Mutex<Option<Arc<Extensions>>> = Mutex::new(None);
    let Ok(mut scanned) = SCANNED.lock() else {
        return Arc::default();
    };
    Arc::clone(scanned.get_or_insert_with(|| Arc::new(scan(&extension_dirs()))))
}

/// The colour themes installed in VS Code, by name, each once.
pub fn color_themes() -> Vec<ThemeInfo> {
    color_themes_in(&extensions())
}

fn color_themes_in(extensions: &Extensions) -> Vec<ThemeInfo> {
    let mut out: Vec<ThemeInfo> = Vec::new();
    for c in &extensions.color_themes {
        let Some(name) = c.label.clone().or_else(|| c.id.clone()) else {
            continue;
        };
        if out.iter().all(|t| t.name != name) {
            out.push(ThemeInfo {
                name,
                dark: theme::ui_dark(c.ui_theme.as_deref()),
                path: c.path.clone(),
            });
        }
    }
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// A colour theme read from its file, once and kept.
pub fn color_theme(info: &ThemeInfo) -> Option<Arc<theme::EditorTheme>> {
    type Cache = HashMap<PathBuf, Option<Arc<theme::EditorTheme>>>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    if let Ok(cache) = CACHE.lock()
        && let Some(found) = cache.as_ref().and_then(|c| c.get(&info.path))
    {
        return found.clone();
    }
    let found = theme::load_file(&info.name, &info.path, info.dark).map(Arc::new);
    if let Ok(mut cache) = CACHE.lock() {
        cache
            .get_or_insert_with(HashMap::new)
            .insert(info.path.clone(), found.clone());
    }
    found
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
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

/// Where VS Code, VS Code Insiders and VSCodium keep their extensions, user and built-in.
fn extension_dirs() -> Vec<PathBuf> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    // (user extensions folder, install folder names)
    let flavours: [(&str, &[&str]); 3] = [
        (
            ".vscode",
            &[
                "Microsoft VS Code",
                "Visual Studio Code.app/Contents/Resources/app",
                "code",
                "visual-studio-code",
            ],
        ),
        (
            ".vscode-insiders",
            &[
                "Microsoft VS Code Insiders",
                "Visual Studio Code - Insiders.app/Contents/Resources/app",
                "code-insiders",
            ],
        ),
        (
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
        .flat_map(|(extensions, installs)| {
            let mut dirs = vec![home.join(extensions).join("extensions")];
            for root in &roots {
                for name in *installs {
                    dirs.extend(builtin_dirs(&root.join(name)));
                }
            }
            dirs
        })
        .collect()
}

/// A colour theme an extension contributes.
pub struct Contribution {
    pub id: Option<String>,
    pub label: Option<String>,
    /// The theme file, absolute.
    pub path: PathBuf,
    /// `uiTheme` (`vs`, `vs-dark`, `hc-black`, `hc-light`).
    pub ui_theme: Option<String>,
}

/// What the installed extensions contribute, user extensions first.
#[derive(Default)]
pub struct Extensions {
    pub color_themes: Vec<Contribution>,
}

fn scan(dirs: &[PathBuf]) -> Extensions {
    let mut out = Extensions::default();
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
            let Some(contributes) = pkg.get("contributes") else {
                continue;
            };
            let mut nls = None;
            let mut contributions = |key: &str| -> Vec<Contribution> {
                let Some(list) = contributes.get(key).and_then(Value::as_array) else {
                    return Vec::new();
                };
                list.iter()
                    .filter_map(|t| {
                        let mut label = t.get("label").and_then(Value::as_str).map(str::to_owned);
                        if let Some(key) = label
                            .as_deref()
                            .and_then(|l| l.strip_prefix('%'))
                            .and_then(|l| l.strip_suffix('%'))
                        {
                            let nls = nls
                                .get_or_insert_with(|| read_jsonc(&ext.join("package.nls.json")));
                            label = nls
                                .as_ref()
                                .and_then(|n| n.get(key))
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        }
                        Some(Contribution {
                            id: t.get("id").and_then(Value::as_str).map(str::to_owned),
                            label,
                            path: ext.join(t.get("path").and_then(Value::as_str)?),
                            ui_theme: t.get("uiTheme").and_then(Value::as_str).map(str::to_owned),
                        })
                    })
                    .collect()
            };
            out.color_themes.extend(contributions("themes"));
        }
    }
    out
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`, as straight (unmultiplied) RGBA.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.trim().strip_prefix('#')?;
    let digit =
        |i: usize, len: usize| u8::from_str_radix(hex.get(i * len..i * len + len)?, 16).ok();
    match hex.len() {
        3 | 4 => {
            let d = |i| digit(i, 1).map(|v| v * 17);
            Some([
                d(0)?,
                d(1)?,
                d(2)?,
                if hex.len() == 4 { d(3)? } else { 255 },
            ])
        }
        6 | 8 => Some([
            digit(0, 2)?,
            digit(1, 2)?,
            digit(2, 2)?,
            if hex.len() == 8 { digit(3, 2)? } else { 255 },
        ]),
        _ => None,
    }
}

pub fn parse_color32(s: &str) -> Option<Color32> {
    let [r, g, b, a] = parse_color(s)?;
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
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
        assert_eq!(parse_color("#fcfcfa"), Some([0xfc, 0xfc, 0xfa, 255]));
        assert_eq!(parse_color("#f0a8"), Some([255, 0, 0xaa, 0x88]));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("red"), None);
    }
}
