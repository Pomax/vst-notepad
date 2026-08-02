//! The scenarios themselves — ordinary markdown writing, performed the way a
//! person performs it: one keystroke at a time, through the plugin's editor
//! window.

use notepad_core::{Key, Mods};

use crate::scenario::{scenario, Scenario, Step::*};

pub fn all() -> Vec<Scenario> {
    vec![
        // -------------------------------------------------------------- text
        scenario(
            "headings and paragraphs",
            vec![
                Type("# Shopping list\nEverything I need this week."),
                ExpectSource("# Shopping list\nEverything I need this week."),
                ExpectRendered("Shopping list\nEverything I need this week."),
            ],
        ),
        scenario(
            "every heading level",
            vec![
                Type("# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six"),
                ExpectRendered("One\nTwo\nThree\nFour\nFive\nSix"),
            ],
        ),
        // ------------------------------------------------------------- lists
        scenario(
            "a star bullet becomes a dash as you type",
            vec![
                Type("* milk"),
                ExpectSource("- milk"),
                ExpectRendered("milk"),
            ],
        ),
        scenario(
            "bullet lists continue themselves",
            vec![
                Type("* milk\neggs\nbread"),
                ExpectSource("- milk\n- eggs\n- bread"),
                ExpectRendered("milk\neggs\nbread"),
            ],
        ),
        scenario(
            "enter on an empty bullet ends the list",
            vec![
                Type("- one\n\nback to prose"),
                ExpectSource("- one\nback to prose"),
            ],
        ),
        scenario(
            "numbered lists renumber as they grow",
            vec![
                Type("1. first\nsecond\nthird"),
                ExpectSource("1. first\n2. second\n3. third"),
                ExpectRendered("first\nsecond\nthird"),
            ],
        ),
        scenario(
            "tab indents a list item",
            vec![
                Type("- top\n"),
                Press(Key::Tab, Mods::NONE),
                Type("nested"),
                ExpectSource("- top\n  - nested"),
            ],
        ),
        // --------------------------------------------------------- checkboxes
        scenario(
            "typing a checkbox produces a task item",
            vec![
                Type("-[] Buy milk"),
                ExpectSource("- [ ] Buy milk"),
                ExpectRendered("Buy milk"),
            ],
        ),
        scenario(
            "a pre-ticked checkbox is recognised",
            vec![
                Type("-[x] Ship it"),
                ExpectSource("- [x] Ship it"),
                ExpectRendered("Ship it"),
            ],
        ),
        // ------------------------------------------------------ styled text
        scenario(
            "bold and italic typed by hand",
            vec![
                Type("**bold** and *italic* and ~~struck~~"),
                ExpectSource("**bold** and *italic* and ~~struck~~"),
                ExpectRendered("bold and italic and struck"),
            ],
        ),
        scenario(
            "ctrl+B wraps the selection in bold",
            vec![
                Type("make this bold"),
                Press(Key::Char('a'), Mods::CTRL),
                Press(Key::Char('b'), Mods::CTRL),
                ExpectSource("**make this bold**"),
                ExpectRendered("make this bold"),
            ],
        ),
        scenario(
            "underscores inside a word stay literal",
            vec![
                Type("call snake_case_name here"),
                ExpectRendered("call snake_case_name here"),
            ],
        ),
        // ------------------------------------------------------------- links
        scenario(
            "ctrl+K turns a selection into a link, ready for the URL",
            vec![
                Type("Anthropic"),
                Press(Key::Char('a'), Mods::CTRL),
                Press(Key::Char('k'), Mods::CTRL),
                ExpectSource("[Anthropic]()"),
                Type("https://www.anthropic.com"),
                ExpectSource("[Anthropic](https://www.anthropic.com)"),
                ExpectRendered("Anthropic"),
            ],
        ),
        // -------------------------------------------------------------- code
        scenario(
            "inline code hides its backticks",
            vec![
                Type("run `cargo test` now"),
                ExpectSource("run `cargo test` now"),
                ExpectRendered("run cargo test now"),
            ],
        ),
        scenario(
            "a fence closes itself and holds raw markdown",
            vec![
                Type("```rust\nlet x = *y;"),
                ExpectSource("```rust\nlet x = *y;\n```"),
            ],
        ),
        // ------------------------------------------------------- blockquotes
        scenario(
            "blockquotes continue on the next line",
            vec![
                Type("> quoted\nstill quoted"),
                ExpectSource("> quoted\n> still quoted"),
                ExpectRendered("quoted\nstill quoted"),
            ],
        ),
        // -------------------------------------------------------------- undo
        scenario(
            "undo and redo a burst of typing",
            vec![
                Type("hello"),
                ExpectSource("hello"),
                Press(Key::Char('z'), Mods::CTRL),
                ExpectSource(""),
                Press(Key::Char('z'), Mods::CTRL_SHIFT),
                ExpectSource("hello"),
            ],
        ),
        // ------------------------------------------------------------- modes
        scenario(
            "the view mode toggles and is remembered across a session",
            vec![
                Type("# Notes"),
                ExpectMode("wysiwyg"),
                Press(Key::Char('/'), Mods::CTRL),
                ExpectMode("raw"),
                ReopenProject,
                ExpectMode("raw"),
                ExpectSource("# Notes"),
            ],
        ),
        // ------------------------------------------------------------ themes
        scenario(
            "the theme starts on auto and cycles with Ctrl+T",
            vec![
                ExpectTheme("auto"),
                Press(Key::Char('t'), Mods::CTRL),
                ExpectTheme("light"),
                Press(Key::Char('t'), Mods::CTRL),
                ExpectTheme("dark"),
                Press(Key::Char('t'), Mods::CTRL),
                ExpectTheme("auto"),
            ],
        ),
        scenario(
            "the chosen theme is remembered across a session",
            vec![
                Type("# Notes"),
                Press(Key::Char('t'), Mods::CTRL),
                Press(Key::Char('t'), Mods::CTRL),
                ExpectTheme("dark"),
                ReopenProject,
                ExpectTheme("dark"),
                ExpectSource("# Notes"),
            ],
        ),
        scenario(
            "auto is remembered as auto, not as whatever it resolved to",
            vec![
                ExpectTheme("auto"),
                ReopenProject,
                ExpectTheme("auto"),
            ],
        ),
        scenario(
            "changing the theme does not touch the document",
            vec![
                Type("- a list item"),
                Press(Key::Char('t'), Mods::CTRL),
                ExpectSource("- a list item"),
                ExpectTheme("light"),
            ],
        ),
        // ------------------------------------------------------------ window
        scenario(
            "the window is resizable and the size is remembered",
            vec![
                ExpectSize(900, 620),
                Resize(1024, 768),
                ExpectSize(1024, 768),
                ReopenProject,
                ExpectSize(1024, 768),
            ],
        ),
        scenario(
            "a too-small window is clamped to a usable size",
            vec![ExpectClampedSize {
                proposed: (100, 50),
                accepted: (320, 200),
            }],
        ),
        // ------------------------------------------------------------- state
        scenario(
            "a whole document survives closing and reopening the project",
            vec![
                Type("# Meeting notes\n\nDiscussed:\n"),
                Type("* the plan\nthe budget\n\n"),
                Type("> quote worth keeping"),
                // The second Enter ends the list by consuming the empty bullet,
                // so the quote lands on that line rather than after a blank one.
                ExpectSource(
                    "# Meeting notes\n\nDiscussed:\n- the plan\n- the budget\n> quote worth keeping",
                ),
                ReopenProject,
                // The second Enter ends the list by consuming the empty bullet,
                // so the quote lands on that line rather than after a blank one.
                ExpectSource(
                    "# Meeting notes\n\nDiscussed:\n- the plan\n- the budget\n> quote worth keeping",
                ),
                ExpectFile(None),
            ],
        ),
        scenario(
            "unicode survives the whole round trip",
            vec![
                Type("# 見出し\n日本語の**太字**とemoji ✓"),
                ExpectSource("# 見出し\n日本語の**太字**とemoji ✓"),
                ExpectRendered("見出し\n日本語の太字とemoji ✓"),
                ReopenProject,
                ExpectSource("# 見出し\n日本語の**太字**とemoji ✓"),
            ],
        ),
        // -------------------------------------------------------- plugin type
        scenario(
            "the plugin is an effect, and says so everywhere a host looks",
            vec![ExpectEffectNotInstrument],
        ),
        // ------------------------------------------------------------- audio
        scenario(
            "audio passes through the plugin untouched",
            vec![ExpectAudioPassThrough],
        ),
        scenario(
            "with no input bus the output is silenced, not left as garbage",
            vec![ExpectSilenceWithNoInput],
        ),
        scenario(
            "a mono track is accepted and reported honestly",
            vec![ExpectMonoIsHonoured, ExpectAudioPassThrough],
        ),
        scenario(
            "audio still passes through while the document is being edited",
            vec![
                Type("# Notes taken while the track plays"),
                ExpectAudioPassThrough,
                ExpectSource("# Notes taken while the track plays"),
            ],
        ),
        // ------------------------------------------------- hostile state
        // A DAW can hand back anything: a truncated chunk, a project written by
        // a newer build, a file someone edited by hand. None of it may crash
        // the plugin, and none of it may lose the words.
        scenario(
            "state that is not JSON at all is kept as note text",
            vec![
                LoadRawState(b"just some words, not json"),
                ExpectSource("just some words, not json"),
            ],
        ),
        scenario(
            "empty state opens an empty document",
            vec![LoadRawState(b""), ExpectSource(""), ExpectCaret(0)],
        ),
        scenario(
            "a caret past the end of the notes is pulled back",
            vec![
                LoadRawState(br#"{"notes":"short","caret":99999}"#),
                ExpectSource("short"),
                ExpectCaret(5),
            ],
        ),
        scenario(
            "a caret inside a multi-byte character does not crash",
            vec![
                // Byte 1 is inside the two-byte "é" (\u{e9} = 0xC3 0xA9).
                LoadRawState(b"{\"notes\":\"\xc3\xa9x\",\"caret\":1}"),
                ExpectSource("éx"),
                ExpectCaret(0),
                Type("!"),
                ExpectSource("!éx"),
            ],
        ),
        scenario(
            "an absurd window size from a corrupt project is clamped",
            vec![
                LoadRawState(br#"{"notes":"x","width":2147483647,"height":2147483647}"#),
                ExpectSize(8192, 8192),
            ],
        ),
        scenario(
            "a negative window size is clamped up to something usable",
            vec![
                LoadRawState(br#"{"notes":"x","width":-5,"height":-5}"#),
                ExpectSize(320, 200),
            ],
        ),
        // ---------------------------------------------------------- movement
        scenario(
            "arrow keys and backspace edit in the middle of a line",
            vec![
                Type("helo"),
                Press(Key::Left, Mods::NONE),
                Type("l"),
                ExpectSource("hello"),
                Press(Key::Home, Mods::NONE),
                ExpectCaret(0),
                Press(Key::End, Mods::NONE),
                ExpectCaret(5),
                Press(Key::Backspace, Mods::NONE),
                ExpectSource("hell"),
            ],
        ),
    ]
}
