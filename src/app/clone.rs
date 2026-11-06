//! Cloning: the dialog, the clone in the background, and asking for credentials.

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

use egui::{Key, RichText, Stroke, Ui, vec2};

use super::{App, Tab, repo_root};
use crate::git;
use crate::theme::{self, Tokens};

pub(super) struct CloneForm {
    url: String,
    parent: String,
    name: String,
    /// The user typed a name; stop deriving it from the URL.
    name_edited: bool,
    error: Option<String>,
    focus: bool,
    /// The remote asked for a username and password / token.
    auth: bool,
    username: String,
    password: String,
}

impl App {
    pub(super) fn start_clone(&mut self) {
        let parent = self
            .clone_parent
            .clone()
            .or_else(|| {
                self.recent
                    .first()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf)
            })
            .or_else(home_dir)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.clone_form = Some(CloneForm {
            url: String::new(),
            parent,
            name: String::new(),
            name_edited: false,
            error: None,
            focus: true,
            auth: false,
            username: String::new(),
            password: String::new(),
        });
    }

    pub(super) fn clone_dialog(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.clone_form else {
            return;
        };
        let t = Tokens::get(ctx);
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.field_border))
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::same(18))
            .shadow(ctx.global_style().visuals.window_shadow);
        let mut submit = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::new("clone-dialog"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(110))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Clone repository").font(theme::semibold(15.0)));
                ui.add_space(10.0);
                form_fields(ui, form, &t);
                if let Some(err) = &form.error {
                    ui.colored_label(t.danger, err);
                }
                ui.add_space(12.0);
                let valid = !form.url.trim().is_empty()
                    && !form.parent.trim().is_empty()
                    && !form.name.trim().is_empty();
                ui.horizontal(|ui| {
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    if crate::repo::primary_button(ui, "Clone", valid, 90.0).clicked()
                        || (enter && valid)
                    {
                        submit = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(vec2(80.0, 30.0)))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        if resp.should_close() {
            cancel = true;
        }

        if submit {
            self.submit_clone(ctx);
        } else if cancel {
            self.clone_form = None;
        }
    }

    /// Starts cloning what the form says, unless a clone is running or the destination is taken.
    fn submit_clone(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.clone_form else {
            return;
        };
        // One clone at a time: a second would take the first one's place, whose result
        // (and its tab) would never come.
        if let Some((running, _)) = &self.cloning {
            form.error = Some(format!(
                "Wait for the clone into {} to finish",
                running.display()
            ));
            return;
        }
        let dest = PathBuf::from(form.parent.trim()).join(form.name.trim());
        let occupied = dest.exists()
            && std::fs::read_dir(&dest)
                .map(|mut d| d.next().is_some())
                .unwrap_or(true);
        if occupied {
            form.error = Some(format!("{} already exists and isn't empty", dest.display()));
            return;
        }
        let url = form.url.trim().to_owned();
        let creds = (form.auth && !form.username.trim().is_empty()).then(|| git::Credentials {
            username: form.username.trim().to_owned(),
            password: form.password.clone(),
        });
        self.clone_parent = Some(PathBuf::from(form.parent.trim()));
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        let dest2 = dest.clone();
        std::thread::spawn(move || {
            let _ = tx.send(git::clone(&url, &dest2, creds.as_ref()));
            ctx2.request_repaint();
        });
        self.cloning = Some((dest, rx));
        self.cloning_form = self.clone_form.take();
        self.error = None;
    }

    pub(super) fn poll_clone(&mut self, ctx: &egui::Context) {
        let Some((dest, rx)) = &self.cloning else {
            return;
        };
        let Ok(result) = rx.try_recv() else { return };
        let dest = dest.clone();
        self.cloning = None;
        match result {
            Ok(()) => {
                // Show it in a start page tab if one is active, otherwise in a new tab.
                self.open_repo(&dest, ctx);
                // The credentials that cloned it also fetch and push in its tab, and in no
                // other: when it couldn't be opened, the active tab is another repository's.
                let root = repo_root(&dest).ok();
                if let Some(form) = self.cloning_form.take().filter(|f| f.auth)
                    && let Some(Tab::Repo(repo)) = self.tabs.get_mut(self.active)
                    && root.as_ref() == Some(&repo.path)
                {
                    repo.set_credentials(git::Credentials {
                        username: form.username.trim().to_owned(),
                        password: form.password,
                    });
                }
            }
            // Back to the form, asking for credentials.
            Err(e) if git::is_auth_error(&e) => match self.cloning_form.take() {
                Some(mut form) => {
                    form.error = Some(if form.auth {
                        "Those credentials were refused.".to_owned()
                    } else {
                        "This repository needs you to sign in.".to_owned()
                    });
                    form.auth = true;
                    form.password.clear();
                    self.clone_form = Some(form);
                }
                None => self.error = Some(format!("Clone failed: {e}")),
            },
            Err(e) => self.error = Some(format!("Clone failed: {e}")),
        }
        self.cloning_form = None;
    }
}

/// URL, folder and name, where that puts the clone, and the credentials once asked for.
fn form_fields(ui: &mut Ui, form: &mut CloneForm, t: &Tokens) {
    ui.label(RichText::new("URL").color(t.text_dim).small());
    let url = ui.add(
        egui::TextEdit::singleline(&mut form.url)
            .hint_text("https://github.com/user/repo.git")
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 6)),
    );
    if form.focus {
        url.request_focus();
        form.focus = false;
    }
    if url.changed() && !form.name_edited {
        form.name = git::clone_dir_name(&form.url);
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Into folder").color(t.text_dim).small());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut form.parent)
                .desired_width(ui.available_width() - 90.0)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        if ui
            .add(egui::Button::new("Browse…").min_size(vec2(80.0, 28.0)))
            .clicked()
            && let Some(dir) = rfd::FileDialog::new().set_title("Clone into").pick_folder()
        {
            form.parent = dir.display().to_string();
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Name").color(t.text_dim).small());
    if ui
        .add(
            egui::TextEdit::singleline(&mut form.name)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 6)),
        )
        .changed()
    {
        form.name_edited = true;
    }
    let dest = PathBuf::from(form.parent.trim()).join(form.name.trim());
    ui.label(
        RichText::new(dest.display().to_string())
            .color(t.text_faint)
            .small(),
    );
    if form.auth {
        ui.add_space(6.0);
        ui.label(
            RichText::new("The remote wants to know who you are. Most hosts take a personal access token as the password.")
                .color(t.text_dim)
                .small(),
        );
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut form.username)
                .hint_text("Username")
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.password)
                .password(true)
                .hint_text("Password or token")
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 6)),
        );
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}
