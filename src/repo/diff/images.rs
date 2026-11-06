//! Images in a diff: both versions read from git and shown before / after.

use std::path::Path;
use std::sync::Arc;

use egui::{Ui, load::Bytes, vec2};

use crate::git::{self, Area};
use crate::repo::DiffKey;
use crate::theme::{self, Tokens};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

pub struct ImagePair {
    pub old: Option<Arc<[u8]>>,
    pub new: Option<Arc<[u8]>>,
    /// Makes the image URIs unique to this diff.
    pub id: String,
}

pub(super) fn is_image(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()))
}

/// The old name of a renamed file, from its patch's header (unless git had to quote it).
pub(super) fn renamed_from(header: &str) -> Option<&str> {
    header
        .lines()
        .find_map(|l| l.strip_prefix("rename from "))
        .filter(|p| !p.starts_with('"'))
}

/// The pictures before and after; the one before under its old name when the file was renamed.
pub(super) fn load_images(
    repo: &Path,
    key: &DiffKey,
    path: &str,
    renamed_from: Option<&str>,
) -> ImagePair {
    let get = |rev: Option<&str>, path: &str| {
        git::file_at(repo, rev, path)
            .ok()
            .map(|b| Arc::from(b.into_boxed_slice()))
    };
    let old_path = match key {
        DiffKey::Work(file) => file.old_path.as_deref(),
        _ => None,
    }
    .or(renamed_from)
    .unwrap_or(path);
    let old = |rev: &str| get(Some(rev), old_path);
    let (old, new) = match key {
        DiffKey::Commit { hash, .. } => (old(&format!("{hash}^")), get(Some(hash), path)),
        DiffKey::Work(file) => match file.area {
            Area::Staged => (old("HEAD"), get(Some(""), path)),
            Area::Unstaged => (get(Some(""), path), get(None, path)),
            Area::Untracked | Area::Conflicted => (None, get(None, path)),
        },
        DiffKey::Range { from, to, .. } => (old(from), get(to.as_deref(), path)),
    };
    let id = image_id(old.as_deref(), new.as_deref());
    ImagePair { old, new, id }
}

/// Names the pictures by what they hold: egui keeps an image by its URI for good, so the same
/// name for new contents would show the old picture, and a new name for each read of the same
/// one would keep another copy every time the diff is read again.
pub(super) fn image_id(old: Option<&[u8]>, new: Option<&[u8]>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (old, new).hash(&mut h);
    format!("{:016x}", h.finish())
}

pub(super) fn image_view(ui: &mut Ui, images: &ImagePair) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(12.0);
    ui.columns(2, |cols| {
        for (col, (label, bytes, side)) in cols.iter_mut().zip([
            ("Before", &images.old, "old"),
            ("After", &images.new, "new"),
        ]) {
            col.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(label)
                        .font(theme::semibold(12.0))
                        .color(t.text_dim),
                );
                ui.add_space(6.0);
                match bytes {
                    Some(bytes) => {
                        let uri = format!("bytes://diff/{}/{side}.png", images.id);
                        let max = vec2(ui.available_width() - 16.0, ui.available_height() - 40.0);
                        ui.add(
                            egui::Image::from_bytes(uri, Bytes::Shared(bytes.clone()))
                                .max_size(max)
                                .fit_to_original_size(1.0)
                                .bg_fill(t.field),
                        );
                        ui.label(
                            egui::RichText::new(format!("{} KB", bytes.len().div_ceil(1024)))
                                .color(t.text_faint)
                                .small(),
                        );
                    }
                    None => {
                        let note = if side == "old" {
                            "(new file)"
                        } else {
                            "(deleted)"
                        };
                        ui.label(egui::RichText::new(note).color(t.text_faint));
                    }
                }
            });
        }
    });
}
