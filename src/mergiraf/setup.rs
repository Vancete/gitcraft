//! Setting git up to merge with Mergiraf in every repository (`merge.mergiraf.*` in the global
//! config, a marked block in the global attributes file), and taking that back.

use std::path::PathBuf;
use std::process::Command;

use super::{Found, find, quiet};

/// The lines gitcraft adds to the global attributes file are between these.
const BEGIN: &str = "# gitcraft: Mergiraf (begin)";
const END: &str = "# gitcraft: Mergiraf (end)";

pub(super) fn in_merges() -> bool {
    let driver = crate::git::run(
        None,
        &["config", "--global", "--get", "merge.mergiraf.driver"],
    )
    .is_ok_and(|d| !d.trim().is_empty());
    driver
        && attributes_file()
            .and_then(|f| std::fs::read_to_string(f).map_err(|e| e.to_string()))
            .is_ok_and(|text| text.contains("merge=mergiraf"))
}

/// The global attributes file git reads.
fn attributes_file() -> Result<PathBuf, String> {
    if let Ok(path) = crate::git::run(
        None,
        &[
            "config",
            "--global",
            "--path",
            "--get",
            "core.attributesFile",
        ],
    ) && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path.trim()));
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(config).join("git").join("attributes"));
    }
    std::env::var_os("HOME")
        .filter(|d| !d.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("git")
                .join("attributes")
        })
        .ok_or_else(|| "No home folder for git's attributes file".to_owned())
}

/// `text` without gitcraft's block, then with `lines` as the block when given.
fn with_block(text: &str, lines: Option<&str>) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        match line.trim() {
            BEGIN => inside = true,
            END if inside => inside = false,
            _ if !inside => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    if let Some(lines) = lines {
        out.push_str(BEGIN);
        out.push('\n');
        for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(END);
        out.push('\n');
    }
    out
}

/// How git's merge driver calls it: by name when it is on the PATH, else by its full path.
fn driver(found: &Found) -> String {
    let program = if found.own {
        format!(
            "\"{}\"",
            found.program.display().to_string().replace('\\', "/")
        )
    } else {
        "mergiraf".to_owned()
    };
    format!("{program} merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L")
}

pub(super) fn enable() -> Result<String, String> {
    let found = find().ok_or("Mergiraf isn't installed")?;
    let out = quiet(Command::new(&found.program))
        .args(["languages", "--gitattributes"])
        .output()
        .map_err(|e| format!("Could not run mergiraf: {e}"))?;
    let languages = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() || !languages.contains("merge=mergiraf") {
        return Err("Mergiraf didn't list the files it merges".to_owned());
    }
    let git = |args: &[&str]| crate::git::run(None, args).map(drop);
    git(&["config", "--global", "merge.mergiraf.name", "mergiraf"])?;
    git(&[
        "config",
        "--global",
        "merge.mergiraf.driver",
        &driver(&found),
    ])?;
    // Mergiraf needs the base of each conflict, which git's default markers leave out.
    if crate::git::run(
        None,
        &["config", "--global", "--get", "merge.conflictStyle"],
    )
    .is_err()
    {
        git(&["config", "--global", "merge.conflictStyle", "diff3"])?;
    }
    let file = attributes_file()?;
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&file, with_block(&text, Some(&languages)))
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok("Git merges with Mergiraf now, in every repository".to_owned())
}

pub(super) fn disable() -> Result<String, String> {
    let file = attributes_file()?;
    if let Ok(text) = std::fs::read_to_string(&file) {
        std::fs::write(&file, with_block(&text, None))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    // Fails only when there is no such section, which is the point.
    let _ = crate::git::run(
        None,
        &["config", "--global", "--remove-section", "merge.mergiraf"],
    );
    Ok("Git merges as it did before Mergiraf".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_added_replaced_and_taken_out_alone() {
        let mine = "*.png binary\n";
        let added = with_block(mine, Some("*.rs merge=mergiraf\r\n\n*.go merge=mergiraf\n"));
        assert_eq!(
            added,
            format!("*.png binary\n{BEGIN}\n*.rs merge=mergiraf\n*.go merge=mergiraf\n{END}\n")
        );
        let again = with_block(
            &format!("{added}*.pdf binary\n"),
            Some("*.py merge=mergiraf"),
        );
        assert_eq!(
            again,
            format!("*.png binary\n*.pdf binary\n{BEGIN}\n*.py merge=mergiraf\n{END}\n")
        );
        assert_eq!(with_block(&again, None), "*.png binary\n*.pdf binary\n");
        assert_eq!(with_block("", None), "");
    }

    #[test]
    fn the_driver_names_its_own_copy_by_path() {
        let found = |program: &str, own| Found {
            program: PathBuf::from(program),
            version: String::new(),
            own,
        };
        assert_eq!(
            driver(&found("mergiraf", false)),
            "mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L"
        );
        assert!(
            driver(&found(r"C:\Users\me\gitcraft\mergiraf.exe", true))
                .starts_with("\"C:/Users/me/gitcraft/mergiraf.exe\" merge --git ")
        );
    }
}
