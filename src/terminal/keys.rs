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

/// What a key sends to the shell (xterm sequences).
pub(super) fn key_bytes(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let csi_or_ss3 = |c: char| {
        if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
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
    }
    let s: String = match key {
        Key::Enter => "\r".into(),
        Key::Tab if mods.shift => "\x1b[Z".into(),
        Key::Tab => "\t".into(),
        Key::Backspace => "\x7f".into(),
        Key::Escape => "\x1b".into(),
        Key::ArrowUp => csi_or_ss3('A'),
        Key::ArrowDown => csi_or_ss3('B'),
        Key::ArrowRight => csi_or_ss3('C'),
        Key::ArrowLeft => csi_or_ss3('D'),
        Key::Home => csi_or_ss3('H'),
        Key::End => csi_or_ss3('F'),
        Key::PageUp => "\x1b[5~".into(),
        Key::PageDown => "\x1b[6~".into(),
        Key::Insert => "\x1b[2~".into(),
        Key::Delete => "\x1b[3~".into(),
        Key::F1 => "\x1bOP".into(),
        Key::F2 => "\x1bOQ".into(),
        Key::F3 => "\x1bOR".into(),
        Key::F4 => "\x1bOS".into(),
        _ => return None,
    };
    Some(s.into_bytes())
}
