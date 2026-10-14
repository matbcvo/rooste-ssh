//! A connection running on the tokio runtime, talking to the UI thread
//! through channels so a slow network never blocks rendering.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;

use futures::future::BoxFuture;
use rooste_core::{Destination, Tunnel, TunnelKind};
use rooste_proto::Transport;
use rooste_proto::ssh::{
    AgentSource, Auth, Forward, HostKeyPolicy, HostKeyPrompt, PtyRequest, SshClient, SshConfig,
    SshError, UnknownHostKey, connect, connect_via, load_private_key,
};
use tokio::sync::{mpsc, oneshot};

/// Everything needed to open a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectRequest {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub key_file: Option<PathBuf>,
    pub key_passphrase: Option<String>,
    pub use_agent: bool,
    pub allow_legacy: bool,
    /// Override for tests; `None` uses `~/.ssh/known_hosts`.
    pub known_hosts: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    /// Ask for a password when no other method works (as `ssh` does).
    pub prompt_for_password: bool,
    /// Jump hosts, as OpenSSH `ProxyJump`.
    pub proxy_jump: Option<String>,
    pub forward_agent: bool,
    /// Agent used for login and forwarding.
    pub agent: AgentSource,
    pub tunnels: Vec<Tunnel>,
}

/// How a session ended unsuccessfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    /// The server would be reachable with legacy algorithms enabled.
    pub legacy_hint: bool,
    /// An established connection dropped (worth reconnecting).
    pub lost: bool,
}

impl Failure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            legacy_hint: false,
            lost: false,
        }
    }

    fn lost(message: impl Into<String>) -> Self {
        Self {
            lost: true,
            ..Self::new(message)
        }
    }
}

impl From<SshError> for Failure {
    fn from(error: SshError) -> Self {
        let legacy_hint = matches!(
            error,
            SshError::NoCommonAlgorithm {
                legacy_would_help: true,
                ..
            }
        );
        Self {
            message: describe(error),
            legacy_hint,
            lost: false,
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

/// State of one saved tunnel after the session started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelStatus {
    pub description: String,
    /// `Err` explains why the tunnel could not start.
    pub result: Result<String, String>,
}

/// What a prompt asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Password { user: String, host: String },
    Passphrase { key_file: PathBuf },
}

const PROMPT_ATTEMPTS: usize = 3;

/// From the session to the UI.
#[derive(Debug)]
pub enum SessionEvent {
    Status(String),
    /// The server's host key is unknown; answer `true` to trust it.
    HostKey {
        key: UnknownHostKey,
        reply: oneshot::Sender<bool>,
    },
    /// A secret is needed; answer `None` to cancel.
    Prompt {
        kind: PromptKind,
        /// The previous attempt was rejected.
        retry: bool,
        reply: oneshot::Sender<Option<String>>,
    },
    /// Login succeeded with this password (so the UI may save it).
    PasswordAccepted(String),
    Connected,
    /// The SFTP session for the file panel is ready.
    Files(crate::files::FilesHandle),
    FilesFailed(String),
    Tunnels(Vec<TunnelStatus>),
    Output(Vec<u8>),
    Closed {
        exit_status: Option<u32>,
    },
    Failed(Failure),
}

/// From the UI to the session.
#[derive(Debug)]
pub enum SessionCommand {
    Input(Vec<u8>),
    /// Start SFTP for the file panel.
    OpenFiles,
    Resize(u16, u16),
    Close,
}

/// Handle owned by the UI.
#[derive(Debug)]
pub struct Session {
    commands: mpsc::UnboundedSender<SessionCommand>,
    events: std_mpsc::Receiver<SessionEvent>,
}

impl Session {
    /// Start connecting. `wake` is called whenever an event is queued, so
    /// the UI can repaint.
    pub fn spawn(
        runtime: &tokio::runtime::Handle,
        request: ConnectRequest,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = std_mpsc::channel();
        let sink = EventSink {
            tx: event_tx,
            wake: Arc::new(wake),
        };
        runtime.spawn(async move {
            let done = match run(request, command_rx, &sink).await {
                Ok(exit_status) => SessionEvent::Closed { exit_status },
                Err(failure) => SessionEvent::Failed(failure),
            };
            sink.send(done);
        });
        Self { commands, events }
    }

    pub fn send(&self, command: SessionCommand) {
        let _ = self.commands.send(command);
    }

