//! The colour theme VS Code is using, read from its settings and extensions and turned into a
//! syntect theme, so diffs and blame colour code like the editor does.

mod editors;
mod jsonc;
mod theme_file;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use syntect::highlighting::Theme;

use self::editors::{Editor, installed_editors};
use self::jsonc::read_jsonc;
use self::theme_file::{build, default_foreground, find_theme, read_theme};

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
