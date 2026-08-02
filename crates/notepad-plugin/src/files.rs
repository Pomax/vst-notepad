//! Open / Save / Save As, with native dialogs.
//!
//! Shared by the key handler and the GUI's toolbar buttons so both behave
//! identically. Every entry point takes the shared editor and returns an error
//! message rather than logging, leaving presentation to the caller.

use notepad_core::Command;

use crate::Shared;

/// Run a host-level command. Returns an error message if it failed; `None`
/// means it succeeded or the user cancelled the dialog.
///
/// The editor mutex is never held while a dialog is open: a modal dialog spins
/// its own event loop, and holding the lock across it would block every other
/// thread that touches the document.
pub fn perform(editor: &Shared, command: Command) -> Option<String> {
    match command {
        Command::Open => {
            let path = dialog().pick_file()?;
            match editor.lock() {
                Ok(mut ed) => ed
                    .open_path(&path)
                    .err()
                    .map(|e| format!("could not open {}: {e}", path.display())),
                Err(_) => Some("editor is unavailable".into()),
            }
        }
        Command::Save => {
            let has_path = editor.lock().map(|e| e.file.is_some()).unwrap_or(false);
            if !has_path {
                // Nothing to save over yet, so Save behaves as Save As.
                return perform(editor, Command::SaveAs);
            }
            match editor.lock() {
                Ok(mut ed) => ed.save().err().map(|e| format!("could not save: {e}")),
                Err(_) => Some("editor is unavailable".into()),
            }
        }
        Command::SaveAs => {
            let suggested = editor
                .lock()
                .ok()
                .and_then(|e| {
                    e.file
                        .as_ref()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                })
                .unwrap_or_else(|| "Untitled.md".to_string());

            let path = dialog().set_file_name(suggested).save_file()?;
            match editor.lock() {
                Ok(mut ed) => ed
                    .save_as(&path)
                    .err()
                    .map(|e| format!("could not save {}: {e}", path.display())),
                Err(_) => Some("editor is unavailable".into()),
            }
        }
    }
}

fn dialog() -> rfd::FileDialog {
    rfd::FileDialog::new()
        .set_title("Notepad")
        .add_filter("Markdown", notepad_core::file::MARKDOWN_EXTENSIONS)
        .add_filter("All files", &["*"])
}
