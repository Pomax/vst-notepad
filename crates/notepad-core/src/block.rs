//! Block-level markdown parsing.
//!
//! Turns the source buffer into one [`Block`] per line. Each block records the
//! byte range of its syntax marker (`## `, `- [ ] `, `> `) separately from its
//! content, so a WYSIWYG renderer can hide the marker and style the content
//! while the raw text underneath stays exactly what the user typed.

use std::ops::Range;

use crate::inline::{parse_inline, Span};
use crate::text;

/// The kind of line, after any blockquote prefix has been stripped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Paragraph,
    Blank,
    Heading(u8),
    Bullet { indent: usize, checked: Option<bool> },
    Numbered { indent: usize, number: u32, checked: Option<bool> },
    /// A ``` or ~~~ fence line. `open` is true for the opening fence.
    Fence { lang: String, open: bool },
    /// A line inside a fenced code block.
    Code,
    Rule,
}

impl BlockKind {
    pub fn is_list(&self) -> bool {
        matches!(self, BlockKind::Bullet { .. } | BlockKind::Numbered { .. })
    }
    pub fn checked(&self) -> Option<bool> {
        match self {
            BlockKind::Bullet { checked, .. } | BlockKind::Numbered { checked, .. } => *checked,
            _ => None,
        }
    }
    pub fn indent(&self) -> usize {
        match self {
            BlockKind::Bullet { indent, .. } | BlockKind::Numbered { indent, .. } => *indent,
            _ => 0,
        }
    }
}

/// One source line, classified and laid out.
#[derive(Clone, Debug)]
pub struct Block {
    pub line: usize,
    /// Byte range of the whole line, excluding the newline.
    pub range: Range<usize>,
    pub kind: BlockKind,
    /// Blockquote nesting level (`> ` prefixes stripped from the marker).
    pub quote_depth: u8,
    /// Byte range of the leading syntax marker, e.g. `## ` or `- [x] `.
    pub marker: Range<usize>,
    /// Whether the marker should be drawn (true when the caret is on this line).
    pub marker_visible: bool,
    /// Byte range of the content after the marker.
    pub content: Range<usize>,
    /// Inline spans covering `content`.
    pub spans: Vec<Span>,
}

impl Block {
    /// The content of this line as the reader sees it, markers removed.
    pub fn visible_text(&self, src: &str) -> String {
        crate::inline::visible_text(src, &self.spans)
    }
    pub fn source<'a>(&self, src: &'a str) -> &'a str {
        &src[self.range.clone()]
    }
}

/// The whole document, laid out for rendering.
#[derive(Clone, Debug)]
pub struct RenderDoc {
    pub blocks: Vec<Block>,
}

impl RenderDoc {
    pub fn block_at_line(&self, line: usize) -> Option<&Block> {
        self.blocks.get(line)
    }
    /// The block containing byte offset `pos`.
    pub fn block_at_offset(&self, pos: usize) -> Option<&Block> {
        self.blocks
            .iter()
            .find(|b| pos >= b.range.start && pos <= b.range.end)
    }
}

/// Parse `src` into renderable blocks. `caret` reveals markers on its line.
pub fn parse_document(src: &str, caret: Option<usize>) -> RenderDoc {
    let caret_line = caret.map(|c| text::line_index(src, c));
    let mut blocks = Vec::new();
    let mut start = 0usize;
    let mut in_fence = false;

    for (index, line) in src.split('\n').enumerate() {
        let end = start + line.len();
        let focused = caret_line == Some(index);
        let block = classify(src, index, start..end, caret, focused, &mut in_fence);
        blocks.push(block);
        start = end + 1;
    }

    RenderDoc { blocks }
}

