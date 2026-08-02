//! Randomised property tests.
//!
//! Every other test in this project is a case someone thought of. These are
//! not: they hammer the editor with arbitrary key sequences and arbitrary
//! documents, and assert the invariants that must hold no matter what.
//!
//! The invariants are chosen to be the ones whose violation would show up as a
//! crash inside a DAW — byte offsets that are not char boundaries, offsets past
//! the end of the buffer, spans that do not tile the source.

use notepad_core::{parse_document, Editor, Key, Mods, PluginState, Theme, ViewMode};

/// xorshift64*, so runs are reproducible without pulling in a rand crate.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Characters chosen to provoke the parser: markdown punctuation, multi-byte
/// text, a combining mark, an emoji, and ordinary letters.
const CHARS: &[char] = &[
    'a', 'b', 'x', ' ', ' ', '#', '*', '_', '-', '+', '`', '~', '[', ']', '(', ')', '!', '>', '.',
    '1', '\\', '|', ':', '/', 'é', '漢', '✓', '👍', '\u{0301}', '\t',
];

const KEYS: &[Key] = &[
    Key::Enter,
    Key::Backspace,
    Key::Delete,
    Key::Tab,
    Key::Left,
    Key::Right,
    Key::Up,
    Key::Down,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
    Key::Escape,
];

const SHORTCUTS: &[char] = &['b', 'i', 'd', 'e', 'k', 'a', 'z', 'y', '/', 't'];

const MODS: &[Mods] = &[Mods::NONE, Mods::SHIFT, Mods::CTRL, Mods::CTRL_SHIFT];

fn random_key(rng: &mut Rng) -> (Key, Mods) {
    match rng.below(10) {
        // Mostly typing, which is what a person mostly does.
        0..=5 => (Key::Char(CHARS[rng.below(CHARS.len())]), Mods::NONE),
        6..=8 => (KEYS[rng.below(KEYS.len())], MODS[rng.below(MODS.len())]),
        _ => (
            Key::Char(SHORTCUTS[rng.below(SHORTCUTS.len())]),
            if rng.below(2) == 0 {
                Mods::CTRL
            } else {
                Mods::CTRL_SHIFT
            },
        ),
    }
}

/// Everything that must be true of an editor after any operation.
fn check_invariants(editor: &Editor, context: &str) {
    let text = editor.text();

    assert!(
        editor.caret() <= text.len(),
        "{context}: caret {} past end {}",
        editor.caret(),
        text.len()
    );
    assert!(
        text.is_char_boundary(editor.caret()),
        "{context}: caret {} is not a char boundary in {text:?}",
        editor.caret()
    );
    assert!(
        editor.anchor() <= text.len(),
        "{context}: anchor {} past end {}",
        editor.anchor(),
        text.len()
    );
    assert!(
        text.is_char_boundary(editor.anchor()),
        "{context}: anchor {} is not a char boundary in {text:?}",
        editor.anchor()
    );

    let selection = editor.selection();
    assert!(
        selection.start <= selection.end && selection.end <= text.len(),
        "{context}: selection {selection:?} is not a valid range of {}",
        text.len()
    );

    assert!(
        editor.width >= notepad_core::MIN_WIDTH && editor.width <= notepad_core::MAX_WIDTH,
        "{context}: width {} out of range",
        editor.width
    );

    // Laying the document out must never panic, and the spans must tile the
    // source exactly — a gap or an overlap means some text would be lost or
    // duplicated on screen.
    let doc = editor.render();
    for block in &doc.blocks {
        assert!(
            block.range.end <= text.len(),
            "{context}: block range {:?} past end",
            block.range
        );
        let mut at = block.marker.end;
        for span in &block.spans {
            assert_eq!(
                span.range.start, at,
                "{context}: span gap/overlap in {text:?}"
            );
            assert!(
                text.is_char_boundary(span.range.start)
                    && text.is_char_boundary(span.range.end),
                "{context}: span {:?} not on char boundaries in {text:?}",
                span.range
            );
            at = span.range.end;
        }
        assert_eq!(at, block.range.end, "{context}: spans do not cover the line");
        let _ = block.visible_text(text);
    }
    let _ = editor.rendered_text();
}

