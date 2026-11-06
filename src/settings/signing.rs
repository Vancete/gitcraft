//! Commit signing: the format, key and program git signs with and whether it signs commits and
//! tags by default, read from and written to the user's global git config.

use std::time::{Duration, Instant};

use egui::{RichText, Ui};

use crate::theme::Tokens;

/// How long values read from the config hold while nothing is being edited: it can change
/// outside gitcraft.
const RECHECK: Duration = Duration::from_secs(30);

/// `gpg.format` values, with what the settings page calls them.
const FORMATS: [(&str, &str); 3] = [
    ("openpgp", "OpenPGP (GPG)"),
    ("ssh", "SSH"),
    ("x509", "X.509 (S/MIME)"),
];

/// The signing part of a git config, as it is set there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Signing {
    /// `gpg.format`; empty when unset (git then uses openpgp).
    pub format: String,
    /// `user.signingkey`.
    pub key: String,
    /// The program for the format: `gpg.program`, `gpg.ssh.program` or `gpg.x509.program`.
    pub program: String,
    /// `commit.gpgSign`.
    pub sign_commits: bool,
    /// `tag.gpgSign`.
    pub sign_tags: bool,
}

impl Signing {
    /// The format git signs with, `openpgp` when none is set.
    fn format(&self) -> &str {
        if self.format.is_empty() {
            "openpgp"
        } else {
            &self.format
        }
    }
}

/// The config key of the signing program for `format`.
fn program_key(format: &str) -> &'static str {
    program_keys(format)[0]
}

/// Every config key git reads the signing program for `format` from, the one gitcraft writes
/// first. OpenPGP's is `gpg.program` or its newer name `gpg.openpgp.program`, whichever comes
/// last.
fn program_keys(format: &str) -> &'static [&'static str] {
    match format {
        "ssh" => &["gpg.ssh.program"],
        "x509" => &["gpg.x509.program"],
        _ => &["gpg.program", "gpg.openpgp.program"],
    }
}

/// The signing settings in `config --list -z` output, entries `key\nvalue\0` (a key alone is
/// true). The last of a key wins, as in git.
pub(super) fn parse(list: &str) -> Signing {
    let entries: Vec<(String, Option<&str>)> = list
        .split('\0')
        .filter(|e| !e.is_empty())
        .map(|e| match e.split_once('\n') {
            Some((key, value)) => (key.to_lowercase(), Some(value)),
            None => (e.to_lowercase(), None),
        })
        .collect();
    let get_any = |keys: &[&str]| {
        entries
            .iter()
            .rev()
            .find(|(k, _)| keys.contains(&k.as_str()))
            .map(|(_, v)| *v)
    };
    let get = |key: &str| get_any(&[key]);
    let text = |key: &str| get(key).flatten().unwrap_or_default().to_owned();
    let flag = |key: &str| {
        get(key).is_some_and(|v| {
            v.is_none_or(|v| ["true", "yes", "on", "1"].contains(&v.trim().to_lowercase().as_str()))
        })
    };
    let format = text("gpg.format");
    let program = get_any(program_keys(&format))
        .flatten()
        .unwrap_or_default()
        .to_owned();
    Signing {
        key: text("user.signingkey"),
        program,
        sign_commits: flag("commit.gpgsign"),
        sign_tags: flag("tag.gpgsign"),
        format,
    }
}

/// Reads the signing settings of the config `scope` names (`--global`, or `--file <path>`).
pub(super) fn read(scope: &[&str]) -> Signing {
    let mut args = vec!["config"];
    args.extend(scope);
    args.extend(["--list", "-z"]);
    // No config file at all is nothing set.
    crate::git::run(None, &args)
        .map(|out| parse(&out))
        .unwrap_or_default()
}

/// Sets `key` in the config `scope` names, or takes it out when `value` is empty. A key set
/// more than once becomes set once: git refuses to overwrite several values with one.
fn set(scope: &[&str], key: &str, value: &str) -> Result<(), String> {
    let mut args = vec!["config"];
    args.extend(scope);
    if value.is_empty() {
        args.extend(["--unset-all", key]);
        // 5: it wasn't set.
        crate::git::run_ok_codes(None, &args, &[0, 5]).map(drop)
    } else {
        args.extend(["--replace-all", key, value]);
        crate::git::run(None, &args).map(drop)
    }
}