fn classify(
    src: &str,
    index: usize,
    range: Range<usize>,
    caret: Option<usize>,
    focused: bool,
    in_fence: &mut bool,
) -> Block {
    let line = &src[range.clone()];

    // Blockquote prefixes come first and can nest: "> > text".
    let mut cursor = 0usize;
    let mut quote_depth = 0u8;
    loop {
        let rest = &line[cursor..];
        let ws = rest.len() - rest.trim_start_matches(' ').len();
        if ws <= 3 && rest[ws..].starts_with('>') {
            cursor += ws + 1;
            if line[cursor..].starts_with(' ') {
                cursor += 1;
            }
            quote_depth = quote_depth.saturating_add(1);
        } else {
            break;
        }
    }
    let quote_marker_end = range.start + cursor;
    let body = &line[cursor..];
    let body_start = quote_marker_end;

    let finish = |kind: BlockKind, marker_len: usize, parse_inlines: bool| -> Block {
        let marker = range.start..quote_marker_end + marker_len;
        let content = marker.end..range.end;
        let spans = if parse_inlines {
            parse_inline(src, content.clone(), caret)
        } else if content.is_empty() {
            Vec::new()
        } else {
            vec![Span {
                range: content.clone(),
                style: if matches!(kind, BlockKind::Code) {
                    crate::inline::Style::CODE
                } else {
                    crate::inline::Style::NONE
                },
                role: crate::inline::SpanRole::Text,
                visible: true,
            }]
        };
        Block {
            line: index,
            range: range.clone(),
            kind,
            quote_depth,
            marker,
            marker_visible: focused,
            content,
            spans,
        }
    };

    // Fences toggle code mode and are recognised even while inside one.
    if let Some(fence) = fence_info(body) {
        let open = !*in_fence;
        *in_fence = !*in_fence;
        let lang = if open { fence.1 } else { String::new() };
        return finish(BlockKind::Fence { lang, open }, body.len(), false);
    }
    if *in_fence {
        return finish(BlockKind::Code, 0, false);
    }

    if body.trim().is_empty() {
        return finish(BlockKind::Blank, 0, false);
    }

    if is_rule(body) {
        return finish(BlockKind::Rule, body.len(), false);
    }

    // ATX heading: up to three spaces of indent, 1-6 hashes, then a space.
    let ws = body.len() - body.trim_start_matches(' ').len();
    if ws <= 3 {
        let after_ws = &body[ws..];
        let hashes = after_ws.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) {
            let after = &after_ws[hashes..];
            if after.starts_with(' ') || after.is_empty() {
                let marker_len = ws + hashes + usize::from(after.starts_with(' '));
                return finish(BlockKind::Heading(hashes as u8), marker_len, true);
            }
        }
    }

    if let Some((marker_len, kind)) = list_info(body, body_start, src) {
        return finish(kind, marker_len, true);
    }

    finish(BlockKind::Paragraph, 0, true)
}

/// Length and kind of a line's list marker, e.g. `- `, `3. `, `- [x] `.
///
/// Exposed for the editing layer, which needs to recognise list items while
/// rewriting text (continuation, indent, renumbering).
pub fn list_marker(line: &str) -> Option<(usize, BlockKind)> {
    list_info(line, 0, "")
}

/// Byte length of the leading `> ` blockquote prefixes, and their nesting depth.
pub fn quote_prefix(line: &str) -> (usize, u8) {
    let mut cursor = 0usize;
    let mut depth = 0u8;
    loop {
        let rest = &line[cursor..];
        let ws = rest.len() - rest.trim_start_matches(' ').len();
        if ws <= 3 && rest[ws..].starts_with('>') {
            cursor += ws + 1;
            if line[cursor..].starts_with(' ') {
                cursor += 1;
            }
            depth = depth.saturating_add(1);
        } else {
            return (cursor, depth);
        }
    }
}

/// True if the line opens or closes a fenced code block.
pub fn is_fence(line: &str) -> bool {
    fence_info(line).is_some()
}

/// True if the line is a thematic break (`---`, `***`, `___`).
pub fn is_thematic_break(line: &str) -> bool {
    is_rule(line)
}

