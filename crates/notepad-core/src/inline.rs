//! Inline markdown parsing.
//!
//! Produces a flat list of styled [`Span`]s over the *source* text. Syntax
//! punctuation is emitted as [`SpanRole::Marker`] spans rather than being
//! stripped, which is what makes Typora-style editing possible: in WYSIWYG mode
//! the renderer hides markers, except on the construct the caret is currently
//! inside, where they are revealed so the user can edit them.

use std::ops::Range;

/// Character styling, as a small bitset.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Style(pub u8);

impl Style {
    pub const NONE: Style = Style(0);
    pub const BOLD: Style = Style(1 << 0);
    pub const ITALIC: Style = Style(1 << 1);
    pub const CODE: Style = Style(1 << 2);
    pub const STRIKE: Style = Style(1 << 3);
    pub const LINK: Style = Style(1 << 4);

    pub fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn with(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Debug for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.contains(Style::BOLD) {
            parts.push("bold");
        }
        if self.contains(Style::ITALIC) {
            parts.push("italic");
        }
        if self.contains(Style::CODE) {
            parts.push("code");
        }
        if self.contains(Style::STRIKE) {
            parts.push("strike");
        }
        if self.contains(Style::LINK) {
            parts.push("link");
        }
        if parts.is_empty() {
            write!(f, "none")
        } else {
            write!(f, "{}", parts.join("+"))
        }
    }
}

/// What a span represents to the renderer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpanRole {
    /// Ordinary visible text.
    Text,
    /// Markdown punctuation (`**`, `[`, `](url)`, …).
    Marker,
    /// Visible link label; carries the destination.
    Link { url: String },
    /// Visible image alt text; carries the destination.
    Image { url: String },
}

/// A run of source text sharing one style and role.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
    pub role: SpanRole,
    /// False for markers that should be hidden in WYSIWYG mode.
    pub visible: bool,
}

impl Span {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.range.clone()]
    }
    pub fn is_marker(&self) -> bool {
        self.role == SpanRole::Marker
    }
}

/// Parse the inline content of `range` within `src`.
///
/// `caret` reveals the markers of whichever construct contains it. Pass `None`
/// to hide every marker (used for non-focused rendering and for tests that
/// assert on the "clean" WYSIWYG output).
pub fn parse_inline(src: &str, range: Range<usize>, caret: Option<usize>) -> Vec<Span> {
    let mut out = Vec::new();
    parse_range(src, range, Style::NONE, caret, &mut out);
    coalesce(out)
}

