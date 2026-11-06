//! Syntax colouring for diffs (syntect with bat's syntaxes, pure-Rust regex engine), chosen by
//! file extension or name.

use std::sync::OnceLock;

use egui::Color32;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

/// Coloured pieces of one line.
pub type Spans = Vec<(Color32, String)>;

/// Bigger diffs stay uncoloured: highlighting is the slow part of opening them.
const MAX_LINES: usize = 8000;
/// Nor much text in few lines (a minified bundle): colouring takes about 2 s per MB.
const MAX_BYTES: usize = 512 * 1024;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn themes() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

/// Colours `lines` (one file's text, in order) for `path`; `None` for plain text, unknown
/// types or very long inputs.
pub fn highlight<'a>(
    path: &str,
    lines: impl ExactSizeIterator<Item = &'a str>,
    dark: bool,
) -> Option<Vec<Spans>> {
    if lines.len() > MAX_LINES {
        return None;
    }
    let lines: Vec<&str> = lines.collect();
    if lines.iter().map(|l| l.len()).sum::<usize>() > MAX_BYTES {
        return None;
    }
    let ss = syntaxes();
    let file = std::path::Path::new(path);
    let name = file.file_name()?.to_string_lossy();
    // By the whole name for files like `Dockerfile` and `CMakeLists.txt` (whose `.txt` alone is
    // plain text), or else by extension.
    let syntax = ss.find_syntax_by_extension(&name).or_else(|| {
        file.extension()
            .and_then(|e| ss.find_syntax_by_extension(&e.to_string_lossy()))
    })?;
    if syntax.name == "Plain Text" {
        return None;
    }
    // VS Code's theme when it suits the background: a dark theme on a light gitcraft reads badly.
    let editor = crate::vscode_theme::enabled()
        .then(crate::vscode_theme::current)
        .flatten()
        .filter(|e| e.dark == dark);
    let theme = match &editor {
        Some(e) => &e.theme,
        None => themes().themes.get(if dark {
            "base16-ocean.dark"
        } else {
            "InspiredGitHub"
        })?,
    };
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in lines {
        let with_nl = format!("{line}\n");
        let ranges = h.highlight_line(&with_nl, ss).ok()?;
        out.push(
            ranges
                .into_iter()
                .map(|(style, text)| {
                    let c = style.foreground;
                    (
                        Color32::from_rgb(c.r, c.g, c.b),
                        text.trim_end_matches('\n').to_owned(),
                    )
                })
                .filter(|(_, t)| !t.is_empty())
                .collect(),
        );
    }
    Some(out)
}

/// Characters of a line shown at most. Laying out and drawing a line costs as much as it is long,
/// every frame it is in sight, and a minified file can be a single line of megabytes.
pub(crate) const MAX_LINE_CHARS: usize = 5000;

/// Cuts a line shown (and its colours) to [`MAX_LINE_CHARS`], saying how much was left out;
/// returns the line as it was when it was cut. Only what is shown: patches are made from the
/// line as git printed it.
pub(crate) fn cap_line(text: &mut String, spans: Option<&mut Spans>) -> Option<String> {
    let (cut, _) = text.char_indices().nth(MAX_LINE_CHARS)?;
    let note = format!(" … {} more characters", text[cut..].chars().count());
    let whole = text.clone();
    text.truncate(cut);
    if let Some(spans) = spans {
        let mut left = MAX_LINE_CHARS;
        spans.retain_mut(|(_, s)| {
            if left == 0 {
                return false;
            }
            match s.char_indices().nth(left) {
                Some((i, _)) => {
                    s.truncate(i);
                    left = 0;
                }
                None => left -= s.chars().count(),
            }
            true
        });
        spans.push((Color32::GRAY, note.clone()));
    }
    text.push_str(&note);
    Some(whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_are_cut_with_their_colours() {
        let red = Color32::RED;
        let mut short = "abc".to_owned();
        assert_eq!(cap_line(&mut short, None), None);
        assert_eq!(short, "abc");

        let mut text = "é".repeat(MAX_LINE_CHARS + 7);
        let mut spans: Spans = vec![
            (red, "é".repeat(MAX_LINE_CHARS - 1)),
            (red, "é".repeat(5)),
            (red, "é".repeat(3)),
        ];
        let whole = text.clone();
        assert_eq!(cap_line(&mut text, Some(&mut spans)), Some(whole));
        assert_eq!(
            text,
            format!("{} … 7 more characters", "é".repeat(MAX_LINE_CHARS))
        );
        let joined: String = spans.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(joined, text);
        assert_eq!(spans.len(), 3);
    }

    #[test]
    fn colours_rust_and_skips_plain_text() {
        let lines = ["fn main() {", "    let x = 1;", "}"];
        let spans = highlight("a.rs", lines.iter().copied(), true);
        let spans = spans.unwrap_or_default();
        assert_eq!(spans.len(), 3);
        let text: String = spans[1].iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(text, "    let x = 1;");
        // More than one colour on a line of code.
        assert!(spans[0].len() > 1);
        assert!(highlight("notes.txt", lines.iter().copied(), true).is_none());
        assert!(highlight("Makefile-without-ext", lines.iter().copied(), true).is_none());
    }

    #[test]
    fn whole_names_win_over_extensions() {
        let lines = ["project(x)", "add_executable(x main.c)"];
        let spans = highlight("src/CMakeLists.txt", lines.iter().copied(), true);
        assert!(spans.is_some_and(|s| s.iter().any(|line| line.len() > 1)));
        assert!(highlight("MAIN.RS", ["fn main() {}"].into_iter(), true).is_some());
    }

    /// A minified file is a few very long lines: too slow to colour.
    #[test]
    fn very_long_text_stays_plain() {
        let line = "var a=1;".repeat(MAX_BYTES / 8 + 1);
        assert!(highlight("app.min.js", [line.as_str()].into_iter(), true).is_none());
        assert!(highlight("app.js", ["var a=1;"].into_iter(), true).is_some());
    }

    /// The languages of a typical web or .NET repository each get more than one colour.
    #[test]
    fn colours_common_languages() {
        let cases: &[(&str, &[&str])] = &[
            ("Program.cs", &["public class A { private int x = 1; }"]),
            ("app.js", &["const x = require('y');"]),
            ("app.ts", &["const x: number = 1;"]),
            ("App.tsx", &["export const A = (): JSX.Element => <div />;"]),
            (
                "Details.vue",
                &[
                    "<script setup lang=\"ts\">",
                    "import { ref } from 'vue'",
                    "</script>",
                ],
            ),
            ("Cargo.toml", &["[package]", "name = \"x\""]),
            ("Dockerfile", &["FROM alpine:3", "RUN echo hi"]),
            ("styles.scss", &[".a { color: red; }"]),
        ];
        let plain: Vec<&str> = cases
            .iter()
            .filter(|(path, lines)| {
                !highlight(path, lines.iter().copied(), true)
                    .is_some_and(|spans| spans.iter().any(|line| line.len() > 1))
            })
            .map(|(path, _)| *path)
            .collect();
        assert!(plain.is_empty(), "not coloured: {plain:?}");
    }
}
