#![allow(dead_code)]

use std::time::Duration;

use rooste_proto::Transport;
use rooste_proto::ssh::SshShell;

/// Read from the shell until `needle` appears; returns everything read.
pub async fn read_until(shell: &mut SshShell, needle: &str) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let n = shell.read(&mut buf).await.expect("read");
            if n == 0 {
                return false;
            }
            out.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&out).contains(needle) {
                return true;
            }
        }
    })
    .await;
    let text = String::from_utf8_lossy(&out).into_owned();
    assert!(
        matches!(result, Ok(true)),
        "did not see {needle:?}, got: {text:?}"
    );
    text
}

/// Read until the remote side closes the channel.
pub async fn read_to_end(shell: &mut SshShell) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let n = shell.read(&mut buf).await.expect("read");
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .expect("channel did not close");
    String::from_utf8_lossy(&out).into_owned()
}
