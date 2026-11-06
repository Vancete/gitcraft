//! Dialogs about a remote: add or edit one, sign in to one, open a pull request on one.

use egui::{RichText, Ui};

use crate::repo::{Action, Refresh};
use crate::theme::Tokens;

use super::{Outcome, buttons, field, title, usable};

/// Adds a remote and fetches it, or renames one and changes its URL.
pub(super) fn remote(
    ui: &mut Ui,
    enter: bool,
    original: &Option<(String, String)>,
    name: &mut String,
    url: &mut String,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    let heading = if original.is_some() {
        "Edit remote"
    } else {
        "Add remote"
    };
    title(
        ui,
        heading,
        "A name (like origin or upstream) and the repository's URL.",
        &t,
    );
    field(ui, name, "origin", focus);
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::singleline(url)
            .hint_text("https://github.com/user/repo.git")
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 6)),
    );
    ui.add_space(8.0);
    let valid = usable(name) && usable(url);
    let label = if original.is_some() { "Save" } else { "Add" };
    buttons(ui, label, valid, enter, || {
        let (name, url) = (name.trim().to_owned(), url.trim().to_owned());
        let steps = match original {
            None => vec![
                vec!["remote".into(), "add".into(), name.clone(), url],
                vec!["fetch".into(), name.clone()],
            ],
            Some((old_name, old_url)) => {
                let mut steps = Vec::new();
                if *old_name != name {
                    steps.push(vec![
                        "remote".into(),
                        "rename".into(),
                        old_name.clone(),
                        name.clone(),
                    ]);
                }
                if *old_url != url {
                    steps.push(vec!["remote".into(), "set-url".into(), name.clone(), url]);
                }
                steps
            }
        };
        Action::Ops {
            label: format!("Remote {name}"),
            ops: steps,
            refresh: Refresh::All,
        }
    })
}

/// Asks for the username and password of `url`, after `refused` ones if they were.
pub(super) fn credentials(
    ui: &mut Ui,
    enter: bool,
    url: &str,
    refused: bool,
    username: &mut String,
    password: &mut String,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    let host = host_of(url);
    title(
        ui,
        &format!("Sign in to {host}"),
        "git needs a username and password for this remote. Most hosts want a personal access \
         token as the password.",
        &t,
    );
    if refused {
        ui.colored_label(
            t.danger,
            "Those credentials were refused. Check them and try again.",
        );
        ui.add_space(6.0);
    }
    field(ui, username, "Username", focus);
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::singleline(password)
            .password(true)
            .hint_text("Password or token")
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 6)),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Used for this tab until you close it. If git has a credential helper (such as Git \
             Credential Manager), it saves them once they work.",
        )
        .color(t.text_faint)
        .small(),
    );
    ui.add_space(8.0);
    let valid = !username.trim().is_empty() && !password.is_empty();
    buttons(ui, "Sign in", valid, enter, || {
        Action::SignIn(crate::git::Credentials {
            username: username.trim().to_owned(),
            password: password.clone(),
        })
    })
}

/// A pull request of `branch` into `base`, once the branch is pushed.
#[allow(clippy::too_many_arguments)]
pub(super) fn create_pr(
    ui: &mut Ui,
    noun: &str,
    branch: &str,
    base: &mut String,
    pr_title: &mut String,
    body: &mut String,
    draft: &mut bool,
    focus: &mut bool,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    title(
        ui,
        &format!("Create {noun}"),
        &format!("{branch} → {base} (the branch is pushed first)"),
        &t,
    );
    field(ui, pr_title, "Title", focus);
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::multiline(body)
            .hint_text("Description")
            .desired_width(f32::INFINITY)
            .desired_rows(5)
            .margin(egui::Margin::symmetric(8, 6)),
    );
    ui.horizontal(|ui| {
        ui.label(RichText::new("Into").color(t.text_dim));
        ui.add(egui::TextEdit::singleline(base).desired_width(160.0));
        ui.checkbox(draft, "Draft");
    });
    ui.add_space(8.0);
    let valid = !pr_title.trim().is_empty() && !base.trim().is_empty();
    buttons(ui, "Create", valid, false, || Action::CreatePr {
        branch: branch.to_owned(),
        base: base.trim().to_owned(),
        title: pr_title.trim().to_owned(),
        body: body.trim().to_owned(),
        draft: *draft,
    })
}

/// `github.com` from `https://user@github.com/owner/repo.git`.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    // The user info ends at the last `@` before the path: one in the path isn't it.
    let authority = rest.split('/').next().unwrap_or(rest);
    let rest = authority.rsplit_once('@').map_or(authority, |(_, r)| r);
    rest.split(['/', ':'])
        .next()
        .filter(|h| !h.is_empty())
        .unwrap_or("the remote")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(
            host_of("https://user@github.com/owner/repo.git"),
            "github.com"
        );
        assert_eq!(
            host_of("https://gitlab.example.com:8443/a/b"),
            "gitlab.example.com"
        );
        assert_eq!(host_of("git@github.com:owner/repo.git"), "github.com");
        // An `@` in the path is not user info.
        assert_eq!(
            host_of("https://good.example/u@evil.example/r"),
            "good.example"
        );
        assert_eq!(host_of(""), "the remote");
    }
}
