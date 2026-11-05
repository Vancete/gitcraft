//! The selected commit or stash: its message (edited in place), author, hashes and files.

use egui::{Key, RichText, Sense, Ui, vec2};

use super::files_header;
use crate::ci::Status;
use crate::git::{CommitDetail, Stash};
use crate::icons;
use crate::repo::{
    Action, DiffKey, Refresh, RepoTab, Reword, branch_tree, ci_status, ops, sidebar, widgets,
};
use crate::theme::{self, Tokens};

impl RepoTab {
    pub(super) fn commit_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        let idle = self.busy_op.is_none();
        let stash = self.stash_view.and_then(|i| self.stashes.get(i)).cloned();
        let title = if stash.is_some() { "Stash" } else { "Commit" };
        widgets::panel_header(ui, title, |ui| {
            if let Some(stash) = &stash {
                let more = icons::small_button(ui, "chevron-down", "Apply, pop or drop");
                egui::Popup::menu(&more)
                    .style(crate::theme::menu_style)
                    .show(|ui| {
                        sidebar::stash_menu_items(ui, acts, &stash.name, idle);
                    });
            }
            branch_tree::files_toggle(ui);
        });
        if let Some(stash) = &stash {
            stash_banner(ui, acts, stash, idle);
        }
        self.want_all_files(ui.ctx());
        if let Some(err) = &self.detail_error {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.colored_label(t.danger, err);
            });
            return;
        }
        let Some(detail) = &self.detail else {
            ui.centered_and_justified(|ui| {
                if self.selected.is_some() {
                    ui.spinner();
                } else {
                    ui.label(RichText::new("Select a commit").color(t.text_dim));
                }
            });
            return;
        };

        // On a forge, the commit's builds: how they went, if that's known.
        let builds = (self.ci_project().is_some() && self.stash_view.is_none())
            .then(|| self.ci.by_sha.get(&detail.hash));
        // Taken out while the pane borrows `self`; put back after it.
        let mut reword = self.reword.take();
        let head = &self.refs.head;
        // A commit of the checked-out branch: double-click its message to edit it (the root
        // commit has no parent to rebase from).
        let editable = idle
            && detail.in_head
            && (detail.hash == *head || !detail.merges_after)
            && (detail.hash == *head || !detail.parents.is_empty());
        // How tall everything below the message was last frame: the message gets what's left.
        let view_h = ui.available_height();
        let rest_id = ui.id().with("message_rest");
        let rest: f32 = ui.data(|d| d.get_temp(rest_id)).unwrap_or(0.0);
        egui::ScrollArea::vertical()
            .id_salt("detail_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                let top = ui.cursor().top();
                let mut message_end = top;
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        let room = view_h - (ui.cursor().top() - top) - rest;
                        message_section(ui, acts, detail, &mut reword, editable, room);
                        message_end = ui.cursor().top();
                        ui.add_space(12.0);
                        author_line(ui, detail);
                        ui.add_space(10.0);
                        meta_grid(ui, acts, detail, builds);
                    });

                let note = (detail.parents.len() > 1).then_some("vs. first parent");
                files_header(ui, &detail.files, note);
                self.all_files_toggle(ui, acts);
                ui.spacing_mut().item_spacing.y = 0.0;
                if self.all_files.on {
                    self.all_files_list(ui, acts, detail);
                } else {
                    self.file_list(
                        ui,
                        acts,
                        "commit",
                        &detail.files,
                        Some(&detail.hash),
                        |path| DiffKey::Commit {
                            hash: detail.hash.clone(),
                            path: path.to_owned(),
                        },
                    );
                }
                let below = ui.cursor().top() - message_end;
                if (below - rest).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(rest_id, below));
                    ui.ctx().request_discard("commit message room");
                }
            });
        self.reword = reword;
    }
}

/// The stash shown, with Pop and Apply.
fn stash_banner(ui: &mut Ui, acts: &mut Vec<Action>, stash: &Stash, idle: bool) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::new()
        .fill(t.accent.gamma_multiply(0.10))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::inline(ui, "archive", 15.0, t.accent);
                ui.label(RichText::new(&stash.name).font(theme::medium(12.5)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (label, args) in [
                        ("Pop", ["stash", "pop", "--index", stash.name.as_str()]),
                        ("Apply", ["stash", "apply", "--index", stash.name.as_str()]),
                    ] {
                        if ui.add_enabled(idle, egui::Button::new(label)).clicked() {
                            acts.push(Action::Ops {
                                label: format!("{label} {}", stash.name),
                                ops: ops(&[&args]),
                                refresh: Refresh::All,
                            });
                        }
                    }
                });
            });
        });
}

