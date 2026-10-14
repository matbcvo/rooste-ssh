//! SSH transport built on `russh`.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use rooste_proto::Transport;
//! use rooste_proto::ssh::{Auth, PtyRequest, SshConfig, connect};
//!
//! let mut client = connect(&SshConfig::new("example.com", 22)).await?;
//! client.authenticate("me", &Auth::password("secret")).await?;
//! let mut shell = client.open_shell(&PtyRequest::default()).await?;
//! shell.write(b"ls\n").await?;
//! # Ok(()) }
//! ```

mod agent;
pub mod algorithms;
mod forward;
pub mod known_hosts;
mod legacy_pem;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, Msg};
use russh::keys::agent::AgentIdentity;
use russh::keys::{self, Algorithm, HashAlg, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect};
use zeroize::Zeroizing;

pub use agent::AgentSource;
#[cfg(windows)]
pub use agent::OPENSSH_AGENT_PIPE;
pub use forward::{Forward, ForwardKind};
pub use known_hosts::{HostKeyPolicy, HostKeyPrompt, HostKeyStatus, UnknownHostKey};
pub use russh::keys::{PrivateKey, PublicKey};

use crate::Transport;

/// Byte stream of a subsystem channel.
pub type SubsystemStream = russh::ChannelStream<Msg>;

/// Errors from connecting, authenticating or using an SSH session.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("could not connect")]
    Connect(#[source] io::Error),
    #[error("connection timed out")]
    Timeout,
    #[error(
        "the host key for {host} has changed (known_hosts line {line}); this could be a \
         man-in-the-middle attack. The server now presents {fingerprint}"
    )]
    HostKeyChanged {
        host: String,
        line: usize,
        fingerprint: String,
    },
    #[error("host key {fingerprint} for {host} was not trusted")]
    HostKeyRejected { host: String, fingerprint: String },
    #[error("no common {kind} algorithm with the server (server offers: {theirs})")]
    NoCommonAlgorithm {
        kind: String,
        theirs: String,
        /// The server would be reachable with legacy algorithms enabled.
        legacy_would_help: bool,
    },
    #[error("could not use known_hosts")]
    KnownHosts(#[source] keys::Error),
    #[error("SSH agent: {0}")]
    Agent(String),
    #[error("could not load key {}", path.display())]
    Key {
        path: PathBuf,
        #[source]
        source: keys::Error,
    },
    #[error("the key {} is encrypted and needs a passphrase", .0.display())]
    KeyEncrypted(PathBuf),
    #[error("{} is a public key; choose the private key file (usually the same name without .pub)", .0.display())]
    PublicKeyFile(PathBuf),
    #[error("{} is not a private key file", .0.display())]
    NotAKey(PathBuf),
    #[error("could not load key {}: {message}", path.display())]
    KeyFormat { path: PathBuf, message: String },
    #[error("could not set up forwarding on {bind}")]
    Forward {
        bind: String,
        #[source]
        source: io::Error,
    },
    #[error("authentication must finish before port forwarding starts")]
    AuthAfterForwarding,
    #[error(transparent)]
    Protocol(#[from] russh::Error),
}

/// Load a private key, decrypting it with `passphrase` if given.
pub fn load_private_key(path: &Path, passphrase: Option<&str>) -> Result<PrivateKey, SshError> {
    // Recognize files that aren't private keys before russh tries every
    // format on them and reports a confusing parse error.
    if let Ok(text) = std::fs::read_to_string(path) {
        let start = text.trim_start();
        if start.starts_with("ssh-")
            || start.starts_with("ecdsa-")
            || start.starts_with("sk-")
            || start.starts_with("---- BEGIN SSH2 PUBLIC KEY")
        {
            return Err(SshError::PublicKeyFile(path.to_owned()));
        }
        if !start.starts_with("PuTTY-User-Key-File-") && !text.contains("PRIVATE KEY-----") {
            return Err(SshError::NotAKey(path.to_owned()));
        }
        if legacy_pem::is_encrypted(&text) {
            let Some(passphrase) = passphrase else {
                return Err(SshError::KeyEncrypted(path.to_owned()));
            };
            let plain = legacy_pem::decrypt(&text, passphrase).map_err(|e| match e {
                legacy_pem::Error::BadPassphrase => SshError::KeyEncrypted(path.to_owned()),
                legacy_pem::Error::Format(message) => SshError::KeyFormat {
                    path: path.to_owned(),
                    message,
                },
            })?;
            return keys::decode_secret_key(&plain, None).map_err(|source| SshError::Key {
                path: path.to_owned(),
                source,
            });
        }
    }
    keys::load_secret_key(path, passphrase).map_err(|e| match e {
        keys::Error::KeyIsEncrypted => SshError::KeyEncrypted(path.to_owned()),
        // PuTTY keys report encryption through ssh-key's own error type.
        _ if passphrase.is_none() && is_encrypted_ppk(path) => {
            SshError::KeyEncrypted(path.to_owned())
        }
        source => SshError::Key {
            path: path.to_owned(),
            source,
        },
    })
}

fn is_encrypted_ppk(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.starts_with("PuTTY-User-Key-File-")
            && text.lines().any(|l| l.trim() == "Encryption: aes256-cbc")
    })
}

