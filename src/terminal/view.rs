//! Painting the screen, and feeding the shell the keyboard, mouse selection and wheel.

use std::sync::atomic::Ordering;

use egui::{
    Event, FontId, Key, Painter, Rect, Response, Sense, Stroke, Ui, Vec2, pos2,
    text::{LayoutJob, TextFormat},
    vec2,
};
use portable_pty::PtySize;

use super::keys::{alt_prefixes_text, clipboard_control, key_bytes, paste_bytes};
use super::palette::Palette;
use super::{Terminal, lock, nz};
use crate::theme::{self, Tokens};

/// Where the cells are on screen.
#[derive(Clone, Copy)]
struct Grid {
    inner: Rect,
    cell: Vec2,
}

impl Grid {
    /// `n` cells of `row` from `col` on.
    fn cells(&self, row: u16, col: u16, n: f32) -> Rect {
        Rect::from_min_size(
            pos2(
                self.inner.left() + col as f32 * self.cell.x,
                self.inner.top() + row as f32 * self.cell.y,
            ),
            vec2(n * self.cell.x, self.cell.y),
        )
    }
}

impl Terminal {
    /// Paints the screen into all of `ui`'s remaining space and handles input.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.drawn
            .store(ui.ctx().cumulative_pass_nr(), Ordering::Relaxed);
        let t = Tokens::get(ui.ctx());
        let font = theme::mono(crate::settings::Settings::get(ui.ctx()).terminal_font);
        let glyph = ui
            .painter()
            .layout_no_wrap("M".to_owned(), font.clone(), t.text);
        let cell = vec2(glyph.size().x.max(1.0), glyph.size().y.max(1.0));

        let rect = ui.available_rect_before_wrap();
        let id = ui.id().with("terminal");
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        ui.allocate_rect(rect, Sense::hover());
        let grid = Grid {
            inner: rect.shrink2(vec2(8.0, 6.0)),
            cell,
        };
        self.pointer(ui, &resp, rect, grid);
        let focused = resp.has_focus();
        if focused {
            // Keep Tab, arrows and Escape for the shell.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        self.fit(grid);
        if focused {
            self.keyboard(ui);
        }
        // Wheel scrolls through history.
        if resp.hovered() {
            self.wheel(ui, cell.y);
        }
        self.paint(ui, rect, grid, &font, focused, &t);
    }

