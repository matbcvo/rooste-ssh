//! Host-key verification against an OpenSSH `known_hosts` file.

use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use russh::keys::{self, HashAlg, PublicKey};

/// Result of looking a host key up in `known_hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    Known,
    Unknown,
    /// A different key of the same type is recorded for this host.
    Changed {
        line: usize,
    },
}

/// The user's `~/.ssh/known_hosts`, if a home directory exists.
pub fn default_known_hosts_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".ssh").join("known_hosts"))
}

/// Look `key` up for `host:port` in the `known_hosts` file at `path`.
/// A missing file means every host is unknown.
pub fn check(
    path: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<HostKeyStatus, keys::Error> {
    // russh compares whole keys, comments included; only key data matters.
    let mut key = key.clone();
    key.set_comment("");
    match keys::known_hosts::check_known_hosts_path(host, port, &key, path) {
        Ok(true) => Ok(HostKeyStatus::Known),
        Ok(false) => Ok(HostKeyStatus::Unknown),
        Err(keys::Error::KeyChanged { line }) => Ok(HostKeyStatus::Changed { line }),
        Err(e) => Err(e),
    }
}

/// Append `key` for `host:port` to the `known_hosts` file at `path`,
/// creating the file and its directory if needed.
pub fn learn(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), keys::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)?;
    let needs_newline = file.seek(SeekFrom::End(-1)).is_ok() && {
        let mut last = [0u8];
        file.read_exact(&mut last)?;
        last[0] != b'\n'
    };

    let mut key = key.clone();
    key.set_comment("");
    let entry = key.to_openssh().map_err(|_| keys::Error::CouldNotReadKey)?;
    let host = if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    };
    let separator = if needs_newline { "\n" } else { "" };
    writeln!(file, "{separator}{host} {entry}")?;
    Ok(())
}

/// SHA-256 fingerprint in OpenSSH format, e.g. `SHA256:abc…`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// A host key not yet recorded in `known_hosts`, shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHostKey {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
}

/// Asks the user whether to trust an unknown host key.
pub trait HostKeyPrompt: Send + Sync {
    fn confirm_unknown(&self, key: UnknownHostKey) -> BoxFuture<'static, bool>;
}

/// What to do when the server presents a host key not in `known_hosts`.
/// A changed key is always rejected.
#[derive(Clone)]
pub enum HostKeyPolicy {
    /// Reject unknown hosts.
    Strict,
    /// Trust and record unknown hosts (OpenSSH `accept-new`).
    AcceptNew,
    /// Ask, and record the key if the user accepts.
    Ask(Arc<dyn HostKeyPrompt>),
}

impl fmt::Debug for HostKeyPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Strict => f.write_str("Strict"),
            Self::AcceptNew => f.write_str("AcceptNew"),
            Self::Ask(_) => f.write_str("Ask(..)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rooste_testkit::public_key;

    fn key_a() -> PublicKey {
        public_key("id_ed25519")
    }

    fn key_b() -> PublicKey {
        public_key("id_ed25519_encrypted")
    }

    #[test]
    fn missing_file_means_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        assert_eq!(
            check(&path, "example.com", 22, &key_a()).unwrap(),
            HostKeyStatus::Unknown
        );
    }

    #[test]
    fn learned_key_is_known_and_other_key_is_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("known_hosts");
        let key = key_a();
        learn(&path, "example.com", 2222, &key).unwrap();

        assert_eq!(
            check(&path, "example.com", 2222, &key).unwrap(),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&path, "example.com", 2222, &key_b()).unwrap(),
            HostKeyStatus::Changed { line: 1 }
        );
        assert_eq!(
            check(&path, "example.com", 22, &key).unwrap(),
            HostKeyStatus::Unknown,
            "port is part of the host identity"
        );
    }

    #[test]
    fn learn_writes_openssh_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, "other.example ssh-ed25519 AAAA").unwrap();
        learn(&path, "example.com", 22, &key_a()).unwrap();
        learn(&path, "example.com", 2222, &key_b()).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert!(lines[1].starts_with("example.com ssh-ed25519 AAAA"));
        assert!(lines[2].starts_with("[example.com]:2222 ssh-ed25519 AAAA"));
        assert!(
            !lines[1].contains("rooste-test"),
            "comment must be stripped"
        );
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn fingerprint_uses_openssh_format() {
        assert!(fingerprint(&key_a()).starts_with("SHA256:"));
    }
}