    /// Events queued since the last call.
    pub fn poll(&self) -> Vec<SessionEvent> {
        self.events.try_iter().collect()
    }
}

#[derive(Clone)]
struct EventSink {
    tx: std_mpsc::Sender<SessionEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl EventSink {
    fn send(&self, event: SessionEvent) {
        if self.tx.send(event).is_ok() {
            (self.wake)();
        }
    }
}

struct UiPrompt(EventSink);

impl HostKeyPrompt for UiPrompt {
    fn confirm_unknown(&self, key: UnknownHostKey) -> BoxFuture<'static, bool> {
        let (reply, answer) = oneshot::channel();
        self.0.send(SessionEvent::HostKey { key, reply });
        Box::pin(async move { answer.await.unwrap_or(false) })
    }
}

async fn run(
    request: ConnectRequest,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    sink: &EventSink,
) -> Result<Option<u32>, Failure> {
    let config_for = |host: &str, port: u16| {
        let mut config = SshConfig::new(host, port);
        config.allow_legacy_algorithms = request.allow_legacy;
        config.host_key_policy = HostKeyPolicy::Ask(Arc::new(UiPrompt(sink.clone())));
        if let Some(path) = &request.known_hosts {
            config.known_hosts = path.clone();
        }
        config
    };

    // Each jump host is reached through the previous one.
    let hops = match request.proxy_jump.as_deref() {
        Some(jump) => Destination::parse_list(jump).map_err(|e| format!("Jump host: {e}"))?,
        None => Vec::new(),
    };
    let mut previous: Option<SshClient> = None;
    for hop in hops {
        let user = hop.user.clone().unwrap_or_else(|| request.user.clone());
        let port = hop.port.unwrap_or(22);
        sink.send(SessionEvent::Status(format!(
            "Connecting to jump host {}:{port}…",
            hop.host
        )));
        let config = config_for(&hop.host, port);
        let mut client = match previous.take() {
            None => connect(&config).await?,
            Some(via) => connect_via(via, &config).await?,
        };
        let login = Login {
            user: &user,
            host: &hop.host,
            password: None,
            target: false,
        };
        if !authenticate(&mut client, &login, &request, sink).await? {
            return Err(format!("Authentication failed for jump host {user}@{}", hop.host).into());
        }
        previous = Some(client);
    }

    sink.send(SessionEvent::Status(format!(
        "Connecting to {}:{}…",
        request.host, request.port
    )));
    let config = config_for(&request.host, request.port);
    let mut client = match previous {
        None => connect(&config).await?,
        Some(via) => connect_via(via, &config).await?,
    };
    sink.send(SessionEvent::Status("Authenticating…".into()));
    let login = Login {
        user: &request.user,
        host: &request.host,
        password: request.password.as_deref(),
        target: true,
    };
    if !authenticate(&mut client, &login, &request, sink).await? {
        return Err(format!(
            "Authentication failed for {}@{}",
            request.user, request.host
        )
        .into());
    }

    let (_forwards, statuses) = start_tunnels(&client, &request.tunnels).await;
    if !statuses.is_empty() {
        sink.send(SessionEvent::Tunnels(statuses));
    }

    let pty = PtyRequest {
        cols: request.cols.max(1),
        rows: request.rows.max(1),
        ..PtyRequest::default()
    };
    let agent = request.forward_agent.then(|| request.agent.clone());
    let mut shell = client.open_shell_with(&pty, agent).await?;
    sink.send(SessionEvent::Connected);

    let mut buf = vec![0u8; 64 * 1024];
    loop {
        tokio::select! {
            read = shell.read(&mut buf) => match read {
                Ok(0) => break,
                Ok(n) => sink.send(SessionEvent::Output(buf[..n].to_vec())),
                Err(e) => return Err(Failure::lost(format!("Connection lost: {e}"))),
            },
            command = commands.recv() => match command {
                Some(SessionCommand::Input(data)) => {
                    if let Err(e) = shell.write(&data).await {
                        return Err(Failure::lost(format!("Connection lost: {e}")));
                    }
                }
                Some(SessionCommand::Resize(cols, rows)) => {
                    let _ = shell.resize(cols, rows).await;
                }
                Some(SessionCommand::OpenFiles) => match rooste_sftp::Sftp::open(&client).await {
                    Ok(sftp) => {
                        let home = sftp.home().await.unwrap_or_else(|_| "/".into());
                        sink.send(SessionEvent::Files(crate::files::spawn(sftp, home, sink.wake.clone())));
                    }
                    Err(e) => sink.send(SessionEvent::FilesFailed(format!("SFTP is not available: {e}"))),
                },
                Some(SessionCommand::Close) | None => {
                    let _ = shell.close().await;
                    let _ = client.disconnect().await;
                    return Ok(shell.exit_status());
                }
            },
        }
    }
    let exit_status = shell.exit_status();
    // The channel closed without an exit status because the connection
    // itself went away.
    if exit_status.is_none() && client.is_closed() {
        return Err(Failure::lost("Connection lost"));
    }
    let _ = client.disconnect().await;
    Ok(exit_status)
}

/// Start saved tunnels, reporting each one's outcome. The forwards stop
/// when the returned values are dropped.
async fn start_tunnels(
    client: &SshClient,
    tunnels: &[Tunnel],
) -> (Vec<Forward>, Vec<TunnelStatus>) {
    let mut forwards = Vec::new();
    let mut statuses = Vec::new();
    for tunnel in tunnels {
        let result = async {
            let (listen_host, listen_port) = tunnel.listen_addr()?;
            let bind = if listen_host.contains(':') {
                format!("[{listen_host}]:{listen_port}")
            } else {
                format!("{listen_host}:{listen_port}")
            };
            let forward = match tunnel.kind {
                TunnelKind::Local => {
                    let (host, port) = tunnel.target_addr()?;
                    client.forward_local(&bind, &host, port).await
                }
                TunnelKind::Dynamic => client.forward_dynamic(&bind).await,
                TunnelKind::Remote => {
                    let (host, port) = tunnel.target_addr()?;
                    client
                        .forward_remote(&listen_host, listen_port, &host, port)
                        .await
                }
            }
            .map_err(describe)?;
            Ok::<_, String>(forward)
        }
        .await;
        statuses.push(TunnelStatus {
            description: tunnel.describe(),
            result: result
                .as_ref()
                .map(|f| format!("listening on {}", f.listen))
                .map_err(Clone::clone),
        });
        if let Ok(forward) = result {
            forwards.push(forward);
        }
    }
    (forwards, statuses)
}

/// Who to log in as on one hop.
struct Login<'a> {
    user: &'a str,
    host: &'a str,
    /// Saved or entered password to try first.
    password: Option<&'a str>,
    /// The destination itself (prompted passwords may be saved).
    target: bool,
}

