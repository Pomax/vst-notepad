//! Disk operations: open a `.md` file, Save, Save As.
//!
//! The dialogs live in the GUI layer; everything here is plain path-in /
//! path-out so the test runner can exercise the same code without a UI.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::edit::Editor;

/// Extensions offered in the open/save dialogs.
pub const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "txt"];

impl Editor {
    /// Load `path` into the editor, replacing the current document.
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)?;
        // Normalise CRLF so caret arithmetic stays byte-exact.
        let contents = contents.replace("\r\n", "\n");
        self.set_text(contents);
        self.set_caret(0);
        self.file = Some(path.to_path_buf());
        self.dirty = false;
        self.clear_history();
        Ok(())
    }

    /// Write to the current file. Fails if the document has no path yet —
    /// the caller should fall back to Save As.
    pub fn save(&mut self) -> io::Result<PathBuf> {
        let Some(path) = self.file.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no file associated with this document",
            ));
        };
        self.write_to(&path)?;
        Ok(path)
    }

    /// Write to `path` and adopt it as the current file.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> io::Result<PathBuf> {
        let path = with_default_extension(path.as_ref());
        self.write_to(&path)?;
        self.file = Some(path.clone());
        Ok(path)
    }

    fn write_to(&mut self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(path, self.text().as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    /// True when there are unsaved changes relative to disk or project state.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

}

/// Append `.md` when the user typed a name without an extension.
fn with_default_extension(path: &Path) -> PathBuf {
    match path.extension() {
        Some(_) => path.to_path_buf(),
        None => path.with_extension("md"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Key, Mods};

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "notepad-core-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn open_reads_a_file_and_adopts_its_path() {
        let dir = temp_dir();
        let path = dir.join("notes.md");
        fs::write(&path, "# Hello\n\n- a\n").unwrap();

        let mut e = Editor::new();
        e.open_path(&path).unwrap();
        assert_eq!(e.text(), "# Hello\n\n- a\n");
        assert_eq!(e.file.as_deref(), Some(path.as_path()));
        assert!(!e.is_dirty());
        assert_eq!(e.caret(), 0);
    }

    #[test]
    fn crlf_files_are_normalised() {
        let dir = temp_dir();
        let path = dir.join("crlf.md");
        fs::write(&path, "line one\r\nline two\r\n").unwrap();
        let mut e = Editor::new();
        e.open_path(&path).unwrap();
        assert_eq!(e.text(), "line one\nline two\n");
    }

    #[test]
    fn save_writes_back_to_the_same_path() {
        let dir = temp_dir();
        let path = dir.join("save.md");
        fs::write(&path, "original").unwrap();

        let mut e = Editor::new();
        e.open_path(&path).unwrap();
        e.set_caret(e.text().len());
        e.handle_key(Key::Char('!'), Mods::NONE);
        assert!(e.is_dirty());

        let written = e.save().unwrap();
        assert_eq!(written, path);
        assert_eq!(fs::read_to_string(&path).unwrap(), "original!");
        assert!(!e.is_dirty());
    }

    #[test]
    fn save_without_a_path_is_an_error() {
        let mut e = Editor::with_text("unsaved");
        assert!(e.save().is_err());
    }

    #[test]
    fn save_as_adopts_the_new_path_and_adds_md() {
        let dir = temp_dir();
        let target = dir.join("fresh");

        let mut e = Editor::with_text("# Fresh\n");
        let written = e.save_as(&target).unwrap();
        assert_eq!(written.extension().unwrap(), "md");
        assert_eq!(fs::read_to_string(&written).unwrap(), "# Fresh\n");
        assert_eq!(e.file.as_deref(), Some(written.as_path()));
        assert!(!e.is_dirty());
    }

    #[test]
    fn save_as_creates_missing_directories() {
        let dir = temp_dir().join("nested").join("deeper");
        let mut e = Editor::with_text("x");
        let written = e.save_as(dir.join("a.md")).unwrap();
        assert!(written.exists());
    }

    #[test]
    fn editing_marks_the_document_dirty() {
        let mut e = Editor::with_text("hi");
        e.dirty = false;
        e.set_caret(e.text().len());
        e.handle_key(Key::Char('!'), Mods::NONE);
        assert!(e.is_dirty());
    }

    #[test]
    fn a_full_open_edit_save_cycle_round_trips() {
        let dir = temp_dir();
        let path = dir.join("cycle.md");
        fs::write(&path, "# Title\n").unwrap();

        let mut e = Editor::new();
        e.open_path(&path).unwrap();
        e.set_caret(e.text().len());
        for c in "- item".chars() {
            e.handle_key(Key::Char(c), Mods::NONE);
        }
        e.save().unwrap();

        let mut reopened = Editor::new();
        reopened.open_path(&path).unwrap();
        assert_eq!(reopened.text(), "# Title\n- item");
    }
}
