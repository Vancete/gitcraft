//! The shells the terminal can run, and which one it starts.

use std::path::Path;

/// A shell the terminal can run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellSpec {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

impl ShellSpec {
    fn new(label: &str, program: &str, args: &[&str]) -> Self {
        Self {
            label: label.to_owned(),
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }
}

/// The shells installed on this machine, the platform default first.
pub fn available_shells() -> Vec<ShellSpec> {
    let mut list = Vec::new();
    #[cfg(windows)]
    {
        list.push(ShellSpec::new(
            "Windows PowerShell",
            "powershell.exe",
            &["-NoLogo"],
        ));
        if on_path("pwsh.exe") {
            list.insert(0, ShellSpec::new("PowerShell 7", "pwsh.exe", &["-NoLogo"]));
        }
        list.push(ShellSpec::new("Command Prompt", "cmd.exe", &[]));
        for dir in [
            r"C:\Program Files\Git\bin",
            r"C:\Program Files (x86)\Git\bin",
        ] {
            let bash = Path::new(dir).join("bash.exe");
            if bash.is_file() {
                let program = bash.display().to_string();
                list.push(ShellSpec::new("Git Bash", &program, &["--login", "-i"]));
                break;
            }
        }
        if on_path("wsl.exe") {
            list.push(ShellSpec::new("WSL", "wsl.exe", &[]));
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(shell) = std::env::var("SHELL") {
            let name = Path::new(&shell)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| shell.clone());
            list.push(ShellSpec::new(&name, &shell, &[]));
        }
        for (label, program) in [
            ("bash", "/bin/bash"),
            ("zsh", "/bin/zsh"),
            ("fish", "/usr/bin/fish"),
            ("sh", "/bin/sh"),
        ] {
            if Path::new(program).is_file() && !list.iter().any(|s| s.program == program) {
                list.push(ShellSpec::new(label, program, &[]));
            }
        }
    }
    list
}

/// The shell to start: the one chosen in the settings when installed, else the default.
pub fn default_shell(ctx: &egui::Context) -> ShellSpec {
    let wanted = crate::settings::Settings::get(ctx).shell;
    let shells = available_shells();
    shells
        .iter()
        .find(|s| !wanted.is_empty() && s.program == wanted)
        .or_else(|| shells.first())
        .cloned()
        .unwrap_or_else(|| ShellSpec::new("sh", "sh", &[]))
}

#[cfg(windows)]
fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(exe).is_file()))
        .unwrap_or(false)
}