/// Connection settings for one SSH session.
#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub known_hosts: PathBuf,
    pub host_key_policy: HostKeyPolicy,
    /// Offer the legacy algorithm set (see [`algorithms`]).
    pub allow_legacy_algorithms: bool,
    pub connect_timeout: Duration,
    pub keepalive_interval: Option<Duration>,
}

impl SshConfig {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            known_hosts: known_hosts::default_known_hosts_path()
                .unwrap_or_else(|| PathBuf::from("known_hosts")),
            host_key_policy: HostKeyPolicy::Strict,
            allow_legacy_algorithms: false,
            connect_timeout: Duration::from_secs(15),
            keepalive_interval: Some(Duration::from_secs(30)),
        }
    }
}

/// An authentication method.
#[derive(Clone)]
pub enum Auth {
    Password(Zeroizing<String>),
    PublicKey(Arc<PrivateKey>),
    Agent(AgentSource),
}

impl Auth {
    pub fn password(password: impl Into<String>) -> Self {
        Self::Password(Zeroizing::new(password.into()))
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password(_) => f.write_str("Password(..)"),
            Self::PublicKey(key) => write!(f, "PublicKey({})", key.algorithm()),
            Self::Agent(source) => write!(f, "Agent({source:?})"),
        }
    }
}

/// Terminal requested for an interactive shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyRequest {
    pub term: String,
    pub cols: u16,
    pub rows: u16,
}

impl Default for PtyRequest {
    fn default() -> Self {
        Self {
            term: "xterm-256color".into(),
            cols: 80,
            rows: 24,
        }
    }
}

/// Output of a non-interactive command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

/// How host-key verification ended, recorded by the handler so `connect`
/// can return a specific error instead of a generic protocol failure.
#[derive(Debug)]
enum HostKeyFailure {
    Changed { line: usize, fingerprint: String },
    Rejected { fingerprint: String },
    KnownHosts(keys::Error),
}

/// Remote forwards: (address, port) the server listens on -> local target.
type Routes = Arc<Mutex<HashMap<(String, u32), (String, u16)>>>;

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    policy: HostKeyPolicy,
    failure: Arc<Mutex<Option<HostKeyFailure>>>,
    routes: Routes,
    /// Agent that forwarded agent connections are relayed to.
    agent: Arc<Mutex<Option<AgentSource>>>,
}

impl ClientHandler {
    fn fail(&self, failure: HostKeyFailure) -> bool {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(failure);
        false
    }

