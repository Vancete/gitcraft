//! Suggested changes in review comments: finding them, and applying them to a file's text.

/// A suggested change to apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub path: String,
    pub start: u64,
    pub end: u64,
    pub text: String,
    pub author: String,
}

/// The suggestion to offer, only if what would be committed is what the comment shows. Text can
/// hide in a comment's source, for one in an HTML comment.
pub fn shown_suggestion(body: &str, host: &str) -> Option<String> {
    let raw = parse_suggestion(body)?;
    let shown = parse_suggestion(&crate::pull_request::safe_markdown(body, host))?;
    (raw == shown).then_some(raw)
}

/// The first suggested change in a comment's text, each line ended, so that no text at all (remove
/// the line) differs from one blank line.
pub fn parse_suggestion(body: &str) -> Option<String> {
    let mut lines = body.lines();
    // A suggestion's fence inside another code block is only text, say one showing how to write
    // a suggestion: GitHub shows it as code, and has nothing to apply.
    let mut fence: Option<(char, usize)> = None;
    lines.find(|line| match fence {
        Some((c, open)) => {
            if closes_fence(line, c, open) {
                fence = None;
            }
            false
        }
        None if line.trim_end() == "```suggestion" => true,
        None => {
            fence = opens_fence(line);
            false
        }
    })?;
    let mut suggestion = String::new();
    for line in lines {
        if line.trim_end() == "```" {
            return Some(suggestion);
        }
        suggestion.push_str(line);
        suggestion.push('\n');
    }
    None
}

/// The character and length of the code fence `line` opens, if it opens one.
fn opens_fence(line: &str) -> Option<(char, usize)> {
    let line = line.trim_start();
    let c = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = line.chars().take_while(|&x| x == c).count();
    (len >= 3).then_some((c, len))
}

/// Whether `line` closes a fence of `open` times `c`: as many of them or more, and nothing else.
fn closes_fence(line: &str, c: char, open: usize) -> bool {
    let line = line.trim();
    line.chars().all(|x| x == c) && line.chars().count() >= open
}

