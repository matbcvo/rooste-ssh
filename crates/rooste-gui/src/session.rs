//! A connection running on the tokio runtime, talking to the UI thread
//! through channels so a slow network never blocks rendering.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;

use futures::future::BoxFuture;
use rooste_proto::Transport;
use rooste_proto::ssh::{
    AgentSource, Auth, HostKeyPolicy, HostKeyPrompt, PtyRequest, SshConfig, SshError,
    UnknownHostKey, connect, load_private_key,
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
}

/// From the session to the UI.
#[derive(Debug)]
pub enum SessionEvent {
    Status(String),
    /// The server's host key is unknown; answer `true` to trust it.
    HostKey {
        key: UnknownHostKey,
        reply: oneshot::Sender<bool>,
    },
    Connected,
    Output(Vec<u8>),
    Closed {
        exit_status: Option<u32>,
    },
    Failed(String),
}

/// From the UI to the session.
#[derive(Debug)]
pub enum SessionCommand {
    Input(Vec<u8>),
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
                Err(message) => SessionEvent::Failed(message),
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
) -> Result<Option<u32>, String> {
    sink.send(SessionEvent::Status(format!(
        "Connecting to {}:{}…",
        request.host, request.port
    )));
    let mut config = SshConfig::new(request.host.clone(), request.port);
    config.allow_legacy_algorithms = request.allow_legacy;
    config.host_key_policy = HostKeyPolicy::Ask(Arc::new(UiPrompt(sink.clone())));
    if let Some(path) = &request.known_hosts {
        config.known_hosts = path.clone();
    }

    let mut client = connect(&config).await.map_err(describe)?;
    sink.send(SessionEvent::Status("Authenticating…".into()));
    if !authenticate(&mut client, &request).await? {
        return Err(format!(
            "Authentication failed for {}@{}",
            request.user, request.host
        ));
    }

    let pty = PtyRequest {
        cols: request.cols.max(1),
        rows: request.rows.max(1),
        ..PtyRequest::default()
    };
    let mut shell = client.open_shell(&pty).await.map_err(describe)?;
    sink.send(SessionEvent::Connected);

    let mut buf = vec![0u8; 64 * 1024];
    loop {
        tokio::select! {
            read = shell.read(&mut buf) => match read {
                Ok(0) => break,
                Ok(n) => sink.send(SessionEvent::Output(buf[..n].to_vec())),
                Err(e) => return Err(format!("Connection lost: {e}")),
            },
            command = commands.recv() => match command {
                Some(SessionCommand::Input(data)) => {
                    shell.write(&data).await.map_err(|e| format!("Connection lost: {e}"))?;
                }
                Some(SessionCommand::Resize(cols, rows)) => {
                    let _ = shell.resize(cols, rows).await;
                }
                Some(SessionCommand::Close) | None => {
                    let _ = shell.close().await;
                    break;
                }
            },
        }
    }
    let exit_status = shell.exit_status();
    let _ = client.disconnect().await;
    Ok(exit_status)
}

async fn authenticate(
    client: &mut rooste_proto::ssh::SshClient,
    request: &ConnectRequest,
) -> Result<bool, String> {
    let user = request.user.as_str();
    if request.use_agent {
        match client
            .authenticate(user, &Auth::Agent(AgentSource::Default))
            .await
        {
            Ok(true) => return Ok(true),
            Ok(false) | Err(SshError::Agent(_)) => {}
            Err(e) => return Err(describe(e)),
        }
    }
    if let Some(path) = &request.key_file {
        let key = load_private_key(path, request.key_passphrase.as_deref()).map_err(describe)?;
        if client
            .authenticate(user, &Auth::PublicKey(Arc::new(key)))
            .await
            .map_err(describe)?
        {
            return Ok(true);
        }
    }
    if let Some(password) = &request.password {
        return client
            .authenticate(user, &Auth::password(password.clone()))
            .await
            .map_err(describe);
    }
    Ok(false)
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
        message.push_str(
            ". This server only supports outdated algorithms; enable \"Allow legacy algorithms\" \
             if you trust the network.",
        );
    }
    message
}
