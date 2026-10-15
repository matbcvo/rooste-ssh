//! Session recordings in asciinema's asciicast v2 format, for replaying a
//! session with its timing (`asciinema play file.cast`, web players).
//!
//! Only output and terminal size changes are recorded, never what the user
//! types, so passwords typed at prompts can't end up in a recording.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A file receiving a tab's output with timestamps.
#[derive(Debug)]
pub struct SessionRecording {
    pub path: PathBuf,
    file: std::io::BufWriter<std::fs::File>,
    start: Instant,
    /// The start of a UTF-8 character split between two chunks.
    pending: Vec<u8>,
}

impl SessionRecording {
    /// Create (or replace) `path` and write the header.
    pub fn create(path: &Path, title: &str, cols: u16, rows: u16) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut recording = Self {
            path: path.to_owned(),
            file: std::io::BufWriter::new(std::fs::File::create(path)?),
            start: Instant::now(),
            pending: Vec::new(),
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let header = serde_json::json!({
            "version": 2,
            "width": cols,
            "height": rows,
            "timestamp": timestamp,
            "title": title,
            "env": { "TERM": "xterm-256color" },
        });
        writeln!(recording.file, "{header}")?;
        recording.file.flush()?;
        Ok(recording)
    }

    /// Record output from the remote side.
    pub fn output(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.pending.extend_from_slice(bytes);
        let text = take_utf8(&mut self.pending);
        if text.is_empty() {
            return Ok(());
        }
        self.event("o", &text)
    }

    /// Record a terminal size change.
    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.event("r", &format!("{cols}x{rows}"))
    }

    fn event(&mut self, kind: &str, data: &str) -> std::io::Result<()> {
        let seconds = self.start.elapsed().as_secs_f64();
        let line = serde_json::json!([(seconds * 1e6).round() / 1e6, kind, data]);
        writeln!(self.file, "{line}")?;
        self.file.flush()
    }
}

/// Take the complete UTF-8 text from the front of `buffer`, leaving an
/// unfinished character at its end for the next chunk. Invalid bytes become
/// U+FFFD.
fn take_utf8(buffer: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(buffer) {
            Ok(valid) => {
                text.push_str(valid);
                buffer.clear();
                return text;
            }
            Err(e) => {
                let (valid, rest) = buffer.split_at(e.valid_up_to());
                text.push_str(std::str::from_utf8(valid).expect("checked"));
                match e.error_len() {
                    // An unfinished character: keep it for later.
                    None => {
                        *buffer = rest.to_vec();
                        return text;
                    }
                    Some(len) => {
                        text.push('\u{fffd}');
                        *buffer = rest[len..].to_vec();
                    }
                }
            }
        }
    }
}

/// A file name for a new recording of session `name`.
pub fn recording_file_name(name: &str) -> String {
    crate::tab::session_file_name(name, "cast")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_between_chunks() {
        let mut buffer = "ab".as_bytes().to_vec();
        buffer.extend_from_slice(&"õ".as_bytes()[..1]);
        assert_eq!(take_utf8(&mut buffer), "ab");
        buffer.extend_from_slice(&"õ".as_bytes()[1..]);
        buffer.extend_from_slice(b"c\xffd");
        assert_eq!(take_utf8(&mut buffer), "õc\u{fffd}d");
        assert!(buffer.is_empty());
    }

    #[test]
    fn asciicast_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.cast");
        let mut recording = SessionRecording::create(&path, "web-1", 80, 24).unwrap();
        recording.output(b"\x1b[32mhello\x1b[0m\r\n").unwrap();
        recording.resize(100, 30).unwrap();
        drop(recording);

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["version"], 2);
        assert_eq!(
            (lines[0]["width"].as_u64(), lines[0]["height"].as_u64()),
            (Some(80), Some(24))
        );
        assert_eq!(lines[0]["title"], "web-1");
        assert_eq!(lines[1][1], "o");
        assert_eq!(lines[1][2], "\u{1b}[32mhello\u{1b}[0m\r\n");
        assert!(lines[1][0].as_f64().unwrap() >= 0.0);
        assert_eq!(lines[2][1], "r");
        assert_eq!(lines[2][2], "100x30");
    }
}
