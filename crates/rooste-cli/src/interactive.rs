//! Interactive session: local terminal in raw mode wired to the remote shell.
//!
//! On Unix, stdin bytes are passed through unchanged: the local terminal
//! already encodes keys and tracks modes set by the remote application.
//! The Windows console delivers key events instead, which are encoded with
//! `rooste_terminal::input`.

use std::io;

use rooste_protocols::Transport;
use rooste_protocols::ssh::SshShell;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

/// Something from the local side for the remote shell.
#[derive(Debug)]
pub enum Input {
    Data(Vec<u8>),
    Resize(u16, u16),
}

enum Event {
    Output(io::Result<usize>),
    Input(Option<Input>),
}

/// Restores the terminal when dropped, including on error.
struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Run until the remote shell closes the channel.
pub async fn run(shell: &mut SshShell) -> anyhow::Result<()> {
    let _raw = RawMode::enable()?;
    let (tx, mut rx) = mpsc::channel(64);
    spawn_input(tx);

    let mut stdout = tokio::io::stdout();
    let mut buf = vec![0u8; 32 * 1024];
    let mut input_open = true;
    loop {
        // `SshShell::read` is cancel-safe, so dropping it when input wins
        // loses no data.
        let event = tokio::select! {
            n = shell.read(&mut buf) => Event::Output(n),
            input = rx.recv(), if input_open => Event::Input(input),
        };
        match event {
            Event::Output(n) => {
                let n = n?;
                if n == 0 {
                    break;
                }
                stdout.write_all(&buf[..n]).await?;
                stdout.flush().await?;
            }
            Event::Input(Some(Input::Data(data))) => shell.write(&data).await?,
            Event::Input(Some(Input::Resize(cols, rows))) => shell.resize(cols, rows).await?,
            Event::Input(None) => input_open = false,
        }
    }
    Ok(())
}

#[cfg(unix)]
fn spawn_input(tx: mpsc::Sender<Input>) {
    use tokio::io::AsyncReadExt;
    use tokio::signal::unix::{SignalKind, signal};

    let data_tx = tx.clone();
    tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut buf).await {
            if n == 0 || data_tx.send(Input::Data(buf[..n].to_vec())).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let Ok(mut winch) = signal(SignalKind::window_change()) else {
            return;
        };
        while winch.recv().await.is_some() {
            if let Ok((cols, rows)) = crossterm::terminal::size()
                && tx.send(Input::Resize(cols, rows)).await.is_err()
            {
                break;
            }
        }
    });
}

#[cfg(windows)]
fn spawn_input(tx: mpsc::Sender<Input>) {
    use crossterm::event::{Event as TermEvent, EventStream};
    use futures::StreamExt;

    tokio::spawn(async move {
        let mut events = EventStream::new();
        while let Some(Ok(event)) = events.next().await {
            let input = match event {
                TermEvent::Key(key) => match windows_keys::encode(key) {
                    Some(bytes) => Input::Data(bytes),
                    None => continue,
                },
                TermEvent::Paste(text) => Input::Data(text.into_bytes()),
                TermEvent::Resize(cols, rows) => Input::Resize(cols, rows),
                _ => continue,
            };
            if tx.send(input).await.is_err() {
                break;
            }
        }
    });
}

#[cfg(windows)]
mod windows_keys {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use rooste_terminal::input::{InputMode, Key, Modifiers, encode as encode_key};

    pub fn encode(event: KeyEvent) -> Option<Vec<u8>> {
        if event.kind == KeyEventKind::Release {
            return None;
        }
        let key = match event.code {
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Enter => Key::Enter,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Tab => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Esc => Key::Escape,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::Insert => Key::Insert,
            KeyCode::Delete => Key::Delete,
            KeyCode::F(n) => Key::F(n),
            _ => return None,
        };
        let mut mods = Modifiers {
            shift: event.modifiers.contains(KeyModifiers::SHIFT),
            alt: event.modifiers.contains(KeyModifiers::ALT),
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
        };
        // AltGr arrives as Ctrl+Alt; characters typed with it (e.g. `@`,
        // `{` on many European layouts) must be sent as-is.
        if let Key::Char(c) = key
            && mods.ctrl
            && mods.alt
            && !c.is_ascii_alphabetic()
        {
            mods = Modifiers::NONE;
        }
        let bytes = encode_key(key, mods, InputMode::default());
        (!bytes.is_empty()).then_some(bytes)
    }
}