    async fn verify(&self, key: &PublicKey) -> bool {
        let fingerprint = known_hosts::fingerprint(key);
        match known_hosts::check(&self.known_hosts, &self.host, self.port, key) {
            Ok(HostKeyStatus::Known) => true,
            Ok(HostKeyStatus::Changed { line }) => {
                self.fail(HostKeyFailure::Changed { line, fingerprint })
            }
            Err(e) => self.fail(HostKeyFailure::KnownHosts(e)),
            Ok(HostKeyStatus::Unknown) => {
                let trusted = match &self.policy {
                    HostKeyPolicy::Strict => false,
                    HostKeyPolicy::AcceptNew => true,
                    HostKeyPolicy::Ask(prompt) => {
                        prompt
                            .confirm_unknown(UnknownHostKey {
                                host: self.host.clone(),
                                port: self.port,
                                algorithm: key.algorithm().to_string(),
                                fingerprint: fingerprint.clone(),
                            })
                            .await
                    }
                };
                if !trusted {
                    return self.fail(HostKeyFailure::Rejected { fingerprint });
                }
                match known_hosts::learn(&self.known_hosts, &self.host, self.port, key) {
                    Ok(()) => true,
                    Err(e) => self.fail(HostKeyFailure::KnownHosts(e)),
                }
            }
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        match server_key {
            keys::PublicKeyOrCertificate::PublicKey { key, .. } => Ok(self.verify(key).await),
            // Host certificates are never requested (see `algorithms`).
            keys::PublicKeyOrCertificate::Certificate(_) => Ok(false),
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let target = lock(&self.routes)
            .get(&(connected_address.to_owned(), connected_port))
            .cloned();
        let Some((host, port)) = target else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            if let Ok(mut tcp) = tokio::net::TcpStream::connect((host.as_str(), port)).await {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
            }
        });
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some(source) = lock(&self.agent).clone() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            if let Ok(mut agent) = agent::connect_stream(&source).await {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut agent).await;
            }
        });
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Connect to an SSH server and verify its host key.
pub async fn connect(config: &SshConfig) -> Result<SshClient, SshError> {
    let parts = Parts::default();
    let connecting = client::connect(
        parts.russh_config(config),
        (config.host.as_str(), config.port),
        parts.handler(config),
    );
    parts.finish(config, connecting, None).await
}

/// Connect to `config`'s server through an authenticated jump host (as
/// OpenSSH `ProxyJump` does). The jump connection stays open as long as the
/// returned client.
pub async fn connect_via(jump: SshClient, config: &SshConfig) -> Result<SshClient, SshError> {
    let channel = jump
        .handle
        .channel_open_direct_tcpip(config.host.as_str(), config.port.into(), "127.0.0.1", 0)
        .await
        .map_err(|e| match e {
            russh::Error::ChannelOpenFailure(_) => SshError::Connect(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!(
                    "the jump host could not reach {}:{}",
                    config.host, config.port
                ),
            )),
            other => SshError::Protocol(other),
        })?;
    let parts = Parts::default();
    let connecting = client::connect_stream(
        parts.russh_config(config),
        channel.into_stream(),
        parts.handler(config),
    );
    parts.finish(config, connecting, Some(Box::new(jump))).await
}

/// State shared between a connection's handler and its client.
#[derive(Default)]
struct Parts {
    failure: Arc<Mutex<Option<HostKeyFailure>>>,
    routes: Routes,
    agent: Arc<Mutex<Option<AgentSource>>>,
}

impl Parts {
    fn handler(&self, config: &SshConfig) -> ClientHandler {
        ClientHandler {
            host: config.host.clone(),
            port: config.port,
            known_hosts: config.known_hosts.clone(),
            policy: config.host_key_policy.clone(),
            failure: self.failure.clone(),
            routes: self.routes.clone(),
            agent: self.agent.clone(),
        }
    }

    fn russh_config(&self, config: &SshConfig) -> Arc<client::Config> {
        Arc::new(client::Config {
            preferred: algorithms::preferred(config.allow_legacy_algorithms),
            keepalive_interval: config.keepalive_interval,
            nodelay: true,
            ..Default::default()
        })
    }

