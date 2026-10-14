//! SFTP against the in-process test server (every OS) and, when available,
//! a real OpenSSH server.

use std::sync::Arc;

use rooste_proto::ssh::{Auth, HostKeyPolicy, SshClient, SshConfig, connect};
use rooste_sftp::{EntryKind, Sftp, SftpError};
use rooste_testkit::{OpenSshFixture, TEST_PASSWORD, TEST_USER, TestServer, private_key};
use tempfile::TempDir;

async fn logged_in(host: &str, port: u16, dir: &TempDir, auth: &Auth, user: &str) -> SshClient {
    let mut config = SshConfig::new(host, port);
    config.known_hosts = dir.path().join("known_hosts");
    config.host_key_policy = HostKeyPolicy::AcceptNew;
    let mut client = connect(&config).await.expect("connect");
    assert!(client.authenticate(user, auth).await.unwrap());
    client
}

async fn exercise(sftp: &Sftp, base: &str, work: &TempDir) {
    let dir = rooste_sftp::join(base, "rooste-sftp-test");
    let _ = sftp
        .remove_file(&rooste_sftp::join(&dir, "renamed.bin"))
        .await;
    let _ = sftp.remove_dir(&dir).await;
    sftp.mkdir(&dir).await.unwrap();

    // Upload ~200 KiB, more than one chunk, with progress.
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let local = work.path().join("upload.bin");
    std::fs::write(&local, &data).unwrap();
    let remote = rooste_sftp::join(&dir, "file.bin");
    let mut updates = Vec::new();
    let sent = sftp
        .upload(&local, &remote, |done, total| {
            updates.push((done, total));
            true
        })
        .await
        .unwrap();
    assert_eq!(sent, data.len() as u64);
    assert!(updates.len() > 1, "progress in several steps");
    assert_eq!(updates.last(), Some(&(sent, sent)));

    let listing = sftp.list(&dir).await.unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].name, "file.bin");
    assert_eq!(listing[0].kind, EntryKind::File);
    assert_eq!(listing[0].size, data.len() as u64);

    let renamed = rooste_sftp::join(&dir, "renamed.bin");
    sftp.rename(&remote, &renamed).await.unwrap();
    let back = work.path().join("download.bin");
    let received = sftp.download(&renamed, &back, |_, _| true).await.unwrap();
    assert_eq!(received, data.len() as u64);
    assert_eq!(std::fs::read(&back).unwrap(), data);

    // A cancelled download leaves no partial file behind.
    let cancelled = work.path().join("cancelled.bin");
    let result = sftp.download(&renamed, &cancelled, |_, _| false).await;
    assert!(matches!(result, Err(SftpError::Cancelled)));
    assert!(!cancelled.exists());

    // Directories are listed first.
    let base_listing = sftp.list(base).await.unwrap();
    let ours = base_listing
        .iter()
        .find(|e| e.name == "rooste-sftp-test")
        .unwrap();
    assert!(ours.is_dir());

    sftp.remove_file(&renamed).await.unwrap();
    sftp.remove_dir(&dir).await.unwrap();
    assert!(sftp.list(&dir).await.is_err());
}

#[tokio::test]
async fn in_process_server() {
    let server = TestServer::start().await;
    std::fs::create_dir(server.sftp_root().join("zdir")).unwrap();
    std::fs::write(server.sftp_root().join("afile.txt"), "hi").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(
        "127.0.0.1",
        server.addr.port(),
        &dir,
        &Auth::password(TEST_PASSWORD),
        TEST_USER,
    )
    .await;
    let sftp = Sftp::open(&client).await.unwrap();
    assert_eq!(sftp.home().await.unwrap(), "/");
    let names: Vec<_> = sftp
        .list("/")
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["zdir", "afile.txt"], "directories first");
    exercise(&sftp, "/", &dir).await;
}

#[tokio::test]
#[ignore = "requires an OpenSSH server"]
async fn openssh_server() {
    let fixture = OpenSshFixture::from_env().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let client = logged_in(
        &fixture.addr.ip().to_string(),
        fixture.addr.port(),
        &dir,
        &Auth::PublicKey(Arc::new(private_key("id_ed25519"))),
        &fixture.user,
    )
    .await;
    let sftp = Sftp::open(&client).await.unwrap();
    let home = sftp.home().await.unwrap();
    exercise(&sftp, &home, &dir).await;
}
