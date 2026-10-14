//! Port forwarding: local (`-L`), remote (`-R`) and dynamic SOCKS (`-D`).

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use russh::client::Handle;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::{ClientHandler, SshClient, SshError, lock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

/// A running forward. Local and dynamic forwards stop when dropped; a
/// remote forward stays until the connection closes.
#[derive(Debug)]
pub struct Forward {
    pub kind: ForwardKind,
    /// Where connections are accepted: a local address, or the port the
    /// server listens on for remote forwards.
    pub listen: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl Drop for Forward {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn bind(bind: &str) -> Result<TcpListener, SshError> {
    TcpListener::bind(bind)
        .await
        .map_err(|source| SshError::Forward {
            bind: bind.to_owned(),
            source,
        })
}

impl SshClient {
    /// Forward connections to the local address `bind` (e.g.
    /// `127.0.0.1:8080`) through the server to `host:port`.
    pub async fn forward_local(
        &self,
        bind_addr: &str,
        host: &str,
        port: u16,
    ) -> Result<Forward, SshError> {
        let listener = bind(bind_addr).await?;
        let listen = listener.local_addr().map_err(|source| SshError::Forward {
            bind: bind_addr.to_owned(),
            source,
        })?;
        let handle = self.handle.clone();
        let host = host.to_owned();
        let task = tokio::spawn(async move {
            while let Ok((tcp, peer)) = listener.accept().await {
                let handle = handle.clone();
                let host = host.clone();
                tokio::spawn(async move {
                    let _ = tunnel(&handle, tcp, peer, &host, port).await;
                });
            }
        });
        Ok(Forward {
            kind: ForwardKind::Local,
            listen,
            task: Some(task),
        })
    }

    /// A SOCKS4a/SOCKS5 proxy on the local address `bind`; every connection
    /// goes out through the server.
    pub async fn forward_dynamic(&self, bind_addr: &str) -> Result<Forward, SshError> {
        let listener = bind(bind_addr).await?;
        let listen = listener.local_addr().map_err(|source| SshError::Forward {
            bind: bind_addr.to_owned(),
            source,
        })?;
        let handle = self.handle.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, peer)) = listener.accept().await {
                let handle = handle.clone();
                tokio::spawn(async move {
                    let _ = socks(&handle, tcp, peer).await;
                });
            }
        });
        Ok(Forward {
            kind: ForwardKind::Dynamic,
            listen,
            task: Some(task),
        })
    }

    /// Ask the server to listen on `remote_addr:remote_port` (port 0 lets
    /// it choose) and forward connections back to `host:port` here.
    pub async fn forward_remote(
        &self,
        remote_addr: &str,
        remote_port: u16,
        host: &str,
        port: u16,
    ) -> Result<Forward, SshError> {
        let route_key = |p: u32| (remote_addr.to_owned(), p);
        // Register first so an early connection already has a route.
        if remote_port != 0 {
            lock(&self.routes).insert(route_key(remote_port.into()), (host.to_owned(), port));
        }
        let allocated = self
            .handle
            .tcpip_forward(remote_addr, remote_port.into())
            .await
            .map_err(|e| {
                lock(&self.routes).remove(&route_key(remote_port.into()));
                match e {
                    russh::Error::RequestDenied => SshError::Forward {
                        bind: format!("{remote_addr}:{remote_port} on the server"),
                        source: io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "the server refused",
                        ),
                    },
                    other => SshError::Protocol(other),
                }
            })?;
        let listening = if remote_port == 0 {
            let allocated = u16::try_from(allocated).unwrap_or(0);
            lock(&self.routes).insert(route_key(allocated.into()), (host.to_owned(), port));
            allocated
        } else {
            remote_port
        };
        let ip = remote_addr
            .parse()
            .unwrap_or(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        Ok(Forward {
            kind: ForwardKind::Remote,
            listen: SocketAddr::new(ip, listening),
            task: None,
        })
    }
}

async fn tunnel(
    handle: &Arc<Handle<ClientHandler>>,
    mut tcp: TcpStream,
    peer: SocketAddr,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let channel = handle
        .channel_open_direct_tcpip(host, port.into(), peer.ip().to_string(), peer.port().into())
        .await
        .map_err(io::Error::other)?;
    let mut stream = channel.into_stream();
    tokio::io::copy_bidirectional(&mut tcp, &mut stream).await?;
    Ok(())
}

/// Where a SOCKS client wants to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SocksTarget {
    pub host: String,
    pub port: u16,
    pub version: u8,
}

