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

/// `text` without gitcraft's block, then with `lines` as the block when given. Only what
/// gitcraft wrote goes: a block whose end marker is gone loses its begin marker and the
/// `merge=mergiraf` lines right after it, not the rest of the file.
fn with_block(text: &str, lines: Option<&str>) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let all: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(line) = all.get(i) {
        i += 1;
        if line.trim() != BEGIN {
            out.push_str(line);
            out.push_str(eol);
            continue;
        }
        // The block ends at its end marker, unless another block begins first.
        let rest = all.get(i..).unwrap_or_default();
        match rest
            .iter()
            .map(|l| l.trim())
            .position(|l| l == BEGIN || l == END)
        {
            Some(n) if rest.get(n).is_some_and(|l| l.trim() == END) => i += n + 1,
            _ => {
                while all.get(i).is_some_and(|l| is_managed(l)) {
                    i += 1;
                }
            }
        }
    }
    if let Some(lines) = lines {
        out.push_str(BEGIN);
        out.push_str(eol);
        for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push_str(line);
            out.push_str(eol);
        }
        out.push_str(END);
        out.push_str(eol);
    }
    out
}

/// A line of the kind Mergiraf lists for the block: `<pattern> merge=mergiraf`.
fn is_managed(line: &str) -> bool {
    let mut words = line.split_whitespace();
    matches!(
        (words.next(), words.next(), words.next()),
        (Some(_), Some("merge=mergiraf"), None)
    )
}

/// The global attributes file's text; none yet is an empty one, but one that can't be read
/// isn't taken for empty (it would be written over).
fn read_attributes(file: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// How git's merge driver calls it: by name when it is on the PATH, else by its full path.
fn driver(found: &Found) -> String {
    let program = if found.own {
        // Git runs it with the shell, after putting its own values in for `%` and a letter.
        let mut quoted = String::from("\"");
        for c in found
            .program
            .display()
            .to_string()
            .replace('\\', "/")
            .chars()
        {
            match c {
                '"' | '$' | '`' => {
                    quoted.push('\\');
                    quoted.push(c);
                }
                '%' => quoted.push_str("%%"),
                _ => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
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
    let text = read_attributes(&file)?.unwrap_or_default();
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&file, with_block(&text, Some(&languages)))
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok("Git merges with Mergiraf now, in every repository".to_owned())
}

pub(super) fn disable() -> Result<String, String> {
    // Without the driver git merges as before even where the attributes still name it.
    // Fails only when there is no such section, which is the point.
    let _ = crate::git::run(
        None,
        &["config", "--global", "--remove-section", "merge.mergiraf"],
    );
    let file = attributes_file()?;
    if let Some(text) = read_attributes(&file)?
        && text.lines().any(|l| l.trim() == BEGIN)
    {
        std::fs::write(&file, with_block(&text, None))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
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
    fn a_block_without_its_end_takes_only_its_own_lines() {
        let text = format!(
            "*.png binary\n{BEGIN}\n*.rs merge=mergiraf\n*.go merge=mergiraf\n\
             # mine\n*.pdf binary\n*.c merge=mergiraf\n"
        );
        let kept = "*.png binary\n# mine\n*.pdf binary\n*.c merge=mergiraf\n";
        assert_eq!(with_block(&text, None), kept);
        assert_eq!(
            with_block(&text, Some("*.py merge=mergiraf")),
            format!("{kept}{BEGIN}\n*.py merge=mergiraf\n{END}\n")
        );
        // A later whole block doesn't stand in for the missing end.
        let two =
            format!("{BEGIN}\n*.rs merge=mergiraf\nmine\n{BEGIN}\n*.go merge=mergiraf\n{END}\n");
        assert_eq!(with_block(&two, None), "mine\n");
    }

    #[test]
    fn windows_line_ends_stay() {
        let text = format!("*.png binary\r\n{BEGIN}\r\n*.rs merge=mergiraf\r\n{END}\r\n");
        assert_eq!(with_block(&text, None), "*.png binary\r\n");
    }

    #[test]
    fn an_unreadable_attributes_file_is_not_taken_for_empty() {
        let dir = std::env::temp_dir().join(format!("gitcraft-attrs-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("attributes");
        let _ = std::fs::write(&file, b"# caf\xe9\n*.png binary\n");
        let latin1 = read_attributes(&file);
        let missing = read_attributes(&dir.join("none"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(latin1.is_err());
        assert_eq!(missing, Ok(None));
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
        // The shell and git's own `%` placeholders leave the path as it is.
        assert!(
            driver(&found(r"C:\Users\$me 100%A\mergiraf.exe", true))
                .starts_with("\"C:/Users/\\$me 100%%A/mergiraf.exe\" merge --git ")
        );
    }
}
