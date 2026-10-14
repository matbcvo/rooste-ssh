//! A built-in SSH agent: keys loaded into rooste-ssh are offered to its own
//! sessions and to any program pointed at the agent's socket (Linux) or
//! named pipe (Windows), like `ssh-agent`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::agent::server::serve;
use russh::keys::{PrivateKey, PublicKey};
use tokio::task::JoinHandle;

use crate::KeyError;

/// A running agent. Stops when dropped.
#[derive(Debug)]
pub struct Agent {
    path: PathBuf,
    task: JoinHandle<()>,
    /// Keys added through this handle. The agent protocol server drops
    /// comments and cannot remove single keys, so both are done from here.
    loaded: Mutex<Vec<PrivateKey>>,
    #[cfg(unix)]
    _dir: Option<tempfile::TempDir>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.task.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.path);
    }
}

fn agent_error(e: russh::keys::Error) -> KeyError {
    KeyError::Io(std::io::Error::other(e.to_string()))
}

impl Agent {
    /// Where programs should connect: set `SSH_AUTH_SOCK` to this.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Start an agent at `path`, or a private default location.
    #[cfg(unix)]
    pub async fn start(path: Option<PathBuf>) -> std::io::Result<Self> {
        use tokio::net::UnixListener;

        let (path, dir) = match path {
            Some(path) => (path, None),
            None => {
                let dir = tempfile::Builder::new().prefix("rooste-agent-").tempdir()?;
                (dir.path().join("agent.sock"), Some(dir))
            }
        };
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        let incoming = Box::pin(futures::stream::unfold(listener, |listener| async move {
            let next = listener.accept().await.map(|(stream, _)| stream);
            Some((next, listener))
        }));
        let task = tokio::spawn(async move {
            let _ = serve(incoming, ()).await;
        });
        Ok(Self {
            path,
            task,
            loaded: Mutex::default(),
            _dir: dir,
        })
    }

    /// Start an agent on the named pipe `path`, or a per-user default.
    #[cfg(windows)]
    pub async fn start(path: Option<PathBuf>) -> std::io::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let name = path
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
                format!(r"\\.\pipe\rooste-ssh-agent-{user}")
            });
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)?;
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
        Ok(Self {
            path: PathBuf::from(name),
            task,
            loaded: Mutex::default(),
        })
    }

    async fn client(&self) -> Result<AgentClient<Box<dyn AgentStream + Send + Unpin>>, KeyError> {
        #[cfg(unix)]
        let client = AgentClient::connect_uds(&self.path)
            .await
            .map_err(agent_error)?;
        #[cfg(windows)]
        let client = AgentClient::connect_named_pipe(&self.path)
            .await
            .map_err(agent_error)?;
        Ok(client.dynamic())
    }

    fn loaded(&self) -> std::sync::MutexGuard<'_, Vec<PrivateKey>> {
        self.loaded.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub async fn add(&self, key: &PrivateKey) -> Result<(), KeyError> {
        self.client()
            .await?
            .add_identity(key, &[])
            .await
            .map_err(agent_error)?;
        let mut loaded = self.loaded();
        loaded.retain(|k| k.public_key().key_data() != key.public_key().key_data());
        loaded.push(key.clone());
        Ok(())
    }

    /// The keys the agent offers, with comments where known.
    pub async fn keys(&self) -> Result<Vec<PublicKey>, KeyError> {
        let identities = self
            .client()
            .await?
            .request_identities()
            .await
            .map_err(agent_error)?;
        let loaded = self.loaded();
        Ok(identities
            .into_iter()
            .map(|identity| {
                let mut key = identity.public_key().into_owned();
                let comment = loaded
                    .iter()
                    .find(|k| k.public_key().key_data() == key.key_data())
                    .map(|k| k.comment().as_str_lossy().to_owned())
                    .unwrap_or_else(|| identity.comment().to_owned());
                key.set_comment(comment);
                key
            })
            .collect())
    }

    /// Send one raw agent request and return the reply's message type.
    /// (russh's client frames `REMOVE_ALL_IDENTITIES` without a length.)
    async fn raw_request(&self, payload: &[u8]) -> Result<u8, KeyError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        #[cfg(unix)]
        let mut stream = tokio::net::UnixStream::connect(&self.path).await?;
        #[cfg(windows)]
        let mut stream = tokio::net::windows::named_pipe::ClientOptions::new().open(&self.path)?;
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(payload);
        stream.write_all(&frame).await?;
        let len = stream.read_u32().await? as usize;
        let mut reply = vec![0u8; len.min(256 * 1024)];
        stream.read_exact(&mut reply).await?;
        Ok(reply.first().copied().unwrap_or(FAILURE))
    }

    /// Remove one key.
    pub async fn remove(&self, key: &PublicKey) -> Result<(), KeyError> {
        self.loaded()
            .retain(|k| k.public_key().key_data() != key.key_data());
        let blob = key.to_bytes()?;
        let mut payload = vec![REMOVE_IDENTITY];
        payload.extend_from_slice(&(blob.len() as u32).to_be_bytes());
        payload.extend_from_slice(&blob);
        match self.raw_request(&payload).await? {
            SUCCESS => Ok(()),
            _ => Err(KeyError::Unsupported(
                "the agent does not hold this key".into(),
            )),
        }
    }

    pub async fn remove_all(&self) -> Result<(), KeyError> {
        self.loaded().clear();
        match self.raw_request(&[REMOVE_ALL_IDENTITIES]).await? {
            SUCCESS => Ok(()),
            _ => Err(KeyError::Unsupported(
                "the agent refused to remove the keys".into(),
            )),
        }
    }
}

const FAILURE: u8 = 5;
const SUCCESS: u8 = 6;
const REMOVE_IDENTITY: u8 = 18;
const REMOVE_ALL_IDENTITIES: u8 = 19;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keygen::{KeyType, generate};

    #[tokio::test]
    async fn add_list_and_remove_keys() {
        let agent = Agent::start(None).await.unwrap();
        assert!(agent.keys().await.unwrap().is_empty());
        let a = generate(KeyType::Ed25519, "first").unwrap();
        let b = generate(KeyType::EcdsaP256, "second").unwrap();
        agent.add(&a).await.unwrap();
        agent.add(&b).await.unwrap();
        let keys = agent.keys().await.unwrap();
        assert_eq!(keys.len(), 2);
        let first = keys
            .iter()
            .find(|k| k.key_data() == a.public_key().key_data())
            .unwrap();
        assert_eq!(first.comment().as_str_lossy(), "first", "comments are kept");

        agent.remove(a.public_key()).await.unwrap();
        let keys = agent.keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_data(), b.public_key().key_data());

        agent.remove_all().await.unwrap();
        assert!(agent.keys().await.unwrap().is_empty());
    }
}
