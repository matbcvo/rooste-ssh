//! SSH client tests against the in-process test server. Run on every OS.

mod common;

use std::borrow::Cow;
use std::sync::Arc;

use futures::future::BoxFuture;
use rooste_protocols::Transport;
use rooste_protocols::ssh::{
    AgentSource, Auth, HostKeyPolicy, HostKeyPrompt, PtyRequest, SshClient, SshConfig, SshError,
    UnknownHostKey, connect, known_hosts, load_private_key,
};
use rooste_testkit::{
    TEST_KEY_PASSPHRASE, TEST_PASSWORD, TEST_USER, TestAgent, TestServer, key_path, private_key,
    public_key,
};
use russh::{Preferred, kex};
use tempfile::TempDir;

use common::{read_to_end, read_until};

fn config(server: &TestServer, dir: &TempDir) -> SshConfig {
    let mut config = SshConfig::new("127.0.0.1", server.addr.port());
    config.known_hosts = dir.path().join("known_hosts");
    config.host_key_policy = HostKeyPolicy::AcceptNew;
    config
}

async fn connected(server: &TestServer, dir: &TempDir) -> SshClient {
    connect(&config(server, dir)).await.expect("connect")
}

#[tokio::test]
async fn password_login_shell_resize_and_exit_status() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    assert!(
        client
            .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
            .await
            .unwrap()
    );

    let pty = PtyRequest {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    let mut shell = client.open_shell(&pty).await.unwrap();
    read_until(&mut shell, "$ ").await;

    shell.write(b"size\r").await.unwrap();
    read_until(&mut shell, "80x24").await;

    shell.resize(120, 40).await.unwrap();
    shell.write(b"size\r").await.unwrap();
    read_until(&mut shell, "120x40").await;

    shell.write(b"exit 3\r").await.unwrap();
    read_to_end(&mut shell).await;
    assert_eq!(shell.exit_status(), Some(3));
}

#[tokio::test]
async fn wrong_password_is_rejected() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    assert!(
        !client
            .authenticate(TEST_USER, &Auth::password("wrong"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn ed25519_key_login() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    let key = Auth::PublicKey(Arc::new(private_key("id_ed25519")));
    assert!(client.authenticate(TEST_USER, &key).await.unwrap());
}

#[tokio::test]
async fn rsa_key_login() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    let key = Auth::PublicKey(Arc::new(private_key("id_rsa")));
    assert!(client.authenticate(TEST_USER, &key).await.unwrap());
}

#[tokio::test]
async fn encrypted_key_needs_passphrase() {
    let path = key_path("id_ed25519_encrypted");
    assert!(matches!(
        load_private_key(&path, None),
        Err(SshError::KeyEncrypted(_))
    ));
    assert!(load_private_key(&path, Some("wrong")).is_err());
    let key = load_private_key(&path, Some(TEST_KEY_PASSPHRASE)).unwrap();

    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    assert!(
        client
            .authenticate(TEST_USER, &Auth::PublicKey(Arc::new(key)))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn unauthorized_key_is_rejected() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    let key = Auth::PublicKey(Arc::new(private_key("host_ed25519")));
    assert!(!client.authenticate(TEST_USER, &key).await.unwrap());
}

#[tokio::test]
async fn agent_login() {
    let agent = TestAgent::start().await;
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    let auth = Auth::Agent(AgentSource::Path(agent.path().to_owned()));
    assert!(client.authenticate(TEST_USER, &auth).await.unwrap());
}

#[tokio::test]
async fn missing_agent_is_an_error() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    let missing = if cfg!(windows) {
        r"\\.\pipe\rooste-no-such-agent".into()
    } else {
        dir.path().join("no-such-agent.sock")
    };
    let result = client
        .authenticate(TEST_USER, &Auth::Agent(AgentSource::Path(missing)))
        .await;
    assert!(matches!(result, Err(SshError::Agent(_))), "{result:?}");
}

#[tokio::test]
async fn exec_collects_output_and_status() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut client = connected(&server, &dir).await;
    client
        .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
        .await
        .unwrap();

    let out = client.exec("echo hello").await.unwrap();
    assert_eq!(out.stdout, b"hello\n");
    assert_eq!(out.exit_status, Some(0));

    let out = client.exec("exit 5").await.unwrap();
    assert_eq!(out.exit_status, Some(5));

    let out = client.exec("bogus").await.unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
    assert_eq!(out.exit_status, Some(127));
}

