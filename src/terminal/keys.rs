//! What keys, pastes and clipboard events send to the shell.

use egui::{Key, Modifiers};

/// What pasting `text` sends: line breaks as Enter, and in bracketed-paste mode the text between
/// the brackets, without escape characters, so pasted text can't end the paste early
/// (`\x1b[201~`) and run the rest as typed commands.
pub(super) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
    } else {
        text.into_bytes()
    }
}

/// The control character a Copy / Cut event stands for (`letter` C or X): only when it came from
/// Ctrl+letter itself, not Cmd+letter on macOS, Ctrl+Shift+C or a Copy key.
pub(super) fn clipboard_control(mods: Modifiers, letter: u8) -> Option<u8> {
    (mods.ctrl && !mods.shift && !mods.alt).then(|| letter - b'A' + 1)
}

/// Whether text typed with these modifiers goes to the shell after an Escape, as Alt+letter does
/// in xterm (readline's Alt+B, Alt+F, Alt+.). Not on macOS, where Option types other characters,
/// nor with Ctrl, which AltGr is on Windows.
pub(super) fn alt_prefixes_text(mods: Modifiers) -> bool {
    mods.alt && !mods.ctrl && !mods.mac_cmd && !cfg!(target_os = "macos")
}

/// What a key sends to the shell (xterm sequences).
pub(super) fn key_bytes(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // xterm's modifier parameter: 1, plus 1 for Shift, 2 for Alt and 4 for Ctrl.
    let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
    // Cursor keys: `ESC [ 1 ; m X` with modifiers (Ctrl+Left: a word left), else SS3 in
    // application cursor mode.
    let cursor = |c: char| {
        if m > 1 {
            format!("\x1b[1;{m}{c}")
        } else if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
    };
    // Editing and function keys: `ESC [ n ~`, `ESC [ n ; m ~` with modifiers.
    let tilde = |n: u8| {
        if m > 1 {
            format!("\x1b[{n};{m}~")
        } else {
            format!("\x1b[{n}~")
        }
    };
    // F1 to F4: SS3 P to S, `ESC [ 1 ; m P` with modifiers.
    let pf = |c: char| {
        if m > 1 {
            format!("\x1b[1;{m}{c}")
        } else {
            format!("\x1bO{c}")
        }
    };
    if mods.ctrl && !mods.alt {
        // Ctrl+letter → control character (Ctrl+C and Ctrl+X arrive as Copy / Cut).
        let name = key.name();
        if name.len() == 1
            && let Some(c) = name.chars().next()
            && c.is_ascii_alphabetic()
        {
            return Some(vec![(c.to_ascii_uppercase() as u8) - b'A' + 1]);
        }
        let control = match key {
            Key::Space => Some(0),
            Key::OpenBracket => Some(0x1b),
            Key::Backslash => Some(0x1c),
            Key::CloseBracket => Some(0x1d),
            Key::Backspace => Some(0x08),
            _ => None,
        };
        if let Some(byte) = control {
            return Some(vec![byte]);
        }
    }
    // Alt with a key that sends a single character sends Escape first.
    let meta = |s: &str| {
        if alt_prefixes_text(mods) {
            format!("\x1b{s}")
        } else {
            s.to_owned()
        }
    };
    let s: String = match key {
        Key::Enter => meta("\r"),
        Key::Tab if mods.shift => "\x1b[Z".into(),
        Key::Tab => meta("\t"),
        Key::Backspace => meta("\x7f"),
        Key::Escape => meta("\x1b"),
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::F1 => pf('P'),
        Key::F2 => pf('Q'),
        Key::F3 => pf('R'),
        Key::F4 => pf('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ => return None,
    };
    Some(s.into_bytes())
}
