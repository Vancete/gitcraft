//! Files kept in Git LFS: their diff is of the pointers git stores, so it is shown as the LFS
//! objects before and after, with their sizes, instead of the pointers' text.

use egui::{RichText, Ui};

use super::{DiffLine, Kind};
use crate::git::{LfsPointer, human_size, parse_lfs_pointer};
use crate::theme::{self, Tokens};

/// The LFS objects either side of a diff; `None` for a side without the file.
#[derive(Debug, PartialEq, Eq)]
pub struct LfsPair {
    pub old: Option<LfsPointer>,
    pub new: Option<LfsPointer>,
}

/// The LFS pointers of a diff, when both sides are one (or one side has no file at all).
pub(super) fn lfs_pair(lines: &[DiffLine]) -> Option<LfsPair> {
    let side = |kind: Kind| {
        let text: Vec<&str> = lines
            .iter()
            .filter(|l| l.kind == Kind::Context || l.kind == kind)
            .map(|l| l.text.as_str())
            .collect();
        if text.is_empty() {
            return Ok(None);
        }
        parse_lfs_pointer(&text.join("\n")).map(Some).ok_or(())
    };
    let (old, new) = (side(Kind::Del).ok()?, side(Kind::Add).ok()?);
    (old.is_some() || new.is_some()).then_some(LfsPair { old, new })
}

pub(super) fn lfs_view(ui: &mut Ui, pair: &LfsPair) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("Stored in Git LFS")
                .color(t.text_faint)
                .small(),
        );
    });
    ui.add_space(6.0);
    ui.columns(2, |cols| {
        for (col, (label, side, missing)) in cols.iter_mut().zip([
            ("Before", &pair.old, "(new file)"),
            ("After", &pair.new, "(deleted)"),
        ]) {
            col.vertical_centered(|ui| {
                ui.label(
                    RichText::new(label)
                        .font(theme::semibold(12.0))
                        .color(t.text_dim),
                );
                ui.add_space(6.0);
                match side {
                    Some(pointer) => {
                        ui.label(
                            RichText::new(human_size(pointer.size)).font(theme::semibold(16.0)),
                        );
                        let oid = pointer
                            .oid
                            .split_once(':')
                            .map_or(&*pointer.oid, |(_, h)| h);
                        ui.label(
                            RichText::new(format!("LFS object {}", oid.get(..12).unwrap_or(oid)))
                                .color(t.text_faint)
                                .small(),
                        )
                        .on_hover_text(&pointer.oid);
                    }
                    None => {
                        ui.label(RichText::new(missing).color(t.text_faint));
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::diff::parse;

    const POINTER: &str = "version https://git-lfs.github.com/spec/v1";

    #[test]
    fn a_changed_lfs_file_is_its_two_objects() {
        let diff = format!(
            "diff --git a/a.bin b/a.bin\nindex 6441bd3..1e68883 100644\n--- a/a.bin\n+++ b/a.bin\n\
             @@ -1,3 +1,3 @@\n {POINTER}\n-oid sha256:75e3\n-size 5000\n+oid sha256:dc35\n+size 7000\n"
        );
        let pair = lfs_pair(&parse(&diff)).map(|p| (p.old.map(|o| o.size), p.new.map(|n| n.oid)));
        assert_eq!(pair, Some((Some(5000), Some("sha256:dc35".to_owned()))));
    }

    #[test]
    fn a_new_lfs_file_has_no_object_before() {
        let diff = format!(
            "diff --git a/a.bin b/a.bin\nnew file mode 100644\nindex 0000000..1e68883\n\
             --- /dev/null\n+++ b/a.bin\n@@ -0,0 +1,3 @@\n+{POINTER}\n+oid sha256:dc35\n+size 7\n"
        );
        let pair = lfs_pair(&parse(&diff));
        assert_eq!(
            pair.map(|p| (p.old, p.new.map(|n| n.size))),
            Some((None, Some(7)))
        );
    }

    #[test]
    fn other_diffs_are_no_lfs_objects() {
        let text = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(lfs_pair(&parse(text)), None);
        // Moved into LFS: the text before is no pointer.
        let into = format!(
            "diff --git a/a.bin b/a.bin\n--- a/a.bin\n+++ b/a.bin\n@@ -1 +1,3 @@\n-raw\n\
             +{POINTER}\n+oid sha256:dc35\n+size 7\n"
        );
        assert_eq!(lfs_pair(&parse(&into)), None);
        assert_eq!(lfs_pair(&[]), None);
    }
}