/// `(fence_char_count, language)` if the line opens or closes a fence.
fn fence_info(line: &str) -> Option<(usize, String)> {
    let ws = line.len() - line.trim_start_matches(' ').len();
    if ws > 3 {
        return None;
    }
    let rest = &line[ws..];
    let c = rest.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = rest.chars().take_while(|d| *d == c).count();
    if n < 3 {
        return None;
    }
    let lang = rest[n..].trim().to_string();
    // An info string may not contain backticks on a ``` fence.
    if c == '`' && lang.contains('`') {
        return None;
    }
    Some((n, lang))
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 {
        return false;
    }
    for c in ['-', '*', '_'] {
        if t.chars().all(|d| d == c || d == ' ') && t.chars().filter(|d| *d == c).count() >= 3 {
            return true;
        }
    }
    false
}

/// `(marker_len, kind)` for bullet and ordered list items, including task boxes.
fn list_info(body: &str, _body_start: usize, _src: &str) -> Option<(usize, BlockKind)> {
    let indent: usize = body
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    let ws = body.len() - body.trim_start_matches([' ', '\t']).len();
    let rest = &body[ws..];

    let (marker_len, mut kind) = if let Some(first) = rest.chars().next() {
        if matches!(first, '-' | '+' | '*') && rest[1..].starts_with(' ') {
            (ws + 2, BlockKind::Bullet { indent, checked: None })
        } else {
            let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            let after = &rest[digits..];
            if !(after.starts_with(". ") || after.starts_with(") ")) {
                return None;
            }
            let number: u32 = rest[..digits].parse().ok()?;
            (ws + digits + 2, BlockKind::Numbered { indent, number, checked: None })
        }
    } else {
        return None;
    };

    // Optional task-list checkbox directly after the list marker.
    let after_marker = &body[marker_len..];
    let mut extra = 0usize;
    let checked = if after_marker.starts_with("[ ] ") || after_marker == "[ ]" {
        extra = if after_marker == "[ ]" { 3 } else { 4 };
        Some(false)
    } else if after_marker.starts_with("[x] ")
        || after_marker.starts_with("[X] ")
        || after_marker == "[x]"
        || after_marker == "[X]"
    {
        extra = if after_marker.len() == 3 { 3 } else { 4 };
        Some(true)
    } else {
        None
    };
    if checked.is_some() {
        match &mut kind {
            BlockKind::Bullet { checked: c, .. } | BlockKind::Numbered { checked: c, .. } => {
                *c = checked
            }
            _ => {}
        }
    }

    Some((marker_len + extra, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> RenderDoc {
        parse_document(src, None)
    }

    #[test]
    fn headings_by_level() {
        let src = "# One\n## Two\n###### Six\n####### Seven";
        let d = doc(src);
        assert_eq!(d.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(d.blocks[1].kind, BlockKind::Heading(2));
        assert_eq!(d.blocks[2].kind, BlockKind::Heading(6));
        // Seven hashes is not a heading.
        assert_eq!(d.blocks[3].kind, BlockKind::Paragraph);
        assert_eq!(d.blocks[0].visible_text(src), "One");
    }

    #[test]
    fn heading_marker_is_hidden_until_focused() {
        let src = "## Title";
        let d = parse_document(src, None);
        assert!(!d.blocks[0].marker_visible);
        assert_eq!(&src[d.blocks[0].marker.clone()], "## ");
        let d = parse_document(src, Some(4));
        assert!(d.blocks[0].marker_visible);
    }

    #[test]
    fn bullets_and_numbers() {
        let src = "- one\n+ two\n* three\n1. first\n2) second";
        let d = doc(src);
        assert!(matches!(d.blocks[0].kind, BlockKind::Bullet { .. }));
        assert!(matches!(d.blocks[1].kind, BlockKind::Bullet { .. }));
        assert!(matches!(d.blocks[2].kind, BlockKind::Bullet { .. }));
        assert!(matches!(d.blocks[3].kind, BlockKind::Numbered { number: 1, .. }));
        assert!(matches!(d.blocks[4].kind, BlockKind::Numbered { number: 2, .. }));
        assert_eq!(d.blocks[3].visible_text(src), "first");
    }

    #[test]
    fn nested_list_indent_is_recorded() {
        let src = "- top\n  - nested\n    - deeper";
        let d = doc(src);
        assert_eq!(d.blocks[0].kind.indent(), 0);
        assert_eq!(d.blocks[1].kind.indent(), 2);
        assert_eq!(d.blocks[2].kind.indent(), 4);
    }

    #[test]
    fn task_list_checkboxes() {
        let src = "- [ ] todo\n- [x] done\n- [X] also done\n- plain";
        let d = doc(src);
        assert_eq!(d.blocks[0].kind.checked(), Some(false));
        assert_eq!(d.blocks[1].kind.checked(), Some(true));
        assert_eq!(d.blocks[2].kind.checked(), Some(true));
        assert_eq!(d.blocks[3].kind.checked(), None);
        assert_eq!(d.blocks[0].visible_text(src), "todo");
        assert_eq!(&src[d.blocks[1].marker.clone()], "- [x] ");
    }

    #[test]
    fn fenced_code_suppresses_markdown() {
        let src = "```rust\nlet x = *y;\n# not a heading\n```\nafter";
        let d = doc(src);
        assert!(matches!(&d.blocks[0].kind, BlockKind::Fence { open: true, lang } if lang == "rust"));
        assert_eq!(d.blocks[1].kind, BlockKind::Code);
        assert_eq!(d.blocks[2].kind, BlockKind::Code);
        assert!(matches!(d.blocks[3].kind, BlockKind::Fence { open: false, .. }));
        assert_eq!(d.blocks[4].kind, BlockKind::Paragraph);
        assert_eq!(d.blocks[2].visible_text(src), "# not a heading");
    }

    #[test]
    fn blockquotes_nest_and_keep_inner_kind() {
        let src = "> quoted\n> > deeper\n> ## quoted heading";
        let d = doc(src);
        assert_eq!(d.blocks[0].quote_depth, 1);
        assert_eq!(d.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(d.blocks[1].quote_depth, 2);
        assert_eq!(d.blocks[2].quote_depth, 1);
        assert_eq!(d.blocks[2].kind, BlockKind::Heading(2));
        assert_eq!(d.blocks[2].visible_text(src), "quoted heading");
    }

    #[test]
    fn horizontal_rules() {
        let src = "---\n***\n___\n- - -\n--";
        let d = doc(src);
        assert_eq!(d.blocks[0].kind, BlockKind::Rule);
        assert_eq!(d.blocks[1].kind, BlockKind::Rule);
        assert_eq!(d.blocks[2].kind, BlockKind::Rule);
        assert_eq!(d.blocks[3].kind, BlockKind::Rule);
        assert_ne!(d.blocks[4].kind, BlockKind::Rule);
    }

    #[test]
    fn blank_lines_and_paragraphs() {
        let src = "para one\n\npara two";
        let d = doc(src);
        assert_eq!(d.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(d.blocks[1].kind, BlockKind::Blank);
        assert_eq!(d.blocks[2].kind, BlockKind::Paragraph);
    }

    #[test]
    fn inline_styles_inside_blocks() {
        let src = "## A **bold** heading";
        let d = doc(src);
        assert_eq!(d.blocks[0].visible_text(src), "A bold heading");
        assert!(d.blocks[0]
            .spans
            .iter()
            .any(|s| s.style.contains(crate::inline::Style::BOLD) && !s.is_marker()));
    }

    #[test]
    fn one_block_per_line_always() {
        let src = "# h\n\n- a\n- b\n\n```\ncode\n```\n> q\n";
        let d = doc(src);
        assert_eq!(d.blocks.len(), text::line_count(src));
    }
}
