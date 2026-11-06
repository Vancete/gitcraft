//! An integrated terminal: the user's shell in a pseudo-terminal (ConPTY on Windows), its output
//! interpreted by `vt100` and painted with egui.

mod keys;
mod palette;
mod shells;
#[cfg(test)]
mod tests;
mod view;

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub use shells::{ShellSpec, available_shells, default_shell};

const SCROLLBACK: usize = 5000;
/// How often output in a terminal out of sight redraws the app.
const HIDDEN_REPAINT: std::time::Duration = std::time::Duration::from_millis(250);

/// What the terminal answers by itself: ConPTY (and some programs) ask where the cursor is and
/// wait for the answer before printing anything (DSR 6 → CPR). Answered while the query is
/// parsed, so a query split across two reads is still seen, and the cursor is where it was at
/// the query, not after the rest of the output read with it.
struct Replies {
    writer: Sender<Vec<u8>>,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if c == 'n' && i1.is_none() && i2.is_none() && params == [&[6u16][..]] {
            let (row, col) = screen.cursor_position();
            let reply = format!("\x1b[{};{}R", row + 1, col + 1);
            let _ = self.writer.send(reply.into_bytes());
        }
    }
}

pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser<Replies>>>,
    /// Bytes for the shell, written by a thread of their own: a write blocks once the pty's
    /// input buffer is full (a big paste while a program isn't reading), which must not freeze
    /// the app. Shared with the parser, which answers the shell's cursor-position queries.
    writer: Sender<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    /// The pass in which the terminal was last drawn ([`egui::Context::cumulative_pass_nr`]):
    /// output a terminal out of sight brings repaints the app only now and then.
    drawn: Arc<AtomicU64>,
    /// (rows, cols) the pty was last sized to.
    size: (u16, u16),
    /// Lines scrolled back into history (0 = following the output).
    scroll: usize,
    /// Fractional wheel scrolling not yet turned into lines.
    scroll_rest: f32,
    /// Selected cells, (row, col) on screen, from where the drag started to where it is.
    selection: Option<((u16, u16), (u16, u16))>,
    pub shell: String,
}

impl Terminal {
    /// Starts `shell` in `cwd`.
    pub fn spawn(cwd: &Path, shell: &ShellSpec, ctx: &egui::Context) -> Result<Self, String> {
        let (rows, cols) = (24, 100);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("could not open a terminal: {e}"))?;
        let program = shell.program.clone();
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(&shell.args);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // The user's own PATH, relative folders and all: not the one for gitcraft's own tools.
        if let Some(path) = crate::shell_path::terminal_path() {
            cmd.env("PATH", path);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("could not start {program}: {e}"))?;
        // The child holds its own handle; ours would keep the pty open after it exits.
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("terminal: {e}"))?;
        let mut pty_writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("terminal: {e}"))?;
        let (writer, input) = mpsc::channel::<Vec<u8>>();
        // Ends once every sender is gone (the terminal and its parser) or the pty is.
        std::thread::spawn(move || {
            for bytes in input {
                if pty_writer
                    .write_all(&bytes)
                    .and_then(|()| pty_writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });

        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            nz(rows),
            nz(cols),
            SCROLLBACK,
            Replies {
                writer: writer.clone(),
            },
        )));
        let exited = Arc::new(AtomicBool::new(false));
        let drawn = Arc::new(AtomicU64::new(0));
        {
            let parser = Arc::clone(&parser);
            let exited = Arc::clone(&exited);
            let drawn = Arc::clone(&drawn);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let bytes = buf.get(..n).unwrap_or_default();
                            // Cursor-position queries are answered as they are parsed.
                            lock(&parser).process(bytes);
                            // A busy program (a build, a log) prints all the time: the whole
                            // app redraws for it only while the terminal is in sight.
                            if ctx.cumulative_pass_nr() <= drawn.load(Ordering::Relaxed) + 2 {
                                ctx.request_repaint();
                            } else {
                                ctx.request_repaint_after(HIDDEN_REPAINT);
                            }
                        }
                    }
                }
                exited.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }

        let shell = shell.label.clone();
        Ok(Self {
            parser,
            writer,
            master: pair.master,
            child,
            exited,
            drawn,
            size: (rows, cols),
            scroll: 0,
            scroll_rest: 0.0,
            selection: None,
            shell,
        })
    }

    pub fn has_exited(&mut self) -> bool {
        if !self.exited.load(Ordering::Relaxed)
            && let Ok(Some(_)) = self.child.try_wait()
        {
            // ConPTY doesn't always end the read when the shell exits.
            self.exited.store(true, Ordering::Relaxed);
        }
        self.exited.load(Ordering::Relaxed)
    }

    fn send(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.writer.send(bytes.to_vec());
        self.selection = None;
        // Typing follows the output again.
        if self.scroll != 0 {
            self.scroll = 0;
            lock(&self.parser).screen_mut().set_scrollback(0);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Only a shell still running: once reaped, its pid may already be another process's.
        if let Ok(None) = self.child.try_wait()
            && self.child.kill().is_ok()
        {
            // Reap it, or it stays a zombie for as long as the app runs.
            let _ = self.child.wait();
        }
    }
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN)
}

/// A poisoned lock still holds usable data.
fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
