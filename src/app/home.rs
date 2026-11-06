//! The start page (open, create, clone, recent repositories) and the git-missing page.

use std::path::Path;

use egui::{Align2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::App;
use crate::git;
use crate::icons;
use crate::theme::{self, Tokens};

enum RecentClick {
    Open,
    Forget,
}

impl App {
    pub(super) fn pick_existing(&mut self, ctx: &egui::Context) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Open a git repository")
            .pick_folder()
        {
            self.open_repo(&dir, ctx);
        }
    }

    pub(super) fn pick_new(&mut self, ctx: &egui::Context) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose a folder for the new repository")
            .pick_folder()
        else {
            return;
        };
        // Already the root of a repository: just open it rather than re-initialising.
        if let Ok(root) = git::toplevel(&dir)
            && same_path(&root, &dir)
        {
            self.open_repo(&dir, ctx);
            return;
        }
        match git::init(&dir) {
            Ok(()) => self.open_repo(&dir, ctx),
            Err(e) => self.error = Some(format!("git init failed: {e}")),
        }
    }

    pub(super) fn home(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        let mut open = None;
        let mut forget = None;
        let mut action = None;

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(640.0);
                    ui.add_space((ui.available_height() * 0.10).max(32.0));
                    self.home_header(ui, &t);
                    ui.add_space(26.0);

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        let w = (ui.available_width() - 24.0) / 3.0;
                        for (i, (icon, title, sub)) in [
                            ("folder-open", "Open", "An existing local repository"),
                            ("folder-plus", "Create", "A new repository (git init)"),
                            ("log-in", "Clone", "Copy a remote repository"),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if action_card(ui, w, icon, title, sub).clicked() {
                                action = Some(i);
                            }
                        }
                    });

                    if let Some(err) = &self.error {
                        ui.add_space(12.0);
                        ui.colored_label(t.danger, err);
                    }
                    if let Some((dest, _)) = &self.cloning {
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(format!("Cloning into {}…", dest.display()));
                        });
                    }

                    if !self.recent.is_empty() {
                        ui.add_space(30.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.label(
                                RichText::new("RECENT")
                                    .font(theme::semibold(10.5))
                                    .color(t.text_faint),
                            );
                            ui.add_space(4.0);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for (i, path) in self.recent.iter().enumerate() {
                                match recent_row(ui, path) {
                                    Some(RecentClick::Open) => open = Some(path.clone()),
                                    Some(RecentClick::Forget) => forget = Some(i),
                                    None => {}
                                }
                            }
                        });
                    }
                });
            });

        match action {
            Some(0) => self.pick_existing(&ctx),
            Some(1) => self.pick_new(&ctx),
            Some(_) => self.start_clone(),
            None => {}
        }
        if let Some(path) = open {
            self.open_repo(&path, &ctx);
        }
        if let Some(i) = forget {
            self.recent.remove(i);
        }
    }

    /// The name, the git version found, and the offer to add an AppImage to the menu.
    fn home_header(&mut self, ui: &mut Ui, t: &Tokens) {
        ui.horizontal(|ui| {
            icons::inline(ui, "git-merge", 34.0, t.accent);
            ui.label(RichText::new("gitcraft").font(theme::semibold(30.0)));
        });
        ui.add_space(4.0);
        if let Ok(v) = &self.git_version {
            ui.horizontal(|ui| {
                icons::inline(ui, "check", 14.0, t.success);
                ui.label(RichText::new(v).color(t.text_dim));
            });
        }
        if let Some(appimage) = crate::desktop_entry::appimage()
            && !crate::desktop_entry::installed()
        {
            ui.add_space(4.0);
            let link = ui
                .link("Add gitcraft to the applications menu")
                .on_hover_text(appimage.display().to_string());
            if link.clicked() {
                self.error = crate::desktop_entry::install(&appimage).err();
            }
        }
    }

    pub(super) fn git_missing(&mut self, ui: &mut Ui, err: &str) {
        let t = Tokens::get(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            icons::inline(ui, "circle-alert", 40.0, t.warning);
            ui.add_space(8.0);
            ui.label(RichText::new("Git was not found").font(theme::semibold(24.0)));
            ui.add_space(8.0);
            ui.label("gitcraft uses the git command line. Install git and make sure `git` is on your PATH.");
            ui.add_space(4.0);
            ui.label(RichText::new(err).color(t.text_faint).small());
            ui.add_space(16.0);
            if crate::repo::primary_button(ui, "Check again", true, 140.0).clicked() {
                self.git_version = git::version();
            }
            ui.add_space(8.0);
            ui.hyperlink_to("Download git", "https://git-scm.com/downloads");
        });
    }
}

/// A large clickable card on the start page.
fn action_card(ui: &mut Ui, width: f32, icon: &str, title: &str, subtitle: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 116.0), Sense::click());
    let fill = if resp.hovered() { t.hover } else { t.card };
    let stroke = if resp.hovered() {
        t.accent
    } else {
        t.field_border
    };
    let p = ui.painter();
    p.rect(
        rect,
        t.radius_lg,
        fill,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.center().x, rect.top() + 34.0), vec2(28.0, 28.0)),
        icon,
        26.0,
        t.accent,
    );
    p.text(
        pos2(rect.center().x, rect.top() + 68.0),
        Align2::CENTER_CENTER,
        title,
        theme::semibold(14.0),
        t.text,
    );
    p.text(
        pos2(rect.center().x, rect.top() + 90.0),
        Align2::CENTER_CENTER,
        subtitle,
        egui::FontId::proportional(11.5),
        t.text_dim,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn recent_row(ui: &mut Ui, path: &Path) -> Option<RecentClick> {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, t.radius, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 20.0, y), vec2(18.0, 18.0)),
        "folder-git-2",
        16.0,
        t.icon,
    );
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Leave room for the remove button; long paths are cut at the row's edge.
    let p = ui.painter_at(rect.with_max_x(rect.right() - 36.0));
    p.text(
        pos2(rect.left() + 40.0, y - 8.0),
        Align2::LEFT_CENTER,
        name,
        theme::medium(12.5),
        t.text,
    );
    p.text(
        pos2(rect.left() + 40.0, y + 8.0),
        Align2::LEFT_CENTER,
        path.display().to_string(),
        egui::FontId::proportional(11.0),
        t.text_faint,
    );
    let mut forget = false;
    if hovered || ui.rect_contains_pointer(rect) {
        let x_rect = Rect::from_center_size(pos2(rect.right() - 18.0, y), vec2(20.0, 20.0));
        let x = ui
            .interact(x_rect, resp.id.with("forget"), Sense::click())
            .on_hover_text("Remove from list");
        if x.hovered() {
            ui.painter().rect_filled(x_rect, t.radius_sm, t.pressed);
        }
        icons::paint(ui, x_rect, "x", 12.0, t.text_faint);
        forget = x.clicked();
    }
    if forget {
        Some(RecentClick::Forget)
    } else if resp.clicked() {
        Some(RecentClick::Open)
    } else {
        None
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
