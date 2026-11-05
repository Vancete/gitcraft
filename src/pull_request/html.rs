//! The HTML in a pull request's Markdown: comments left out, tags taken away, `<img>` as images.

/// `body` without the HTML tags people and bots put in Markdown, which the viewer would show as
/// text. What they enclose stays; code stays as it is.
pub(super) fn without_html_tags(body: &str) -> String {
    let mut fenced = false;
    let lines: Vec<String> = body
        .split('\n')
        .map(|line| {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                return line.to_owned();
            }
            if fenced {
                return line.to_owned();
            }
            // Every other piece between backticks is inline code.
            line.split('`')
                .enumerate()
                .map(|(i, piece)| {
                    if i % 2 == 0 {
                        strip_tags(piece)
                    } else {
                        piece.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("`")
        })
        .collect();
    lines.join("\n")
}

/// The HTML that shows up in pull requests. Anything else in angle brackets is left alone: it may
/// well be `Vec<String>`.
const HTML_TAGS: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "center",
    "code",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "kbd",
    "li",
    "ol",
    "p",
    "picture",
    "pre",
    "s",
    "small",
    "source",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
];

fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match tag_at(after) {
            Some((name, len)) => {
                if name.eq_ignore_ascii_case("br") {
                    out.push('\n');
                }
                rest = &after[len..];
            }
            None => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The name and length of the known tag `text` starts with. A `>` in an attribute's quotes doesn't
/// end the tag.
fn tag_at(text: &str) -> Option<(&str, usize)> {
    let inner = text.strip_prefix('<')?;
    let inner = inner.strip_prefix('/').unwrap_or(inner);
    let len = inner
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(inner.len());
    let name = &inner[..len];
    if !HTML_TAGS.iter().any(|tag| tag.eq_ignore_ascii_case(name)) {
        return None;
    }
    let mut quote = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (None, '>') => return Some((name, i + 1)),
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if open == c => quote = None,
            _ => {}
        }
    }
    None
}

pub(super) fn without_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

pub(super) fn img_tags_to_markdown(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<img") {
        let Some(len) = rest[start..].find('>') else {
            break;
        };
        out.push_str(&rest[..start]);
        let tag = &rest[start..start + len];
        if let Some(src) = attribute(tag, "src") {
            out.push_str(&format!(
                "![{}]({src})",
                attribute(tag, "alt").unwrap_or("")
            ));
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// The value of `name="…"` in an HTML tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

#[cfg(test)]
mod tests {
    use crate::pull_request::safe_markdown;

    #[test]
    fn img_tags_become_images_by_the_same_rule() {
        assert_eq!(
            safe_markdown(
                r#"<img width="400" alt="Shot" src="https://github.com/user-attachments/assets/a" />"#,
                "github.com"
            ),
            "![Shot](https://github.com/user-attachments/assets/a)"
        );
        assert_eq!(
            safe_markdown(r#"<img src="https://evil.example/x.png">"#, "github.com"),
            "[image](https://evil.example/x.png)"
        );
    }

    #[test]
    fn html_comments_are_left_out() {
        assert_eq!(
            safe_markdown("a<!-- template\nhint -->b", "github.com"),
            "ab"
        );
    }

    #[test]
    fn html_tags_go_and_what_they_enclose_stays() {
        let shown = safe_markdown(
            "<details>\n<summary><strong>Title</strong></summary>\n\nInner text\n</details>",
            "github.com",
        );
        assert!(!shown.contains('<'), "{shown}");
        assert!(shown.contains("Title"), "{shown}");
        assert!(shown.contains("Inner text"), "{shown}");
    }

    #[test]
    fn tags_with_attributes_go_too() {
        assert_eq!(
            safe_markdown(r#"<strong title="a > b">Auto</strong> fix"#, "github.com"),
            "Auto fix"
        );
        assert_eq!(
            safe_markdown(
                r#"<picture><source media="x" srcset="u"></picture>"#,
                "github.com"
            ),
            ""
        );
    }

    #[test]
    fn a_line_break_tag_breaks_the_line() {
        assert_eq!(safe_markdown("a<br/>b<br>c", "github.com"), "a\nb\nc");
    }

    #[test]
    fn what_only_looks_like_a_tag_stays() {
        for text in ["a < b and c > d", "Vec<String> is <unknown>", "x <- y"] {
            assert_eq!(safe_markdown(text, "github.com"), text);
        }
    }

    #[test]
    fn tags_in_code_stay() {
        let inline = "Use `<details>` here";
        assert_eq!(safe_markdown(inline, "github.com"), inline);
        let block = "```html\n<details>x</details>\n```";
        assert_eq!(safe_markdown(block, "github.com"), block);
    }
}