fn parse_range(
    src: &str,
    range: Range<usize>,
    base: Style,
    caret: Option<usize>,
    out: &mut Vec<Span>,
) {
    let end = range.end;
    let mut i = range.start;
    let mut run_start = i;

    // Flush pending plain text as one span.
    macro_rules! flush {
        ($upto:expr) => {
            if $upto > run_start {
                out.push(Span {
                    range: run_start..$upto,
                    style: base,
                    role: SpanRole::Text,
                    visible: true,
                });
            }
        };
    }

    while i < end {
        let rest = &src[i..end];
        let c = match rest.chars().next() {
            Some(c) => c,
            None => break,
        };
        let clen = c.len_utf8();

        // Backslash escape: the escaped character is literal text.
        if c == '\\' && i + clen < end {
            flush!(i);
            out.push(Span {
                range: i..i + clen,
                style: base,
                role: SpanRole::Marker,
                visible: reveal(caret, i, i + clen + 1),
            });
            let n = crate::text::next_boundary(src, i + clen);
            out.push(Span {
                range: i + clen..n,
                style: base,
                role: SpanRole::Text,
                visible: true,
            });
            i = n;
            run_start = i;
            continue;
        }

        // Code span: `code` / ``code with ` inside``.
        if c == '`' {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            let open = &src[i..i + ticks];
            if let Some(close) = find_literal(src, i + ticks, end, open) {
                flush!(i);
                let full = i..close + ticks;
                let vis = reveal(caret, full.start, full.end);
                out.push(marker(i..i + ticks, base, vis));
                out.push(Span {
                    range: i + ticks..close,
                    style: base.with(Style::CODE),
                    role: SpanRole::Text,
                    visible: true,
                });
                out.push(marker(close..close + ticks, base, vis));
                i = close + ticks;
                run_start = i;
                continue;
            }
        }

        // Image ![alt](url) and link [text](url).
        if c == '!' || c == '[' {
            let is_image = c == '!';
            let bracket = if is_image { i + 1 } else { i };
            if src[bracket..end].starts_with('[') {
                if let Some((label, dest, full_end)) = parse_link(src, bracket, end) {
                    flush!(i);
                    let vis = reveal(caret, i, full_end);
                    let url = src[dest.clone()].to_string();
                    out.push(marker(i..label.start, base, vis));
                    if is_image {
                        out.push(Span {
                            range: label.clone(),
                            style: base,
                            role: SpanRole::Image { url: url.clone() },
                            visible: true,
                        });
                    } else {
                        // Link labels may contain emphasis; parse them, then
                        // retag the visible text runs as link text.
                        let mut inner = Vec::new();
                        parse_range(src, label.clone(), base.with(Style::LINK), caret, &mut inner);
                        for mut span in inner {
                            if span.role == SpanRole::Text {
                                span.role = SpanRole::Link { url: url.clone() };
                            }
                            out.push(span);
                        }
                    }
                    out.push(marker(label.end..full_end, base, vis));
                    i = full_end;
                    run_start = i;
                    continue;
                }
            }
        }

        // Autolink <https://example.com>
        if c == '<' {
            if let Some(gt) = src[i..end].find('>') {
                let inner = i + 1..i + gt;
                let body = &src[inner.clone()];
                if body.starts_with("http://")
                    || body.starts_with("https://")
                    || body.starts_with("mailto:")
                {
                    flush!(i);
                    let full_end = i + gt + 1;
                    let vis = reveal(caret, i, full_end);
                    out.push(marker(i..i + 1, base, vis));
                    out.push(Span {
                        range: inner,
                        style: base.with(Style::LINK),
                        role: SpanRole::Link { url: body.to_string() },
                        visible: true,
                    });
                    out.push(marker(full_end - 1..full_end, base, vis));
                    i = full_end;
                    run_start = i;
                    continue;
                }
            }
        }

        // Emphasis runs: ***x***, **x**, *x*, ~~x~~, __x__, _x_.
        if matches!(c, '*' | '_' | '~') {
            let run = rest.chars().take_while(|d| *d == c).count();
            let candidates: &[usize] = match c {
                '~' => &[2],
                _ => &[3, 2, 1],
            };
            let mut matched = false;
            for &n in candidates {
                if run < n {
                    continue;
                }
                let delim = &src[i..i + n];
                if !left_flanking(src, i, i + n, end) {
                    continue;
                }
                if let Some(close) = find_closer(src, i + n, end, delim) {
                    let style = match (c, n) {
                        ('~', _) => Style::STRIKE,
                        (_, 3) => Style::BOLD.with(Style::ITALIC),
                        (_, 2) => Style::BOLD,
                        _ => Style::ITALIC,
                    };
                    flush!(i);
                    let full_end = close + n;
                    let vis = reveal(caret, i, full_end);
                    out.push(marker(i..i + n, base, vis));
                    parse_range(src, i + n..close, base.with(style), caret, out);
                    out.push(marker(close..full_end, base, vis));
                    i = full_end;
                    run_start = i;
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
        }

        i += clen;
    }

    flush!(end);
}

fn marker(range: Range<usize>, style: Style, visible: bool) -> Span {
    Span { range, style, role: SpanRole::Marker, visible }
}

fn reveal(caret: Option<usize>, start: usize, end: usize) -> bool {
    match caret {
        Some(c) => c >= start && c <= end,
        None => false,
    }
}

/// Find `needle` in `src[from..end]`, ignoring backslash-escaped positions.
fn find_literal(src: &str, from: usize, end: usize, needle: &str) -> Option<usize> {
    let mut i = from;
    while i < end {
        if src[i..].starts_with('\\') {
            i = crate::text::next_boundary(src, crate::text::next_boundary(src, i));
            continue;
        }
        if src[i..end].starts_with(needle) {
            return Some(i);
        }
        i = crate::text::next_boundary(src, i);
    }
    None
}

/// Find the closing delimiter for an emphasis run.
///
/// Skips escaped characters and code spans, requires non-empty content, and
/// rejects a closer that is immediately followed by more of the same character
/// (so `**a**` closes at the right place rather than mid-run).
fn find_closer(src: &str, from: usize, end: usize, delim: &str) -> Option<usize> {
    let dc = delim.chars().next()?;
    let n = delim.len();
    let mut i = from;
    while i < end {
        let rest = &src[i..end];
        if rest.starts_with('\\') {
            i = crate::text::next_boundary(src, crate::text::next_boundary(src, i));
            continue;
        }
        if rest.starts_with('`') {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            if let Some(close) = find_literal(src, i + ticks, end, &src[i..i + ticks]) {
                i = close + ticks;
                continue;
            }
        }
        if rest.starts_with(delim) && i > from && right_flanking(src, i, i + n) {
            let run = rest.chars().take_while(|c| *c == dc).count();
            if run == n || dc == '~' {
                return Some(i);
            }
            // A longer run closes the shorter inner delimiter at its tail.
            if run > n {
                return Some(i);
            }
        }
        i = crate::text::next_boundary(src, i);
    }
    None
}

/// An opener must be followed by non-whitespace; `_` additionally may not open
/// inside a word, so `snake_case_names` stay literal.
fn left_flanking(src: &str, start: usize, after: usize, end: usize) -> bool {
    if src[after..end].chars().next().map(|c| c.is_whitespace()).unwrap_or(true) {
        return false;
    }
    if src[start..].starts_with('_') {
        let prev = src[..start].chars().next_back();
        if prev.map(|c| c.is_alphanumeric()).unwrap_or(false) {
            return false;
        }
    }
    true
}

/// A closer must be preceded by non-whitespace, and `_` may not close inside a word.
fn right_flanking(src: &str, start: usize, after: usize) -> bool {
    let prev = src[..start].chars().next_back();
    if prev.map(|c| c.is_whitespace()).unwrap_or(true) {
        return false;
    }
    if src[start..].starts_with('_') {
        let next = src[after..].chars().next();
        if next.map(|c| c.is_alphanumeric()).unwrap_or(false) {
            return false;
        }
    }
    true
}

/// Parse `[label](dest)` starting at the `[`. Returns (label, dest, end).
fn parse_link(
    src: &str,
    open: usize,
    end: usize,
) -> Option<(Range<usize>, Range<usize>, usize)> {
    let mut depth = 0i32;
    let mut i = open;
    let mut close = None;
    while i < end {
        let rest = &src[i..end];
        if rest.starts_with('\\') {
            i = crate::text::next_boundary(src, crate::text::next_boundary(src, i));
            continue;
        }
        if rest.starts_with('[') {
            depth += 1;
        } else if rest.starts_with(']') {
            depth -= 1;
            if depth == 0 {
                close = Some(i);
                break;
            }
        }
        i = crate::text::next_boundary(src, i);
    }
    let close = close?;
    if !src[close + 1..end.max(close + 1)].starts_with('(') {
        return None;
    }
    let mut depth = 0i32;
    let mut j = close + 1;
    while j < end {
        let rest = &src[j..end];
        if rest.starts_with('\\') {
            j = crate::text::next_boundary(src, crate::text::next_boundary(src, j));
            continue;
        }
        if rest.starts_with('(') {
            depth += 1;
        } else if rest.starts_with(')') {
            depth -= 1;
            if depth == 0 {
                return Some((open + 1..close, close + 2..j, j + 1));
            }
        }
        j = crate::text::next_boundary(src, j);
    }
    None
}

/// Merge adjacent spans that share style, role and visibility.
fn coalesce(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if span.range.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.range.end == span.range.start
                && last.style == span.style
                && last.role == span.role
                && last.visible == span.visible
            {
                last.range.end = span.range.end;
                continue;
            }
        }
        out.push(span);
    }
    out
}

