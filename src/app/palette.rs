//! The command palette (Ctrl+P): app commands, themes and the open repository's actions.

use std::path::PathBuf;

use egui::{Align2, Key, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::{App, Tab};
use crate::repo::Action;
use crate::theme::{self, ThemeKind, Tokens};

/// The command palette (Ctrl+P): type to filter, Enter runs.
pub(super) struct Palette {
    query: String,
    selected: usize,
    focus: bool,
    /// The entries, made once rather than every frame (a repository with thousands of branches
    /// has thousands of them), and what they were made from.
    entries: Option<(EntriesKey, Vec<Item>)>,
}

/// What the palette's entries were made from: the active tab, and its repository (a tab's
/// view can change to another worktree) and that repository's palette generation.
type EntriesKey = (usize, Option<(PathBuf, (u64, u64))>);

/// A palette entry: its label, the label lowercased (what the query is matched against), its
/// hint, what it runs.
type Item = (String, String, String, Entry);

/// What a palette entry does outside the open repository.
enum AppCommand {
    NewTab,
    ReopenTab,
    Open,
    Create,
    Clone,
    Settings,
    Manual,
    Theme(ThemeKind),
}

enum Entry {
    App(AppCommand),
    /// One of the active repository's palette items.
    Repo(Action),
}

impl App {
    pub(super) fn open_palette(&mut self) {
        self.palette = Some(Palette {
            query: String::new(),
            selected: 0,
            focus: true,
            entries: None,
        });
    }

    /// Every palette entry: (label, hint, what it runs).
    fn palette_entries(&self) -> Vec<(String, String, Entry)> {
        let mut list = vec![
            (
                "New tab".to_owned(),
                "Ctrl+T".to_owned(),
                Entry::App(AppCommand::NewTab),
            ),
            (
                "Open repository…".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Open),
            ),
            (
                "Create repository…".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Create),
            ),
            (
                "Clone repository…".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Clone),
            ),
            (
                "Settings".to_owned(),
                "app".to_owned(),
                Entry::App(AppCommand::Settings),
            ),
            (
                "Manual and keyboard shortcuts".to_owned(),
                "F1".to_owned(),
                Entry::App(AppCommand::Manual),
            ),
        ];
        if !self.closed.is_empty() {
            list.insert(
                1,
                (
                    "Reopen closed tab".to_owned(),
                    "Ctrl+Shift+T".to_owned(),
                    Entry::App(AppCommand::ReopenTab),
                ),
            );
        }
        for kind in ThemeKind::ALL {
            list.push((
                format!("Theme: {}", kind.label()),
                "theme".to_owned(),
                Entry::App(AppCommand::Theme(kind)),
            ));
        }
        if let Some(Tab::Repo(repo)) = self.tabs.get(self.active) {
            for (label, hint, action) in repo.palette_items() {
                list.push((label, hint, Entry::Repo(action)));
            }
        }
        list
    }

    fn run_entry(&mut self, entry: Entry, ctx: &egui::Context) {
        match entry {
            Entry::App(AppCommand::NewTab) => self.new_tab(),
            Entry::App(AppCommand::ReopenTab) => self.reopen_closed_tab(ctx),
            Entry::App(AppCommand::Open) => self.pick_existing(ctx),
            Entry::App(AppCommand::Create) => self.pick_new(ctx),
            Entry::App(AppCommand::Clone) => self.start_clone(),
            Entry::App(AppCommand::Settings) => self.open_settings(),
            Entry::App(AppCommand::Manual) => self.manual_open = true,
            Entry::App(AppCommand::Theme(kind)) => self.set_theme(ctx, kind),
            Entry::Repo(action) => {
                if let Some(Tab::Repo(repo)) = self.tabs.get_mut(self.active) {
                    repo.queue(action);
                    ctx.request_repaint();
                }
            }
        }
    }

    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if self.palette.is_none() {
            return;
        }
        let t = Tokens::get(ctx);
        let key = (
            self.active,
            match self.tabs.get(self.active) {
                Some(Tab::Repo(repo)) => Some((repo.path.clone(), repo.palette_generation())),
                _ => None,
            },
        );
        if self
            .palette
            .as_ref()
            .is_some_and(|p| p.entries.as_ref().is_none_or(|(made, _)| *made != key))
        {
            let entries = self
                .palette_entries()
                .into_iter()
                .map(|(label, hint, entry)| {
                    let lower = label.to_lowercase();
                    (label, lower, hint, entry)
                })
                .collect();
            if let Some(palette) = &mut self.palette {
                palette.entries = Some((key, entries));
            }
        }
        let Some(palette) = &mut self.palette else {
            return;
        };
        let Palette {
            query,
            selected: chosen,
            focus,
            entries,
        } = palette;
        let entries: &[Item] = entries.as_ref().map_or(&[], |(_, e)| e);
        let lowered = query.to_lowercase();
        let shown: Vec<usize> = (0..entries.len())
            .filter(|&i| fuzzy(&entries[i].1, &lowered))
            .take(60)
            .collect();
        *chosen = (*chosen).min(shown.len().saturating_sub(1));
        let (up, down, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        if down {
            *chosen = (*chosen + 1).min(shown.len().saturating_sub(1));
        }
        if up {
            *chosen = chosen.saturating_sub(1);
        }
        let mut run = None;
        let mut close = esc;
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::same(10))
            .shadow(ctx.global_style().visuals.window_shadow);
        let area = egui::Area::new(egui::Id::new("palette"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 70.0));
        let resp = area.show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(560.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(query)
                        .hint_text("Type a command or a branch…")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(10, 8))
                        .font(theme::medium(14.0)),
                );
                if std::mem::take(focus) {
                    edit.request_focus();
                }
                if edit.changed() {
                    *chosen = 0;
                }
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (k, &i) in shown.iter().enumerate() {
                            let (label, _, hint, _) = &entries[i];
                            let selected = k == *chosen;
                            if palette_row(ui, &t, label, hint, selected, up || down) {
                                run = Some(i);
                            }
                        }
                        if shown.is_empty() {
                            ui.label(RichText::new("Nothing matches").color(t.text_faint));
                        }
                    });
            });
        });
        if enter && let Some(&i) = shown.get(*chosen) {
            run = Some(i);
        }
        if resp.response.clicked_elsewhere()
            && ctx.input(|i| i.pointer.any_pressed())
            && run.is_none()
        {
            close = true;
        }
        if close || run.is_some() {
            let entries = self.palette.take().and_then(|p| p.entries);
            if let Some(i) = run
                && let Some((_, _, _, entry)) = entries.and_then(|(_, e)| e.into_iter().nth(i))
            {
                self.run_entry(entry, ctx);
            }
        }
    }
}

/// One entry, its hint on the right; `scroll` brings the selected one into view. True when
/// clicked.
fn palette_row(
    ui: &mut Ui,
    t: &Tokens,
    label: &str,
    hint: &str,
    selected: bool,
    scroll: bool,
) -> bool {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, t.radius_sm, t.row_selected);
        if scroll {
            ui.scroll_to_rect(rect, None);
        }
    } else if r.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    ui.painter().text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        t.text,
    );
    ui.painter().text(
        pos2(rect.right() - 10.0, rect.center().y),
        Align2::RIGHT_CENTER,
        hint,
        egui::FontId::proportional(11.0),
        t.text_faint,
    );
    r.clicked()
}

/// `needle`'s characters appear in `haystack` in order ("cmain" → "Check out main").
pub(super) fn fuzzy(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|n| chars.any(|h| h == n))
}
