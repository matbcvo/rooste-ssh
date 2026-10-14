//! Translating egui keyboard events into bytes for the remote side.
//!
//! egui reports printable input as `Text` and everything else as `Key`.
//! Ctrl/Alt combinations with printable keys are encoded from the `Key`
//! event (egui sends no `Text` for Ctrl, but may for Alt, which we then
//! skip). Ctrl+Alt together is AltGr on Windows and is left to `Text`.
//!
//! egui turns Ctrl+C/X/V into `Copy`/`Cut`/`Paste` before we see a key, so
//! those are mapped back here: with Shift they act on the clipboard,
//! without Shift they send the control character (Ctrl+C copies instead
//! when text is selected).

use egui::{Event, Key, Modifiers};
use rooste_terminal::input::{self, InputMode, Key as TermKey, Modifiers as TermMods};

/// What a single egui event means for the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Send(Vec<u8>),
    /// Copy the selection to the clipboard.
    Copy,
    /// Paste this text (already read from the clipboard by egui).
    Paste(String),
}

/// Stateful per frame: remembers Alt+key so the duplicate `Text` is dropped.
#[derive(Debug, Default)]
pub struct KeyTranslator {
    skip_text: Option<char>,
}

impl KeyTranslator {
    /// Translate one event. `modifiers` are the frame's current modifiers
    /// (egui's `Copy`/`Paste` events carry none). `has_selection` decides
    /// whether Ctrl+C copies or interrupts.
    pub fn translate(
        &mut self,
        event: &Event,
        modifiers: Modifiers,
        mode: InputMode,
        has_selection: bool,
    ) -> Option<Action> {
        match event {
            Event::Text(text) => {
                if let Some(skip) = self.skip_text.take()
                    && text.chars().eq([skip])
                {
                    return None;
                }
                Some(Action::Send(text.as_bytes().to_vec()))
            }
            // On macOS ⌘C, ⌘X and ⌘V are the clipboard keys; Ctrl+C reaches
            // the terminal as a key press and interrupts as usual.
            Event::Copy | Event::Cut if cfg!(target_os = "macos") => Some(Action::Copy),
            Event::Paste(text) if cfg!(target_os = "macos") => Some(Action::Paste(text.clone())),
            Event::Copy if modifiers.shift || has_selection => Some(Action::Copy),
            Event::Copy => Some(Action::Send(vec![0x03])),
            Event::Cut => Some(Action::Send(vec![0x18])),
            Event::Paste(text) if modifiers.shift => Some(Action::Paste(text.clone())),
            Event::Paste(_) => Some(Action::Send(vec![0x16])),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => self.key(*key, *modifiers, mode).map(Action::Send),
            _ => None,
        }
    }

    fn key(&mut self, key: Key, mods: Modifiers, mode: InputMode) -> Option<Vec<u8>> {
        let term_mods = TermMods {
            shift: mods.shift,
            alt: mods.alt,
            ctrl: mods.ctrl,
        };
        if let Some(named) = named_key(key) {
            return Some(input::encode(named, term_mods, mode));
        }
        let ch = printable_key(key)?;
        // AltGr (Ctrl+Alt) and unmodified keys arrive as `Text`.
        if mods.ctrl == mods.alt {
            return None;
        }
        if mods.alt {
            self.skip_text = Some(if mods.shift {
                ch.to_ascii_uppercase()
            } else {
                ch
            });
            let ch = if mods.shift {
                ch.to_ascii_uppercase()
            } else {
                ch
            };
            return Some(input::encode(
                TermKey::Char(ch),
                TermMods {
                    shift: false,
                    ..term_mods
                },
                mode,
            ));
        }
        let bytes = input::encode(TermKey::Char(ch), term_mods, mode);
        // Ctrl with a key that has no control code: let `Text` handle it.
        (bytes != ch.to_string().as_bytes()).then_some(bytes)
    }
}

fn named_key(key: Key) -> Option<TermKey> {
    Some(match key {
        Key::Enter => TermKey::Enter,
        Key::Backspace => TermKey::Backspace,
        Key::Tab => TermKey::Tab,
        Key::Escape => TermKey::Escape,
        Key::ArrowUp => TermKey::Up,
        Key::ArrowDown => TermKey::Down,
        Key::ArrowLeft => TermKey::Left,
        Key::ArrowRight => TermKey::Right,
        Key::Home => TermKey::Home,
        Key::End => TermKey::End,
        Key::PageUp => TermKey::PageUp,
        Key::PageDown => TermKey::PageDown,
        Key::Insert => TermKey::Insert,
        Key::Delete => TermKey::Delete,
        Key::F1 => TermKey::F(1),
        Key::F2 => TermKey::F(2),
        Key::F3 => TermKey::F(3),
        Key::F4 => TermKey::F(4),
        Key::F5 => TermKey::F(5),
        Key::F6 => TermKey::F(6),
        Key::F7 => TermKey::F(7),
        Key::F8 => TermKey::F(8),
        Key::F9 => TermKey::F(9),
        Key::F10 => TermKey::F(10),
        Key::F11 => TermKey::F(11),
        Key::F12 => TermKey::F(12),
        _ => return None,
    })
}

