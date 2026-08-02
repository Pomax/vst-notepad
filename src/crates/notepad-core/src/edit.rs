//! The editor: caret, selection, key handling and as-you-type markdown conversion.
//!
//! The markdown source is always the single source of truth. "Conversion" comes
//! in two flavours, matching how Typora behaves:
//!
//! * **Rewrites** — the source itself changes as you type: `* ` becomes `- `,
//!   Enter continues a list and renumbers it, an empty item exits the list,
//!   a fence auto-closes, selections get wrapped in emphasis markers.
//! * **Rendering** — the source is left alone and the *display* changes:
//!   `## Title` draws as a large heading with the `## ` hidden, until the caret
//!   moves onto that line and the marker is revealed for editing.
//!
//! Everything here is headless and deterministic, so the same code path serves
//! the GUI and the VST3 host's synthetic key injection.

use std::ops::Range;
use std::path::PathBuf;

use crate::block::{self, BlockKind, RenderDoc};
use crate::text;

/// A logical key press.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Escape,
}

/// Modifier state accompanying a key press.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
    pub const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
    pub const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
    pub const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };
}

/// Which view the editor is presenting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    /// Rendered markdown with markers hidden except on the caret's line.
    Wysiwyg,
    /// The raw markdown source.
    Raw,
}

impl ViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewMode::Wysiwyg => "wysiwyg",
            ViewMode::Raw => "raw",
        }
    }
    pub fn from_str(s: &str) -> ViewMode {
        match s {
            "raw" => ViewMode::Raw,
            _ => ViewMode::Wysiwyg,
        }
    }
    pub fn toggled(self) -> ViewMode {
        match self {
            ViewMode::Wysiwyg => ViewMode::Raw,
            ViewMode::Raw => ViewMode::Wysiwyg,
        }
    }
}

/// Which colour scheme the editor draws in.
///
/// `Auto` defers to the operating system. Resolving it needs a platform call,
/// which this crate deliberately does not make — the GUI layer passes what the
/// system reports into [`Theme::is_dark`], keeping the decision itself testable
/// without an OS in the loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    Light,
    Dark,
    /// Follow the system setting.
    #[default]
    Auto,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::Auto => "auto",
        }
    }

    /// Parse a stored theme. Anything unrecognised falls back to `Auto`, so a
    /// state blob from a newer version degrades to following the system rather
    /// than to an arbitrary choice.
    pub fn from_str(s: &str) -> Theme {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::Auto,
        }
    }

    /// The order the Ctrl+T shortcut and the toolbar button walk through.
    pub fn cycled(self) -> Theme {
        match self {
            Theme::Auto => Theme::Light,
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Auto,
        }
    }

    /// Whether to draw dark, given what the system currently reports.
    ///
    /// `system_dark` is only consulted for [`Theme::Auto`].
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Theme::Light => false,
            Theme::Dark => true,
            Theme::Auto => system_dark,
        }
    }
}

/// An action the editor cannot perform itself because it needs the host
/// (native file dialogs, disk access).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Open,
    Save,
    SaveAs,
}

/// Outcome of a key press.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KeyResult {
    /// The editor recognised the key.
    pub handled: bool,
    /// The document text changed.
    pub changed: bool,
    /// A host-level action was requested.
    pub command: Option<Command>,
}

impl KeyResult {
    fn ignored() -> Self {
        Self::default()
    }
    fn moved() -> Self {
        KeyResult { handled: true, changed: false, command: None }
    }
    fn edited() -> Self {
        KeyResult { handled: true, changed: true, command: None }
    }
    fn command(c: Command) -> Self {
        KeyResult { handled: true, changed: false, command: Some(c) }
    }
}

#[derive(Clone)]
struct Snapshot {
    text: String,
    caret: usize,
    anchor: usize,
}

const UNDO_LIMIT: usize = 256;
pub const MIN_WIDTH: i32 = 320;
pub const MIN_HEIGHT: i32 = 200;
/// Upper bound on the editor window, so a corrupt state cannot ask the host
/// for an absurd one.
pub const MAX_WIDTH: i32 = 8192;
pub const MAX_HEIGHT: i32 = 8192;
pub const DEFAULT_WIDTH: i32 = 900;
pub const DEFAULT_HEIGHT: i32 = 620;