/// Writes what differs between `before` and `after` to the config `scope` names. The program
/// is that of `after`'s format; a new format with the same program keeps the one set for it.
pub(super) fn write(scope: &[&str], before: &Signing, after: &Signing) -> Result<(), String> {
    let flag = |on: bool| if on { "true" } else { "false" };
    if after.format() != before.format() {
        set(scope, "gpg.format", &after.format)?;
    }
    if after.program != before.program
        && let Some((key, others)) = program_keys(after.format()).split_first()
    {
        for other in others {
            // It would win over the one written.
            set(scope, other, "")?;
        }
        set(scope, key, after.program.trim())?;
    }
    if after.key != before.key {
        set(scope, "user.signingkey", after.key.trim())?;
    }
    if after.sign_commits != before.sign_commits {
        set(scope, "commit.gpgSign", flag(after.sign_commits))?;
    }
    if after.sign_tags != before.sign_tags {
        set(scope, "tag.gpgSign", flag(after.sign_tags))?;
    }
    Ok(())
}

/// What the settings page holds between frames: the values as read, as being edited, and the
/// last error writing them.
#[derive(Clone)]
struct Form {
    saved: Signing,
    edit: Signing,
    read_at: Instant,
    error: Option<String>,
}

const GLOBAL: &[&str] = &["--global"];

