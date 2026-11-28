//! The Log tab of the right pane: every operation gitcraft ran (push, pull, merge, rebase…),
//! its git commands and what they printed, live while they run.

use egui::{Color32, RichText, Ui};

use super::widgets;
use super::{Action, RepoTab};
use crate::git::{self, LogEntry, LogKind};
use crate::icons;
use crate::theme::{self, Tokens};

impl RepoTab {
    /// The dot on the Log tab: an operation is running, or the last one failed.
    pub(super) fn log_mark(&self, ctx: &egui::Context) -> Option<Color32> {
        let t = Tokens::get(ctx);
        if self.is_working() {
            Some(t.danger)
        } else if self.op_log.last_failed() {
            Some(t.warning)
        } else {
            None
        }
    }

    /// `other`: the title of the tab next to the log (Commit, Changes…).
    pub(super) fn log_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>, other: &str) {
        let t = Tokens::get(ui.ctx());
        let entries = self.op_log.entries();
        let mark = self.log_mark(ui.ctx());
        let tabs = [(other, None), ("Log", mark)];
        let back = widgets::panel_tabs(ui, &tabs, 1, |ui| {
            let finished = entries.iter().any(|e| e.done.is_some());
            if ui
                .add_enabled_ui(finished, |ui| {
                    icons::small_button(ui, "x", "Clear the finished operations")
                })
                .inner
                .clicked()
            {
                acts.push(Action::ClearLog);
            }
            if icons::small_button(ui, "copy", "Copy the whole log").clicked() {
                ui.ctx().copy_text(as_text(&entries));
            }
        });
        if back.is_some() {
            acts.push(Action::ShowLog(false));
        }
        if entries.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Pushes, pulls, merges and the rest show up here as they run.")
                        .color(t.text_faint),
                );
            });
            return;
        }
        let now = git::now();
        egui::ScrollArea::vertical()
            .id_salt("op_log")
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for e in &entries {
                    entry_ui(ui, e, now, &t);
                }
                ui.add_space(8.0);
            });
    }
}

fn entry_ui(ui: &mut Ui, e: &LogEntry, now: i64, t: &Tokens) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match e.done {
                    None => {
                        ui.add(egui::Spinner::new().size(12.0).color(t.danger));
                    }
                    Some((true, _)) => icons::inline(ui, "check", 13.0, t.success),
                    Some((false, _)) => icons::inline(ui, "circle-alert", 13.0, t.danger),
                }
                ui.label(RichText::new(&e.label).font(theme::semibold(12.5)));
                let mut when = git::relative_time(e.time, now);
                match e.done {
                    Some((_, d)) => when.push_str(&format!(" · {}", duration(d))),
                    None => when.push_str(&format!(" · running {}", duration(e.started.elapsed()))),
                }
                if e.background {
                    when.push_str(" · background");
                }
                ui.label(RichText::new(when).size(11.0).color(t.text_faint));
            });
            let mono = theme::mono(11.0);
            for line in &e.lines {
                let (text, color) = match line.kind {
                    LogKind::Command => (format!("$ {}", line.text), t.accent),
                    LogKind::Out => (line.text.clone(), t.text_dim),
                    LogKind::Err => (line.text.clone(), t.text_dim),
                };
                ui.add(
                    egui::Label::new(RichText::new(text).font(mono.clone()).color(color)).wrap(),
                );
            }
        });
    if e.done.is_none() {
        // Keeps the "running" time ticking.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", d.as_secs_f32())
    } else {
        format!("{} min {} s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

fn as_text(entries: &[LogEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let state = match e.done {
            None => "running".to_owned(),
            Some((ok, d)) => format!("{} in {}", if ok { "done" } else { "failed" }, duration(d)),
        };
        out.push_str(&format!("== {} ({state})\n", e.label));
        for l in &e.lines {
            if l.kind == LogKind::Command {
                out.push_str("$ ");
            }
            out.push_str(&l.text);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}
