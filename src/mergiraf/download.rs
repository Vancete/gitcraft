//! Downloading Mergiraf's latest release for this system from Codeberg.

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::{EXE, own_dir, probe, quiet};

const LATEST: &str = "https://codeberg.org/api/v1/repos/mergiraf/mergiraf/releases/latest";
/// Where its release files are; a download from anywhere else is refused.
const DOWNLOADS: &str = "https://codeberg.org/mergiraf/mergiraf/releases/download/";
const MAX_DOWNLOAD: u64 = 200 << 20;

/// The release file for this system. Windows on ARM runs the x86-64 one.
fn asset() -> Option<&'static str> {
    let arm = cfg!(target_arch = "aarch64");
    if cfg!(windows) {
        Some("mergiraf_x86_64-pc-windows-gnu.zip")
    } else if cfg!(target_os = "macos") {
        Some(if arm {
            "mergiraf_aarch64-apple-darwin.tar.gz"
        } else {
            "mergiraf_x86_64-apple-darwin.tar.gz"
        })
    } else if cfg!(target_os = "linux") && (arm || cfg!(target_arch = "x86_64")) {
        Some(if arm {
            "mergiraf_aarch64-unknown-linux-musl.tar.gz"
        } else {
            "mergiraf_x86_64-unknown-linux-musl.tar.gz"
        })
    } else {
        None
    }
}

/// Windows' own tar, which also opens zip files (Git's GNU tar earlier on the PATH doesn't).
fn tar() -> Command {
    let program = if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
            .unwrap_or_else(|| PathBuf::from("tar.exe"))
    } else {
        PathBuf::from("tar")
    };
    quiet(Command::new(program))
}

/// Where the latest release's `asset` downloads from.
fn release_url(agent: &ureq::Agent, asset: &str) -> Result<String, String> {
    let release = agent
        .get(LATEST)
        .call()
        .map_err(|e| format!("Could not ask Codeberg for Mergiraf's latest release: {e}"))?
        .into_string()
        .map_err(|e| format!("Codeberg's answer about Mergiraf: {e}"))?;
    let release: serde_json::Value = serde_json::from_str(&release)
        .map_err(|e| format!("Codeberg's answer about Mergiraf: {e}"))?;
    release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == asset)
        .and_then(|a| a["browser_download_url"].as_str())
        .filter(|url| url.starts_with(DOWNLOADS))
        .map(str::to_owned)
        .ok_or_else(|| format!("Mergiraf's latest release has no {asset}"))
}

/// The file at `url`, refused when bigger than [`MAX_DOWNLOAD`].
fn fetch(agent: &ureq::Agent, url: &str, asset: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    agent
        .get(url)
        .call()
        .map_err(|e| format!("Could not download {asset}: {e}"))?
        .into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("Could not download {asset}: {e}"))?;
    if body.len() as u64 > MAX_DOWNLOAD {
        return Err(format!("{asset} is bigger than expected"));
    }
    Ok(body)
}

pub(super) fn download() -> Result<String, String> {
    let asset = asset().ok_or("Mergiraf publishes no build for this system")?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .user_agent("gitcraft")
        .build();
    let url = release_url(&agent, asset)?;

    let dir = own_dir().ok_or("No folder to keep Mergiraf in")?;
    // Unpacked apart, so only the program is taken from the archive.
    let unpack = dir.join("unpack");
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack).map_err(|e| format!("{}: {e}", unpack.display()))?;
    let body = fetch(&agent, &url, asset)?;
    let archive = unpack.join(asset);
    std::fs::write(&archive, &body).map_err(|e| format!("{}: {e}", archive.display()))?;
    let unpacked = tar()
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpack)
        .output()
        .map_err(|e| format!("Could not run tar: {e}"))?;
    let program = dir.join(EXE);
    let moved = if unpacked.status.success() {
        std::fs::rename(unpack.join(EXE), &program).map_err(|e| format!("{asset}: {e}"))
    } else {
        Err(format!(
            "Could not unpack {asset}: {}",
            String::from_utf8_lossy(&unpacked.stderr).trim()
        ))
    };
    let _ = std::fs::remove_dir_all(&unpack);
    moved?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", program.display()))?;
    }
    let version = probe(&program).ok_or("The downloaded Mergiraf doesn't start")?;
    Ok(format!("Installed {version}"))
}