/// The editor state. Owns the document and everything persisted as plugin state.
pub struct Editor {
    text: String,
    caret: usize,
    anchor: usize,
    goal_column: Option<usize>,
    pub mode: ViewMode,
    pub theme: Theme,
    pub width: i32,
    pub height: i32,
    pub file: Option<PathBuf>,
    pub dirty: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    coalescing: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    pub fn new() -> Editor {
        Editor {
            text: String::new(),
            caret: 0,
            anchor: 0,
            goal_column: None,
            mode: ViewMode::Wysiwyg,
            theme: Theme::Auto,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            file: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            coalescing: false,
        }
    }

    pub fn with_text(text: impl Into<String>) -> Editor {
        let mut e = Editor::new();
        e.text = text.into();
        e.caret = e.text.len();
        e.anchor = e.caret;
        e
    }

    // ---- accessors -------------------------------------------------------

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The selected range, empty when there is no selection.
    pub fn selection(&self) -> Range<usize> {
        let (a, b) = (self.caret.min(self.anchor), self.caret.max(self.anchor));
        a..b
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    pub fn caret_line(&self) -> usize {
        text::line_index(&self.text, self.caret)
    }

    pub fn caret_column(&self) -> usize {
        text::column_of(&self.text, self.caret)
    }

    /// Replace the whole document, e.g. when loading a file or plugin state.
    pub fn set_text(&mut self, new: impl Into<String>) {
        self.push_undo();
        self.text = new.into();
        self.caret = self.caret.min(self.text.len());
        self.caret = text::clamp_boundary(&self.text, self.caret);
        self.anchor = self.caret;
        self.coalescing = false;
    }

    pub fn set_caret(&mut self, pos: usize) {
        self.caret = text::clamp_boundary(&self.text, pos);
        self.anchor = self.caret;
        self.goal_column = None;
        self.coalescing = false;
    }

    pub fn select(&mut self, range: Range<usize>) {
        self.anchor = text::clamp_boundary(&self.text, range.start);
        self.caret = text::clamp_boundary(&self.text, range.end);
        self.coalescing = false;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// Set the window size, clamped to something a window can actually be.
    ///
    /// The upper bound matters as much as the lower one: this value can arrive
    /// from a project file, and a corrupt or hand-edited state carrying
    /// `i32::MAX` would otherwise be handed to the host as a real window size.
    pub fn set_size(&mut self, width: i32, height: i32) {
        self.width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        self.height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    }

    /// Step to the next theme: Auto → Light → Dark → Auto.
    pub fn cycle_theme(&mut self) {
        self.theme = self.theme.cycled();
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    pub fn toggle_mode(&mut self) {
        self.mode = self.mode.toggled();
    }

    /// Lay the document out for rendering. In raw mode markers are always
    /// visible, since the user is looking at the source.
    pub fn render(&self) -> RenderDoc {
        match self.mode {
            ViewMode::Wysiwyg => block::parse_document(&self.text, Some(self.caret)),
            ViewMode::Raw => {
                let mut doc = block::parse_document(&self.text, Some(self.caret));
                for b in &mut doc.blocks {
                    b.marker_visible = true;
                    for s in &mut b.spans {
                        s.visible = true;
                    }
                }
                doc
            }
        }
    }

    /// The document as the reader sees it, with markers stripped. Handy for
    /// asserting on WYSIWYG output in tests.
    pub fn rendered_text(&self) -> String {
        let doc = block::parse_document(&self.text, None);
        doc.blocks
            .iter()
            .map(|b| b.visible_text(&self.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---- undo ------------------------------------------------------------

    fn push_undo(&mut self) {
        self.undo.push(Snapshot {
            text: self.text.clone(),
            caret: self.caret,
            anchor: self.anchor,
        });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Push an undo entry unless we are mid-run of plain typing.
    fn push_undo_coalesced(&mut self) {
        if !self.coalescing {
            self.push_undo();
            self.coalescing = true;
        } else {
            self.redo.clear();
        }
    }

    /// Drop undo history — used after loading a document, so the user cannot
    /// undo their way back into the previous file's contents.
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.coalescing = false;
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(Snapshot {
                text: std::mem::replace(&mut self.text, prev.text),
                caret: self.caret,
                anchor: self.anchor,
            });
            self.caret = prev.caret.min(self.text.len());
            self.anchor = prev.anchor.min(self.text.len());
            self.coalescing = false;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop() {
            self.undo.push(Snapshot {
                text: std::mem::replace(&mut self.text, next.text),
                caret: self.caret,
                anchor: self.anchor,
            });
            self.caret = next.caret.min(self.text.len());
            self.anchor = next.anchor.min(self.text.len());
            self.coalescing = false;
            true
        } else {
            false
        }
    }

    // ---- primitive edits -------------------------------------------------

    fn replace_range(&mut self, range: Range<usize>, with: &str) {
        self.text.replace_range(range.clone(), with);
        self.caret = range.start + with.len();
        self.anchor = self.caret;
        self.dirty = true;
    }

    fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        if sel.is_empty() {
            return false;
        }
        self.text.replace_range(sel.clone(), "");
        self.caret = sel.start;
        self.anchor = sel.start;
        self.dirty = true;
        true
    }

    /// Insert text at the caret, replacing any selection.
    pub fn insert_str(&mut self, s: &str) {
        self.push_undo();
        self.delete_selection();
        let at = self.caret;
        self.text.insert_str(at, s);
        self.caret = at + s.len();
        self.anchor = self.caret;
        self.dirty = true;
        self.coalescing = false;
    }

    // ---- key handling ----------------------------------------------------

    pub fn handle_key(&mut self, key: Key, mods: Mods) -> KeyResult {
        if key != Key::Char(' ') && !matches!(key, Key::Char(_)) {
            self.coalescing = false;
        }
        if mods.ctrl {
            if let Some(r) = self.handle_ctrl(key, mods) {
                return r;
            }
        }
        match key {
            Key::Char(c) => self.insert_char(c),
            Key::Enter => self.on_enter(),
            Key::Backspace => self.on_backspace(mods),
            Key::Delete => self.on_delete(mods),
            Key::Tab => self.on_tab(mods.shift),
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
                self.on_motion(key, mods)
            }
            Key::PageUp | Key::PageDown => self.on_motion(key, mods),
            Key::Escape => {
                self.anchor = self.caret;
                KeyResult::moved()
            }
        }
    }

    fn handle_ctrl(&mut self, key: Key, mods: Mods) -> Option<KeyResult> {
        let c = match key {
            Key::Char(c) => c.to_ascii_lowercase(),
            _ => return None,
        };
        let r = match c {
            'b' => {
                self.wrap_or_insert("**");
                KeyResult::edited()
            }
            'i' => {
                self.wrap_or_insert("*");
                KeyResult::edited()
            }
            'd' => {
                self.wrap_or_insert("~~");
                KeyResult::edited()
            }
            'e' | '`' => {
                self.wrap_or_insert("`");
                KeyResult::edited()
            }
            'k' => {
                self.make_link();
                KeyResult::edited()
            }
            'a' => {
                self.select_all();
                KeyResult::moved()
            }
            'z' => {
                let changed = if mods.shift { self.redo() } else { self.undo() };
                KeyResult { handled: true, changed, command: None }
            }
            'y' => {
                let changed = self.redo();
                KeyResult { handled: true, changed, command: None }
            }
            '/' => {
                self.toggle_mode();
                KeyResult::moved()
            }
            't' => {
                self.cycle_theme();
                KeyResult::moved()
            }
            'o' => KeyResult::command(Command::Open),
            's' => {
                if mods.shift {
                    KeyResult::command(Command::SaveAs)
                } else {
                    KeyResult::command(Command::Save)
                }
            }
            _ => return None,
        };
        Some(r)
    }

    /// Insert a character, applying any as-you-type rewrite it triggers.
    fn insert_char(&mut self, c: char) -> KeyResult {
        // Typing an emphasis character over a selection wraps it instead.
        if self.has_selection() {
            if let Some(delim) = wrap_delimiter(c) {
                self.wrap_selection(delim);
                return KeyResult::edited();
            }
            if c == '[' {
                self.make_link();
                return KeyResult::edited();
            }
        }

        self.push_undo_coalesced();
        self.delete_selection();

        if c == ' ' {
            if self.rewrite_on_space() {
                return KeyResult::edited();
            }
        }

        let at = self.caret;
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        self.text.insert_str(at, s);
        self.caret = at + s.len();
        self.anchor = self.caret;
        self.goal_column = None;
        self.dirty = true;
        KeyResult::edited()
    }

    /// Block-marker normalisations that fire when space is typed.
    ///
    /// Returns true if the space was consumed as part of a rewrite.
    fn rewrite_on_space(&mut self) -> bool {
        let ls = text::line_start(&self.text, self.caret);
        let prefix = self.text[ls..self.caret].to_string();
        let (quote_len, _) = block::quote_prefix(&prefix);
        let body = &prefix[quote_len..];
        let indent_len = body.len() - body.trim_start_matches([' ', '\t']).len();
        let token = &body[indent_len..];
        let token_start = ls + quote_len + indent_len;

        // `*` / `+` bullets normalise to `-`.
        if token == "*" || token == "+" {
            self.replace_range(token_start..self.caret, "- ");
            return true;
        }
        // `- []` / `-[]` become a proper task box.
        if token == "-[]" || token == "- []" || token == "*[]" || token == "* []" {
            self.replace_range(token_start..self.caret, "- [ ] ");
            return true;
        }
        if token == "-[x]" || token == "- [x]" {
            self.replace_range(token_start..self.caret, "- [x] ");
            return true;
        }
        false
    }

    fn on_enter(&mut self) -> KeyResult {
        self.push_undo();
        self.delete_selection();

        let ls = text::line_start(&self.text, self.caret);
        let le = text::line_end(&self.text, self.caret);
        let line = self.text[ls..le].to_string();

        // Inside a fenced code block, Enter is just a newline.
        if self.in_code_block() {
            self.insert_plain_newline("");
            return KeyResult::edited();
        }

        // An opening fence with no closing fence gets one, with the caret
        // parked on the blank line between them.
        if block::is_fence(&line) && self.caret == le && !self.fence_is_closed() {
            let fence_char: String = line.trim_start().chars().take(3).collect();
            let insert = format!("\n\n{fence_char}");
            let at = self.caret;
            self.text.insert_str(at, &insert);
            self.caret = at + 1;
            self.anchor = self.caret;
            self.dirty = true;
            return KeyResult::edited();
        }

        let (quote_len, quote_depth) = block::quote_prefix(&line);

        if let Some((marker_len, kind)) = block::list_marker(&line[quote_len..]) {
            let marker_len = quote_len + marker_len;
            let content = line[marker_len..].trim();
            let at_or_past_marker = self.caret >= ls + marker_len;

            // Enter on an empty item ends the list (or outdents a nested one).
            if content.is_empty() && at_or_past_marker {
                let indent = kind.indent();
                if indent >= 2 {
                    let reduced = indent - 2;
                    let rebuilt = format!(
                        "{}{}{}",
                        &line[..quote_len],
                        " ".repeat(reduced),
                        line[quote_len..].trim_start_matches([' ', '\t'])
                    );
                    self.replace_range(ls..le, &rebuilt);
                    self.renumber_around(self.caret);
                } else {
                    self.replace_range(ls..le, &line[..quote_len]);
                }
                return KeyResult::edited();
            }

            // Otherwise continue the list with a fresh marker.
            let next_marker = continuation_marker(&line[quote_len..], &kind);
            let prefix = format!("{}{}", &line[..quote_len], next_marker);
            self.insert_plain_newline(&prefix);
            self.renumber_around(self.caret);
            return KeyResult::edited();
        }

        if quote_depth > 0 {
            let content = line[quote_len..].trim();
            if content.is_empty() {
                self.replace_range(ls..le, "");
                return KeyResult::edited();
            }
            let prefix = line[..quote_len].to_string();
            self.insert_plain_newline(&prefix);
            return KeyResult::edited();
        }

        // Enter ends the block. A single newline is only a soft break — the two
        // lines stay one paragraph — so a blank line goes in as well, unless
        // the caret is already on a blank line or one already follows.
        let on_blank_line = line.trim().is_empty();
        let blank_follows = self.text[self.caret..].starts_with("\n\n");
        if on_blank_line || blank_follows {
            self.insert_plain_newline("");
        } else {
            self.insert_plain_newline("\n");
        }
        KeyResult::edited()
    }

    fn insert_plain_newline(&mut self, prefix: &str) {
        let insert = format!("\n{prefix}");
        let at = self.caret;
        self.text.insert_str(at, &insert);
        self.caret = at + insert.len();
        self.anchor = self.caret;
        self.goal_column = None;
        self.dirty = true;
    }

    fn on_backspace(&mut self, mods: Mods) -> KeyResult {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection();
            return KeyResult::edited();
        }
        if self.caret == 0 {
            return KeyResult::moved();
        }
        self.push_undo();

        if mods.ctrl {
            let start = text::prev_word(&self.text, self.caret);
            self.text.replace_range(start..self.caret, "");
            self.caret = start;
            self.anchor = start;
            self.dirty = true;
            return KeyResult::edited();
        }

        // Backspace at the start of a block's content removes the whole marker,
        // turning the heading/list item back into a paragraph.
        let ls = text::line_start(&self.text, self.caret);
        let le = text::line_end(&self.text, self.caret);
        let line = self.text[ls..le].to_string();
        if let Some(marker_len) = marker_length(&line) {
            if marker_len > 0 && self.caret == ls + marker_len {
                let kind_indent = block::list_marker(&line)
                    .map(|(_, k)| k.indent())
                    .unwrap_or(0);
                if kind_indent >= 2 {
                    // Outdent one level before removing the marker entirely.
                    let trimmed = line.trim_start_matches([' ', '\t']);
                    let rebuilt = format!("{}{}", " ".repeat(kind_indent - 2), trimmed);
                    let delta = line.len() - rebuilt.len();
                    self.text.replace_range(ls..le, &rebuilt);
                    self.caret -= delta;
                    self.anchor = self.caret;
                } else {
                    self.text.replace_range(ls..ls + marker_len, "");
                    self.caret = ls;
                    self.anchor = ls;
                }
                self.dirty = true;
                self.renumber_around(self.caret);
                return KeyResult::edited();
            }
        }

        let prev = text::prev_boundary(&self.text, self.caret);
        self.text.replace_range(prev..self.caret, "");
        self.caret = prev;
        self.anchor = prev;
        self.goal_column = None;
        self.dirty = true;
        KeyResult::edited()
    }

    fn on_delete(&mut self, mods: Mods) -> KeyResult {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection();
            return KeyResult::edited();
        }
        if self.caret >= self.text.len() {
            return KeyResult::moved();
        }
        self.push_undo();
        let end = if mods.ctrl {
            text::next_word(&self.text, self.caret)
        } else {
            text::next_boundary(&self.text, self.caret)
        };
        self.text.replace_range(self.caret..end, "");
        self.dirty = true;
        KeyResult::edited()
    }

    fn on_tab(&mut self, outdent: bool) -> KeyResult {
        let ls = text::line_start(&self.text, self.caret);
        let le = text::line_end(&self.text, self.caret);
        let line = self.text[ls..le].to_string();
        let (quote_len, _) = block::quote_prefix(&line);

        if block::list_marker(&line[quote_len..]).is_some() {
            self.push_undo();
            let body = &line[quote_len..];
            let indent = body.len() - body.trim_start_matches([' ', '\t']).len();
            let new_indent = if outdent {
                indent.saturating_sub(2)
            } else {
                indent + 2
            };
            let rebuilt = format!(
                "{}{}{}",
                &line[..quote_len],
                " ".repeat(new_indent),
                body.trim_start_matches([' ', '\t'])
            );
            let delta = rebuilt.len() as isize - line.len() as isize;
            self.text.replace_range(ls..le, &rebuilt);
            self.caret = (self.caret as isize + delta).max(ls as isize) as usize;
            self.anchor = self.caret;
            self.dirty = true;
            self.renumber_around(self.caret);
            return KeyResult::edited();
        }

        if outdent {
            let body = &line[quote_len..];
            let indent = body.len() - body.trim_start_matches(' ').len();
            if indent == 0 {
                return KeyResult::moved();
            }
            self.push_undo();
            let remove = indent.min(2);
            self.text
                .replace_range(ls + quote_len..ls + quote_len + remove, "");
            self.caret = self.caret.saturating_sub(remove).max(ls);
            self.anchor = self.caret;
            self.dirty = true;
            return KeyResult::edited();
        }

        self.insert_str("  ");
        KeyResult::edited()
    }

    fn on_motion(&mut self, key: Key, mods: Mods) -> KeyResult {
        let had_selection = self.has_selection();
        let sel = self.selection();
        let target = match key {
            Key::Left => {
                if had_selection && !mods.shift {
                    sel.start
                } else if mods.ctrl {
                    text::prev_word(&self.text, self.caret)
                } else {
                    text::prev_boundary(&self.text, self.caret)
                }
            }
            Key::Right => {
                if had_selection && !mods.shift {
                    sel.end
                } else if mods.ctrl {
                    text::next_word(&self.text, self.caret)
                } else {
                    text::next_boundary(&self.text, self.caret)
                }
            }
            Key::Home => {
                if mods.ctrl {
                    0
                } else {
                    text::line_start(&self.text, self.caret)
                }
            }
            Key::End => {
                if mods.ctrl {
                    self.text.len()
                } else {
                    text::line_end(&self.text, self.caret)
                }
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                let step: isize = match key {
                    Key::Up => -1,
                    Key::Down => 1,
                    Key::PageUp => -20,
                    _ => 20,
                };
                let col = self
                    .goal_column
                    .unwrap_or_else(|| text::column_of(&self.text, self.caret));
                let line = self.caret_line() as isize + step;
                let max = text::line_count(&self.text) as isize - 1;
                let line = line.clamp(0, max) as usize;
                let pos = text::offset_of_column(&self.text, line, col).unwrap_or(self.caret);
                self.goal_column = Some(col);
                if mods.shift {
                    self.caret = pos;
                } else {
                    self.caret = pos;
                    self.anchor = pos;
                }
                return KeyResult::moved();
            }
            _ => return KeyResult::ignored(),
        };

        self.goal_column = None;
        self.caret = text::clamp_boundary(&self.text, target);
        if !mods.shift {
            self.anchor = self.caret;
        }
        KeyResult::moved()
    }

    // ---- markdown helpers ------------------------------------------------

    fn wrap_selection(&mut self, delim: &str) {
        self.push_undo();
        let sel = self.selection();
        let inner = self.text[sel.clone()].to_string();
        let wrapped = format!("{delim}{inner}{delim}");
        self.text.replace_range(sel.clone(), &wrapped);
        self.anchor = sel.start + delim.len();
        self.caret = self.anchor + inner.len();
        self.dirty = true;
        self.coalescing = false;
    }

    /// Wrap the selection, or drop in an empty pair with the caret inside.
    fn wrap_or_insert(&mut self, delim: &str) {
        if self.has_selection() {
            self.wrap_selection(delim);
        } else {
            self.push_undo();
            let at = self.caret;
            let pair = format!("{delim}{delim}");
            self.text.insert_str(at, &pair);
            self.caret = at + delim.len();
            self.anchor = self.caret;
            self.dirty = true;
            self.coalescing = false;
        }
    }

    /// Turn the selection into `[selection]()` with the caret in the parens,
    /// or insert an empty link skeleton.
    fn make_link(&mut self) {
        self.push_undo();
        let sel = self.selection();
        let label = self.text[sel.clone()].to_string();
        let replacement = format!("[{label}]()");
        self.text.replace_range(sel.clone(), &replacement);
        self.caret = sel.start + replacement.len() - 1;
        self.anchor = self.caret;
        self.dirty = true;
        self.coalescing = false;
    }

    /// Toggle the task checkbox on `line`, adding one if the item lacks it.
    pub fn toggle_checkbox(&mut self, line: usize) -> bool {
        let Some((ls, le)) = text::line_range(&self.text, line) else {
            return false;
        };
        let src = self.text[ls..le].to_string();
        let (quote_len, _) = block::quote_prefix(&src);
        let Some((marker_len, kind)) = block::list_marker(&src[quote_len..]) else {
            return false;
        };
        self.push_undo();
        let abs_marker_end = ls + quote_len + marker_len;
        match kind.checked() {
            Some(true) => {
                let box_start = abs_marker_end - 4;
                self.text.replace_range(box_start..abs_marker_end, "[ ] ");
            }
            Some(false) => {
                let box_start = abs_marker_end - 4;
                self.text.replace_range(box_start..abs_marker_end, "[x] ");
            }
            None => {
                self.text.insert_str(abs_marker_end, "[ ] ");
                if self.caret >= abs_marker_end {
                    self.caret += 4;
                    self.anchor = self.caret;
                }
            }
        }
        self.dirty = true;
        true
    }

    fn in_code_block(&self) -> bool {
        let line = self.caret_line();
        let mut inside = false;
        for (i, l) in self.text.split('\n').enumerate() {
            if i >= line {
                break;
            }
            if block::is_fence(l) {
                inside = !inside;
            }
        }
        inside && !block::is_fence(text::line_at(&self.text, self.caret))
    }

    /// Is there a closing fence after the caret's line?
    fn fence_is_closed(&self) -> bool {
        let line = self.caret_line();
        self.text
            .split('\n')
            .skip(line + 1)
            .any(|l| block::is_fence(l))
    }

    /// Renumber the ordered-list run containing `pos` so it counts 1, 2, 3…
    /// from whatever the first item's number is.
    fn renumber_around(&mut self, pos: usize) {
        let line = text::line_index(&self.text, pos);
        let lines: Vec<String> = self.text.split('\n').map(|s| s.to_string()).collect();
        let Some(current) = lines.get(line) else {
            return;
        };
        let (quote_len, _) = block::quote_prefix(current);
        let Some((_, kind)) = block::list_marker(&current[quote_len..]) else {
            return;
        };
        let BlockKind::Numbered { indent, .. } = kind else {
            return;
        };

        // Walk up to the first item of this run at the same indent.
        let mut first = line;
        while first > 0 {
            let prev = &lines[first - 1];
            let (q, _) = block::quote_prefix(prev);
            match block::list_marker(&prev[q..]) {
                Some((_, BlockKind::Numbered { indent: i, .. })) if i == indent => first -= 1,
                _ => break,
            }
        }

        let start_number = {
            let l = &lines[first];
            let (q, _) = block::quote_prefix(l);
            match block::list_marker(&l[q..]) {
                Some((_, BlockKind::Numbered { number, .. })) => number,
                _ => 1,
            }
        };

        let mut out = lines.clone();
        let mut n = start_number;
        let mut i = first;
        let mut delta_before_caret: isize = 0;
        while i < out.len() {
            let l = out[i].clone();
            let (q, _) = block::quote_prefix(&l);
            let Some((_, BlockKind::Numbered { indent: ind, number, .. })) =
                block::list_marker(&l[q..])
            else {
                break;
            };
            if ind != indent {
                // Skip nested items without renumbering them here.
                if ind > indent {
                    i += 1;
                    continue;
                }
                break;
            }
            if number != n {
                let body = &l[q..];
                let ws = body.len() - body.trim_start_matches([' ', '\t']).len();
                let digits = body[ws..].chars().take_while(|c| c.is_ascii_digit()).count();
                let old_len = l.len();
                let rebuilt = format!(
                    "{}{}{}{}",
                    &l[..q],
                    &body[..ws],
                    n,
                    &body[ws + digits..]
                );
                if i < line || (i == line) {
                    delta_before_caret += rebuilt.len() as isize - old_len as isize;
                }
                out[i] = rebuilt;
            }
            n += 1;
            i += 1;
        }

        let rebuilt_text = out.join("\n");
        if rebuilt_text != self.text {
            self.text = rebuilt_text;
            let shifted = (self.caret as isize + delta_before_caret).max(0) as usize;
            self.caret = text::clamp_boundary(&self.text, shifted.min(self.text.len()));
            self.anchor = self.caret;
            self.dirty = true;
        }
    }
}

/// The delimiter a bare character wraps a selection in.
fn wrap_delimiter(c: char) -> Option<&'static str> {
    match c {
        '*' => Some("*"),
        '_' => Some("_"),
        '`' => Some("`"),
        '~' => Some("~~"),
        _ => None,
    }
}

/// Total marker length for any block type, used by the "backspace unstyles" rule.
fn marker_length(line: &str) -> Option<usize> {
    let (quote_len, depth) = block::quote_prefix(line);
    let body = &line[quote_len..];
    if let Some((len, _)) = block::list_marker(body) {
        return Some(quote_len + len);
    }
    let ws = body.len() - body.trim_start_matches(' ').len();
    if ws <= 3 {
        let hashes = body[ws..].chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && body[ws + hashes..].starts_with(' ') {
            return Some(quote_len + ws + hashes + 1);
        }
    }
    if depth > 0 {
        return Some(quote_len);
    }
    None
}

/// The marker a new line inherits when Enter continues a list.
fn continuation_marker(body: &str, kind: &BlockKind) -> String {
    let indent = " ".repeat(kind.indent());
    match kind {
        BlockKind::Bullet { checked, .. } => {
            let bullet = body
                .trim_start_matches([' ', '\t'])
                .chars()
                .next()
                .unwrap_or('-');
            match checked {
                Some(_) => format!("{indent}{bullet} [ ] "),
                None => format!("{indent}{bullet} "),
            }
        }
        BlockKind::Numbered { number, checked, .. } => {
            let sep = if body.contains(") ") { ')' } else { '.' };
            match checked {
                Some(_) => format!("{indent}{}{sep} [ ] ", number + 1),
                None => format!("{indent}{}{sep} ", number + 1),
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_round_trip() {
        for theme in [Theme::Light, Theme::Dark, Theme::Auto] {
            assert_eq!(Theme::from_str(theme.as_str()), theme);
        }
    }

    #[test]
    fn theme_cycles_auto_light_dark() {
        let mut t = Theme::Auto;
        t = t.cycled();
        assert_eq!(t, Theme::Light);
        t = t.cycled();
        assert_eq!(t, Theme::Dark);
        t = t.cycled();
        assert_eq!(t, Theme::Auto, "cycling must return to where it started");
    }

    #[test]
    fn auto_follows_the_system_and_the_others_do_not() {
        // (theme, system is dark) -> draws dark
        let cases = [
            (Theme::Light, true, false),
            (Theme::Light, false, false),
            (Theme::Dark, true, true),
            (Theme::Dark, false, true),
            (Theme::Auto, true, true),
            (Theme::Auto, false, false),
        ];
        for (theme, system_dark, want) in cases {
            assert_eq!(
                theme.is_dark(system_dark),
                want,
                "{theme:?} with system_dark={system_dark}"
            );
        }
    }

    #[test]
    fn ctrl_t_cycles_the_theme_without_touching_the_document() {
        let mut e = Editor::with_text("# Notes");
        assert_eq!(e.theme, Theme::Auto);

        let result = e.handle_key(Key::Char('t'), Mods::CTRL);
        assert!(result.handled);
        assert!(!result.changed, "changing theme is not a document edit");
        assert_eq!(e.theme, Theme::Light);
        assert_eq!(e.text(), "# Notes");

        e.handle_key(Key::Char('t'), Mods::CTRL);
        assert_eq!(e.theme, Theme::Dark);
        e.handle_key(Key::Char('t'), Mods::CTRL);
        assert_eq!(e.theme, Theme::Auto);
    }

    #[test]
    fn a_plain_t_is_typed_rather_than_cycling_the_theme() {
        let mut e = Editor::new();
        e.handle_key(Key::Char('t'), Mods::NONE);
        assert_eq!(e.text(), "t");
        assert_eq!(e.theme, Theme::Auto);
    }

    #[test]
    fn the_theme_is_not_undoable() {
        // Undo restores document snapshots; a view preference is not part of
        // the document and must survive an undo.
        let mut e = Editor::new();
        for c in "hello".chars() {
            e.handle_key(Key::Char(c), Mods::NONE);
        }
        e.handle_key(Key::Char('t'), Mods::CTRL);
        assert_eq!(e.theme, Theme::Light);

        assert!(e.undo());
        assert_eq!(e.text(), "");
        assert_eq!(e.theme, Theme::Light, "undo must not revert the theme");
    }
}
