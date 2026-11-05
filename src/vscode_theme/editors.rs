//! Where the VS Code flavours (Code, Insiders, VSCodium) keep their settings and extensions.

use std::path::{Path, PathBuf};

/// One VS Code flavour: its user settings file and where its extensions live.
pub(super) struct Editor {
    pub(super) settings: PathBuf,
    pub(super) extension_dirs: Vec<PathBuf>,
}

/// (settings folder, user extensions folder, install folder names)
const FLAVOURS: [(&str, &str, &[&str]); 3] = [
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

/// Folders VS Code may be installed under.
fn install_roots() -> Vec<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(l) = &local {
        roots.push(l.join("Programs"));
    }
    if let Some(p) = &program_files {
        roots.push(p.clone());
    }
    roots.extend(["/Applications", "/usr/share", "/usr/lib", "/opt"].map(PathBuf::from));
    roots
}

pub(super) fn installed_editors() -> Vec<Editor> {
    let (Some(config), Some(home)) = (config_dir(), home()) else {
        return Vec::new();
    };
    let roots = install_roots();
    FLAVOURS
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
