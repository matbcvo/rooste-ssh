//! In-process SSH agent: a Unix socket on Linux, a named pipe on Windows.

use std::path::{Path, PathBuf};

use russh::keys::agent::client::AgentClient;
use russh::keys::agent::server::serve;
use tokio::task::JoinHandle;

use crate::private_key;

/// A running SSH agent holding the `id_ed25519` test key. Stops when dropped.
#[derive(Debug)]
pub struct TestAgent {
    path: PathBuf,
    task: JoinHandle<()>,
    #[cfg(unix)]
    _dir: tempfile::TempDir,
}

impl TestAgent {
    /// Socket path (Linux) or pipe name (Windows) to connect to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(unix)]
    pub async fn start() -> Self {
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&path).expect("bind agent socket");
        let incoming = Box::pin(futures::stream::unfold(listener, |listener| async move {
            let next = listener.accept().await.map(|(stream, _)| stream);
            Some((next, listener))
        }));
        let task = tokio::spawn(async move {
            let _ = serve(incoming, ()).await;
        });

        let mut client = AgentClient::connect_uds(&path)
            .await
            .expect("connect agent");
        add_test_key(&mut client).await;
        Self {
            path,
            task,
            _dir: dir,
        }
    }

    #[cfg(windows)]
    pub async fn start() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::net::windows::named_pipe::ServerOptions;

        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            r"\\.\pipe\rooste-test-agent-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .expect("create agent pipe");
        let incoming = Box::pin(futures::stream::unfold(
            (name.clone(), first),
            |(name, server)| async move {
                let connected = server.connect().await;
                let next = ServerOptions::new().create(&name).ok()?;
                Some((connected.map(|()| server), (name, next)))
            },
        ));
        let task = tokio::spawn(async move {
            let _ = serve(incoming, ()).await;
        });

        let mut client = AgentClient::connect_named_pipe(&name)
            .await
            .expect("connect agent");
        add_test_key(&mut client).await;
        Self {
            path: PathBuf::from(name),
            task,
        }
    }
}

async fn add_test_key<S>(client: &mut AgentClient<S>)
where
    S: russh::keys::agent::client::AgentStream + Unpin,
{
    client
        .add_identity(&private_key("id_ed25519"), &[])
        .await
        .expect("add test key to agent");
}

impl Drop for TestAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}