#[test]
fn random_typing_never_breaks_the_editor() {
    for seed in 1..=200u64 {
        let mut editor = Editor::new();
        let mut rng = Rng::new(seed);

        for step in 0..300 {
            let (key, mods) = random_key(&mut rng);
            editor.handle_key(key, mods);
            check_invariants(&editor, &format!("seed {seed} step {step} key {key:?} {mods:?}"));
        }
    }
}

/// Hammer the public API directly, not just the keyboard.
///
/// `select`, `insert_str`, `set_text`, `set_caret` and `toggle_checkbox` are all
/// callable by the GUI — a click lands in `set_caret`, a checkbox click in
/// `toggle_checkbox` — and each takes an offset or a line number that can be
/// anything, including ones that are not char boundaries.
#[test]
fn random_api_calls_never_break_the_editor() {
    for seed in 1..=200u64 {
        let mut editor = Editor::new();
        let mut rng = Rng::new(seed.wrapping_mul(7919));

        for step in 0..200 {
            let len = editor.text().len().max(1);
            match rng.below(9) {
                0 => {
                    let text: String = (0..rng.below(30))
                        .map(|_| CHARS[rng.below(CHARS.len())])
                        .collect();
                    editor.set_text(text);
                }
                // Deliberately arbitrary offsets, including mid-character ones.
                1 => editor.set_caret(rng.below(len + 4)),
                2 => {
                    let a = rng.below(len + 4);
                    let b = rng.below(len + 4);
                    editor.select(a.min(b)..a.max(b));
                }
                3 => {
                    let text: String = (0..rng.below(6))
                        .map(|_| CHARS[rng.below(CHARS.len())])
                        .collect();
                    editor.insert_str(&text);
                }
                4 => {
                    // Line numbers past the end included on purpose.
                    editor.toggle_checkbox(rng.below(12));
                }
                5 => editor.select_all(),
                6 => {
                    editor.undo();
                }
                7 => {
                    editor.redo();
                }
                _ => {
                    let (key, mods) = random_key(&mut rng);
                    editor.handle_key(key, mods);
                }
            }
            check_invariants(&editor, &format!("seed {seed} step {step}"));
        }
    }
}

#[test]
fn random_documents_survive_a_state_round_trip() {
    for seed in 1..=200u64 {
        let mut editor = Editor::new();
        let mut rng = Rng::new(seed);
        for _ in 0..120 {
            let (key, mods) = random_key(&mut rng);
            editor.handle_key(key, mods);
        }
        editor.mode = if rng.below(2) == 0 {
            ViewMode::Raw
        } else {
            ViewMode::Wysiwyg
        };
        editor.theme = [Theme::Light, Theme::Dark, Theme::Auto][rng.below(3)];

        let bytes = editor.state_bytes();
        let mut restored = Editor::new();
        restored.load_state_bytes(&bytes);

        assert_eq!(restored.text(), editor.text(), "seed {seed}: notes changed");
        assert_eq!(restored.caret(), editor.caret(), "seed {seed}: caret moved");
        assert_eq!(restored.mode, editor.mode, "seed {seed}: mode changed");
        assert_eq!(restored.theme, editor.theme, "seed {seed}: theme changed");
        check_invariants(&restored, &format!("seed {seed} after restore"));
    }
}

#[test]
fn arbitrary_bytes_as_state_never_panic() {
    let mut rng = Rng::new(0xDEAD_BEEF);
    for _ in 0..500 {
        let len = rng.below(64);
        let bytes: Vec<u8> = (0..len).map(|_| rng.below(256) as u8).collect();

        // Must not panic, whatever the bytes are.
        let state = PluginState::from_bytes(&bytes);
        let mut editor = Editor::new();
        editor.load_state(&state);
        check_invariants(&editor, "random state bytes");
    }
}

#[test]
fn arbitrary_text_parses_without_panicking() {
    let mut rng = Rng::new(12345);
    for _ in 0..2000 {
        let len = rng.below(40);
        let text: String = (0..len).map(|_| CHARS[rng.below(CHARS.len())]).collect();

        // Every caret position, including none, on a boundary or not requested.
        let doc = parse_document(&text, None);
        for block in &doc.blocks {
            let _ = block.visible_text(&text);
        }
        for caret in 0..=text.len() {
            if text.is_char_boundary(caret) {
                let doc = parse_document(&text, Some(caret));
                for block in &doc.blocks {
                    let _ = block.visible_text(&text);
                }
            }
        }
    }
}
