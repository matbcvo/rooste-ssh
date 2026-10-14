//! SSH client tests against a real OpenSSH server (Docker on Linux, Windows
//! OpenSSH Server on Windows). Ignored by default; CI runs them with
//! `cargo nextest run --run-ignored only`.

mod common;

use std::sync::Arc;

use rooste_proto::ssh::{
    AgentSource, Auth, HostKeyPolicy, SshClient, SshConfig, SshError, connect, known_hosts,
    load_private_key,
};
use rooste_testkit::{
    OpenSshFixture, TEST_KEY_PASSPHRASE, TestAgent, key_path, private_key, public_key,
};
use tempfile::TempDir;

fn fixture() -> OpenSshFixture {
    OpenSshFixture::from_env().expect("fixture address")
}

fn config(fixture: &OpenSshFixture, dir: &TempDir) -> SshConfig {
    let mut config = SshConfig::new(fixture.addr.ip().to_string(), fixture.addr.port());
    config.known_hosts = dir.path().join("known_hosts");
    config.host_key_policy = HostKeyPolicy::AcceptNew;
    config
}

async fn login(auth: &Auth) -> SshClient {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut client = connect(&config(&fixture, &dir)).await.expect("connect");
    assert!(
        client.authenticate(&fixture.user, auth).await.unwrap(),
        "login with {auth:?} failed"
    );
    client
}

async fn assert_echo(client: &SshClient) {
    let out = client.exec("echo hello").await.unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("hello"),
        "{out:?}"
    );
    assert_eq!(out.exit_status, Some(0));
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn password_login_and_exec() {
    let client = login(&Auth::password(fixture().password)).await;
    assert_echo(&client).await;
    client.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn ed25519_key_login() {
    let client = login(&Auth::PublicKey(Arc::new(private_key("id_ed25519")))).await;
    assert_echo(&client).await;
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn encrypted_key_login() {
    let key =
        load_private_key(&key_path("id_ed25519_encrypted"), Some(TEST_KEY_PASSPHRASE)).unwrap();
    login(&Auth::PublicKey(Arc::new(key))).await;
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn rsa_sha2_key_login() {
    login(&Auth::PublicKey(Arc::new(private_key("id_rsa")))).await;
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn agent_login() {
    let agent = TestAgent::start().await;
    login(&Auth::Agent(AgentSource::Path(agent.path().to_owned()))).await;
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn wrong_password_is_rejected() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut client = connect(&config(&fixture, &dir)).await.unwrap();
    assert!(
        !client
            .authenticate(&fixture.user, &Auth::password("definitely-wrong"))
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn strict_policy_rejects_unknown_host() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&fixture, &dir);
    config.host_key_policy = HostKeyPolicy::Strict;
    let err = connect(&config).await.unwrap_err();
    assert!(matches!(err, SshError::HostKeyRejected { .. }), "{err:?}");
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn changed_host_key_is_refused() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    let config = config(&fixture, &dir);
    known_hosts::learn(
        &config.known_hosts,
        &config.host,
        config.port,
        &public_key("id_ed25519"),
    )
    .unwrap();
    let err = connect(&config).await.unwrap_err();
    assert!(matches!(err, SshError::HostKeyChanged { .. }), "{err:?}");
}

/// Uses `stty`, so only against the Linux (Docker) server.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn pty_shell_resize_and_exit_status() {
    use common::{read_to_end, read_until};
    use rooste_proto::Transport;
    use rooste_proto::ssh::PtyRequest;

    let client = login(&Auth::PublicKey(Arc::new(private_key("id_ed25519")))).await;
    let pty = PtyRequest {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    let mut shell = client.open_shell(&pty).await.unwrap();

    shell.write(b"stty size\n").await.unwrap();
    read_until(&mut shell, "24 80").await;

    shell.resize(132, 43).await.unwrap();
    shell.write(b"stty size\n").await.unwrap();
    read_until(&mut shell, "43 132").await;

    shell.write(b"exit 7\n").await.unwrap();
    read_to_end(&mut shell).await;
    assert_eq!(shell.exit_status(), Some(7));
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn jump_host_to_itself() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    let jump = login(&Auth::PublicKey(Arc::new(private_key("id_ed25519")))).await;
    // From the server's point of view, it is reachable on its own port 22.
    let mut target = SshConfig::new("127.0.0.1", 22);
    target.known_hosts = dir.path().join("known_hosts");
    target.host_key_policy = HostKeyPolicy::AcceptNew;
    let mut client = rooste_proto::ssh::connect_via(jump, &target)
        .await
        .expect("via jump");
    assert!(
        client
            .authenticate(&fixture.user, &Auth::password(fixture.password.clone()))
            .await
            .unwrap()
    );
    assert_echo(&client).await;
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn local_forward_to_the_servers_own_sshd() {
    use tokio::io::AsyncBufReadExt;
    let client = login(&Auth::PublicKey(Arc::new(private_key("id_ed25519")))).await;
    let forward = client
        .forward_local("127.0.0.1:0", "127.0.0.1", 22)
        .await
        .unwrap();
    let stream = tokio::net::TcpStream::connect(forward.listen)
        .await
        .unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(stream)
        .read_line(&mut line)
        .await
        .unwrap();
    assert!(line.starts_with("SSH-2.0-OpenSSH"), "{line:?}");
}

/// Uses `nc` on the server, so only against the Linux (Docker) server.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn remote_forward_back_to_a_local_echo_server() {
    let echo = rooste_testkit::EchoServer::start().await;
    let client = login(&Auth::PublicKey(Arc::new(private_key("id_ed25519")))).await;
    let forward = client
        .forward_remote("127.0.0.1", 0, "127.0.0.1", echo.addr.port())
        .await
        .unwrap();
    let out = client
        .exec(&format!(
            "echo ping | nc -w 2 127.0.0.1 {}",
            forward.listen.port()
        ))
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ping");
}

/// Uses `ssh-add` on the server, so only against the Linux (Docker) server.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn agent_forwarding_is_visible_to_ssh_add() {
    let agent = TestAgent::start().await;
    let client = login(&Auth::PublicKey(Arc::new(private_key("id_ed25519")))).await;
    let out = client
        .exec_with(
            "ssh-add -l",
            Some(AgentSource::Path(agent.path().to_owned())),
        )
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("ED25519"),
        "{text:?} {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}