async fn authenticate(
    client: &mut SshClient,
    login: &Login<'_>,
    request: &ConnectRequest,
    sink: &EventSink,
) -> Result<bool, String> {
    let user = login.user;
    if request.use_agent {
        match client
            .authenticate(user, &Auth::Agent(request.agent.clone()))
            .await
        {
            Ok(true) => return Ok(true),
            Ok(false) | Err(SshError::Agent(_)) => {}
            Err(e) => return Err(describe(e)),
        }
    }
    if let Some(path) = &request.key_file
        && let Some(key) = load_key(path, request.key_passphrase.as_deref(), sink).await?
        && client
            .authenticate(user, &Auth::PublicKey(Arc::new(key)))
            .await
            .map_err(describe)?
    {
        return Ok(true);
    }

    let mut retry = false;
    if let Some(password) = login.password {
        if try_password(client, user, password).await? {
            return Ok(true);
        }
        retry = true;
    }
    if !request.prompt_for_password {
        return Ok(false);
    }
    for _ in 0..PROMPT_ATTEMPTS {
        let kind = PromptKind::Password {
            user: user.to_owned(),
            host: login.host.to_owned(),
        };
        let Some(password) = prompt(sink, kind, retry).await else {
            return Err("Login cancelled".into());
        };
        if try_password(client, user, &password).await? {
            if login.target {
                sink.send(SessionEvent::PasswordAccepted(password));
            }
            return Ok(true);
        }
        retry = true;
    }
    Ok(false)
}

async fn try_password(
    client: &mut rooste_proto::ssh::SshClient,
    user: &str,
    password: &str,
) -> Result<bool, String> {
    client
        .authenticate(user, &Auth::password(password))
        .await
        .map_err(describe)
}

/// Load a key, asking for its passphrase if it is encrypted and none was
/// given. `None` if the user cancels.
async fn load_key(
    path: &std::path::Path,
    passphrase: Option<&str>,
    sink: &EventSink,
) -> Result<Option<rooste_proto::ssh::PrivateKey>, String> {
    match load_private_key(path, passphrase) {
        Ok(key) => return Ok(Some(key)),
        Err(SshError::KeyEncrypted(_)) => {}
        Err(e) if passphrase.is_none() => return Err(describe(e)),
        Err(_) => {}
    }
    let mut retry = passphrase.is_some();
    for _ in 0..PROMPT_ATTEMPTS {
        let kind = PromptKind::Passphrase {
            key_file: path.to_owned(),
        };
        let Some(passphrase) = prompt(sink, kind, retry).await else {
            return Ok(None);
        };
        if let Ok(key) = load_private_key(path, Some(&passphrase)) {
            return Ok(Some(key));
        }
        retry = true;
    }
    Ok(None)
}

async fn prompt(sink: &EventSink, kind: PromptKind, retry: bool) -> Option<String> {
    let (reply, answer) = oneshot::channel();
    sink.send(SessionEvent::Prompt { kind, retry, reply });
    answer.await.ok().flatten()
}

/// A user-facing message for an error, including its causes.
pub fn describe(error: SshError) -> String {
    use std::error::Error;
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    if let SshError::NoCommonAlgorithm {
        legacy_would_help: true,
        ..
    } = error
    {
        message.push_str(". This server only supports outdated algorithms.");
    }
    message
}
