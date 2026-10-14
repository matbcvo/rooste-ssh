//! Interoperability with other SSH servers: several OpenSSH versions,
//! Dropbear, and a server limited to legacy algorithms. Run nightly in CI:
//!
//! ```text
//! ROOSTE_INTEROP_SERVERS="openssh-9.3=127.0.0.1:2301,dropbear=127.0.0.1:2303" \
//! ROOSTE_LEGACY_ADDR=127.0.0.1:2304 cargo test -p rooste-proto --test interop -- --ignored
//! ```

use std::sync::Arc;

use rooste_proto::ssh::{Auth, HostKeyPolicy, PtyRequest, SshClient, SshConfig, SshError, connect};
use rooste_testkit::{TEST_PASSWORD, TEST_USER, private_key};
use tempfile::TempDir;

/// `name=host:port` pairs from `ROOSTE_INTEROP_SERVERS`.
fn servers() -> Vec<(String, String, u16)> {
    let list = std::env::var("ROOSTE_INTEROP_SERVERS").expect("set ROOSTE_INTEROP_SERVERS");
    list.split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|entry| {
            let (name, addr) = entry.split_once('=').expect("name=host:port");
            let (host, port) = addr.rsplit_once(':').expect("host:port");
            (
                name.trim().to_owned(),
                host.to_owned(),
                port.parse().expect("port"),
            )
        })
        .collect()
}

fn config(host: &str, port: u16, dir: &TempDir) -> SshConfig {
    let mut config = SshConfig::new(host, port);
    config.known_hosts = dir.path().join("known_hosts");
    config.host_key_policy = HostKeyPolicy::AcceptNew;
    config
}

async fn check_session(name: &str, client: &SshClient) {
    let out = client
        .exec("echo interop")
        .await
        .unwrap_or_else(|e| panic!("{name}: exec: {e}"));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "interop",
        "{name}"
    );
    assert_eq!(out.exit_status, Some(0), "{name}");
    client
        .open_shell(&PtyRequest::default())
        .await
        .unwrap_or_else(|e| panic!("{name}: shell: {e}"));
}

#[tokio::test]
#[ignore = "requires the interop servers"]
async fn every_server_accepts_keys_and_passwords() {
    for (name, host, port) in servers() {
        let dir = tempfile::tempdir().unwrap();
        for key in ["id_ed25519", "id_rsa"] {
            let mut client = connect(&config(&host, port, &dir))
                .await
                .unwrap_or_else(|e| panic!("{name}: connect: {e}"));
            let auth = Auth::PublicKey(Arc::new(private_key(key)));
            assert!(
                client.authenticate(TEST_USER, &auth).await.unwrap(),
                "{name}: {key}"
            );
            check_session(&name, &client).await;
        }
        let mut client = connect(&config(&host, port, &dir)).await.unwrap();
        assert!(
            client
                .authenticate(TEST_USER, &Auth::password(TEST_PASSWORD))
                .await
                .unwrap(),
            "{name}: password"
        );
        check_session(&name, &client).await;
        println!("{name}: ok");
    }
}

#[tokio::test]
#[ignore = "requires the interop servers"]
async fn legacy_server_needs_opt_in() {
    let addr = std::env::var("ROOSTE_LEGACY_ADDR").expect("set ROOSTE_LEGACY_ADDR");
    let (host, port) = addr.rsplit_once(':').unwrap();
    let port: u16 = port.parse().unwrap();
    let dir = tempfile::tempdir().unwrap();

    let err = connect(&config(host, port, &dir)).await.unwrap_err();
    assert!(
        matches!(
            err,
            SshError::NoCommonAlgorithm {
                legacy_would_help: true,
                ..
            }
        ),
        "must fail with a legacy hint by default: {err:?}"
    );

    let mut legacy = config(host, port, &dir);
    legacy.allow_legacy_algorithms = true;
    let mut client = connect(&legacy)
        .await
        .expect("connects with legacy enabled");
    // `ssh-rsa` (SHA-1) signatures are needed for RSA keys on this server.
    let auth = Auth::PublicKey(Arc::new(private_key("id_rsa")));
    assert!(
        client.authenticate(TEST_USER, &auth).await.unwrap(),
        "RSA/SHA-1 key"
    );
    check_session("legacy", &client).await;
}
