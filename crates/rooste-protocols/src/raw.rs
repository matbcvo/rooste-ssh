//! Raw TCP: bytes go straight to and from the socket.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::Transport;

/// Connect to `host:port` within `timeout`.
pub async fn connect_tcp(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let stream = tokio::time::timeout(timeout, TcpStream::connect((host, port)))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connection timed out"))??;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// A raw TCP session.
#[derive(Debug)]
pub struct RawTransport {
    stream: TcpStream,
}

impl RawTransport {
    pub async fn connect(host: &str, port: u16, timeout: Duration) -> io::Result<Self> {
        Ok(Self {
            stream: connect_tcp(host, port, timeout).await?,
        })
    }
}

impl Transport for RawTransport {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf).await
    }

    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.stream.write_all(data).await
    }

    async fn close(&mut self) -> io::Result<()> {
        self.stream.shutdown().await
    }
}
