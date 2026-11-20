//! Syntax colouring for diffs (syntect with bat's syntaxes, pure-Rust regex engine), chosen by
//! file extension or name, in the colours picked in the settings (see `crate::code_theme`).

use std::sync::OnceLock;

use egui::Color32;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::theme::{ThemeKind, Tokens};

/// Coloured pieces of one line.
pub type Spans = Vec<(Color32, String)>;

/// Bigger diffs stay uncoloured: highlighting is the slow part of opening them.
const MAX_LINES: usize = 8000;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// The syntax for `path`, by extension or by the whole name for files like `Dockerfile` and
/// `Makefile`; `None` for plain text.
fn syntax_for(path: &str) -> Option<&'static SyntaxReference> {
    let ss = syntaxes();
    let file = std::path::Path::new(path);
    let name = file.file_name()?.to_string_lossy();
    file.extension()
        .and_then(|e| ss.find_syntax_by_extension(&e.to_string_lossy().to_lowercase()))
        .or_else(|| ss.find_syntax_by_extension(&name))
        .filter(|s| s.name != "Plain Text")
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
    let syntax = syntax_for(path)?;
    let theme = crate::code_theme::current_theme(dark);
    // gitcraft's styles leave plain text uncoloured, for the text colour of the theme.
    let plain = Tokens::for_kind(if dark {
        ThemeKind::Dark
    } else {
        ThemeKind::Light
    })
    .text;
    colour(syntax, &theme, lines, plain)
}

/// Colours `lines` of `path` with `theme`, plain text in `plain`: the settings' theme previews.
pub fn highlight_with<'a>(
    path: &str,
    theme: &Theme,
    lines: impl Iterator<Item = &'a str>,
    plain: Color32,
) -> Option<Vec<Spans>> {
    colour(syntax_for(path)?, theme, lines, plain)
}

fn colour<'a>(
    syntax: &SyntaxReference,
    theme: &Theme,
    lines: impl Iterator<Item = &'a str>,
    plain: Color32,
) -> Option<Vec<Spans>> {
    let ss = syntaxes();
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
                    let color = if c.a == 0 {
                        plain
                    } else {
                        Color32::from_rgb(c.r, c.g, c.b)
                    };
                    (color, text.trim_end_matches('\n').to_owned())
                })
                .filter(|(_, t)| !t.is_empty())
                .collect(),
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
