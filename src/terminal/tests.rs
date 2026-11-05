//! Key, paste and clipboard encoding, and the colour palette.

use egui::{Color32, Key, Modifiers};

use super::keys::{clipboard_control, key_bytes, paste_bytes};
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