#[tokio::test]
async fn strict_policy_rejects_unknown_host() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&server, &dir);
    config.host_key_policy = HostKeyPolicy::Strict;
    let err = connect(&config).await.unwrap_err();
    assert!(matches!(err, SshError::HostKeyRejected { .. }), "{err:?}");
    assert!(!config.known_hosts.exists(), "nothing must be recorded");
}

#[tokio::test]
async fn accept_new_records_key_then_strict_succeeds() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&server, &dir);
    connect(&config).await.expect("accept-new");

    let status = known_hosts::check(
        &config.known_hosts,
        "127.0.0.1",
        server.addr.port(),
        &server.host_key,
    )
    .unwrap();
    assert_eq!(status, known_hosts::HostKeyStatus::Known);

    config.host_key_policy = HostKeyPolicy::Strict;
    connect(&config).await.expect("known host");
}

#[tokio::test]
async fn changed_host_key_is_refused_even_with_accept_new() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let config = config(&server, &dir);
    // Record a different Ed25519 key for this host.
    known_hosts::learn(
        &config.known_hosts,
        "127.0.0.1",
        server.addr.port(),
        &public_key("id_ed25519"),
    )
    .unwrap();

    let err = connect(&config).await.unwrap_err();
    match err {
        SshError::HostKeyChanged {
            line, fingerprint, ..
        } => {
            assert_eq!(line, 1);
            assert_eq!(fingerprint, known_hosts::fingerprint(&server.host_key));
        }
        other => panic!("expected HostKeyChanged, got {other:?}"),
    }
}

struct FixedAnswer(bool);

impl HostKeyPrompt for FixedAnswer {
    fn confirm_unknown(&self, key: UnknownHostKey) -> BoxFuture<'static, bool> {
        assert_eq!(key.algorithm, "ssh-ed25519");
        assert!(key.fingerprint.starts_with("SHA256:"));
        let answer = self.0;
        Box::pin(async move { answer })
    }
}

#[tokio::test]
async fn prompt_decides_unknown_host() {
    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&server, &dir);

    config.host_key_policy = HostKeyPolicy::Ask(Arc::new(FixedAnswer(false)));
    let err = connect(&config).await.unwrap_err();
    assert!(matches!(err, SshError::HostKeyRejected { .. }), "{err:?}");

    config.host_key_policy = HostKeyPolicy::Ask(Arc::new(FixedAnswer(true)));
    connect(&config).await.expect("accepted by prompt");
    config.host_key_policy = HostKeyPolicy::Strict;
    connect(&config).await.expect("recorded after prompt");
}

#[tokio::test]
async fn legacy_only_server_needs_opt_in() {
    let legacy = Preferred {
        kex: Cow::Borrowed(&[kex::DH_G14_SHA1]),
        ..Preferred::default()
    };
    let server = TestServer::start_with(legacy).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&server, &dir);

    let err = connect(&config).await.unwrap_err();
    match err {
        SshError::NoCommonAlgorithm {
            legacy_would_help, ..
        } => assert!(legacy_would_help),
        other => panic!("expected NoCommonAlgorithm, got {other:?}"),
    }

    config.allow_legacy_algorithms = true;
    let mut client = connect(&config).await.expect("legacy opt-in");
    assert!(
        client
            .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn refused_connection_is_a_connect_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let err = connect(&SshConfig::new("127.0.0.1", port))
        .await
        .unwrap_err();
    assert!(matches!(err, SshError::Connect(_)), "{err:?}");
}

#[test]
fn legacy_encrypted_pem_keys_load() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in ["rsa_des3.pem", "rsa_aes256.pem"] {
        let path = fixtures.join(name);
        assert!(matches!(
            load_private_key(&path, None),
            Err(SshError::KeyEncrypted(_))
        ));
        assert!(matches!(
            load_private_key(&path, Some("wrong")),
            Err(SshError::KeyEncrypted(_))
        ));
        load_private_key(&path, Some("secret")).unwrap();
    }
}

#[test]
fn files_that_are_not_private_keys_get_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let public = dir.path().join("id_ed25519.pub");
    std::fs::write(&public, public_key("id_ed25519").to_openssh().unwrap()).unwrap();
    let error = load_private_key(&public, None).unwrap_err();
    assert!(matches!(error, SshError::PublicKeyFile(_)), "{error}");
    assert!(error.to_string().contains("public key"));

    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "just some text\n").unwrap();
    let error = load_private_key(&notes, None).unwrap_err();
    assert!(matches!(error, SshError::NotAKey(_)), "{error}");
}
