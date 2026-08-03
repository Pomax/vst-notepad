//! Translation from VST3 key events to [`notepad_core::Key`].
//!
//! `IPlugView::onKeyDown(char16 key, int16 keyCode, int16 modifiers)` is the
//! standard way a host delivers keyboard input to a plugin editor. Hosts send
//! either a printable UTF-16 unit in `key` (with `keyCode` == -1) or a virtual
//! key in `keyCode` (with `key` == 0), so both paths are handled here.
//!
//! The numeric constants are the stable VST3 ABI values from `keycodes.h`; they
//! are spelled out rather than imported so this mapping is readable on its own
//! and testable without a plugin instance.

use notepad_core::{Key, Mods};

// Virtual key codes (Steinberg::VirtualKeyCodes).
pub const KEY_BACK: i16 = 1;
pub const KEY_TAB: i16 = 2;
pub const KEY_RETURN: i16 = 4;
pub const KEY_ESCAPE: i16 = 6;
pub const KEY_SPACE: i16 = 7;
pub const KEY_END: i16 = 9;
pub const KEY_HOME: i16 = 10;
pub const KEY_LEFT: i16 = 11;
pub const KEY_UP: i16 = 12;
pub const KEY_RIGHT: i16 = 13;
pub const KEY_DOWN: i16 = 14;
pub const KEY_PAGEUP: i16 = 15;
pub const KEY_PAGEDOWN: i16 = 16;
pub const KEY_ENTER: i16 = 19;
pub const KEY_DELETE: i16 = 22;

// Modifier mask (Steinberg::KeyModifier).
pub const MOD_SHIFT: i16 = 1 << 0;
pub const MOD_ALT: i16 = 1 << 1;
pub const MOD_COMMAND: i16 = 1 << 2;
pub const MOD_CONTROL: i16 = 1 << 3;

/// Decode the modifier mask.
///
/// VST3 distinguishes "command" (Cmd on macOS, Ctrl on Windows) from "control"
/// (Ctrl on macOS). Both map onto our single `ctrl` flag so shortcuts behave
/// natively on either platform.
pub fn decode_mods(modifiers: i16) -> Mods {
    Mods {
        ctrl: modifiers & (MOD_COMMAND | MOD_CONTROL) != 0,
        shift: modifiers & MOD_SHIFT != 0,
        alt: modifiers & MOD_ALT != 0,
    }
}

/// Map a VST3 key event onto an editor key, or `None` if it is not ours.
pub fn decode_key(key_char: u16, key_code: i16) -> Option<Key> {
    // A virtual key code takes precedence when the host supplies one.
    if key_code > 0 {
        return match key_code {
            KEY_BACK => Some(Key::Backspace),
            KEY_TAB => Some(Key::Tab),
            KEY_RETURN | KEY_ENTER => Some(Key::Enter),
            KEY_ESCAPE => Some(Key::Escape),
            KEY_SPACE => Some(Key::Char(' ')),
            KEY_END => Some(Key::End),
            KEY_HOME => Some(Key::Home),
            KEY_LEFT => Some(Key::Left),
            KEY_UP => Some(Key::Up),
            KEY_RIGHT => Some(Key::Right),
            KEY_DOWN => Some(Key::Down),
            KEY_PAGEUP => Some(Key::PageUp),
            KEY_PAGEDOWN => Some(Key::PageDown),
            KEY_DELETE => Some(Key::Delete),
            _ => None,
        };
    }

    if key_char == 0 {
        return None;
    }

    // Control characters arrive as printable-slot values in some hosts.
    match key_char {
        8 => return Some(Key::Backspace),
        9 => return Some(Key::Tab),
        10 | 13 => return Some(Key::Enter),
        27 => return Some(Key::Escape),
        127 => return Some(Key::Delete),
        _ => {}
    }

    let c = char::from_u32(key_char as u32)?;
    if c.is_control() {
        return None;
    }
    Some(Key::Char(c))
}

/// Whether a key event must be reported to the host as consumed.
///
/// `IPlugView::onKeyDown` returning `kResultFalse` does not mean "no comment",
/// it means "this keystroke went unused" — and a host is then entitled to run
/// its own shortcut for it. Getting this wrong is not a subtle bug: the
/// character is inserted into the document *and* the DAW acts on it, so typing
/// a space starts playback and typing letters fires tool shortcuts.
///
/// * With the editor window open, every key is claimed. The window has already
///   delivered it natively, and a key this editor happens to ignore is still
///   not the DAW's to act on while the notepad holds focus.
/// * With no window there is no editor to hold focus, and this is the only
///   input path, so the answer is simply whether the key did something.
pub fn claims_key(window_open: bool, decoded: Option<Key>, handled: bool) -> bool {
    if window_open {
        return true;
    }
    decoded.is_some() && handled
}