    async fn finish(
        self,
        config: &SshConfig,
        connecting: impl std::future::Future<Output = Result<Handle<ClientHandler>, russh::Error>>,
        via: Option<Box<SshClient>>,
    ) -> Result<SshClient, SshError> {
        let result = tokio::time::timeout(config.connect_timeout, connecting)
            .await
            .map_err(|_| SshError::Timeout)?;
        let handle = result.map_err(|e| match lock(&self.failure).take() {
            Some(failure) => host_key_error(&config.host, failure),
            None => connect_error(e, config.allow_legacy_algorithms),
        })?;
        Ok(SshClient {
            handle: Arc::new(handle),
            allow_legacy_algorithms: config.allow_legacy_algorithms,
            routes: self.routes,
            agent: self.agent,
            _via: via,
        })
    }
}

fn host_key_error(host: &str, failure: HostKeyFailure) -> SshError {
    let host = host.to_owned();
    match failure {
        HostKeyFailure::Changed { line, fingerprint } => SshError::HostKeyChanged {
            host,
            line,
            fingerprint,
        },
        HostKeyFailure::Rejected { fingerprint } => SshError::HostKeyRejected { host, fingerprint },
        HostKeyFailure::KnownHosts(e) => SshError::KnownHosts(e),
    }
}

fn connect_error(error: russh::Error, legacy_enabled: bool) -> SshError {
    match error {
        russh::Error::IO(e) => SshError::Connect(e),
        russh::Error::NoCommonAlgo { kind, theirs, .. } => SshError::NoCommonAlgorithm {
            kind: format!("{kind:?}").to_lowercase(),
            legacy_would_help: !legacy_enabled && theirs.iter().any(|n| algorithms::is_legacy(n)),
            theirs: theirs.join(", "),
        },
        other => SshError::Protocol(other),
    }
}

/// A connected SSH session. Authenticate, then open shells or run commands.
pub struct SshClient {
    handle: Arc<Handle<ClientHandler>>,
    allow_legacy_algorithms: bool,
    routes: Routes,
    agent: Arc<Mutex<Option<AgentSource>>>,
    /// Jump host this connection is tunneled through.
    _via: Option<Box<SshClient>>,
}

impl fmt::Debug for SshClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshClient")
            .field("closed", &self.handle.is_closed())
            .finish_non_exhaustive()
    }
}

impl SshClient {
    /// Try one authentication method. Returns `Ok(false)` if the server
    /// rejected it, so the caller can try the next method.
    pub async fn authenticate(&mut self, user: &str, auth: &Auth) -> Result<bool, SshError> {
        match auth {
            Auth::Password(password) => Ok(self
                .handle_mut()?
                .authenticate_password(user, password.as_str())
                .await?
                .success()),
            Auth::PublicKey(key) => {
                let hash = self.rsa_hash(key.algorithm()).await?;
                let key = PrivateKeyWithHashAlg::new(key.clone(), hash);
                Ok(self
                    .handle_mut()?
                    .authenticate_publickey(user, key)
                    .await?
                    .success())
            }
            Auth::Agent(source) => self.authenticate_agent(user, source).await,
        }
    }

