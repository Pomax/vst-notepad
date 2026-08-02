//! Render a multi-script sample to see which scripts the system fonts cover.
//!
//! ```text
//! cargo run -p notepad-plugin --features snapshots --example scripts
//! ```

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use notepad_core::{Editor, Theme};

const SAMPLE: &str = "\
# Script coverage

Latin: The quick brown fox
Greek: \u{0393}\u{03b5}\u{03b9}\u{03ac} \u{03c3}\u{03bf}\u{03c5}
Cyrillic: \u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}
Hebrew: \u{05e9}\u{05dc}\u{05d5}\u{05dd}
Farsi: \u{0633}\u{0644}\u{0627}\u{0645}
Devanagari: \u{0928}\u{092e}\u{0938}\u{094d}\u{0924}\u{0947}
Thai: \u{0e2a}\u{0e27}\u{0e31}\u{0e2a}\u{0e14}\u{0e35}
Japanese: \u{3053}\u{3093}\u{306b}\u{3061}\u{306f}
Korean: \u{c548}\u{b155}\u{d558}\u{c138}\u{c694}
Chinese: \u{4f60}\u{597d}\u{4e16}\u{754c}
";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new("target/snapshots");
    std::fs::create_dir_all(out)?;

    // Start with Latin only — the base font is all that is loaded.
    let editor = Arc::new(Mutex::new(Editor::with_text(
        "# Script coverage\n\nLatin: The quick brown fox\n",
    )));
    if let Ok(mut e) = editor.lock() {
        e.theme = Theme::Light;
        e.set_caret(0);
    }

    let typed_into = Arc::clone(&editor);
    let mut state = notepad_plugin::gui::TestGui::new(editor, false);

    let mut harness = Harness::builder()
        .with_size(egui::vec2(700.0, 420.0))
        .wgpu()
        .build_ui(move |ui| notepad_plugin::gui::draw_frame_for_test(ui, &mut state));

    harness.run_steps(3);
    harness.render()?.save(out.join("scripts-latin.png"))?;

    // Now text in other scripts arrives, as if typed.
    if let Ok(mut e) = typed_into.lock() {
        e.set_text(SAMPLE);
        e.set_caret(0);
    }

    harness.run_steps(4);
    harness.render()?.save(out.join("scripts.png"))?;

    println!("wrote {}", out.join("scripts.png").display());
    Ok(())
}
