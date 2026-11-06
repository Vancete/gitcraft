//! Key, paste and clipboard encoding, and the colour palette.

use egui::{Color32, Key, Modifiers};

use super::keys::{alt_prefixes_text, clipboard_control, key_bytes, paste_bytes};
use super::palette::Palette;
use crate::theme::Tokens;

#[test]
fn keys_to_bytes() {
    assert_eq!(
        key_bytes(Key::Enter, Modifiers::NONE, false),
        Some(b"\r".to_vec())
    );
    assert_eq!(
        key_bytes(Key::ArrowUp, Modifiers::NONE, false),
        Some(b"\x1b[A".to_vec())
    );
    assert_eq!(
        key_bytes(Key::ArrowUp, Modifiers::NONE, true),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(key_bytes(Key::D, Modifiers::CTRL, false), Some(vec![4]));
    assert_eq!(key_bytes(Key::A, Modifiers::NONE, false), None);
}

/// Modified cursor and editing keys, function keys past F4, and Alt and Ctrl with the keys
/// that send one character, as xterm sends them.
#[test]
fn modified_and_function_keys() {
    let key = |k, m| key_bytes(k, m, true);
    // Ctrl+Left moves a word left in readline; plain arrows keep SS3 in application mode.
    assert_eq!(
        key(Key::ArrowLeft, Modifiers::CTRL),
        Some(b"\x1b[1;5D".to_vec())
    );
    assert_eq!(
        key(Key::ArrowRight, Modifiers::SHIFT),
        Some(b"\x1b[1;2C".to_vec())
    );
    assert_eq!(key(Key::End, Modifiers::NONE), Some(b"\x1bOF".to_vec()));
    assert_eq!(
        key(Key::Delete, Modifiers::CTRL),
        Some(b"\x1b[3;5~".to_vec())
    );
    assert_eq!(key(Key::F1, Modifiers::SHIFT), Some(b"\x1b[1;2P".to_vec()));
    assert_eq!(key(Key::F5, Modifiers::NONE), Some(b"\x1b[15~".to_vec()));
    assert_eq!(key(Key::F10, Modifiers::NONE), Some(b"\x1b[21~".to_vec()));
    assert_eq!(key(Key::F12, Modifiers::NONE), Some(b"\x1b[24~".to_vec()));
    assert_eq!(key(Key::Space, Modifiers::CTRL), Some(vec![0]));
    assert_eq!(key(Key::Backspace, Modifiers::CTRL), Some(vec![8]));
    if cfg!(target_os = "macos") {
        assert!(!alt_prefixes_text(Modifiers::ALT));
    } else {
        assert_eq!(
            key(Key::Backspace, Modifiers::ALT),
            Some(b"\x1b\x7f".to_vec())
        );
        assert!(alt_prefixes_text(Modifiers::ALT));
    }
    // AltGr on Windows is Ctrl+Alt: it types characters of its own.
    assert!(!alt_prefixes_text(Modifiers::CTRL | Modifiers::ALT));
    assert!(!alt_prefixes_text(Modifiers::NONE));
}

#[test]
fn pastes() {
    assert_eq!(paste_bytes("a\r\nb\n", false), b"a\rb\r".to_vec());
    assert_eq!(
        paste_bytes("x\x1b[201~rm -rf ~\n", true),
        b"\x1b[200~x[201~rm -rf ~\r\x1b[201~".to_vec()
    );
}

#[test]
fn clipboard_controls() {
    assert_eq!(clipboard_control(Modifiers::CTRL, b'C'), Some(3));
    assert_eq!(clipboard_control(Modifiers::CTRL, b'X'), Some(0x18));
    assert_eq!(
        clipboard_control(Modifiers::CTRL | Modifiers::SHIFT, b'C'),
        None
    );
    assert_eq!(clipboard_control(Modifiers::MAC_CMD, b'C'), None);
    assert_eq!(clipboard_control(Modifiers::NONE, b'C'), None);
}

#[test]
fn indexed_colours() {
    let p = Palette::new(&Tokens::for_kind(crate::theme::ThemeKind::Dark));
    assert_eq!(
        p.color(vt100::Color::Idx(16)),
        Some(Color32::from_rgb(0, 0, 0))
    );
    assert_eq!(
        p.color(vt100::Color::Idx(231)),
        Some(Color32::from_rgb(255, 255, 255))
    );
    assert_eq!(
        p.color(vt100::Color::Idx(255)),
        Some(Color32::from_gray(238))
    );
}

/// The shell's cursor-position query is answered even when it arrives split across two reads,
/// with the cursor where it was at the query.
#[test]
fn cursor_position_queries_are_answered() {
    let (writer, replies) = std::sync::mpsc::channel();
    let mut parser = vt100::Parser::new_with_callbacks(
        super::nz(24),
        super::nz(80),
        0,
        super::Replies { writer },
    );
    parser.process(b"ab\x1b[");
    parser.process(b"6nmore text");
    parser.process(b"\r\n\x1b[6n\x1b[?6n\x1b[5n");
    let got: Vec<Vec<u8>> = replies.try_iter().collect();
    assert_eq!(got, [b"\x1b[1;3R".to_vec(), b"\x1b[2;1R".to_vec()]);
}