/// The message of `detail`, or its editor while it is being reworded. `editable`: whether a
/// double-click starts that; `room`: how tall the message may be.
fn message_section(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    detail: &CommitDetail,
    reword: &mut Option<Reword>,
    editable: bool,
    room: f32,
) {
    let (subject, body) = detail
        .message
        .split_once('\n')
        .map(|(s, b)| (s, b.trim()))
        .unwrap_or((detail.message.as_str(), ""));
    let mut close = false;
    match reword.as_mut().filter(|r| r.hash == detail.hash) {
        Some(r) => {
            if let Some(save) = reword_editor(ui, r) {
                let message = r.message();
                if save && message != detail.message.trim() {
                    acts.push(Action::Reword {
                        hash: detail.hash.clone(),
                        message,
                    });
                }
                close = true;
            }
        }
        None => {
            let clicked = fitted_message(ui, detail, subject, body, editable, room);
            if clicked && editable {
                *reword = Some(Reword {
                    hash: detail.hash.clone(),
                    subject: subject.to_owned(),
                    body: body.to_owned(),
                    focus: true,
                    fill: false,
                });
            }
        }
    }
    if close {
        *reword = None;
    }
}

/// `subject` and `body`, cut to `room`: the description first, then the title down to its first
/// row. What was cut shows on hover. Returns whether it was double-clicked.
fn fitted_message(
    ui: &mut Ui,
    detail: &CommitDetail,
    subject: &str,
    body: &str,
    editable: bool,
    room: f32,
) -> bool {
    let t = Tokens::get(ui.ctx());
    let sense = if editable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let gap = ui.spacing().item_spacing.y;
    let budget = room - gap;
    let title = RichText::new(subject).font(theme::semibold(15.0));
    let desc = RichText::new(body).color(t.text_dim);
    let (title_rows, title_h) = text_rows(ui, title.clone());
    let (body_rows, body_h) = if body.is_empty() {
        (0, 0.0)
    } else {
        text_rows(ui, desc.clone())
    };
    let fit = |room: f32, h: f32| (room / h).floor().max(0.0) as usize;
    let show_title = fit(budget, title_h).clamp(1, title_rows);
    let show_body = if show_title < title_rows {
        0
    } else {
        let room = budget - title_rows as f32 * title_h - gap - 2.0;
        fit(room, body_h).min(body_rows)
    };
    let hint = if editable {
        Some("Double-click to edit the message")
    } else if detail.in_head && detail.merges_after {
        Some("Merges came after this commit: editing its message would flatten them")
    } else {
        None
    };
    // What was cut shows on hover.
    let full = (show_title < title_rows || show_body < body_rows).then(|| detail.message.trim());
    let tip = match (full, hint) {
        (Some(m), Some(h)) => Some(format!("{h}\n\n{m}")),
        (Some(m), None) => Some(m.to_owned()),
        (None, h) => h.map(str::to_owned),
    };
    let message = |ui: &mut Ui, text: RichText, rows: usize| {
        let job = clamped(ui, text, rows);
        let r = ui.add(
            egui::Label::new(job)
                .wrap()
                .sense(sense)
                .show_tooltip_when_elided(false),
        );
        match &tip {
            Some(tip) => r.on_hover_text(tip.as_str()),
            None => r,
        }
        .double_clicked()
    };
    let mut clicked = message(ui, title, show_title);
    if show_body > 0 {
        ui.add_space(2.0);
        clicked |= message(ui, desc, show_body);
    }
    clicked
}

/// Author line with an initials avatar, and the committer when someone else.
fn author_line(ui: &mut Ui, detail: &CommitDetail) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        widgets::avatar(ui, rect.center(), 16.0, &detail.author, &detail.email);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(&detail.author).font(theme::medium(12.5)))
                .on_hover_text(&detail.email);
            ui.label(
                RichText::new(&detail.author_date)
                    .color(t.text_faint)
                    .font(theme::mono(11.0)),
            );
        });
    });
    if detail.committer != detail.author {
        ui.label(
            RichText::new(format!(
                "Committed by {} · {}",
                detail.committer, detail.commit_date
            ))
            .color(t.text_faint)
            .small(),
        );
    }
}

