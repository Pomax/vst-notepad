//! Plugin state serialisation.
//!
//! This is what the DAW writes into its project file. It carries the notes, the
//! view mode, the editor window size and the path of the file on disk (if the
//! notes came from one), so reopening a project restores the session exactly.

use serde::{Deserialize, Serialize};

use crate::edit::{Editor, Theme, ViewMode, DEFAULT_HEIGHT, DEFAULT_WIDTH};

/// Bumped whenever the layout changes incompatibly.
pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    /// "light", "dark" or "auto". Absent in state written before themes
    /// existed, which is why it defaults rather than failing the whole parse.
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_height")]
    pub height: i32,
    #[serde(default)]
    pub caret: usize,
    #[serde(default)]
    pub file: Option<String>,
}

fn default_version() -> u32 {
    STATE_VERSION
}
fn default_mode() -> String {
    "wysiwyg".to_string()
}
fn default_theme() -> String {
    Theme::default().as_str().to_string()
}
fn default_width() -> i32 {
    DEFAULT_WIDTH
}
fn default_height() -> i32 {
    DEFAULT_HEIGHT
}

impl Default for PluginState {
    fn default() -> Self {
        PluginState {
            version: STATE_VERSION,
            notes: String::new(),
            mode: default_mode(),
            theme: default_theme(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            caret: 0,
            file: None,
        }
    }
}

impl PluginState {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_else(|_| b"{}".to_vec())
    }

    /// Parse state written by [`PluginState::to_bytes`].
    ///
    /// Anything that is not valid JSON is treated as raw note text, so a state
    /// blob from a hand-written host (or a future format we fail to parse)
    /// still restores the user's words rather than losing them.
    pub fn from_bytes(bytes: &[u8]) -> PluginState {
        if bytes.is_empty() {
            return PluginState::default();
        }
        match serde_json::from_slice::<PluginState>(bytes) {
            Ok(state) => state,
            Err(_) => PluginState {
                notes: String::from_utf8_lossy(bytes).into_owned(),
                ..PluginState::default()
            },
        }
    }
}

impl Editor {
    /// Capture everything the DAW should persist.
    pub fn save_state(&self) -> PluginState {
        PluginState {
            version: STATE_VERSION,
            notes: self.text().to_string(),
            mode: self.mode.as_str().to_string(),
            theme: self.theme.as_str().to_string(),
            width: self.width,
            height: self.height,
            caret: self.caret(),
            file: self
                .file
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }

    /// Restore from persisted state.
    pub fn load_state(&mut self, state: &PluginState) {
        self.set_text(state.notes.clone());
        self.mode = ViewMode::from_str(&state.mode);
        self.theme = Theme::from_str(&state.theme);
        self.set_size(state.width, state.height);
        self.set_caret(state.caret.min(self.text().len()));
        self.file = state.file.as_ref().map(std::path::PathBuf::from);
        self.dirty = false;
        self.clear_history();
    }

    pub fn state_bytes(&self) -> Vec<u8> {
        self.save_state().to_bytes()
    }

    pub fn load_state_bytes(&mut self, bytes: &[u8]) {
        let state = PluginState::from_bytes(bytes);
        self.load_state(&state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::ViewMode;

    #[test]
    fn round_trips_everything() {
        let mut e = Editor::with_text("# Notes\n\n- one\n- two");
        e.mode = ViewMode::Raw;
        e.set_size(1024, 768);
        e.file = Some("C:/notes/todo.md".into());
        e.set_caret(3);

        let bytes = e.state_bytes();
        let mut restored = Editor::new();
        restored.load_state_bytes(&bytes);

        assert_eq!(restored.text(), "# Notes\n\n- one\n- two");
        assert_eq!(restored.mode, ViewMode::Raw);
        assert_eq!(restored.width, 1024);
        assert_eq!(restored.height, 768);
        assert_eq!(restored.caret(), 3);
        assert_eq!(
            restored.file.as_ref().unwrap().to_string_lossy(),
            "C:/notes/todo.md"
        );
        assert!(!restored.dirty);
    }

    #[test]
    fn theme_round_trips() {
        for theme in [Theme::Light, Theme::Dark, Theme::Auto] {
            let mut e = Editor::with_text("x");
            e.theme = theme;
            let mut restored = Editor::new();
            restored.load_state_bytes(&e.state_bytes());
            assert_eq!(restored.theme, theme, "theme {theme:?} did not survive");
        }
    }

    #[test]
    fn theme_defaults_to_auto() {
        assert_eq!(Editor::new().theme, Theme::Auto);
        assert_eq!(PluginState::default().theme, "auto");
    }

    #[test]
    fn state_written_before_themes_existed_still_loads() {
        // No "theme" key at all — the field must default rather than the whole
        // parse failing and the notes being treated as raw text.
        let s = PluginState::from_bytes(br#"{"notes":"older project","mode":"raw"}"#);
        assert_eq!(s.notes, "older project");
        assert_eq!(s.theme, "auto");

        let mut e = Editor::new();
        e.load_state(&s);
        assert_eq!(e.theme, Theme::Auto);
        assert_eq!(e.mode, ViewMode::Raw);
    }

    #[test]
    fn an_unknown_theme_falls_back_to_auto() {
        let s = PluginState::from_bytes(br#"{"notes":"x","theme":"solarized"}"#);
        let mut e = Editor::new();
        e.load_state(&s);
        assert_eq!(e.theme, Theme::Auto);
    }

    #[test]
    fn empty_state_is_a_fresh_document() {
        let s = PluginState::from_bytes(&[]);
        assert_eq!(s.notes, "");
        assert_eq!(s.width, DEFAULT_WIDTH);
        assert_eq!(s.mode, "wysiwyg");
    }

    #[test]
    fn non_json_state_is_kept_as_note_text() {
        let s = PluginState::from_bytes(b"just some notes");
        assert_eq!(s.notes, "just some notes");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s = PluginState::from_bytes(br#"{"notes":"hi"}"#);
        assert_eq!(s.notes, "hi");
        assert_eq!(s.mode, "wysiwyg");
        assert_eq!(s.height, DEFAULT_HEIGHT);
    }

    #[test]
    fn size_is_clamped_to_the_minimum_on_load() {
        let s = PluginState::from_bytes(br#"{"notes":"x","width":10,"height":10}"#);
        let mut e = Editor::new();
        e.load_state(&s);
        assert!(e.width >= crate::edit::MIN_WIDTH);
        assert!(e.height >= crate::edit::MIN_HEIGHT);
    }

    #[test]
    fn unicode_survives_the_round_trip() {
        let e = Editor::with_text("# 見出し\n\n- 日本語 **太字** ✓\n");
        let mut restored = Editor::new();
        restored.load_state_bytes(&e.state_bytes());
        assert_eq!(restored.text(), e.text());
    }
}