fn printable_key(key: Key) -> Option<char> {
    let name = key.symbol_or_name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_graphic() || c == ' ' => Some(c.to_ascii_lowercase()),
        _ if key == Key::Space => Some(' '),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn send(t: &mut KeyTranslator, event: Event) -> Option<Vec<u8>> {
        match t.translate(&event, Modifiers::NONE, InputMode::default(), false) {
            Some(Action::Send(bytes)) => Some(bytes),
            Some(other) => panic!("unexpected {other:?}"),
            None => None,
        }
    }

    #[test]
    fn text_and_named_keys() {
        let mut t = KeyTranslator::default();
        assert_eq!(
            send(&mut t, Event::Text("õ".into())).unwrap(),
            "õ".as_bytes()
        );
        assert_eq!(
            send(&mut t, key(Key::Enter, Modifiers::NONE)).unwrap(),
            b"\r"
        );
        assert_eq!(
            send(&mut t, key(Key::ArrowUp, Modifiers::NONE)).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            send(&mut t, key(Key::F5, Modifiers::NONE)).unwrap(),
            b"\x1b[15~"
        );
        // Plain letters come through `Text`, not `Key`.
        assert_eq!(send(&mut t, key(Key::A, Modifiers::NONE)), None);
    }

    #[test]
    fn app_cursor_mode() {
        let mut t = KeyTranslator::default();
        let app = InputMode { app_cursor: true };
        assert_eq!(
            t.translate(
                &key(Key::ArrowLeft, Modifiers::NONE),
                Modifiers::NONE,
                app,
                false
            ),
            Some(Action::Send(b"\x1bOD".to_vec()))
        );
    }

    #[test]
    fn ctrl_combinations() {
        let mut t = KeyTranslator::default();
        assert_eq!(send(&mut t, key(Key::D, Modifiers::CTRL)).unwrap(), [0x04]);
        assert_eq!(
            send(&mut t, key(Key::OpenBracket, Modifiers::CTRL)).unwrap(),
            [0x1b]
        );
        assert_eq!(
            send(&mut t, key(Key::Space, Modifiers::CTRL)).unwrap(),
            [0x00]
        );
    }

    #[test]
    fn alt_combinations_skip_duplicate_text() {
        let mut t = KeyTranslator::default();
        assert_eq!(send(&mut t, key(Key::B, Modifiers::ALT)).unwrap(), b"\x1bb");
        assert_eq!(send(&mut t, Event::Text("b".into())), None);
        // Only the immediately following duplicate is dropped.
        assert_eq!(send(&mut t, Event::Text("b".into())).unwrap(), b"b");
    }

    #[test]
    fn altgr_is_left_to_text() {
        let mut t = KeyTranslator::default();
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(send(&mut t, key(Key::Num2, altgr)), None);
        assert_eq!(send(&mut t, Event::Text("@".into())).unwrap(), b"@");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn clipboard_shortcuts_on_macos() {
        let mut t = KeyTranslator::default();
        let mode = InputMode::default();
        for event in [Event::Copy, Event::Cut] {
            assert_eq!(
                t.translate(&event, Modifiers::COMMAND, mode, false),
                Some(Action::Copy)
            );
        }
        assert_eq!(
            t.translate(&Event::Paste("x".into()), Modifiers::COMMAND, mode, false),
            Some(Action::Paste("x".into()))
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn clipboard_shortcuts() {
        let mut t = KeyTranslator::default();
        let mode = InputMode::default();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            t.translate(&Event::Copy, Modifiers::CTRL, mode, false),
            Some(Action::Send(vec![0x03]))
        );
        assert_eq!(
            t.translate(&Event::Copy, Modifiers::CTRL, mode, true),
            Some(Action::Copy)
        );
        assert_eq!(
            t.translate(&Event::Copy, shift, mode, false),
            Some(Action::Copy)
        );
        assert_eq!(
            t.translate(&Event::Paste("x".into()), Modifiers::CTRL, mode, false),
            Some(Action::Send(vec![0x16]))
        );
        assert_eq!(
            t.translate(&Event::Paste("x".into()), shift, mode, false),
            Some(Action::Paste("x".into()))
        );
        assert_eq!(
            t.translate(&Event::Cut, Modifiers::CTRL, mode, false),
            Some(Action::Send(vec![0x18]))
        );
    }
}
