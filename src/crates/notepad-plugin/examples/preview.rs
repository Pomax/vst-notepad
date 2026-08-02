//! Open the plugin's editor as a standalone window.
//!
//! The plugin GUI normally lives inside a window supplied by a DAW, which makes
//! it awkward to look at while developing. This runs the identical drawing and
//! input code in a window of its own:
//!
//! ```text
//! cargo run -p notepad-plugin --example preview
//! ```

use std::sync::{Arc, Mutex};

use notepad_core::{Editor, Theme};

const SAMPLE: &str = "\
# Notepad

A markdown editor that lives in your DAW. Type as you would in **Typora** —
the markdown *converts as you write it*.

## What works

- headings, at every level
- bullet lists, which continue themselves
- [x] task lists you can tick
- [ ] ...and untick
- ~~struck-through~~ text and `inline code`

1. numbered lists
2. renumber themselves
3. as they grow

> Blockquotes continue on the next line, too.

```rust
// Fenced code keeps its markdown literal.
let heading = \"# not a heading\";
```

Links look like [this](https://www.anthropic.com).
";

fn main() {
    // Optional theme argument, so a screenshot tool can ask for a specific one
    // instead of driving Ctrl+T through a window it cannot type into.
    let theme = std::env::args()
        .nth(1)
        .map(|a| Theme::from_str(&a))
        .unwrap_or(Theme::Auto);

    let editor = Arc::new(Mutex::new(Editor::with_text(SAMPLE)));
    if let Ok(mut e) = editor.lock() {
        e.theme = theme;
        e.set_caret(0);
    }
    notepad_plugin::gui::open_blocking(editor, 900, 620);
}
