//! Port forwarding, jump hosts and agent forwarding against the in-process
//! test server. Run on every OS.

mod common;

use std::time::Duration;

use rooste_protocols::Transport;
use rooste_protocols::ssh::{
    AgentSource, Auth, ForwardKind, HostKeyPolicy, PtyRequest, SshClient, SshConfig, SshError,
    connect, connect_via,
};
use rooste_testkit::{EchoServer, TEST_PASSWORD, TEST_USER, TestAgent, TestServer};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use common::read_until;

fn config(server: &TestServer, dir: &TempDir) -> SshConfig {
    let mut config = SshConfig::new("127.0.0.1", server.addr.port());
    config.known_hosts = dir.path().join("known_hosts");
    config.host_key_policy = HostKeyPolicy::AcceptNew;
    config
}

async fn logged_in(server: &TestServer, dir: &TempDir) -> SshClient {
    let mut client = connect(&config(server, dir)).await.expect("connect");
    assert!(
        client
            .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
            .await
            .unwrap()
    );
    client
}

/// Write `message` to `stream` and expect it echoed back.
async fn round_trip(stream: &mut TcpStream, message: &[u8]) {
    stream.write_all(message).await.unwrap();
    let mut back = vec![0u8; message.len()];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut back))
        .await
        .expect("echo timed out")
        .unwrap();
    assert_eq!(back, message);
}

#[tokio::test]
async fn local_forward_reaches_the_target() {
    let echo = EchoServer::start().await;
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(&server, &dir).await;

    let forward = client
        .forward_local("127.0.0.1:0", "127.0.0.1", echo.addr.port())
        .await
        .unwrap();
    assert_eq!(forward.kind, ForwardKind::Local);
    // Two connections through the same forward.
    for message in [&b"hello"[..], b"second connection"] {
        let mut stream = TcpStream::connect(forward.listen).await.unwrap();
        round_trip(&mut stream, message).await;
    }

    let listen = forward.listen;
    drop(forward);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        TcpStream::connect(listen).await.is_err(),
        "listener closed when the forward is dropped"
    );
}

#[tokio::test]
async fn local_forward_bind_errors_are_reported() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(&server, &dir).await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let err = client
        .forward_local(&addr, "127.0.0.1", 1)
        .await
        .unwrap_err();
    assert!(matches!(err, SshError::Forward { .. }), "{err:?}");
}

#[tokio::test]
async fn dynamic_forward_speaks_socks5_and_socks4a() {
    let echo = EchoServer::start().await;
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(&server, &dir).await;
    let socks = client.forward_dynamic("127.0.0.1:0").await.unwrap();
    assert_eq!(socks.kind, ForwardKind::Dynamic);

    // SOCKS5 with a domain name.
    let mut s = TcpStream::connect(socks.listen).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    s.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 9];
    req.extend_from_slice(b"localhost");
    req.extend_from_slice(&echo.addr.port().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0, "SOCKS5 success");
    round_trip(&mut s, b"through socks5").await;

    // SOCKS4a with a host name.
    let mut s = TcpStream::connect(socks.listen).await.unwrap();
    let mut req = vec![4, 1];
    req.extend_from_slice(&echo.addr.port().to_be_bytes());
    req.extend_from_slice(&[0, 0, 0, 1, 0]);
    req.extend_from_slice(b"127.0.0.1\0");
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 8];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0x5a, "SOCKS4 granted");
    round_trip(&mut s, b"through socks4a").await;

    // A target the server cannot reach gets a failure reply.
    let mut s = TcpStream::connect(socks.listen).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    s.read_exact(&mut method).await.unwrap();
    s.write_all(&[5, 1, 0, 1, 127, 0, 0, 1, 0, 1])
        .await
        .unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 5, "connection refused");
}

#[tokio::test]
async fn remote_forward_comes_back_to_a_local_target() {
    let echo = EchoServer::start().await;
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(&server, &dir).await;

    let forward = client
        .forward_remote("127.0.0.1", 0, "127.0.0.1", echo.addr.port())
        .await
        .unwrap();
    assert_eq!(forward.kind, ForwardKind::Remote);
    assert_ne!(forward.listen.port(), 0, "server chose a port");

    // The test server listens on this machine, so connect directly.
    let mut stream = TcpStream::connect(forward.listen).await.unwrap();
    round_trip(&mut stream, b"remote forward").await;
}

#[tokio::test]
async fn jump_host_connection() {
    let jump_server = TestServer::start().await;
    let target_server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();

    let jump = logged_in(&jump_server, &dir).await;
    let mut target = connect_via(jump, &config(&target_server, &dir))
        .await
        .expect("connect through the jump host");
    assert!(
        target
            .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
            .await
            .unwrap()
    );
    let out = target.exec("echo via jump").await.unwrap();
    assert_eq!(out.stdout, b"via jump\n");

    // The target's host key was checked and recorded under its own address.
    let known = std::fs::read_to_string(dir.path().join("known_hosts")).unwrap();
    assert!(known.contains(&format!("[127.0.0.1]:{}", target_server.addr.port())));
}

#[tokio::test]
async fn jump_host_that_cannot_reach_the_target() {
    let jump_server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let jump = logged_in(&jump_server, &dir).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let mut target = SshConfig::new("127.0.0.1", port);
    target.known_hosts = dir.path().join("known_hosts");
    let err = connect_via(jump, &target).await.unwrap_err();
    assert!(matches!(err, SshError::Connect(_)), "{err:?}");
}

#[tokio::test]
async fn agent_forwarding_in_shell_and_exec() {
    let agent = TestAgent::start().await;
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(&server, &dir).await;
    let source = || Some(AgentSource::Path(agent.path().to_owned()));

    let out = client.exec_with("agent-keys", source()).await.unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("agent: 1 keys"), "{text}");

    let mut shell = client
        .open_shell_with(&PtyRequest::default(), source())
        .await
        .unwrap();
    read_until(&mut shell, "$ ").await;
    shell.write(b"agent-keys\r").await.unwrap();
    read_until(&mut shell, "agent: 1 keys").await;
}

#[tokio::test]
async fn no_agent_forwarding_unless_requested() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(&server, &dir).await;
    let out = client.exec("agent-keys").await.unwrap();
    assert_eq!(out.stdout, b"agent: not forwarded\n");
}

#[tokio::test]
async fn authentication_after_forwarding_is_refused() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connect(&config(&server, &dir)).await.unwrap();
    let _forward = client.forward_dynamic("127.0.0.1:0").await.unwrap();
    let err = client
        .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
        .await
        .unwrap_err();
    assert!(matches!(err, SshError::AuthAfterForwarding), "{err:?}");
}