/// Encode a character the way a host would deliver it — used by the test host.
pub fn encode_char(c: char) -> (u16, i16) {
    let mut buf = [0u16; 2];
    let units = c.encode_utf16(&mut buf);
    (units[0], -1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_characters_pass_through() {
        assert_eq!(decode_key(b'a' as u16, -1), Some(Key::Char('a')));
        assert_eq!(decode_key(b'#' as u16, -1), Some(Key::Char('#')));
        assert_eq!(decode_key(' ' as u16, -1), Some(Key::Char(' ')));
    }

    #[test]
    fn virtual_keys_win_over_the_character_slot() {
        assert_eq!(decode_key(0, KEY_RETURN), Some(Key::Enter));
        assert_eq!(decode_key(b'x' as u16, KEY_BACK), Some(Key::Backspace));
        assert_eq!(decode_key(0, KEY_LEFT), Some(Key::Left));
        assert_eq!(decode_key(0, KEY_DELETE), Some(Key::Delete));
    }

    #[test]
    fn control_characters_map_to_editing_keys() {
        assert_eq!(decode_key(13, -1), Some(Key::Enter));
        assert_eq!(decode_key(8, -1), Some(Key::Backspace));
        assert_eq!(decode_key(9, -1), Some(Key::Tab));
    }

    #[test]
    fn unknown_and_empty_events_are_ignored() {
        assert_eq!(decode_key(0, -1), None);
        assert_eq!(decode_key(0, 9999), None);
    }

    #[test]
    fn unicode_characters_survive() {
        assert_eq!(decode_key('é' as u16, -1), Some(Key::Char('é')));
        let (unit, code) = encode_char('ß');
        assert_eq!(decode_key(unit, code), Some(Key::Char('ß')));
    }

    #[test]
    fn modifiers_decode_on_both_platforms() {
        assert_eq!(decode_mods(0), Mods::NONE);
        assert!(decode_mods(MOD_COMMAND).ctrl);
        assert!(decode_mods(MOD_CONTROL).ctrl);
        assert!(decode_mods(MOD_SHIFT).shift);
        assert!(decode_mods(MOD_ALT).alt);
        let both = decode_mods(MOD_COMMAND | MOD_SHIFT);
        assert!(both.ctrl && both.shift && !both.alt);
    }

    #[test]
    fn an_open_editor_claims_every_key_so_nothing_reaches_the_daw() {
        // The point of the fix: not merely the keys the editor understands.
        // Anything left unclaimed is offered to the host as an unused key.
        assert!(claims_key(true, Some(Key::Char('a')), true));
        assert!(claims_key(true, Some(Key::Char(' ')), true));
        assert!(claims_key(true, Some(Key::Escape), false));
        assert!(claims_key(true, None, false));
    }

    #[test]
    fn a_space_typed_into_an_open_editor_is_never_offered_to_the_host() {
        // Space is the one that gives the bug away: in every DAW it is
        // play/stop, so an unclaimed space starts the transport mid-sentence.
        let (unit, code) = encode_char(' ');
        let decoded = decode_key(unit, code);
        assert_eq!(decoded, Some(Key::Char(' ')));
        assert!(claims_key(true, decoded, true));
        assert!(claims_key(true, decoded, false));
    }

    #[test]
    fn with_no_window_only_keys_the_editor_used_are_claimed() {
        // Nothing has focus, so an unused key belongs to the host.
        assert!(claims_key(false, Some(Key::Char('a')), true));
        assert!(!claims_key(false, Some(Key::Char('a')), false));
        assert!(!claims_key(false, None, false));
    }

    #[test]
    fn an_unrecognised_key_is_never_claimed_without_a_window() {
        // Function keys and the like decode to nothing and must stay the
        // host's, or the plugin would swallow shortcuts it has no use for.
        assert_eq!(decode_key(0, 9999), None);
        assert!(!claims_key(false, decode_key(0, 9999), true));
    }

    #[test]
    fn round_trip_of_a_typed_sentence() {
        let mut editor = notepad_core::Editor::new();
        for c in "# Hi".chars() {
            let (unit, code) = encode_char(c);
            let key = decode_key(unit, code).expect("printable");
            editor.handle_key(key, decode_mods(0));
        }
        assert_eq!(editor.text(), "# Hi");
    }
}
