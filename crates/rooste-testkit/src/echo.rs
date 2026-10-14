//! A TCP server that sends back everything it receives.

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A running echo server on `127.0.0.1`. Stops when dropped.
#[derive(Debug)]
pub struct EchoServer {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl EchoServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
        let addr = listener.local_addr().expect("echo address");
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut read, mut write) = socket.split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
            }
        });
        Self { addr, task }
    }
}

impl Drop for EchoServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
