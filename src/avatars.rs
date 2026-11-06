//! Author avatars, fetched in the background, cached on disk, painted round. A repository on
//! GitHub or GitLab tells us (through [`hint`]) where its authors' account pictures are; other
//! addresses go to Gravatar (or GitHub for its noreply ones). Initials stand in until (or unless)
//! one loads.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use egui::{Rect, Vec2, load::Bytes};

/// A missing avatar is asked for again after this long.
const RETRY_MISSING: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone)]
enum State {
    Pending,
    /// The picture, and its cache key (which says where it came from).
    Ready(Arc<[u8]>, String),
    Missing,
}

struct Avatars {
    map: Mutex<HashMap<String, State>>,
    queue: Mutex<Sender<String>>,
}

/// Where an address's picture is, when a forge knows better than Gravatar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    /// The account's avatar URL (GitHub).
    Url(String),
    /// Ask this GitLab host's avatar API.
    GitLab(String),
}

static AVATARS: OnceLock<Avatars> = OnceLock::new();

fn hints() -> &'static Mutex<HashMap<String, Hint>> {
    static HINTS: OnceLock<Mutex<HashMap<String, Hint>>> = OnceLock::new();
    HINTS.get_or_init(Default::default)
}

/// Records where `email`'s picture is; one already shown from elsewhere is fetched again.
pub fn hint(email: &str, hint: Hint) {
    let k = email.trim().to_lowercase();
    let mut h = lock(hints());
    if h.get(&k) == Some(&hint) {
        return;
    }
    h.insert(k.clone(), hint);
    drop(h);
    if let Some(av) = AVATARS.get() {
        lock(&av.map).remove(&k);
    }
}

fn cache_dir() -> Option<PathBuf> {
    eframe::storage_dir("gitcraft").map(|d| d.join("avatars"))
}

fn avatars(ctx: &egui::Context) -> &'static Avatars {
    AVATARS.get_or_init(|| {
        let (tx, rx) = channel::<String>();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let agent = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(8))
                .user_agent("gitcraft")
                .build();
            while let Ok(email) = rx.recv() {
                let hint = lock(hints()).get(&email).cloned();
                let state = fetch(&agent, &email, hint.as_ref());
                if let Some(map) = AVATARS.get() {
                    // A hint that came in meanwhile asked for this address afresh: what was
                    // fetched without it mustn't stand in for that answer.
                    let mut map = lock(&map.map);
                    if is_current(&lock(hints()), &email, hint.as_ref()) {
                        map.insert(email, state);
                    }
                }
                ctx.request_repaint();
            }
        });
        Avatars {
            map: Mutex::new(HashMap::new()),
            queue: Mutex::new(tx),
        }
    })
}

/// Whether `used` is still what `hints` say about `email`.
fn is_current(hints: &HashMap<String, Hint>, email: &str, used: Option<&Hint>) -> bool {
    hints.get(email) == used
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn key(email: &str) -> String {
    format!("{:x}", md5::compute(email.trim().to_lowercase()))
}

/// Where to ask: GitHub knows its noreply addresses, Gravatar the rest.
fn url(email: &str) -> String {
    let email = email.trim().to_lowercase();
    // The address comes from a commit, so whatever it says goes into the URL only when it is
    // what GitHub would have put there: an account's number, or its name.
    if let Some(local) = email.strip_suffix("@users.noreply.github.com") {
        match local.split_once('+') {
            Some((id, _)) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => {
                return format!("https://avatars.githubusercontent.com/u/{id}?s=80");
            }
            None if is_login(local) => return format!("https://github.com/{local}.png?size=80"),
            _ => {}
        }
    }
    format!("https://www.gravatar.com/avatar/{}?s=80&d=404", key(&email))
}

/// Whether `name` can be a GitHub account's name: letters, digits and single dashes inside.
fn is_login(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 39
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Cache file name: Gravatar's hash, plus where the picture came from when a forge said.
fn cache_key(email: &str, hint: Option<&Hint>) -> String {
    match hint {
        None => key(email),
        Some(Hint::Url(_)) => format!("{}-gh", key(email)),
        Some(Hint::GitLab(host)) => format!("{}-gl-{}", key(email), key(host)),
    }
}

/// The picture's URL: a forge's, else [`url`]. False with it when the forge couldn't be asked
/// which picture it has: Gravatar's then stands in for this session, but isn't written down
/// as that forge's answer.
fn source(agent: &ureq::Agent, email: &str, hint: Option<&Hint>) -> (String, bool) {
    match hint {
        Some(Hint::Url(u)) => (
            format!("{u}{}s=80", if u.contains('?') { '&' } else { '?' }),
            true,
        ),
        Some(Hint::GitLab(host)) => {
            let api = format!(
                "https://{host}/api/v4/avatar?email={}&size=80",
                query_escape(email)
            );
            let found = match agent.get(&api).call() {
                // An answer cut short or not JSON says nothing either.
                Ok(resp) => match resp
                    .into_string()
                    .ok()
                    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                {
                    Some(v) => v
                        .get("avatar_url")
                        .and_then(|u| u.as_str())
                        .map(str::to_owned),
                    None => return (url(email), false),
                },
                Err(e) if transient(&e) => return (url(email), false),
                Err(_) => None,
            };
            let picture = match found {
                // GitLab falls back to a generated Gravatar; initials read better.
                Some(u) if !u.contains("gravatar.com") => u,
                _ => url(email),
            };
            (picture, true)
        }
        None => (url(email), true),
    }
}

/// Whether a failed request may well work later: no answer, or a server busy or failing.
fn transient(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::Status(code, _) => *code == 408 || *code == 429 || *code >= 500,
        ureq::Error::Transport(_) => true,
    }
}

/// The largest picture taken; a bigger one isn't an avatar.
const MAX_BYTES: u64 = 512 * 1024;

/// What asking for a picture came to.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Picture(Vec<u8>),
    /// There is none (or none worth showing): asked again after [`RETRY_MISSING`].
    None,
    /// No answer to go by (offline, a server failing, a login page instead of a picture):
    /// asked again next time the app runs.
    Unreachable,
}

