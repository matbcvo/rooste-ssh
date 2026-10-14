//! Encoding key presses as the byte sequences an xterm-compatible terminal
//! sends to the remote side.

/// A key, independent of the UI toolkit that reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    BackTab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// Function key F1–F12.
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        alt: false,
        ctrl: false,
    };

    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }

    /// xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    fn param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }
}

/// Terminal modes, set by the remote application, that change key encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputMode {
    /// DECCKM: cursor keys send `ESC O x` instead of `ESC [ x`.
    pub app_cursor: bool,
}

/// Encode one key press. Returns an empty vector for unmappable keys.
pub fn encode(key: Key, mods: Modifiers, mode: InputMode) -> Vec<u8> {
    match key {
        Key::Char(c) => encode_char(c, mods),
        Key::Enter => alt_prefixed(b"\r", mods),
        Key::Tab => alt_prefixed(b"\t", mods),
        Key::BackTab => b"\x1b[Z".to_vec(),
        Key::Escape => alt_prefixed(b"\x1b", mods),
        Key::Backspace if mods.ctrl => alt_prefixed(b"\x08", mods),
        Key::Backspace => alt_prefixed(b"\x7f", mods),
        Key::Up => cursor(b'A', mods, mode),
        Key::Down => cursor(b'B', mods, mode),
        Key::Right => cursor(b'C', mods, mode),
        Key::Left => cursor(b'D', mods, mode),
        Key::Home => cursor(b'H', mods, mode),
        Key::End => cursor(b'F', mods, mode),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n @ 1..=4) => {
            let final_byte = b'P' + (n - 1);
            if mods.any() {
                format!("\x1b[1;{}{}", mods.param(), final_byte as char).into_bytes()
            } else {
                vec![0x1b, b'O', final_byte]
            }
        }
        Key::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(CODES[usize::from(n - 5)], mods)
        }
        Key::F(_) => Vec::new(),
    }
}

fn encode_char(c: char, mods: Modifiers) -> Vec<u8> {
    if mods.ctrl
        && let Some(byte) = ctrl_byte(c)
    {
        return alt_prefixed(&[byte], mods);
    }
    let mut buf = [0u8; 4];
    alt_prefixed(c.encode_utf8(&mut buf).as_bytes(), mods)
}

/// The control character produced by Ctrl+`c`, if any.
fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Alt is sent as an ESC prefix for plain keys.
fn alt_prefixed(bytes: &[u8], mods: Modifiers) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn cursor(final_byte: u8, mods: Modifiers, mode: InputMode) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{}", mods.param(), final_byte as char).into_bytes()
    } else if mode.app_cursor {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn tilde(code: u8, mods: Modifiers) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{code};{}~", mods.param()).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// A mouse button or wheel direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    /// Pointer moved; `button` is the button held, if any.
    Motion,
}

/// Encode a mouse event for the remote application, or `None` if the
/// current mode does not report it. `col`/`row` are zero-based cells.
pub fn encode_mouse(
    button: Option<MouseButton>,
    action: MouseAction,
    col: usize,
    row: usize,
    mods: Modifiers,
    mode: crate::MouseMode,
) -> Option<Vec<u8>> {
    use crate::MouseReport;

    let reported = match (mode.report, action) {
        (MouseReport::None, _) => false,
        (_, MouseAction::Press | MouseAction::Release) => true,
        (MouseReport::Drag, MouseAction::Motion) => button.is_some(),
        (MouseReport::Motion, MouseAction::Motion) => true,
        (MouseReport::Click, MouseAction::Motion) => false,
    };
    if !reported {
        return None;
    }
    // Wheel "buttons" have no release.
    if action == MouseAction::Release
        && matches!(button, Some(MouseButton::WheelUp | MouseButton::WheelDown))
    {
        return None;
    }

    let base: u8 = match button {
        Some(MouseButton::Left) => 0,
        Some(MouseButton::Middle) => 1,
        Some(MouseButton::Right) => 2,
        Some(MouseButton::WheelUp) => 64,
        Some(MouseButton::WheelDown) => 65,
        None => 3,
    };
    let mut code =
        base + 4 * u8::from(mods.shift) + 8 * u8::from(mods.alt) + 16 * u8::from(mods.ctrl);
    if action == MouseAction::Motion {
        code += 32;
    }

    if mode.sgr {
        let final_byte = if action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{};{}{final_byte}", col + 1, row + 1).into_bytes());
    }
    // Legacy X10 encoding: release has no button, coordinates are limited.
    if action == MouseAction::Release {
        code = 3 + (code & !3);
    }
    let coord = |v: usize| u8::try_from(v + 1 + 32).ok();
    Some(vec![0x1b, b'[', b'M', 32 + code, coord(col)?, coord(row)?])
}

