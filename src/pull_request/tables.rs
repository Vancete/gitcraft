//! Tables of images in a pull request's text, taken out so the app can lay them out itself.

/// A part of a pull request's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Markdown(String),
    /// A table with images, which the app lays out itself so that they share the width.
    ImageTable {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

/// `markdown` with its tables of images taken out as blocks of their own. What is in a code block
/// isn't a table, whatever it looks like.
pub fn split_image_tables(markdown: &str) -> Vec<Block> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut fenced = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && is_table_row(line) && lines.get(i + 1).is_some_and(|l| is_delimiter(l)) {
            let end = (i + 2..lines.len())
                .find(|&k| !is_table_row(lines[k]))
                .unwrap_or(lines.len());
            let rows = &lines[i + 2..end];
            if rows.iter().any(|row| row.contains("![")) {
                flush_text(&mut text, &mut blocks);
                blocks.push(Block::ImageTable {
                    header: table_cells(line),
                    rows: rows.iter().map(|row| table_cells(row)).collect(),
                });
                i = end;
                continue;
            }
        }
        text.push(line);
        i += 1;
    }
    flush_text(&mut text, &mut blocks);
    blocks
}

fn flush_text(text: &mut Vec<&str>, blocks: &mut Vec<Block>) {
    if text.iter().any(|line| !line.trim().is_empty()) {
        blocks.push(Block::Markdown(text.join("\n")));
    }
    text.clear();
}

fn is_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn is_delimiter(line: &str) -> bool {
    is_table_row(line)
        && line.contains('-')
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn table_cells(row: &str) -> Vec<String> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    row.split('|').map(|cell| cell.trim().to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(markdown: &str) -> Block {
        Block::Markdown(markdown.to_owned())
    }

    #[test]
    fn text_without_tables_stays_one_block() {
        assert_eq!(split_image_tables("a\n\nb"), [text("a\n\nb")]);
    }

    #[test]
    fn no_text_makes_no_blocks() {
        assert_eq!(split_image_tables(""), []);
    }

    #[test]
    fn a_table_of_images_becomes_a_block_between_the_text_around_it() {
        let markdown = "before\n\n| A | B |\n|---|:--:|\n| ![a](u) | ![b](v) |\n\nafter";
        assert_eq!(
            split_image_tables(markdown),
            [
                text("before\n"),
                Block::ImageTable {
                    header: vec!["A".to_owned(), "B".to_owned()],
                    rows: vec![vec!["![a](u)".to_owned(), "![b](v)".to_owned()]],
                },
                text("\nafter"),
            ]
        );
    }

    #[test]
    fn a_table_without_images_is_left_to_the_markdown() {
        let markdown = "| A |\n|---|\n| text |";
        assert_eq!(split_image_tables(markdown), [text(markdown)]);
    }

    #[test]
    fn a_table_at_the_very_start_and_end_has_no_empty_text_around_it() {
        let blocks = split_image_tables("| A |\n|---|\n| ![a](u) |");
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], Block::ImageTable { .. }));
    }

    #[test]
    fn windows_line_ends_make_no_difference() {
        let blocks = split_image_tables("| A |\r\n|---|\r\n| ![a](u) |\r\n");
        assert!(matches!(blocks[0], Block::ImageTable { .. }));
    }

    #[test]
    fn a_table_in_a_code_block_is_code() {
        let markdown = "```\n| A |\n|---|\n| ![a](u) |\n```";
        assert_eq!(split_image_tables(markdown), [text(markdown)]);
    }

    #[test]
    fn pipes_without_a_delimiter_row_are_not_a_table() {
        let markdown = "| ![a](u) |\n| ![b](v) |";
        assert_eq!(split_image_tables(markdown), [text(markdown)]);
    }
}
