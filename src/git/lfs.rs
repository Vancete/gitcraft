//! Git LFS: whether a repository uses it, whether git-lfs is installed, and the pointer files
//! git keeps in place of the objects.

use std::path::Path;
use std::sync::OnceLock;

use super::run;

/// What git stores for a file kept in LFS: the object's hash and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsPointer {
    /// `sha256:<hex>`.
    pub oid: String,
    pub size: u64,
}

/// Whether the repository keeps files in LFS: its `.gitattributes` sends some through the lfs
/// filter, or it has an `.lfsconfig`.
pub fn uses_lfs(repo: &Path) -> bool {
    repo.join(".lfsconfig").is_file()
        || std::fs::read_to_string(repo.join(".gitattributes"))
            .is_ok_and(|text| attributes_use_lfs(&text))
}

/// Whether a `.gitattributes` has a pattern with `filter=lfs`.
pub(super) fn attributes_use_lfs(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .any(|l| l.split_whitespace().skip(1).any(|a| a == "filter=lfs"))
}

/// Whether git-lfs is installed (`git lfs version` works); asked once.
pub fn lfs_installed() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| run(None, &["lfs", "version"]).is_ok())
}

/// The LFS pointer in a file's text, if the text is one: `version https://git-lfs…`, then
/// `key value` lines with `oid` and `size` among them.
pub fn parse_lfs_pointer(text: &str) -> Option<LfsPointer> {
    // Pointers are small; a large text is the file itself.
    if text.len() > 1024 {
        return None;
    }
    let mut lines = text.lines();
    let version = lines.next()?.strip_prefix("version ")?;
    if !version.starts_with("https://git-lfs.github.com/spec/")
        && !version.starts_with("https://hawser.github.com/spec/")
    {
        return None;
    }
    let (mut oid, mut size) = (None, None);
    for line in lines.filter(|l| !l.is_empty()) {
        let (key, value) = line.split_once(' ')?;
        match key {
            "oid" => oid = Some(value.to_owned()),
            "size" => size = value.parse().ok(),
            _ => {}
        }
    }
    Some(LfsPointer {
        oid: oid?,
        size: size?,
    })
}

/// A size in bytes as people read it: `512 B`, `12.3 KB`, `4.0 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_file() {
        let text = "version https://git-lfs.github.com/spec/v1\n\
                    oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
                    size 12345\n";
        assert_eq!(
            parse_lfs_pointer(text),
            Some(LfsPointer {
                oid: "sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"
                    .to_owned(),
                size: 12345,
            })
        );
        // With an extension line, and CRLF line ends.
        let ext = "version https://git-lfs.github.com/spec/v1\r\n\
                   ext-0-foo sha256:00\r\n\
                   oid sha256:ab\r\n\
                   size 7\r\n";
        assert_eq!(parse_lfs_pointer(ext).map(|p| p.size), Some(7));
    }

    #[test]
    fn not_a_pointer_file() {
        for text in [
            "",
            "hello\n",
            "version 1.0\noid sha256:ab\nsize 7\n",
            "version https://git-lfs.github.com/spec/v1\noid sha256:ab\n",
            "version https://git-lfs.github.com/spec/v1\noid sha256:ab\nsize lots\n",
            "version https://git-lfs.github.com/spec/v1\nnot a key-value line\n",
        ] {
            assert_eq!(parse_lfs_pointer(text), None, "{text:?}");
        }
        let long = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:ab\nsize 7\n{}",
            "x".repeat(2000)
        );
        assert_eq!(parse_lfs_pointer(&long), None);
    }

    #[test]
    fn attributes_with_lfs() {
        assert!(attributes_use_lfs(
            "*.txt text\n*.psd filter=lfs diff=lfs merge=lfs -text\n"
        ));
        assert!(!attributes_use_lfs("*.txt text eol=lf\n"));
        assert!(!attributes_use_lfs("# *.psd filter=lfs\n"));
    }

    #[test]
    fn sizes_people_read() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
