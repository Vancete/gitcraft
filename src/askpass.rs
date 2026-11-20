//! Questions ssh asks while git runs: a password, a key's passphrase, whether to trust a host
//! seen for the first time. gitcraft runs git with no terminal, so ssh can't ask there; it asks
//! the program in `SSH_ASKPASS` instead, which is gitcraft itself started again as a helper.
//!
//! The helper passes the question to the running window over a localhost port (with a random
//! token, so nothing else can ask), the window shows it in a dialog, and the answer goes back
//! the same way for the helper to print to ssh. Windows' git brings its own dialogs, so this is
//! for macOS and Linux.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Tells the helper where the window listens: `port token`.
const ENV: &str = "GITCRAFT_ASKPASS";
/// How long a question waits for its answer before ssh is told no.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(600);

/// A question from ssh, waiting for the user.
pub struct Request {
    pub prompt: String,
    reply: Sender<Option<String>>,
}

impl Request {
    /// A yes / no question (trusting a new host) rather than a secret to type.
    pub fn is_confirmation(&self) -> bool {
        self.prompt.contains("(yes/no")
    }

    /// Sends the answer; `None` cancels, and ssh gives up.
    pub fn answer(self, answer: Option<String>) {
        let _ = self.reply.send(answer);
    }
}

struct Server {
    port: u16,
    token: String,
}

static CTX: OnceLock<egui::Context> = OnceLock::new();
static SERVER: OnceLock<Option<Server>> = OnceLock::new();
static QUEUE: Mutex<Vec<Request>> = Mutex::new(Vec::new());

/// The window to wake when a question arrives.
pub fn init(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// The next question waiting, if any.
pub fn take() -> Option<Request> {
    let mut queue = QUEUE.lock().ok()?;
    (!queue.is_empty()).then(|| queue.remove(0))
}

/// Lets ssh in `cmd` ask its questions in the window (macOS and Linux).
pub fn apply(cmd: &mut Command) {
    if cfg!(windows) {
        return;
    }
    let (Some(server), Ok(exe)) = (server(), std::env::current_exe()) else {
        return;
    };
    cmd.env("SSH_ASKPASS", exe);
    // Ask even when gitcraft was started from a terminal (OpenSSH 8.4 and later)…
    cmd.env("SSH_ASKPASS_REQUIRE", "force");
    // …and older ones only ask with a display set.
    if std::env::var_os("DISPLAY").is_none() {
        cmd.env("DISPLAY", ":0");
    }
    cmd.env(ENV, format!("{} {}", server.port, server.token));
}

/// Starts listening the first time it's needed.
fn server() -> Option<&'static Server> {
    SERVER.get_or_init(start).as_ref()
}

fn start() -> Option<Server> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = format!(
        "{:016x}{:016x}",
        RandomState::new().build_hasher().finish(),
        RandomState::new().build_hasher().finish()
    );
    let expected = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let token = expected.clone();
            // Each question waits for its answer on a thread of its own.
            std::thread::spawn(move || serve(stream, &token));
        }
    });
    Some(Server { port, token })
}

/// One helper's question: the token on the first line, then the prompt; back goes `ok` and
/// the answer, or `cancel`.
fn serve(stream: TcpStream, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(&stream);
    let mut sent = String::new();
    if reader.read_line(&mut sent).is_err() || sent.trim_end() != token {
        return;
    }
    let mut prompt = String::new();
    if reader.read_to_string(&mut prompt).is_err() {
        return;
    }
    let (reply, answer) = channel();
    let Ok(mut queue) = QUEUE.lock() else {
        return;
    };
    queue.push(Request {
        prompt: prompt.trim().to_owned(),
        reply,
    });
    drop(queue);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
    let out = match answer.recv_timeout(ANSWER_TIMEOUT) {
        Ok(Some(answer)) => format!("ok\n{answer}"),
        _ => "cancel\n".to_owned(),
    };
    let _ = (&stream).write_all(out.as_bytes());
}

/// When this process was started by ssh as `SSH_ASKPASS`: asks the window and returns the
/// exit code (0 with the answer printed, 1 when cancelled). `None` for a normal start.
pub fn run_if_asked() -> Option<i32> {
    let info = std::env::var(ENV).ok()?;
    let prompt = std::env::args().nth(1).unwrap_or_default();
    // git's own username / password prompts for HTTPS keep failing as before: gitcraft asks
    // for those itself and remembers them for the tab.
    if prompt.starts_with("Username for '") || prompt.starts_with("Password for '") {
        return Some(1);
    }
    Some(match ask(&info, &prompt) {
        Some(answer) => {
            println!("{answer}");
            0
        }
        None => 1,
    })
}

/// Asks the window `prompt` as ssh would, for `examples/ui_shot.rs`.
#[doc(hidden)]
pub fn ask_window(prompt: &str) -> Option<String> {
    let server = server()?;
    ask(&format!("{} {}", server.port, server.token), prompt)
}

fn ask(info: &str, prompt: &str) -> Option<String> {
    let (port, token) = info.trim().split_once(' ')?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port.parse::<u16>().ok()?));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    write!(stream, "{token}\n{prompt}").ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    let answer = reply.strip_prefix("ok\n")?;
    Some(answer.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_reaches_the_window_and_its_answer_comes_back() {
        let Some(server) = server() else {
            return;
        };
        let info = format!("{} {}", server.port, server.token);
        let asker = std::thread::spawn(move || ask(&info, "git@nas.local's password: "));
        let request = loop {
            if let Some(r) = take() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(request.prompt, "git@nas.local's password:");
        assert!(!request.is_confirmation());
        request.answer(Some("hunter2".to_owned()));
        assert_eq!(asker.join().ok().flatten().as_deref(), Some("hunter2"));
        // A wrong token is never asked.
        let bad = format!("{} nope", server.port);
        assert_eq!(ask(&bad, "x"), None);
    }
}
