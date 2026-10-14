//! Terminal output as plain text, for session logs: escape sequences
//! (colors, cursor movement) are dropped and line endings normalized.

use alacritty_terminal::vte::{Parser, Perform};

/// Turns a stream of terminal output into plain text. Feed it output in
/// any chunks; sequences split between chunks are handled.
#[derive(Default)]
pub struct PlainText {
    parser: Parser,
}

impl std::fmt::Debug for PlainText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlainText").finish_non_exhaustive()
    }
}

impl PlainText {
    pub fn new() -> Self {
        Self::default()
    }

    /// The printable text in `bytes`, with `\n` line endings.
    pub fn convert(&mut self, bytes: &[u8]) -> String {
        let mut text = Text(String::new());
        self.parser.advance(&mut text, bytes);
        text.0
    }
}

struct Text(String);

impl Perform for Text {
    fn print(&mut self, c: char) {
        self.0.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.0.push('\n'),
            b'\t' => self.0.push('\t'),
            // Backspace erases what was just typed (within this chunk).
            0x08 => {
                self.0.pop();
            }
            // Carriage returns, bells and other controls are dropped.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_escapes_and_carriage_returns() {
        let mut text = PlainText::new();
        let out = text.convert(b"\x1b[1;32muser@host\x1b[0m:~$ ls\r\nfile\tnotes\r\n\x07");
        assert_eq!(out, "user@host:~$ ls\nfile\tnotes\n");
    }

    #[test]
    fn sequences_split_between_chunks() {
        let mut text = PlainText::new();
        let mut out = text.convert(b"red \x1b[3");
        out += &text.convert(b"1mtext\x1b]0;window title\x07 done\r\n");
        assert_eq!(out, "red text done\n");
    }

    #[test]
    fn backspace_and_utf8() {
        let mut text = PlainText::new();
        assert_eq!(text.convert("lss\x08 õäöü €\n".as_bytes()), "ls õäöü €\n");
    }
}
