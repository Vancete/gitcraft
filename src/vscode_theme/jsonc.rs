//! The JSON with comments VS Code writes its settings and themes in.

use std::path::Path;

use serde_json::Value;

pub(super) fn read_jsonc(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&strip_jsonc(text.trim_start_matches('\u{feff}'))).ok()
}

/// Removes the comments and trailing commas VS Code allows in its JSON files.
fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                out.push(' ');
            }
            (']' | '}', _) => {
                // Drop a comma left right before the closing bracket.
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.truncate(kept - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let src = r#"{
            // a comment
            "a": "http://x", /* block */ "b": [1, 2,],
            "c": "quote \" // not a comment",
        }"#;
        let v: Value = serde_json::from_str(&strip_jsonc(src)).unwrap_or_default();
        assert_eq!(v["a"], "http://x");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"], "quote \" // not a comment");
    }
}
