//! Render the editor GUI headlessly and write PNGs.
//!
//! The plugin's UI normally only exists inside a window, which makes it
//! impossible to check without a human looking at a screen. This renders the
//! *same* drawing code through a real GPU rasteriser with no window involved,
//! so the result can be inspected, diffed and kept honest.
//!
//! ```text
//! cargo run -p notepad-plugin --example snapshot
//! ```
//!
//! Writes `target/snapshots/{light,dark}.png` and prints, for each, the colour
//! of the top-left pixel and of the text — the two numbers that say whether a
//! theme actually took effect.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use notepad_core::{Editor, Theme};

const SAMPLE: &str = "\
# Notepad

Markdown that **converts** as you *write* it, with `inline code`,
~~strikethrough~~ and [links](https://www.anthropic.com).

- a bullet list
- [x] a ticked task
- [ ] an unticked one

1. numbered
2. items

> A blockquote.
";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new("target/snapshots");
    std::fs::create_dir_all(out)?;

    for (name, theme) in [("light", Theme::Light), ("dark", Theme::Dark)] {
        let editor = Arc::new(Mutex::new(Editor::with_text(SAMPLE)));
        if let Ok(mut e) = editor.lock() {
            e.theme = theme;
            e.set_caret(0);
        }

        // `system_dark` is irrelevant for an explicit theme, but pass something
        // deterministic so snapshots never depend on the machine rendering them.
        let mut state = notepad_plugin::gui::TestGui::new(editor, true);

        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 620.0))
            .wgpu()
            .build_ui(move |ui| {
                notepad_plugin::gui::draw_frame_for_test(ui, &mut state);
            });

        // Several passes: the first applies the theme, later ones draw with it
        // and let the scroll area settle.
        harness.run_steps(3);

        let image = harness.render()?;
        let path = out.join(format!("{name}.png"));
        image.save(&path)?;

        let (background, darkest, brightest) = measure(&image);
        println!(
            "{name:>5}: {}  background {background:.0}, \
             darkest {darkest:.0}, brightest {brightest:.0}  (0 = black, 255 = white)",
            path.display(),
        );
    }

    Ok(())
}

/// Background brightness (the most common opaque colour) plus the darkest and
/// brightest pixels.
///
/// Transparent pixels are skipped: the rasteriser leaves a transparent margin
/// around the drawn area, and sampling it reports black regardless of theme.
fn measure(image: &image::RgbaImage) -> (f32, f32, f32) {
    let mut counts: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    let mut darkest = 255.0f32;
    let mut brightest = 0.0f32;

    for p in image.pixels() {
        if p[3] < 255 {
            continue;
        }
        *counts.entry([p[0], p[1], p[2]]).or_default() += 1;
        let brightness = (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
        darkest = darkest.min(brightness);
        brightest = brightest.max(brightness);
    }

    let background = counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(c, _)| (c[0] as f32 + c[1] as f32 + c[2] as f32) / 3.0)
        .unwrap_or(0.0);

    (background, darkest, brightest)
}
