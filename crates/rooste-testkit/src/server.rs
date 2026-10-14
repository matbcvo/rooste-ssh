//! In-process SSH server with a fake line-based shell.
//!
//! Shell commands (end lines with `\r` or `\n`):
//! - `size` prints the current PTY size as `COLSxROWS`
//! - `whoami` prints the authenticated user
//! - `exit N` exits with status `N`
//! - anything else is echoed back as `echo: <line>`
//!
//! `exec` supports `echo <text>` and `exit N`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::PublicKey;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, Preferred, Pty};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::{TEST_PASSWORD, TEST_USER, private_key, public_key};

/// A running in-process SSH server. Stops when dropped.
#[derive(Debug)]
pub struct TestServer {
    pub addr: SocketAddr,
    pub host_key: PublicKey,
    task: JoinHandle<()>,
}

impl TestServer {
    /// Start a server offering russh's default algorithms.
    pub async fn start() -> Self {
        Self::start_with(Preferred::default()).await
    }

    /// Start a server offering only the given algorithms, e.g. to emulate a
    /// legacy device.
    pub async fn start_with(preferred: Preferred) -> Self {
        let host_key = private_key("host_ed25519");
        let host_public = host_key.public_key().clone();
        let config = Arc::new(server::Config {
            keys: vec![host_key],
            preferred,
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        });
        let authorized: Arc<[PublicKey]> = ["id_ed25519", "id_ed25519_encrypted", "id_rsa"]
            .map(public_key)
            .into();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let config = config.clone();
                let handler = ShellHandler {
                    authorized: authorized.clone(),
                    user: String::new(),
                    shells: HashMap::new(),
                };
                tokio::spawn(async move {
                    if let Ok(session) = server::run_stream(config, socket, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });
        Self {
            addr,
            host_key: host_public,
            task,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct FakeShell {
    cols: u32,
    rows: u32,
    line: Vec<u8>,
}

struct ShellHandler {
    authorized: Arc<[PublicKey]>,
    user: String,
    shells: HashMap<ChannelId, FakeShell>,
}

impl ShellHandler {
    fn run_line(
        &mut self,
        channel: ChannelId,
        line: &str,
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        let shell = self.shells.entry(channel).or_default();
        let line = line.trim();
        if let Some(code) = line.strip_prefix("exit") {
            let code = code.trim().parse().unwrap_or(0);
            session.exit_status_request(channel, code)?;
            session.eof(channel)?;
            return session.close(channel);
        }
        let reply = match line {
            "" => String::new(),
            "size" => format!("{}x{}\r\n", shell.cols, shell.rows),
            "whoami" => format!("{}\r\n", self.user),
            other => format!("echo: {other}\r\n"),
        };
        session.data(channel, format!("{reply}$ ").into_bytes())
    }
}

impl server::Handler for ShellHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == TEST_USER && password == TEST_PASSWORD {
            self.user = user.to_owned();
            return Ok(Auth::Accept);
        }
        Ok(Auth::reject())
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        if user == TEST_USER
            && self
                .authorized
                .iter()
                .any(|k| k.key_data() == key.key_data())
        {
            self.user = user.to_owned();
            return Ok(Auth::Accept);
        }
        Ok(Auth::reject())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.shells.insert(channel.id(), FakeShell::default());
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let shell = self.shells.entry(channel).or_default();
        shell.cols = cols;
        shell.rows = rows;
        session.channel_success(channel)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let shell = self.shells.entry(channel).or_default();
        shell.cols = cols;
        shell.rows = rows;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"$ ".to_vec())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let command = String::from_utf8_lossy(command);
        let status = if let Some(text) = command.strip_prefix("echo ") {
            session.data(channel, format!("{text}\n").into_bytes())?;
            0
        } else if let Some(code) = command.strip_prefix("exit ") {
            code.trim().parse().unwrap_or(0)
        } else {
            session.extended_data(
                channel,
                1,
                format!("unknown command: {command}\n").into_bytes(),
            )?;
            127
        };
        session.exit_status_request(channel, status)?;
        session.eof(channel)?;
        session.close(channel)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        for &byte in data {
            if byte == b'\r' || byte == b'\n' {
                let shell = self.shells.entry(channel).or_default();
                let line = String::from_utf8_lossy(&std::mem::take(&mut shell.line)).into_owned();
                session.data(channel, b"\r\n".to_vec())?;
                self.run_line(channel, &line, session)?;
            } else {
                self.shells.entry(channel).or_default().line.push(byte);
                session.data(channel, vec![byte])?;
            }
        }
        Ok(())
    }
}