/// The commit signing form. Choices are written at once, text fields when they lose focus.
pub(super) fn signing_ui(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let id = egui::Id::new("settings-signing");
    let mut form = ui
        .data(|d| d.get_temp::<Form>(id))
        .filter(|f| f.edit != f.saved || f.read_at.elapsed() < RECHECK)
        .unwrap_or_else(|| {
            let saved = read(GLOBAL);
            Form {
                edit: saved.clone(),
                saved,
                read_at: Instant::now(),
                error: None,
            }
        });
    let mut commit = false;
    egui::Grid::new("settings-signing")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Format");
            let shown = FORMATS
                .iter()
                .find(|(value, _)| *value == form.edit.format())
                .map_or(form.edit.format(), |(_, label)| label);
            egui::ComboBox::from_id_salt("settings-signing-format")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (value, label) in FORMATS {
                        if ui
                            .selectable_label(form.edit.format() == value, label)
                            .clicked()
                            && form.edit.format() != value
                        {
                            // Read back with the program set for it.
                            form.edit.format = value.to_owned();
                            commit = true;
                        }
                    }
                })
                .response
                .on_hover_text("gpg.format");
            ui.end_row();

            let (key_hint, program_hint) = match form.edit.format() {
                "ssh" => ("~/.ssh/id_ed25519.pub, or key::ssh-ed25519 …", "ssh-keygen"),
                "x509" => ("Certificate ID", "gpgsm"),
                _ => ("Key ID or fingerprint", "gpg"),
            };
            ui.label("Signing key");
            let key = ui
                .add(
                    egui::TextEdit::singleline(&mut form.edit.key)
                        .hint_text(key_hint)
                        .desired_width(300.0),
                )
                .on_hover_text("user.signingkey; empty lets git pick one from your identity");
            commit |= key.lost_focus();
            ui.end_row();

            ui.label("Program");
            let program = ui
                .add(
                    egui::TextEdit::singleline(&mut form.edit.program)
                        .hint_text(program_hint)
                        .desired_width(300.0),
                )
                .on_hover_text(format!(
                    "{}; empty runs {program_hint} from the PATH",
                    program_key(form.edit.format())
                ));
            commit |= program.lost_focus();
            ui.end_row();

            ui.label("Sign by default");
            ui.horizontal(|ui| {
                commit |= ui
                    .checkbox(&mut form.edit.sign_commits, "Commits")
                    .on_hover_text("commit.gpgSign")
                    .changed();
                commit |= ui
                    .checkbox(&mut form.edit.sign_tags, "Tags")
                    .on_hover_text("tag.gpgSign: annotated tags")
                    .changed();
            });
            ui.end_row();
        });
    match &form.error {
        Some(e) => ui.label(RichText::new(e).color(t.danger).small()),
        None => ui.label(
            RichText::new("Your global git config; a repository's own config can override it.")
                .color(t.text_faint)
                .small(),
        ),
    };
    if commit && form.edit != form.saved {
        form.error = write(GLOBAL, &form.saved, &form.edit).err();
        form.saved = read(GLOBAL);
        form.read_at = Instant::now();
        if form.error.is_none() {
            form.edit = form.saved.clone();
        }
    }
    ui.data_mut(|d| d.insert_temp(id, form));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_settings_in_a_config_list() {
        let list = "user.name\nMe\0gpg.format\nssh\0user.signingkey\n~/.ssh/id.pub\0\
                    gpg.program\ngpg2\0gpg.ssh.program\nC:/ssh/ssh-keygen.exe\0\
                    commit.gpgsign\0tag.gpgsign\nfalse\0tag.gpgsign\nYes\0";
        assert_eq!(
            parse(list),
            Signing {
                format: "ssh".into(),
                key: "~/.ssh/id.pub".into(),
                program: "C:/ssh/ssh-keygen.exe".into(),
                sign_commits: true,
                sign_tags: true,
            }
        );
        let plain = parse("gpg.program\ngpg2\0commit.gpgsign\nfalse\0");
        assert_eq!(
            (plain.format(), plain.program.as_str()),
            ("openpgp", "gpg2")
        );
        assert!(!plain.sign_commits && !plain.sign_tags);
        assert_eq!(parse(""), Signing::default());
    }

    /// Against a scratch config file, never the user's own.
    #[test]
    fn signing_settings_are_written_and_read_back() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("gitcraft-signing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join("config");
        let file = file.to_string_lossy();
        let scope = ["--file", &*file];
        assert_eq!(read(&scope), Signing::default());

        let ssh = Signing {
            format: "ssh".into(),
            key: "key::ssh-ed25519 AAAA me".into(),
            program: "ssh-keygen".into(),
            sign_commits: true,
            sign_tags: false,
        };
        write(&scope, &Signing::default(), &ssh)?;
        assert_eq!(read(&scope), ssh);

        // Back to OpenPGP without a key: the key goes, SSH's program stays SSH's.
        let openpgp = Signing {
            format: "openpgp".into(),
            sign_tags: true,
            ..Signing::default()
        };
        write(
            &scope,
            &ssh,
            &Signing {
                program: ssh.program.clone(),
                ..openpgp.clone()
            },
        )?;
        assert_eq!(read(&scope), openpgp);
        let gpg = Signing {
            program: "gpg2".into(),
            ..openpgp.clone()
        };
        write(&scope, &openpgp, &gpg)?;
        assert_eq!(read(&scope), gpg);
        let text = std::fs::read_to_string(dir.join("config")).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!text.contains("signingkey"), "{text}");
        assert!(text.contains("program = ssh-keygen"), "{text}");
        assert!(text.contains("program = gpg2"), "{text}");
        Ok(())
    }

    /// OpenPGP's program under its newer name, and keys set twice in a hand-edited config.
    #[test]
    fn signing_settings_set_twice_or_under_the_newer_name() -> Result<(), String> {
        let newer = parse("gpg.program\ngpg2\0gpg.openpgp.program\n/opt/gpg\0");
        assert_eq!(newer.program, "/opt/gpg");

        let dir = std::env::temp_dir().join(format!("gitcraft-signing2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("config");
        std::fs::write(
            &path,
            "[commit]\n\tgpgsign = true\n[commit]\n\tgpgsign = false\n\
             [gpg \"openpgp\"]\n\tprogram = /opt/gpg\n",
        )
        .map_err(|e| e.to_string())?;
        let file = path.to_string_lossy();
        let scope = ["--file", &*file];
        let before = read(&scope);
        assert_eq!(before.program, "/opt/gpg");
        let after = Signing {
            program: "gpg2".into(),
            sign_commits: true,
            ..before.clone()
        };
        let written = write(&scope, &before, &after);
        let back = read(&scope);
        let _ = std::fs::remove_dir_all(&dir);
        written?;
        assert_eq!(back, after);
        Ok(())
    }
}