    /// Drag to select; right-click copies the selection (like Windows Terminal).
    fn pointer(&mut self, ui: &Ui, resp: &Response, rect: Rect, grid: Grid) {
        let Grid { inner, cell } = grid;
        let cell_at = |p: egui::Pos2| -> (u16, u16) {
            let col = ((p.x - inner.left()) / cell.x).floor().max(0.0) as u16;
            let row = ((p.y - inner.top()) / cell.y).floor().max(0.0) as u16;
            (row.min(self.size.0.saturating_sub(1)), col.min(self.size.1))
        };
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let c = cell_at(p);
            self.selection = Some((c, c));
        } else if resp.dragged()
            && let (Some(p), Some((start, _))) = (resp.interact_pointer_pos(), self.selection)
        {
            self.selection = Some((start, cell_at(p)));
        }
        if resp.secondary_clicked() && self.copy_selection(ui.ctx()) {
            resp.request_focus();
        } else if resp.clicked() || resp.secondary_clicked() {
            self.selection = None;
            resp.request_focus();
        } else if resp.has_focus()
            && ui.input(|i| i.pointer.any_pressed())
            && !ui.rect_contains_pointer(rect)
        {
            // Like a text field: a click anywhere else gives the keyboard back to the app.
            resp.surrender_focus();
        }
    }

    /// Sizes the pty to the panel.
    fn fit(&mut self, grid: Grid) {
        let Grid { inner, cell } = grid;
        let cols = ((inner.width() / cell.x).floor() as u16).clamp(2, 1000);
        let rows = ((inner.height() / cell.y).floor() as u16).clamp(1, 500);
        if (rows, cols) != self.size {
            self.size = (rows, cols);
            // The cells it held have moved, and may be past the new edge (vt100 then
            // subtracts past zero copying it).
            self.selection = None;
            let _ = self.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            lock(&self.parser).set_size(nz(rows), nz(cols));
        }
    }

    /// Typed text, pastes, clipboard keys and other keys go to the shell.
    fn keyboard(&mut self, ui: &Ui) {
        let (events, mods, app_cursor, bracketed) = {
            let parser = lock(&self.parser);
            let screen = parser.screen();
            let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
            (
                events,
                mods,
                screen.application_cursor(),
                screen.bracketed_paste(),
            )
        };
        // Whether the key the text comes from was pressed with Alt (egui sends the key first).
        let mut alt = false;
        for event in events {
            match event {
                Event::Text(text) if alt => self.send(format!("\x1b{text}").as_bytes()),
                Event::Text(text) => self.send(text.as_bytes()),
                Event::Paste(text) => self.send(&paste_bytes(&text, bracketed)),
                // egui turns Ctrl+C / Ctrl+X (Cmd on macOS, Ctrl+Shift+C…) into clipboard
                // events: copy a selection, otherwise the shell gets Ctrl+C / Ctrl+X.
                Event::Copy => {
                    if !self.copy_selection(ui.ctx())
                        && let Some(byte) = clipboard_control(mods, b'C')
                    {
                        self.send(&[byte]);
                    }
                }
                Event::Cut => {
                    if let Some(byte) = clipboard_control(mods, b'X') {
                        self.send(&[byte]);
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    alt = alt_prefixes_text(modifiers);
                    if key == Key::C && modifiers.ctrl && modifiers.shift {
                        self.copy_selection(ui.ctx());
                    } else if let Some(bytes) = key_bytes(key, modifiers, app_cursor) {
                        self.send(&bytes);
                    }
                }
                _ => {}
            }
        }
    }

    /// Scrolls the history by the wheel, `line_height` points a line.
    fn wheel(&mut self, ui: &Ui, line_height: f32) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            self.scroll_rest += dy / line_height;
            let lines = self.scroll_rest.trunc();
            self.scroll_rest -= lines;
            let mut parser = lock(&self.parser);
            let wanted = (self.scroll as f32 + lines).max(0.0) as usize;
            parser.screen_mut().set_scrollback(wanted);
            self.scroll = parser.screen().scrollback();
        }
    }

    /// The cells, the selection under them, the cursor and how far back the view is.
    fn paint(&self, ui: &Ui, rect: Rect, grid: Grid, font: &FontId, focused: bool, t: &Tokens) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, t.field);
        let parser = lock(&self.parser);
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let (rows, cols) = (rows.get(), cols.get());
        let palette = Palette::new(t);
        let selected = self
            .selection
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        for row in 0..rows {
            // Selection highlight, under the text.
            if let Some(((r0, c0), (r1, c1))) = selected
                && row >= r0
                && row <= r1
            {
                let from = if row == r0 { c0 } else { 0 };
                let to = if row == r1 { c1 } else { cols };
                if to > from {
                    painter.rect_filled(
                        grid.cells(row, from, (to - from) as f32),
                        0.0,
                        t.accent.gamma_multiply(0.35),
                    );
                }
            }
            let job = paint_row(&painter, screen, row, cols, grid, &palette, font);
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(grid.cells(row, 0, 0.0).left_top(), galley, t.text);
        }

        // Cursor (only when following the output).
        if self.scroll == 0 && !screen.hide_cursor() && !self.exited.load(Ordering::Relaxed) {
            let (cr, cc) = screen.cursor_position();
            let r = grid.cells(cr, cc, 1.0);
            if focused {
                painter.rect_filled(r, 0.0, t.text.gamma_multiply(0.7));
            } else {
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.0, t.text_faint),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if self.scroll > 0 {
            painter.text(
                rect.right_top() + vec2(-10.0, 8.0),
                egui::Align2::RIGHT_TOP,
                format!("↑ {} lines", self.scroll),
                FontId::proportional(11.0),
                t.text_faint,
            );
        }
    }

    /// Copies the selected text; false when nothing is selected.
    fn copy_selection(&mut self, ctx: &egui::Context) -> bool {
        let Some((a, b)) = self.selection else {
            return false;
        };
        let ((r0, c0), (r1, c1)) = if a <= b { (a, b) } else { (b, a) };
        if (r0, c0) == (r1, c1) {
            return false;
        }
        let text = lock(&self.parser).screen().contents_between(r0, c0, r1, c1);
        let text = text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        ctx.copy_text(text);
        self.selection = None;
        true
    }
}

/// Paints the backgrounds of `row`'s cells and returns its text, coloured.
fn paint_row(
    painter: &Painter,
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    grid: Grid,
    palette: &Palette,
    font: &FontId,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    // Cells look alike in runs: one section per run, not per cell (a full screen is
    // thousands of cells, laid out again whenever the output moves).
    let mut run = String::new();
    let mut run_format: Option<TextFormat> = None;
    let mut col = 0;
    while col < cols {
        let Some(c) = screen.cell(row, col) else {
            break;
        };
        let (fg, bg) = palette.colors(c);
        if let Some(bg) = bg {
            let w = if c.is_wide() { 2.0 } else { 1.0 };
            painter.rect_filled(grid.cells(row, col, w), 0.0, bg);
        }
        if !c.is_wide_continuation() {
            let underline = if c.underline() {
                Stroke::new(1.0, fg)
            } else {
                Stroke::NONE
            };
            let same = run_format.as_ref().is_some_and(|f| {
                f.color == fg && f.underline == underline && f.italics == c.italic()
            });
            if !same {
                if let Some(format) = run_format.take() {
                    job.append(&run, 0.0, format);
                    run.clear();
                }
                run_format = Some(TextFormat {
                    font_id: font.clone(),
                    color: fg,
                    underline,
                    italics: c.italic(),
                    ..Default::default()
                });
            }
            run.push_str(if c.has_contents() { c.contents() } else { " " });
        }
        col += 1;
    }
    if let Some(format) = run_format {
        job.append(&run, 0.0, format);
    }
    job
}
