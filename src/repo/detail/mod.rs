//! Right pane: the selected commit, or the working-tree changes with staging and the commit box.

mod all_files;
mod changes;
mod commit;
mod commit_box;
mod compare;

pub(super) use all_files::AllFiles;

use egui::{Align2, Sense, Ui, pos2, vec2};

use super::branch_tree;
use super::widgets::{self, RowExtra};
use super::{Action, DiffKey, DiffView, RepoTab};
use crate::git::ChangedFile;
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(super) fn detail_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let wip = self.selected.is_some_and(|r| self.is_wip(r));
        if self.pr_view.is_some() {
            self.pr_files_panel(ui, acts);
        } else if self.compare.is_some() {
            self.compare_panel(ui, acts);
        } else if wip {
            self.changes_panel(ui, acts);
        } else {
            self.commit_panel(ui, acts);
        }
    }

    fn open_key(&self) -> Option<&DiffKey> {
        self.diff.as_ref().map(|d: &DiffView| &d.key)
    }

    /// Changed files, each opening its diff (`key`) when clicked; `rev`: where the files are,
    /// for their menu (`None`: the working tree); `tree`: which list this is, for the folders
    /// opened and closed in it.
    pub(super) fn file_list(
        &self,
        ui: &mut Ui,
        acts: &mut Vec<Action>,
        tree: &str,
        files: &[ChangedFile],
        rev: Option<&str>,
        key: impl Fn(&str) -> DiffKey,
    ) {
        let id = egui::Id::new(("file-folder", self.repo_key(), tree));
        let selected = |f: &&ChangedFile| self.open_key() == Some(&key(&f.path));
        branch_tree::files(
            ui,
            (id, true),
            files,
            |f| &f.path,
            &selected,
            &mut |ui, indent, f| {
                let r = widgets::file_row(
                    ui,
                    f.status,
                    &f.path,
                    f.old_path.as_deref(),
                    indent,
                    selected(&f),
                    RowExtra::Stats(f.added, f.removed),
                );
                widgets::file_menu(&r.row, &f.path, rev, false, acts);
                if r.row.clicked() {
                    acts.push(Action::OpenDiff(key(&f.path)));
                }
            },
        );
    }
}

/// "N files changed +a −d", and an optional note on the right.
pub(super) fn files_header(ui: &mut Ui, files: &[ChangedFile], note: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| {
        (a + f.added.unwrap_or(0), d + f.removed.unwrap_or(0))
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.tab_strip);
    let y = rect.center().y;
    let r = p.text(
        pos2(rect.left() + 14.0, y),
        Align2::LEFT_CENTER,
        format!(
            "{} file{} changed",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ),
        theme::semibold(11.5),
        t.text_dim,
    );
    let r = p.text(
        pos2(r.right() + 10.0, y),
        Align2::LEFT_CENTER,
        format!("+{adds}"),
        theme::mono(11.0),
        t.success,
    );
    p.text(
        pos2(r.right() + 6.0, y),
        Align2::LEFT_CENTER,
        format!("−{dels}"),
        theme::mono(11.0),
        t.danger,
    );
    if let Some(note) = note {
        p.text(
            pos2(rect.right() - 10.0, y),
            Align2::RIGHT_CENTER,
            note,
            egui::FontId::proportional(11.0),
            t.text_faint,
        );
    }
}
