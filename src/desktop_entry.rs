//! Running from an AppImage on Linux: an entry in the user's applications menu that starts this
//! AppImage file (the .deb and .rpm packages install their own).

use std::path::{Path, PathBuf};

/// File name of the entry, apart from the `gitcraft.desktop` a package installs.
const ENTRY: &str = "gitcraft-appimage.desktop";
const ICON: &[u8] = include_bytes!("../assets/app-icon/gitcraft-256.png");

/// The AppImage file gitcraft runs from (its runtime sets `APPIMAGE`), on Linux.
pub fn appimage() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            // An empty or relative HOME would put the entry under the current folder.
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|h| h.is_absolute())
                .map(|h| h.join(".local/share"))
        })
}

fn entry_path(data: &Path) -> PathBuf {
    data.join("applications").join(ENTRY)
}

fn icon_path(data: &Path) -> PathBuf {
    data.join("icons/hicolor/256x256/apps/gitcraft.png")
}

/// Whether the menu entry exists.
pub fn installed() -> bool {
    data_home().is_some_and(|d| entry_path(&d).is_file())
}

/// Adds (or rewrites) the menu entry for `appimage`.
pub fn install(appimage: &Path) -> Result<(), String> {
    let data = data_home().ok_or("No home folder")?;
    write_entry(&data, appimage).map_err(|e| e.to_string())
}

/// Removes the menu entry and its icon.
pub fn uninstall() -> Result<(), String> {
    let data = data_home().ok_or("No home folder")?;
    for path in [entry_path(&data), icon_path(&data)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(())
}

/// At startup: an AppImage that was moved or replaced by a newer file keeps its menu entry
/// pointing at the file running now.
pub fn refresh() {
    let (Some(appimage), Some(data)) = (appimage(), data_home()) else {
        return;
    };
    let path = entry_path(&data);
    let Ok(current) = std::fs::read_to_string(&path) else {
        return;
    };
    if current != entry(&appimage) {
        let _ = write_entry(&data, &appimage);
    }
}

fn write_entry(data: &Path, appimage: &Path) -> std::io::Result<()> {
    let icon = icon_path(data);
    if let Some(dir) = icon.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&icon, ICON)?;
    let path = entry_path(data);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry(appimage))
}

fn entry(appimage: &Path) -> String {
    let exec = quote_exec(&appimage.to_string_lossy());
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=gitcraft\n\
         Comment=A small visual git client\n\
         Exec={exec} %f\n\
         TryExec={}\n\
         Terminal=false\n\
         Categories=Development;RevisionControl;\n\
         Icon=gitcraft\n\
         StartupWMClass=gitcraft\n",
        escape_value(&appimage.to_string_lossy())
    )
}

/// A program path as the quoted first word of `Exec` (Desktop Entry spec: `"`, `` ` ``, `$` and
/// `\` escaped inside the quotes, then the value's own escaping, `%` doubled).
fn quote_exec(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    escape_value(&quoted).replace('%', "%%")
}

/// A string value of a desktop file: backslashes and line breaks escaped.
fn escape_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_the_appimage_path() {
        assert_eq!(
            quote_exec("/home/me/Apps/gitcraft.AppImage"),
            "\"/home/me/Apps/gitcraft.AppImage\""
        );
        assert_eq!(
            quote_exec("/home/me/My Apps/$x 100%/a\"b"),
            "\"/home/me/My Apps/\\\\$x 100%%/a\\\\\"b\""
        );
    }

    #[test]
    fn writes_and_refreshes_the_entry() {
        let data = std::env::temp_dir().join(format!("gitcraft-desktop-{}", std::process::id()));
        let old = Path::new("/opt/gitcraft-v0.2.0.AppImage");
        let ok = write_entry(&data, old).is_ok();
        let text = std::fs::read_to_string(entry_path(&data)).unwrap_or_default();
        let icon = std::fs::metadata(icon_path(&data))
            .map(|m| m.len())
            .unwrap_or(0);
        let _ = std::fs::remove_dir_all(&data);
        assert!(ok);
        assert!(text.contains("Exec=\"/opt/gitcraft-v0.2.0.AppImage\" %f\n"));
        assert!(text.contains("TryExec=/opt/gitcraft-v0.2.0.AppImage\n"));
        assert_eq!(icon, ICON.len() as u64);
        assert_ne!(text, entry(Path::new("/opt/gitcraft-v0.2.1.AppImage")));
    }
}