/// Read a SOCKS4/4a/5 CONNECT request, answering the SOCKS5 method
/// negotiation along the way.
pub(crate) async fn read_socks_request<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
) -> io::Result<SocksTarget> {
    let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidData, msg.to_owned());
    match stream.read_u8().await? {
        4 => {
            let command = stream.read_u8().await?;
            let port = stream.read_u16().await?;
            let mut ip = [0u8; 4];
            stream.read_exact(&mut ip).await?;
            read_cstring(stream).await?; // user id
            if command != 1 {
                return Err(invalid("only SOCKS CONNECT is supported"));
            }
            // SOCKS4a: 0.0.0.x means a host name follows.
            let host = if ip[..3] == [0, 0, 0] && ip[3] != 0 {
                read_cstring(stream).await?
            } else {
                Ipv4Addr::from(ip).to_string()
            };
            Ok(SocksTarget {
                host,
                port,
                version: 4,
            })
        }
        5 => {
            let methods = stream.read_u8().await?;
            let mut offered = vec![0u8; usize::from(methods)];
            stream.read_exact(&mut offered).await?;
            if !offered.contains(&0) {
                stream.write_all(&[5, 0xff]).await?;
                return Err(invalid("SOCKS client requires authentication"));
            }
            stream.write_all(&[5, 0]).await?;
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await?;
            if header[0] != 5 || header[1] != 1 {
                stream.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
                return Err(invalid("only SOCKS CONNECT is supported"));
            }
            let host = match header[3] {
                1 => {
                    let mut ip = [0u8; 4];
                    stream.read_exact(&mut ip).await?;
                    Ipv4Addr::from(ip).to_string()
                }
                3 => {
                    let len = stream.read_u8().await?;
                    let mut name = vec![0u8; usize::from(len)];
                    stream.read_exact(&mut name).await?;
                    String::from_utf8(name).map_err(|_| invalid("invalid host name"))?
                }
                4 => {
                    let mut ip = [0u8; 16];
                    stream.read_exact(&mut ip).await?;
                    Ipv6Addr::from(ip).to_string()
                }
                _ => return Err(invalid("unknown SOCKS address type")),
            };
            let port = stream.read_u16().await?;
            Ok(SocksTarget {
                host,
                port,
                version: 5,
            })
        }
        _ => Err(invalid("not a SOCKS request")),
    }
}

async fn read_cstring<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        match stream.read_u8().await? {
            0 => break,
            b if bytes.len() < 255 => bytes.push(b),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "SOCKS string too long",
                ));
            }
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The reply telling the SOCKS client whether the connection worked.
pub(crate) fn socks_reply(version: u8, ok: bool) -> Vec<u8> {
    match (version, ok) {
        (4, true) => vec![0, 0x5a, 0, 0, 0, 0, 0, 0],
        (4, false) => vec![0, 0x5b, 0, 0, 0, 0, 0, 0],
        (_, true) => vec![5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
        (_, false) => vec![5, 5, 0, 1, 0, 0, 0, 0, 0, 0],
    }
}

async fn socks(
    handle: &Arc<Handle<ClientHandler>>,
    mut tcp: TcpStream,
    peer: SocketAddr,
) -> io::Result<()> {
    let target = read_socks_request(&mut tcp).await?;
    let channel = handle
        .channel_open_direct_tcpip(
            target.host.as_str(),
            target.port.into(),
            peer.ip().to_string(),
            peer.port().into(),
        )
        .await;
    let Ok(channel) = channel else {
        tcp.write_all(&socks_reply(target.version, false)).await?;
        return Ok(());
    };
    tcp.write_all(&socks_reply(target.version, true)).await?;
    let mut stream = channel.into_stream();
    tokio::io::copy_bidirectional(&mut tcp, &mut stream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(request: &[u8]) -> (io::Result<SocksTarget>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(request).await.unwrap();
        // End of input, so a truncated request fails instead of waiting.
        client.shutdown().await.unwrap();
        let result = read_socks_request(&mut server).await;
        drop(server);
        let mut written = Vec::new();
        client.read_to_end(&mut written).await.unwrap();
        (result, written)
    }

    #[tokio::test]
    async fn socks5_domain_ipv4_and_ipv6() {
        let mut req = vec![5, 1, 0, 5, 1, 0, 3, 11];
        req.extend_from_slice(b"example.com");
        req.extend_from_slice(&443u16.to_be_bytes());
        let (target, written) = parse(&req).await;
        assert_eq!(
            target.unwrap(),
            SocksTarget {
                host: "example.com".into(),
                port: 443,
                version: 5
            }
        );
        assert_eq!(written, [5, 0], "no-auth method chosen");

        let (target, _) = parse(&[5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 1, 0, 22]).await;
        assert_eq!(target.unwrap().host, "10.0.0.1");

        let mut req = vec![5, 1, 0, 5, 1, 0, 4];
        req.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        req.extend_from_slice(&80u16.to_be_bytes());
        assert_eq!(parse(&req).await.0.unwrap().host, "::1");
    }

    #[tokio::test]
    async fn socks5_rejects_auth_only_and_bind() {
        let (target, written) = parse(&[5, 1, 2]).await;
        assert!(target.is_err());
        assert_eq!(written, [5, 0xff]);

        let (target, written) = parse(&[5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80]).await;
        assert!(target.is_err());
        assert_eq!(written[..2], [5, 0]);
        assert_eq!(written[2..4], [5, 7], "command not supported");
    }

    #[tokio::test]
    async fn socks4_and_4a() {
        let mut req = vec![4, 1, 0, 80, 192, 168, 1, 2];
        req.extend_from_slice(b"user\0");
        let target = parse(&req).await.0.unwrap();
        assert_eq!(
            (target.host.as_str(), target.port, target.version),
            ("192.168.1.2", 80, 4)
        );

        let mut req = vec![4, 1, 0x1f, 0x90, 0, 0, 0, 1];
        req.extend_from_slice(b"\0intranet.local\0");
        let target = parse(&req).await.0.unwrap();
        assert_eq!(
            (target.host.as_str(), target.port),
            ("intranet.local", 8080)
        );
    }

    #[tokio::test]
    async fn garbage_is_rejected() {
        assert!(parse(&[9, 9, 9]).await.0.is_err());
        assert!(parse(&[]).await.0.is_err());
    }

    #[test]
    fn replies() {
        assert_eq!(socks_reply(4, true)[1], 0x5a);
        assert_eq!(socks_reply(4, false)[1], 0x5b);
        assert_eq!(socks_reply(5, true)[1], 0);
        assert_eq!(socks_reply(5, false)[1], 5);
    }
}
