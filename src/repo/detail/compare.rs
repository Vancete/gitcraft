//! The comparison of two commits, or of one with the working tree.

use egui::{RichText, Ui};

use super::files_header;
use crate::icons;
use crate::repo::{Action, Compare, DiffKey, RepoTab, branch_tree, short, widgets};
use crate::theme::{self, Tokens};

impl RepoTab {
    /// Right pane while comparing: the two ends, how far apart they are, and the files that
    /// differ.
    pub(super) fn compare_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let Some(c) = &self.compare else { return };
        widgets::panel_header(ui, "Compare", |ui| {
            if icons::small_button(ui, "x", "Close (Esc)").clicked() {
                acts.push(Action::CloseCompare);
            }
            branch_tree::files_toggle(ui);
        });
        compare_summary(ui, acts, c);

        match &c.data {
            None => {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.colored_label(t.danger, e);
                });
            }
            Some(Ok(d)) => {
                files_header(ui, &d.files, None);
                if d.files.is_empty() {
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No differences").color(t.text_dim));
                    });
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("compare_scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.file_list(ui, acts, "compare", &d.files, d.to.as_deref(), |path| {
                            DiffKey::Range {
                                from: d.from.clone(),
                                to: d.to.clone(),
                                path: path.to_owned(),
                            }
                        });
                    });
            }
        }
    }
}

/// The two ends of `c`, how far apart they are, and the switches for how to compare them.
fn compare_summary(ui: &mut Ui, acts: &mut Vec<Action>, c: &Compare) {
    let t = Tokens::get(ui.ctx());
    let data = c.data.as_ref().and_then(|d| d.as_ref().ok());
    let target_name = if c.target.is_some() {
        c.target_label.as_str()
    } else {
        "the working tree"
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            egui::Grid::new("compare_ends")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    // A name, and the commit it stands for (`note` before the hash).
                    let end = |ui: &mut Ui, label: &str, hash: Option<&str>, note: &str| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(label).font(theme::medium(12.5)));
                            // The hash, unless the label already is it.
                            if let Some(hash) = hash.filter(|h| short(h) != label) {
                                ui.label(
                                    RichText::new(format!("{note}{}", short(hash)))
                                        .font(theme::mono(11.5))
                                        .color(t.text_faint),
                                )
                                .on_hover_text(hash);
                            }
                        });
                    };
                    let note = if c.since_fork { "merge base " } else { "" };
                    ui.label(RichText::new("From").color(t.text_faint));
                    end(ui, &c.base_label, data.map(|d| d.from.as_str()), note);
                    ui.end_row();
                    ui.label(RichText::new("To").color(t.text_faint));
                    end(ui, target_name, data.and_then(|d| d.to.as_deref()), "");
                    ui.end_row();
                });
            if let Some(d) = data {
                let (behind, ahead) = d.apart;
                let commits = |n: u32| format!("{n} commit{}", if n == 1 { "" } else { "s" });
                let other = if c.target.is_some() {
                    target_name
                } else {
                    "HEAD"
                };
                let text = match (ahead, behind) {
                    (0, 0) => "Same history".to_owned(),
                    (a, 0) => format!("{other} is {} ahead", commits(a)),
                    (0, b) => format!("{other} is {} behind", commits(b)),
                    (a, b) => format!("{other} is {} ahead, {b} behind", commits(a)),
                };
                ui.label(RichText::new(text).color(t.text_dim).small());
            }
            ui.horizontal(|ui| {
                let mut since = c.since_fork;
                let label = format!("Only what {target_name} changed since they diverged");
                if ui
                    .checkbox(&mut since, label)
                    .on_hover_text(
                        "Diff from the merge base, as a pull request shows it, instead of \
                         every difference between the two",
                    )
                    .changed()
                {
                    acts.push(Action::CompareSinceFork(since));
                }
            });
            if c.target.is_some() && ui.button("Swap").clicked() {
                acts.push(Action::SwapCompare);
            }
        });
}