/// Whether `bytes` are a picture the app can show.
fn is_picture(bytes: &[u8]) -> bool {
    use image::ImageFormat as F;
    matches!(
        image::guess_format(bytes),
        Ok(F::Png | F::Jpeg | F::Gif | F::Bmp | F::WebP)
    )
}

/// The answer a server's `body` makes, at most [`MAX_BYTES`] of which were read.
fn answer(body: std::io::Result<Vec<u8>>) -> Answer {
    match body {
        Ok(bytes) if bytes.is_empty() || bytes.len() as u64 > MAX_BYTES => Answer::None,
        Ok(bytes) if is_picture(&bytes) => Answer::Picture(bytes),
        Ok(_) | Err(_) => Answer::Unreachable,
    }
}

fn download(agent: &ureq::Agent, url: &str) -> Answer {
    match agent.get(url).call() {
        Ok(resp) => {
            use std::io::Read;
            let mut bytes = Vec::new();
            // One byte more than taken tells a picture too big from one that just fits.
            let body = resp
                .into_reader()
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            answer(body)
        }
        Err(e) if transient(&e) => Answer::Unreachable,
        Err(_) => Answer::None,
    }
}

fn query_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b'@' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn fetch(agent: &ureq::Agent, email: &str, hint: Option<&Hint>) -> State {
    let dir = cache_dir();
    let k = cache_key(email, hint);
    if let Some(dir) = &dir {
        if let Ok(bytes) = std::fs::read(dir.join(format!("{k}.img")))
            && is_picture(&bytes)
        {
            return State::Ready(Arc::from(bytes.into_boxed_slice()), k);
        }
        let missing = dir.join(format!("{k}.none"));
        let recent = std::fs::metadata(&missing)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < RETRY_MISSING);
        if recent {
            return State::Missing;
        }
    }
    let (url, keep) = source(agent, email, hint);
    let dir = dir.filter(|_| keep);
    match download(agent, &url) {
        Answer::Picture(bytes) => {
            if let Some(dir) = &dir {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join(format!("{k}.img")), &bytes);
            }
            State::Ready(Arc::from(bytes.into_boxed_slice()), k)
        }
        Answer::None => {
            if let Some(dir) = &dir {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join(format!("{k}.none")), b"");
            }
            State::Missing
        }
        // Not written down: being offline once mustn't hide everyone's picture for a week.
        Answer::Unreachable => State::Missing,
    }
}