/// Concatenate the visible text of `spans` — what the reader actually sees.
pub fn visible_text(src: &str, spans: &[Span]) -> String {
    spans
        .iter()
        .filter(|s| s.visible)
        .map(|s| s.text(src))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<Span> {
        parse_inline(src, 0..src.len(), None)
    }

    fn styled(src: &str, want: Style) -> String {
        parse(src)
            .iter()
            .filter(|s| s.visible && s.style.contains(want) && !s.is_marker())
            .map(|s| s.text(src))
            .collect()
    }

    #[test]
    fn plain_text_is_one_span() {
        let src = "just some words";
        let spans = parse(src);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, Style::NONE);
        assert_eq!(visible_text(src, &spans), src);
    }

    #[test]
    fn bold_italic_strike_and_code() {
        assert_eq!(styled("a **bold** b", Style::BOLD), "bold");
        assert_eq!(styled("a *ital* b", Style::ITALIC), "ital");
        assert_eq!(styled("a _ital_ b", Style::ITALIC), "ital");
        assert_eq!(styled("a __bold__ b", Style::BOLD), "bold");
        assert_eq!(styled("a ~~gone~~ b", Style::STRIKE), "gone");
        assert_eq!(styled("a `code()` b", Style::CODE), "code()");
    }

    #[test]
    fn triple_marker_is_bold_and_italic() {
        let src = "***loud***";
        assert_eq!(styled(src, Style::BOLD.with(Style::ITALIC)), "loud");
    }

    #[test]
    fn markers_are_hidden_but_text_survives() {
        let src = "the **quick** brown";
        let spans = parse(src);
        assert_eq!(visible_text(src, &spans), "the quick brown");
        assert!(spans.iter().any(|s| s.is_marker() && !s.visible));
    }

    #[test]
    fn caret_inside_a_construct_reveals_its_markers() {
        let src = "the **quick** brown";
        // Caret inside "quick"
        let spans = parse_inline(src, 0..src.len(), Some(8));
        assert_eq!(visible_text(src, &spans), src);
        // Caret far away leaves them hidden.
        let spans = parse_inline(src, 0..src.len(), Some(0));
        assert_eq!(visible_text(src, &spans), "the quick brown");
    }

    #[test]
    fn nested_emphasis() {
        let src = "**bold with *italic* inside**";
        let both: String = parse(src)
            .iter()
            .filter(|s| {
                s.visible && !s.is_marker() && s.style.contains(Style::BOLD.with(Style::ITALIC))
            })
            .map(|s| s.text(src))
            .collect();
        assert_eq!(both, "italic");
    }

    #[test]
    fn links_expose_their_destination() {
        let src = "see [the docs](https://example.com/x) now";
        let spans = parse(src);
        assert_eq!(visible_text(src, &spans), "see the docs now");
        let link = spans
            .iter()
            .find(|s| matches!(s.role, SpanRole::Link { .. }))
            .expect("link span");
        assert_eq!(link.text(src), "the docs");
        match &link.role {
            SpanRole::Link { url } => assert_eq!(url, "https://example.com/x"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn images_keep_alt_text_visible() {
        let src = "![a cat](cat.png)";
        let spans = parse(src);
        let img = spans
            .iter()
            .find(|s| matches!(s.role, SpanRole::Image { .. }))
            .expect("image span");
        assert_eq!(img.text(src), "a cat");
    }

    #[test]
    fn autolinks_work() {
        let src = "<https://example.com>";
        let spans = parse(src);
        assert!(spans.iter().any(|s| matches!(&s.role, SpanRole::Link { url } if url == "https://example.com")));
    }

    #[test]
    fn underscores_inside_words_are_literal() {
        let src = "call snake_case_name here";
        assert_eq!(styled(src, Style::ITALIC), "");
        assert_eq!(visible_text(src, &parse(src)), src);
    }

    #[test]
    fn unclosed_delimiters_stay_literal() {
        let src = "2 * 3 * 4 is not emphasis if spaced";
        assert_eq!(visible_text(src, &parse(src)), src);
    }

    #[test]
    fn escapes_are_literal() {
        let src = r"not \*emphasis\*";
        assert_eq!(styled(src, Style::ITALIC), "");
        assert_eq!(visible_text(src, &parse(src)), "not *emphasis*");
    }

    #[test]
    fn code_spans_swallow_emphasis_markers() {
        let src = "`a * b * c`";
        assert_eq!(styled(src, Style::ITALIC), "");
        assert_eq!(styled(src, Style::CODE), "a * b * c");
    }

    #[test]
    fn spans_tile_the_source_exactly() {
        for src in [
            "plain",
            "**b** and *i* and `c`",
            "[l](u) trailing",
            "***x*** ~~y~~",
            r"esc \* here",
        ] {
            let spans = parse_inline(src, 0..src.len(), None);
            let mut at = 0;
            for s in &spans {
                assert_eq!(s.range.start, at, "gap in {src:?}");
                at = s.range.end;
            }
            assert_eq!(at, src.len(), "short coverage in {src:?}");
        }
    }
}
