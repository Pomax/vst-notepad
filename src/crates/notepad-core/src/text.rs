//! Byte-offset text utilities.
//!
//! The document keeps markdown source in a `String` and addresses positions as
//! byte offsets. Every helper here is UTF-8 safe: offsets handed back always sit
//! on a char boundary.

/// Byte offset of the start of the line containing `pos`.
pub fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Byte offset of the end of the line containing `pos`, excluding the newline.
pub fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map(|i| pos + i).unwrap_or(text.len())
}

/// The line containing `pos`, without its trailing newline.
pub fn line_at(text: &str, pos: usize) -> &str {
    &text[line_start(text, pos)..line_end(text, pos)]
}

/// Zero-based index of the line containing `pos`.
pub fn line_index(text: &str, pos: usize) -> usize {
    text[..pos].bytes().filter(|b| *b == b'\n').count()
}

/// Byte range of line `index`, excluding the trailing newline.
pub fn line_range(text: &str, index: usize) -> Option<(usize, usize)> {
    let mut start = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        let end = start + line.len();
        if i == index {
            return Some((start, end));
        }
        start = end + 1;
    }
    None
}

/// Number of lines. A trailing newline yields a final empty line, matching editors.
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|b| *b == b'\n').count() + 1
}

/// Previous char boundary before `pos`, or `pos` if already at the start.
pub fn prev_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos - 1;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Next char boundary after `pos`, or `pos` if already at the end.
pub fn next_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut i = pos + 1;
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Clamp an arbitrary offset onto the nearest char boundary at or below it.
pub fn clamp_boundary(text: &str, pos: usize) -> usize {
    let mut i = pos.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Start of the previous word relative to `pos`.
pub fn prev_word(text: &str, pos: usize) -> usize {
    let mut i = pos;
    while i > 0 {
        let p = prev_boundary(text, i);
        if !text[p..i].chars().next().map(char::is_whitespace).unwrap_or(false) {
            break;
        }
        i = p;
    }
    while i > 0 {
        let p = prev_boundary(text, i);
        let c = text[p..i].chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            break;
        }
        i = p;
    }
    i
}

/// End of the next word relative to `pos`.
pub fn next_word(text: &str, pos: usize) -> usize {
    let mut i = pos;
    while i < text.len() {
        let n = next_boundary(text, i);
        if !text[i..n].chars().next().map(char::is_whitespace).unwrap_or(false) {
            break;
        }
        i = n;
    }
    while i < text.len() {
        let n = next_boundary(text, i);
        let c = text[i..n].chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            break;
        }
        i = n;
    }
    i
}

/// Leading whitespace of a line, as a string slice.
pub fn indent_of(line: &str) -> &str {
    let n = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..n]
}

/// Column (in chars) of `pos` within its line.
pub fn column_of(text: &str, pos: usize) -> usize {
    let start = line_start(text, pos);
    text[start..pos].chars().count()
}

/// Byte offset of char-column `col` on line `index`, clamped to the line's length.
pub fn offset_of_column(text: &str, index: usize, col: usize) -> Option<usize> {
    let (start, end) = line_range(text, index)?;
    let line = &text[start..end];
    for (n, (i, _)) in line.char_indices().enumerate() {
        if n == col {
            return Some(start + i);
        }
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_helpers() {
        let t = "alpha\nbeta\ngamma";
        assert_eq!(line_start(t, 7), 6);
        assert_eq!(line_end(t, 7), 10);
        assert_eq!(line_at(t, 7), "beta");
        assert_eq!(line_index(t, 7), 1);
        assert_eq!(line_range(t, 2), Some((11, 16)));
        assert_eq!(line_count(t), 3);
    }

    #[test]
    fn trailing_newline_makes_an_empty_last_line() {
        let t = "one\n";
        assert_eq!(line_count(t), 2);
        assert_eq!(line_range(t, 1), Some((4, 4)));
    }

    #[test]
    fn boundaries_are_utf8_safe() {
        let t = "aé漢";
        assert_eq!(next_boundary(t, 0), 1);
        assert_eq!(next_boundary(t, 1), 3);
        assert_eq!(next_boundary(t, 3), 6);
        assert_eq!(prev_boundary(t, 6), 3);
        assert_eq!(prev_boundary(t, 3), 1);
        assert_eq!(clamp_boundary(t, 2), 1);
    }

    #[test]
    fn word_motion() {
        let t = "hello brave world";
        assert_eq!(prev_word(t, 17), 12);
        assert_eq!(next_word(t, 0), 5);
    }

    #[test]
    fn indent_and_columns() {
        assert_eq!(indent_of("    - x"), "    ");
        // `é` occupies bytes 5..7, so end-of-line is byte 7 — column 3.
        let t = "ab\ncdé";
        assert_eq!(column_of(t, 7), 3);
        assert_eq!(offset_of_column(t, 1, 2), Some(5));
        // Past the end of the line clamps to the line end, never past it.
        assert_eq!(offset_of_column(t, 1, 99), Some(7));
    }
}