    async fn authenticate_agent(
        &mut self,
        user: &str,
        source: &AgentSource,
    ) -> Result<bool, SshError> {
        let agent_error = |e: keys::Error| SshError::Agent(e.to_string());
        let mut agent = agent::connect(source).await.map_err(agent_error)?;
        let identities = agent.request_identities().await.map_err(agent_error)?;
        for identity in identities {
            let AgentIdentity::PublicKey { key, .. } = identity else {
                continue;
            };
            let hash = self.rsa_hash(key.algorithm()).await?;
            let result = self
                .handle_mut()?
                .authenticate_publickey_with(user, key, hash, &mut agent)
                .await
                .map_err(|e| SshError::Agent(e.to_string()))?;
            if result.success() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Exclusive access for authentication, which must happen before any
    /// forwarding task shares the connection.
    fn handle_mut(&mut self) -> Result<&mut Handle<ClientHandler>, SshError> {
        Arc::get_mut(&mut self.handle).ok_or(SshError::AuthAfterForwarding)
    }

    /// Signature hash for an RSA key: SHA-2 unless the server only supports
    /// SHA-1 and legacy algorithms are enabled.
    async fn rsa_hash(&self, algorithm: Algorithm) -> Result<Option<HashAlg>, SshError> {
        if !matches!(algorithm, Algorithm::Rsa { .. }) {
            return Ok(None);
        }
        Ok(match self.handle.best_supported_rsa_hash().await? {
            Some(Some(hash)) => Some(hash),
            _ if self.allow_legacy_algorithms => None,
            _ => Some(HashAlg::Sha512),
        })
    }

    /// Open an interactive shell on a pseudo-terminal.
    pub async fn open_shell(&self, pty: &PtyRequest) -> Result<SshShell, SshError> {
        self.open_shell_with(pty, None).await
    }

    /// Open a shell, optionally forwarding the given SSH agent to it.
    pub async fn open_shell_with(
        &self,
        pty: &PtyRequest,
        forward_agent: Option<AgentSource>,
    ) -> Result<SshShell, SshError> {
        let channel = self.handle.channel_open_session().await?;
        self.request_agent_forwarding(&channel, forward_agent)
            .await?;
        channel
            .request_pty(
                false,
                &pty.term,
                pty.cols.into(),
                pty.rows.into(),
                0,
                0,
                &[],
            )
            .await?;
        channel.request_shell(false).await?;
        let (reader, writer) = channel.split();
        Ok(SshShell {
            reader,
            writer,
            pending: Vec::new(),
            pos: 0,
            closed: false,
            exit_status: None,
        })
    }

    async fn request_agent_forwarding(
        &self,
        channel: &russh::Channel<Msg>,
        forward_agent: Option<AgentSource>,
    ) -> Result<(), SshError> {
        if let Some(source) = forward_agent {
            *lock(&self.agent) = Some(source);
            channel.agent_forward(false).await?;
        }
        Ok(())
    }

    /// Open a channel running a subsystem such as `sftp`.
    pub async fn open_subsystem(&self, name: &str) -> Result<SubsystemStream, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, name).await?;
        Ok(channel.into_stream())
    }

    /// Run a command without a terminal and collect its output.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, SshError> {
        self.exec_with(command, None).await
    }

    /// Run a command, optionally forwarding the given SSH agent to it.
    pub async fn exec_with(
        &self,
        command: &str,
        forward_agent: Option<AgentSource>,
    ) -> Result<ExecOutput, SshError> {
        let mut channel = self.handle.channel_open_session().await?;
        self.request_agent_forwarding(&channel, forward_agent)
            .await?;
        channel.exec(true, command).await?;
        let mut output = ExecOutput::default();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => output.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => output.exit_status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(output)
    }

    /// Close the connection.
    pub async fn disconnect(&self) -> Result<(), SshError> {
        self.handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await?;
        Ok(())
    }
}

/// An interactive shell channel.
pub struct SshShell {
    reader: ChannelReadHalf,
    writer: ChannelWriteHalf<Msg>,
    pending: Vec<u8>,
    pos: usize,
    closed: bool,
    exit_status: Option<u32>,
}

impl fmt::Debug for SshShell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshShell")
            .field("closed", &self.closed)
            .field("exit_status", &self.exit_status)
            .finish_non_exhaustive()
    }
}

impl SshShell {
    /// Exit status of the remote shell, once it has exited.
    pub fn exit_status(&self) -> Option<u32> {
        self.exit_status
    }
}

impl Transport for SshShell {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.pos < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.pos);
                buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if self.closed {
                return Ok(0);
            }
            match self.reader.wait().await {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    self.pending.clear();
                    self.pending.extend_from_slice(&data);
                    self.pos = 0;
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.exit_status = Some(exit_status)
                }
                Some(ChannelMsg::Close) | None => self.closed = true,
                Some(_) => {}
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer
            .data_bytes(data.to_vec())
            .await
            .map_err(io::Error::other)
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.writer
            .window_change(cols.into(), rows.into(), 0, 0)
            .await
            .map_err(io::Error::other)
    }

    async fn close(&mut self) -> io::Result<()> {
        self.writer.eof().await.map_err(io::Error::other)?;
        self.writer.close().await.map_err(io::Error::other)
    }
}
