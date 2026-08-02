//! System fonts, loaded at runtime.
//!
//! Nothing is bundled. The editor starts with the operating system's UI font,
//! which covers Latin, Greek, Cyrillic, Hebrew and Arabic. When text arrives
//! that needs something else, the font for that script is loaded from the
//! system and added as a fallback.

use std::collections::HashSet;
use std::sync::Arc;

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::FontFamily;
use font_kit::family_name::FamilyName;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;

/// A group of characters served by one font.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Script {
    Japanese,
    Korean,
    Han,
    Indic,
    Thai,
    Ethiopic,
    Emoji,
    /// Anything else the base font does not cover.
    Other,
}

/// Which script a character belongs to, or `None` when the base UI font is
/// expected to have it.
fn script_of(c: char) -> Option<Script> {
    let c = c as u32;
    match c {
        // Latin, Greek, Cyrillic, Hebrew, Arabic, punctuation, symbols.
        0x0000..=0x07FF | 0x2000..=0x25FF | 0xFB00..=0xFB4F => None,

        0x0900..=0x0DFF => Some(Script::Indic),
        0x0E00..=0x0E7F => Some(Script::Thai),
        0x1200..=0x139F => Some(Script::Ethiopic),
        0x3040..=0x30FF | 0x31F0..=0x31FF => Some(Script::Japanese),
        0x1100..=0x11FF | 0xA960..=0xA97F | 0xAC00..=0xD7FF => Some(Script::Korean),
        0x2E80..=0x2FDF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => {
            Some(Script::Han)
        }
        0x1F000..=0x1FAFF | 0x2600..=0x27BF => Some(Script::Emoji),
        _ => Some(Script::Other),
    }
}

/// Font families that ship with the platform, in preference order.
fn families_for(script: Script) -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        match script {
            Script::Japanese => &["Yu Gothic UI", "Yu Gothic", "Meiryo", "MS Gothic"],
            Script::Korean => &["Malgun Gothic", "Gulim"],
            Script::Han => &["Microsoft YaHei UI", "Microsoft YaHei", "Microsoft JhengHei UI", "SimSun"],
            Script::Indic => &["Nirmala UI", "Mangal"],
            Script::Thai => &["Leelawadee UI", "Tahoma"],
            Script::Ethiopic => &["Ebrima", "Nyala"],
            Script::Emoji => &["Segoe UI Emoji", "Segoe UI Symbol"],
            Script::Other => &["Segoe UI Historic", "Segoe UI Symbol", "Arial Unicode MS"],
        }
    }
    #[cfg(target_os = "macos")]
    {
        match script {
            Script::Japanese => &["Hiragino Sans", "Hiragino Kaku Gothic ProN"],
            Script::Korean => &["Apple SD Gothic Neo", "AppleGothic"],
            Script::Han => &["PingFang SC", "PingFang TC", "Heiti SC"],
            Script::Indic => &["Kohinoor Devanagari", "Devanagari Sangam MN"],
            Script::Thai => &["Thonburi", "Ayuthaya"],
            Script::Ethiopic => &["Kefa"],
            Script::Emoji => &["Apple Color Emoji"],
            Script::Other => &["Arial Unicode MS", "Apple Symbols"],
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        match script {
            Script::Japanese | Script::Korean | Script::Han => {
                &["Noto Sans CJK JP", "Noto Sans CJK SC", "Source Han Sans"]
            }
            Script::Indic => &["Noto Sans Devanagari", "Lohit Devanagari"],
            Script::Thai => &["Noto Sans Thai", "Garuda"],
            Script::Ethiopic => &["Noto Sans Ethiopic"],
            Script::Emoji => &["Noto Color Emoji", "Noto Emoji"],
            Script::Other => &["Noto Sans", "DejaVu Sans"],
        }
    }
}

/// Ask the system for a font by family name and hand back its bytes.
fn load_family(name: &str) -> Option<(String, Vec<u8>)> {
    let handle = SystemSource::new()
        .select_best_match(
            &[FamilyName::Title(name.to_string())],
            &Properties::new(),
        )
        .ok()?;
    let font = handle.load().ok()?;
    let data = font.copy_font_data()?;
    Some((font.full_name(), data.as_ref().clone()))
}

/// Ask the system for one of its generic families.
fn load_generic(family: FamilyName) -> Option<(String, Vec<u8>)> {
    let handle = SystemSource::new()
        .select_best_match(&[family], &Properties::new())
        .ok()?;
    let font = handle.load().ok()?;
    let data = font.copy_font_data()?;
    Some((font.full_name(), data.as_ref().clone()))
}

/// Install the base UI and monospace faces.
pub fn install_base(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();

    let mut install = |loaded: Option<(String, Vec<u8>)>, target: FontFamily| -> bool {
        let Some((name, bytes)) = loaded else {
            return false;
        };
        fonts
            .font_data
            .entry(name.clone())
            .or_insert_with(|| Arc::new(egui::FontData::from_owned(bytes)));
        fonts.families.entry(target).or_default().push(name);
        true
    };

    let sans = install(load_generic(FamilyName::SansSerif), FontFamily::Proportional);
    let mono = install(load_generic(FamilyName::Monospace), FontFamily::Monospace);

    // A family with no font renders nothing, so each falls back to the other.
    if !sans {
        let names = fonts
            .families
            .get(&FontFamily::Monospace)
            .cloned()
            .unwrap_or_default();
        fonts.families.insert(FontFamily::Proportional, names);
    }
    if !mono {
        let names = fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts.families.insert(FontFamily::Monospace, names);
    }

    ctx.set_fonts(fonts);
}

/// Load whatever `text` needs and the base font does not provide.
///
/// `loaded` records the scripts already dealt with, including those where no
/// font could be found, so a document full of unsupported characters does not
/// re-query the system every frame. Returns true when a font was added, which
/// means the frame should be drawn again.
pub fn ensure_coverage(ctx: &egui::Context, text: &str, loaded: &mut HashSet<Script>) -> bool {
    let mut wanted: Vec<Script> = Vec::new();
    for c in text.chars() {
        if let Some(script) = script_of(c) {
            if !loaded.contains(&script) && !wanted.contains(&script) {
                wanted.push(script);
            }
        }
    }
    if wanted.is_empty() {
        return false;
    }

    let mut added = false;
    for script in wanted {
        loaded.insert(script);
        let Some((name, bytes)) = families_for(script).iter().find_map(|f| load_family(f)) else {
            continue;
        };
        ctx.add_font(FontInsert::new(
            &name,
            egui::FontData::from_owned(bytes),
            vec![
                InsertFontFamily {
                    family: FontFamily::Proportional,
                    priority: FontPriority::Lowest,
                },
                InsertFontFamily {
                    family: FontFamily::Monospace,
                    priority: FontPriority::Lowest,
                },
            ],
        ));
        added = true;
    }
    added
}
