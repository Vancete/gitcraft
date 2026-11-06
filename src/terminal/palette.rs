//! The terminal's colours: the 16 ANSI ones per theme, the 256-colour cube and true colour.

use egui::Color32;

use crate::theme::Tokens;

/// Terminal colours tuned to the app theme.
pub(super) struct Palette {
    fg: Color32,
    ansi: [Color32; 16],
}

impl Palette {
    pub(super) fn new(t: &Tokens) -> Self {
        let ansi = if t.dark() {
            [
                Color32::from_rgb(40, 40, 40),
                Color32::from_rgb(236, 91, 98),
                Color32::from_rgb(92, 196, 122),
                Color32::from_rgb(232, 176, 70),
                Color32::from_rgb(64, 156, 255),
                Color32::from_rgb(200, 110, 230),
                Color32::from_rgb(60, 205, 210),
                Color32::from_rgb(200, 200, 200),
                Color32::from_rgb(110, 110, 110),
                Color32::from_rgb(255, 120, 125),
                Color32::from_rgb(130, 225, 150),
                Color32::from_rgb(250, 205, 100),
                Color32::from_rgb(110, 180, 255),
                Color32::from_rgb(220, 150, 245),
                Color32::from_rgb(110, 225, 230),
                Color32::from_rgb(245, 245, 245),
            ]
        } else {
            [
                Color32::from_rgb(20, 20, 20),
                Color32::from_rgb(200, 40, 50),
                Color32::from_rgb(30, 140, 70),
                Color32::from_rgb(170, 110, 0),
                Color32::from_rgb(20, 100, 210),
                Color32::from_rgb(150, 60, 190),
                Color32::from_rgb(0, 140, 150),
                Color32::from_rgb(110, 110, 110),
                Color32::from_rgb(90, 90, 90),
                Color32::from_rgb(230, 60, 70),
                Color32::from_rgb(40, 170, 90),
                Color32::from_rgb(200, 140, 0),
                Color32::from_rgb(40, 120, 240),
                Color32::from_rgb(180, 80, 220),
                Color32::from_rgb(0, 170, 180),
                Color32::from_rgb(40, 40, 40),
            ]
        };
        Self { fg: t.text, ansi }
    }

    pub(super) fn color(&self, c: vt100::Color) -> Option<Color32> {
        match c {
            vt100::Color::Default => None,
            vt100::Color::Idx(i) if i < 16 => self.ansi.get(i as usize).copied(),
            vt100::Color::Idx(i) if i < 232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Some(Color32::from_rgb(
                    level(i / 36),
                    level((i / 6) % 6),
                    level(i % 6),
                ))
            }
            vt100::Color::Idx(i) => {
                let v = 8 + (i - 232).saturating_mul(10);
                Some(Color32::from_gray(v))
            }
            vt100::Color::Rgb(r, g, b) => Some(Color32::from_rgb(r, g, b)),
        }
    }

    /// (foreground, background-if-not-default) of a cell.
    pub(super) fn colors(&self, cell: &vt100::Cell) -> (Color32, Option<Color32>) {
        let mut fg = self.color(cell.fgcolor()).unwrap_or(self.fg);
        let mut bg = self.color(cell.bgcolor());
        if cell.bold()
            && let vt100::Color::Idx(i) = cell.fgcolor()
            && i < 8
        {
            fg = self.ansi.get(i as usize + 8).copied().unwrap_or(fg);
        }
        if cell.dim() {
            fg = fg.gamma_multiply(0.6);
        }
        if cell.inverse() {
            let old_fg = fg;
            fg = bg.unwrap_or(Color32::from_rgb(30, 30, 30));
            bg = Some(old_fg);
        }
        (fg, bg)
    }
}
