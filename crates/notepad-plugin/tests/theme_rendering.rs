//! Rendering tests for the themes.
//!
//! These render the real GUI through a real rasteriser and look at the pixels.
//! They exist because the light theme once "worked" by every other measure —
//! the state was right, the visuals were right — while the window stayed black,
//! since nothing painted the background. Only pixels catch that.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use notepad_core::{Editor, Theme};

const SAMPLE: &str = "# Heading\n\nSome **bold** text and `code`.\n\n> A quote\n";

/// Render the editor at a given theme and hand back the image.
fn render(theme: Theme, system_dark: bool) -> image::RgbaImage {
    let editor = Arc::new(Mutex::new(Editor::with_text(SAMPLE)));
    if let Ok(mut e) = editor.lock() {
        e.theme = theme;
        e.set_caret(0);
    }
    let mut state = notepad_plugin::gui::TestGui::new(editor, system_dark);

    let mut harness = Harness::builder()
        .with_size(egui::vec2(700.0, 400.0))
        .wgpu()
        .build_ui(move |ui| notepad_plugin::gui::draw_frame_for_test(ui, &mut state));

    harness.run_steps(3);
    harness.render().expect("rendering failed")
}

/// Average brightness of the fully opaque pixels, 0 (black) to 255 (white).
///
/// Transparent margins around the drawn area are skipped so they cannot drag
/// the average toward whatever the rasteriser left there.
fn mean_brightness(image: &image::RgbaImage) -> f32 {
    let mut total = 0f64;
    let mut count = 0u32;
    for p in image.pixels() {
        if p[3] < 255 {
            continue;
        }
        total += (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
        count += 1;
    }
    assert!(count > 0, "image was entirely transparent");
    (total / count as f64) as f32
}

/// Brightness of the most common colour — the background, since it covers most
/// of the window.
fn background_brightness(image: &image::RgbaImage) -> f32 {
    let mut counts: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    for p in image.pixels() {
        if p[3] == 255 {
            *counts.entry([p[0], p[1], p[2]]).or_default() += 1;
        }
    }
    let (colour, _) = counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .expect("image was entirely transparent");
    (colour[0] as f32 + colour[1] as f32 + colour[2] as f32) / 3.0
}

#[test]
fn light_theme_has_a_light_background() {
    let image = render(Theme::Light, true);
    let background = background_brightness(&image);
    assert!(
        background > 200.0,
        "light theme background should be near-white, was {background}"
    );
}

#[test]
fn dark_theme_has_a_dark_background() {
    let image = render(Theme::Dark, false);
    let background = background_brightness(&image);
    assert!(
        background < 80.0,
        "dark theme background should be near-black, was {background}"
    );
}

#[test]
fn light_theme_draws_dark_text_on_it() {
    let image = render(Theme::Light, true);
    let background = background_brightness(&image);
    // Something substantially darker than the background must exist, or the
    // text is invisible.
    let darkest = image
        .pixels()
        .filter(|p| p[3] == 255)
        .map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3)
        .min()
        .expect("image was entirely transparent") as f32;
    assert!(
        background - darkest > 120.0,
        "light theme needs dark text: background {background}, darkest pixel {darkest}"
    );
}

#[test]
fn dark_theme_draws_light_text_on_it() {
    let image = render(Theme::Dark, false);
    let background = background_brightness(&image);
    let brightest = image
        .pixels()
        .filter(|p| p[3] == 255)
        .map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3)
        .max()
        .expect("image was entirely transparent") as f32;
    assert!(
        brightest - background > 120.0,
        "dark theme needs light text: background {background}, brightest pixel {brightest}"
    );
}

#[test]
fn the_two_themes_actually_differ() {
    let light = mean_brightness(&render(Theme::Light, true));
    let dark = mean_brightness(&render(Theme::Dark, false));
    assert!(
        light - dark > 100.0,
        "the themes should look nothing alike: light {light}, dark {dark}"
    );
}

#[test]
fn auto_follows_the_system_setting() {
    let when_system_is_dark = background_brightness(&render(Theme::Auto, true));
    let when_system_is_light = background_brightness(&render(Theme::Auto, false));
    assert!(
        when_system_is_dark < 80.0,
        "auto on a dark system should render dark, was {when_system_is_dark}"
    );
    assert!(
        when_system_is_light > 200.0,
        "auto on a light system should render light, was {when_system_is_light}"
    );
}