/// Prepare clipboard text for sending: newlines become carriage returns
/// and, in bracketed-paste mode, the text is wrapped in paste markers
/// (with any embedded escape characters removed so the paste cannot end
/// early).
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.bytes().filter(|&b| b != 0x1b));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: Modifiers = Modifiers::NONE;
    const CTRL: Modifiers = Modifiers { ctrl: true, ..N };
    const ALT: Modifiers = Modifiers { alt: true, ..N };
    const SHIFT: Modifiers = Modifiers { shift: true, ..N };

    fn enc(key: Key, mods: Modifiers) -> Vec<u8> {
        encode(key, mods, InputMode::default())
    }

    #[test]
    fn plain_and_unicode_chars() {
        assert_eq!(enc(Key::Char('a'), N), b"a");
        assert_eq!(enc(Key::Char('õ'), N), "õ".as_bytes());
        assert_eq!(enc(Key::Char('A'), SHIFT), b"A");
    }

    #[test]
    fn ctrl_chars() {
        assert_eq!(enc(Key::Char('c'), CTRL), [0x03]);
        assert_eq!(enc(Key::Char('C'), CTRL), [0x03]);
        assert_eq!(enc(Key::Char(' '), CTRL), [0x00]);
        assert_eq!(enc(Key::Char('['), CTRL), [0x1b]);
        assert_eq!(enc(Key::Char('?'), CTRL), [0x7f]);
        // No control equivalent: sent as the plain character.
        assert_eq!(enc(Key::Char('õ'), CTRL), "õ".as_bytes());
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc(Key::Char('x'), ALT), b"\x1bx");
        assert_eq!(
            enc(Key::Char('x'), Modifiers { alt: true, ..CTRL }),
            b"\x1b\x18"
        );
        assert_eq!(enc(Key::Backspace, ALT), b"\x1b\x7f");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(Key::Enter, N), b"\r");
        assert_eq!(enc(Key::Tab, N), b"\t");
        assert_eq!(enc(Key::BackTab, SHIFT), b"\x1b[Z");
        assert_eq!(enc(Key::Backspace, N), b"\x7f");
        assert_eq!(enc(Key::Backspace, CTRL), b"\x08");
        assert_eq!(enc(Key::Escape, N), b"\x1b");
        assert_eq!(enc(Key::Delete, N), b"\x1b[3~");
        assert_eq!(enc(Key::PageUp, N), b"\x1b[5~");
        assert_eq!(enc(Key::PageDown, CTRL), b"\x1b[6;5~");
    }

    #[test]
    fn cursor_keys_follow_decckm() {
        assert_eq!(enc(Key::Up, N), b"\x1b[A");
        let app = InputMode { app_cursor: true };
        assert_eq!(encode(Key::Up, N, app), b"\x1bOA");
        assert_eq!(encode(Key::Home, N, app), b"\x1bOH");
        // Modified cursor keys always use the CSI form.
        assert_eq!(encode(Key::Right, CTRL, app), b"\x1b[1;5C");
        assert_eq!(
            enc(
                Key::Left,
                Modifiers {
                    shift: true,
                    alt: true,
                    ctrl: false
                }
            ),
            b"\x1b[1;4D"
        );
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(Key::F(1), N), b"\x1bOP");
        assert_eq!(enc(Key::F(4), N), b"\x1bOS");
        assert_eq!(enc(Key::F(1), SHIFT), b"\x1b[1;2P");
        assert_eq!(enc(Key::F(5), N), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), N), b"\x1b[24~");
        assert_eq!(enc(Key::F(12), CTRL), b"\x1b[24;5~");
        assert!(enc(Key::F(13), N).is_empty());
    }

    #[test]
    fn mouse_reporting_modes() {
        use crate::{MouseMode, MouseReport};
        let sgr = |report| MouseMode { report, sgr: true };
        let left = Some(MouseButton::Left);

        assert_eq!(
            encode_mouse(left, MouseAction::Press, 0, 0, N, MouseMode::default()),
            None
        );
        assert_eq!(
            encode_mouse(left, MouseAction::Press, 4, 9, N, sgr(MouseReport::Click)).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            encode_mouse(left, MouseAction::Release, 4, 9, N, sgr(MouseReport::Click)).unwrap(),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            encode_mouse(left, MouseAction::Motion, 1, 1, N, sgr(MouseReport::Click)),
            None
        );
        assert_eq!(
            encode_mouse(left, MouseAction::Motion, 1, 1, N, sgr(MouseReport::Drag)).unwrap(),
            b"\x1b[<32;2;2M"
        );
        assert_eq!(
            encode_mouse(None, MouseAction::Motion, 1, 1, N, sgr(MouseReport::Drag)),
            None
        );
        assert_eq!(
            encode_mouse(
                Some(MouseButton::WheelUp),
                MouseAction::Press,
                0,
                0,
                CTRL,
                sgr(MouseReport::Click)
            )
            .unwrap(),
            b"\x1b[<80;1;1M"
        );
    }

    #[test]
    fn legacy_mouse_encoding() {
        use crate::{MouseMode, MouseReport};
        let x10 = MouseMode {
            report: MouseReport::Click,
            sgr: false,
        };
        assert_eq!(
            encode_mouse(Some(MouseButton::Right), MouseAction::Press, 0, 0, N, x10).unwrap(),
            [0x1b, b'[', b'M', 34, 33, 33]
        );
        assert_eq!(
            encode_mouse(Some(MouseButton::Right), MouseAction::Release, 0, 0, N, x10).unwrap(),
            [0x1b, b'[', b'M', 35, 33, 33]
        );
        // Out of range for X10.
        assert_eq!(
            encode_mouse(Some(MouseButton::Left), MouseAction::Press, 300, 0, N, x10),
            None
        );
    }

    #[test]
    fn paste_encoding() {
        assert_eq!(encode_paste("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(
            encode_paste("x\x1b[201~y", true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
    }
}