/// The commit's hash, with a copy button, its parents, each selecting it when clicked, and
/// its `builds` on a forge (with their status if it's known).
fn meta_grid(
    ui: &mut Ui,
    acts: &mut Vec<Action>,
    detail: &CommitDetail,
    builds: Option<Option<&Status>>,
) {
    let t = Tokens::get(ui.ctx());
    egui::Grid::new("commit_meta")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Commit").color(t.text_faint));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(detail.hash.get(..12).unwrap_or(&detail.hash))
                        .font(theme::mono(12.0)),
                )
                .on_hover_text(&detail.hash);
                if icons::small_button(ui, "copy", "Copy full hash").clicked() {
                    ui.ctx().copy_text(detail.hash.clone());
                }
            });
            ui.end_row();
            if !detail.parents.is_empty() {
                ui.label(
                    RichText::new(if detail.parents.len() > 1 {
                        "Parents"
                    } else {
                        "Parent"
                    })
                    .color(t.text_faint),
                );
                ui.horizontal_wrapped(|ui| {
                    for p in &detail.parents {
                        if ui
                            .link(RichText::new(p.get(..8).unwrap_or(p)).font(theme::mono(12.0)))
                            .clicked()
                        {
                            acts.push(Action::SelectHash(p.clone()));
                        }
                    }
                });
                ui.end_row();
            }
            if let Some(status) = builds {
                ui.label(RichText::new("Builds").color(t.text_faint));
                ui.horizontal(|ui| {
                    if let Some(status) = status {
                        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ci_status::paint_dot(ui.painter(), &t, dot.center(), 3.5, status.state);
                        let tally = match crate::ci::tally_label(&status.checks) {
                            tally if tally.is_empty() => ci_status::state_word(status.state).into(),
                            tally => tally,
                        };
                        ui.label(RichText::new(tally).color(t.text_dim));
                    }
                    if ui.link("Show").clicked() {
                        acts.push(Action::OpenCi {
                            sha: detail.hash.clone(),
                            run: None,
                        });
                    }
                });
                ui.end_row();
            }
        });
}

impl Reword {
    /// The summary, then the description after a blank line.
    fn message(&self) -> String {
        let (subject, body) = (self.subject.trim(), self.body.trim());
        if body.is_empty() {
            subject.to_owned()
        } else {
            format!("{subject}\n\n{body}")
        }
    }
}

/// The summary and description being edited, with Save and Cancel: `Some(true)` to save,
/// `Some(false)` to cancel.
fn reword_editor(ui: &mut Ui, reword: &mut Reword) -> Option<bool> {
    let t = Tokens::get(ui.ctx());
    let summary = ui.add(
        egui::TextEdit::singleline(&mut reword.subject)
            .font(theme::semibold(14.0))
            .hint_text("Summary")
            .desired_width(f32::INFINITY),
    );
    if reword.focus {
        summary.request_focus();
        reword.focus = false;
    }
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::multiline(&mut reword.body)
            .hint_text("Description (optional)")
            .desired_rows(4)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);
    let can_save = !reword.subject.trim().is_empty();
    let (save_keys, cancel_keys) = ui.input(|i| {
        (
            i.modifiers.command && i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
        )
    });
    let mut out = None;
    ui.horizontal(|ui| {
        if widgets::primary_button(ui, "Save message", can_save, 110.0).clicked() {
            out = Some(true);
        }
        if ui.button("Cancel").clicked() {
            out = Some(false);
        }
        ui.label(
            RichText::new("Ctrl+Enter saves · Esc cancels")
                .small()
                .color(t.text_faint),
        );
    });
    if save_keys && can_save {
        out = Some(true);
    } else if cancel_keys {
        out = Some(false);
    }
    out
}

/// How many rows `text` wraps to across the space left in `ui`, and how tall each is.
fn text_rows(ui: &Ui, text: RichText) -> (usize, f32) {
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        ui.available_width(),
        egui::FontSelection::Default,
    );
    let rows = galley.rows.len().max(1);
    (rows, galley.size().y / rows as f32)
}

/// `text` wrapped to at most `rows` rows, the last one ending in "…" when cut.
fn clamped(ui: &Ui, text: RichText, rows: usize) -> egui::text::LayoutJob {
    let job = egui::WidgetText::from(text).into_layout_job(
        ui.style(),
        egui::FontSelection::Default,
        ui.text_valign(),
    );
    let mut job = std::sync::Arc::unwrap_or_clone(job);
    job.wrap.max_rows = rows;
    job
}
