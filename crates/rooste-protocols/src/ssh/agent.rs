//! Connecting to a running SSH agent.

use std::path::PathBuf;

use russh::keys;
use russh::keys::agent::client::{AgentClient, AgentStream};

pub(crate) type DynAgent = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;

/// Default pipe of the Windows OpenSSH agent service.
#[cfg(windows)]
pub const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// Where to find the SSH agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AgentSource {
    /// Linux: `SSH_AUTH_SOCK`. Windows: `SSH_AUTH_SOCK`, then the OpenSSH
    /// agent pipe, then Pageant.
    #[default]
    Default,
    /// A Unix socket path, or a named pipe on Windows.
    Path(PathBuf),
}

#[cfg(unix)]
pub(crate) async fn connect(source: &AgentSource) -> Result<DynAgent, keys::Error> {
    let agent = match source {
        AgentSource::Default => AgentClient::connect_env().await?,
        AgentSource::Path(path) => AgentClient::connect_uds(path).await?,
    };
    Ok(agent.dynamic())
}

#[cfg(windows)]
pub(crate) async fn connect(source: &AgentSource) -> Result<DynAgent, keys::Error> {
    if let AgentSource::Path(path) = source {
        return Ok(AgentClient::connect_named_pipe(path).await?.dynamic());
    }
    if let Some(sock) = std::env::var_os("SSH_AUTH_SOCK")
        && let Ok(agent) = AgentClient::connect_named_pipe(&sock).await
    {
        return Ok(agent.dynamic());
    }
    if let Ok(agent) = AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE).await {
        return Ok(agent.dynamic());
    }
    Ok(AgentClient::connect_pageant().await?.dynamic())
}

/// A byte stream to an agent, for relaying forwarded agent connections.
pub(crate) trait AgentByteStream:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send
{
}

impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> AgentByteStream for T {}

#[cfg(unix)]
pub(crate) async fn connect_stream(
    source: &AgentSource,
) -> std::io::Result<Box<dyn AgentByteStream>> {
    let path = match source {
        AgentSource::Path(path) => path.clone(),
        AgentSource::Default => std::env::var_os("SSH_AUTH_SOCK")
            .map(PathBuf::from)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "SSH_AUTH_SOCK is not set")
            })?,
    };
    Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
}

#[cfg(windows)]
pub(crate) async fn connect_stream(
    source: &AgentSource,
) -> std::io::Result<Box<dyn AgentByteStream>> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let open = |pipe: &std::ffi::OsStr| ClientOptions::new().open(pipe);
    let client = match source {
        AgentSource::Path(path) => open(path.as_os_str())?,
        AgentSource::Default => match std::env::var_os("SSH_AUTH_SOCK") {
            Some(sock) => open(&sock).or_else(|_| open(OPENSSH_AGENT_PIPE.as_ref()))?,
            None => open(OPENSSH_AGENT_PIPE.as_ref())?,
        },
    };
    Ok(Box::new(client))
}