/// Paints `email`'s avatar into `rect` if it is loaded; returns false (and queues a download)
/// otherwise, so the caller keeps its initials.
pub fn paint(ui: &egui::Ui, rect: Rect, email: &str) -> bool {
    if email.is_empty() || !crate::settings::Settings::get(ui.ctx()).avatars {
        return false;
    }
    let av = avatars(ui.ctx());
    let k = email.trim().to_lowercase();
    let state = {
        let mut map = lock(&av.map);
        match map.get(&k) {
            Some(s) => s.clone(),
            None => {
                map.insert(k.clone(), State::Pending);
                let _ = lock(&av.queue).send(k.clone());
                State::Pending
            }
        }
    };
    let State::Ready(bytes, source) = state else {
        return false;
    };
    // egui keeps an image per URI: one per source, so a forge's picture replaces Gravatar's.
    let uri = format!("bytes://avatar/{source}.png");
    egui::Image::from_bytes(uri, Bytes::Shared(bytes))
        .fit_to_exact_size(Vec2::splat(rect.width()))
        .corner_radius(rect.width() / 2.0)
        .show_loading_spinner(false)
        .paint_at(ui, rect);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hinted_sources() {
        let agent = ureq::AgentBuilder::new().build();
        assert_eq!(
            source(
                &agent,
                "a@b.c",
                Some(&Hint::Url("https://av/u/1?v=4".into()))
            ),
            ("https://av/u/1?v=4&s=80".to_owned(), true)
        );
        assert_eq!(query_escape("me+git@x.org"), "me%2Bgit@x.org");
        assert_ne!(
            cache_key("a@b.c", None),
            cache_key("a@b.c", Some(&Hint::Url(String::new())))
        );
    }

    #[test]
    fn a_picture_fetched_before_a_hint_came_is_not_kept_as_its_answer() {
        let mut hints = HashMap::new();
        assert!(is_current(&hints, "a@b.c", None));
        let hint = Hint::Url("https://av/u/1".into());
        hints.insert("a@b.c".to_owned(), hint.clone());
        // Fetched from Gravatar while GitHub's answer was on its way.
        assert!(!is_current(&hints, "a@b.c", None));
        assert!(is_current(&hints, "a@b.c", Some(&hint)));
    }

    #[test]
    fn only_a_whole_picture_is_kept() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        assert_eq!(answer(Ok(png.clone())), Answer::Picture(png));
        // A login page or a proxy's notice in place of the picture is no answer to remember.
        assert_eq!(
            answer(Ok(b"<html>sign in</html>".to_vec())),
            Answer::Unreachable
        );
        assert_eq!(
            answer(Err(std::io::ErrorKind::ConnectionReset.into())),
            Answer::Unreachable
        );
        // Too big to be an avatar: not cut short and kept as if it were whole.
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_BYTES as usize + 1, 0);
        assert_eq!(answer(Ok(big)), Answer::None);
        assert_eq!(answer(Ok(Vec::new())), Answer::None);
    }

    #[test]
    fn only_a_missing_picture_is_remembered_as_missing() {
        let failed = |code: u16| {
            let resp = ureq::Response::new(code, "", "").ok()?;
            Some(transient(&ureq::Error::Status(code, resp)))
        };
        assert_eq!(failed(404), Some(false));
        assert_eq!(failed(410), Some(false));
        assert_eq!(failed(429), Some(true));
        assert_eq!(failed(408), Some(true));
        assert_eq!(failed(503), Some(true));
    }

    #[test]
    fn a_commit_address_only_names_a_github_account_it_can_be() {
        // Anything else goes to Gravatar, rather than into a path on GitHub.
        for email in [
            "../../x@users.noreply.github.com",
            "a/b@users.noreply.github.com",
            "12a/../3+x@users.noreply.github.com",
            "+x@users.noreply.github.com",
            "-x@users.noreply.github.com",
            "x?y=1@users.noreply.github.com",
        ] {
            assert!(
                url(email).starts_with("https://www.gravatar.com/"),
                "{email}"
            );
        }
        assert_eq!(
            url("my-name@users.noreply.github.com"),
            "https://github.com/my-name.png?size=80"
        );
    }

    #[test]
    fn avatar_urls() {
        assert_eq!(
            url("12345+octo@users.noreply.github.com"),
            "https://avatars.githubusercontent.com/u/12345?s=80"
        );
        assert_eq!(
            url("octo@users.noreply.github.com"),
            "https://github.com/octo.png?size=80"
        );
        // Gravatar hashes the trimmed, lowercased address.
        assert_eq!(
            url(" MyEmailAddress@example.com "),
            "https://www.gravatar.com/avatar/0bc83cb571cd1c50ba6f3e8a78ef1346?s=80&d=404"
        );
    }
}