/// `content` with lines `start` to `end` (counted from 1) replaced by `replacement`. Line ends stay
/// what the file uses, and a last line without one stays without.
pub(crate) fn apply_lines(
    content: &str,
    start: u64,
    end: u64,
    replacement: &str,
) -> Result<String, String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if start == 0 || start > end || end as usize > lines.len() {
        return Err(format!(
            "lines {start} to {end} aren't in the file, which has {}",
            lines.len()
        ));
    }
    let (from, to) = (start as usize - 1, end as usize);
    // The line end of the lines replaced (or of the line before, for a last line without one):
    // a file with a stray CRLF elsewhere keeps LF here.
    let ended =
        (lines[from..to].iter().chain(lines[..from].iter().rev())).find(|l| l.ends_with('\n'));
    let line_end = match ended {
        Some(line) if line.ends_with("\r\n") => "\r\n",
        _ => "\n",
    };
    let keeps_no_end = to == lines.len() && !lines[to - 1].ends_with('\n');
    let mut out = lines[..from].concat();
    if !replacement.is_empty() {
        out.push_str(&replacement.lines().collect::<Vec<_>>().join(line_end));
        if !keeps_no_end {
            out.push_str(line_end);
        }
    }
    out.push_str(&lines[to..].concat());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_suggested_change() {
        let body = "Clearer:\n\n```suggestion\nlet a = 1;\nlet b = 2;\n```\nThanks";
        assert_eq!(
            parse_suggestion(body).as_deref(),
            Some("let a = 1;\nlet b = 2;\n")
        );
    }

    #[test]
    fn a_blank_line_is_not_the_same_as_no_line() {
        let blank = parse_suggestion("```suggestion\n\n```").unwrap_or_default();
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, &blank).as_deref(),
            Ok("a\n\nc\n")
        );
        let gone = parse_suggestion("```suggestion\n```").unwrap_or_default();
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, &gone).as_deref(),
            Ok("a\nc\n")
        );
        let ends_blank = parse_suggestion("```suggestion\nB\n\n```").unwrap_or_default();
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, &ends_blank).as_deref(),
            Ok("a\nB\n\nc\n")
        );
    }

    #[test]
    fn a_suggestion_shown_as_code_is_no_suggestion() {
        let how = "Write it like this:\n\n````\n```suggestion\nnew();\n```\n````";
        assert_eq!(parse_suggestion(how), None);
        let tilde = "~~~\n```suggestion\nnew();\n```\n~~~";
        assert_eq!(parse_suggestion(tilde), None);
        // One after a code block still counts.
        let after = "```rust\nold();\n```\n```suggestion\nnew();\n```";
        assert_eq!(parse_suggestion(after).as_deref(), Some("new();\n"));
    }

    #[test]
    fn spaces_at_the_end_of_a_suggested_line_stay() {
        assert_eq!(
            parse_suggestion("```suggestion\nx  \n```  ").as_deref(),
            Some("x  \n")
        );
    }

    #[test]
    fn a_suggestion_hidden_from_the_reader_is_not_offered() {
        let hidden = "Looks fine\n<!--\n```suggestion\nevil();\n```\n-->";
        assert_eq!(shown_suggestion(hidden, "github.com"), None);
        let decoy = "<!--\n```suggestion\nevil();\n```\n-->\n```suggestion\ngood();\n```";
        assert_eq!(shown_suggestion(decoy, "github.com"), None);
        let inside = "```suggestion\ngood(); <!-- evil(); -->\n```";
        assert_eq!(shown_suggestion(inside, "github.com"), None);
    }

    #[test]
    fn a_suggestion_in_plain_view_is_offered() {
        let body = "Clearer:\n\n```suggestion\nlet a = 1;\n```";
        assert_eq!(
            shown_suggestion(body, "github.com").as_deref(),
            Some("let a = 1;\n")
        );
    }

    #[test]
    fn a_suggestion_can_be_to_remove_the_line() {
        assert_eq!(parse_suggestion("```suggestion\n```").as_deref(), Some(""));
    }

    #[test]
    fn other_code_blocks_are_no_suggestion() {
        assert_eq!(parse_suggestion("```rust\nlet a = 1;\n```"), None);
        assert_eq!(parse_suggestion("no code"), None);
        assert_eq!(parse_suggestion("```suggestion\nnever closed"), None);
    }

    #[test]
    fn windows_line_ends_in_a_comment_make_no_difference() {
        assert_eq!(
            parse_suggestion("```suggestion\r\nx\r\n```\r\n").as_deref(),
            Some("x\n")
        );
    }

    #[test]
    fn replaces_a_line() {
        assert_eq!(
            apply_lines("a\nb\nc\n", 2, 2, "B").as_deref(),
            Ok("a\nB\nc\n")
        );
    }

    #[test]
    fn replaces_several_lines_with_several() {
        assert_eq!(
            apply_lines("a\nb\nc\nd\n", 2, 3, "x\ny\nz").as_deref(),
            Ok("a\nx\ny\nz\nd\n")
        );
    }

    #[test]
    fn an_empty_suggestion_removes_the_line() {
        assert_eq!(apply_lines("a\nb\nc\n", 2, 2, "").as_deref(), Ok("a\nc\n"));
    }

    #[test]
    fn keeps_the_line_ends_of_the_file() {
        assert_eq!(
            apply_lines("a\r\nb\r\nc\r\n", 2, 2, "x\ny").as_deref(),
            Ok("a\r\nx\r\ny\r\nc\r\n")
        );
    }

    #[test]
    fn keeps_the_line_ends_of_the_lines_replaced() {
        assert_eq!(
            apply_lines("a\r\nb\nc\n", 2, 2, "x\ny").as_deref(),
            Ok("a\r\nx\ny\nc\n")
        );
        assert_eq!(
            apply_lines("a\nb\r\nc", 3, 3, "x\ny").as_deref(),
            Ok("a\nb\r\nx\r\ny")
        );
    }

    #[test]
    fn keeps_a_last_line_without_a_line_end() {
        assert_eq!(apply_lines("a\nb", 2, 2, "B").as_deref(), Ok("a\nB"));
    }

    #[test]
    fn lines_that_are_not_there_are_an_error() {
        assert!(apply_lines("a\n", 3, 3, "x").is_err());
        assert!(apply_lines("a\n", 0, 1, "x").is_err());
        assert!(apply_lines("a\nb\n", 2, 1, "x").is_err());
    }
}
