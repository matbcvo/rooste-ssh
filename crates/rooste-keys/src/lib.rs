//! SSH keys: generating them, converting to and from PuTTY's `.ppk` format,
//! and a built-in SSH agent.

pub mod agent;
pub mod keygen;
pub mod ppk;

pub use agent::Agent;
pub use keygen::KeyType;
pub use russh::keys::{PrivateKey, PublicKey};

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("{0}")]
    Key(#[from] russh::keys::ssh_key::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the key is encrypted; enter its passphrase")]
    Encrypted,
    #[error("wrong passphrase or damaged key file")]
    BadPassphrase,
    #[error("not a supported key: {0}")]
    Unsupported(String),
}

/// SHA-256 fingerprint in OpenSSH format.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(russh::keys::HashAlg::Sha256).to_string()
}

/// Load a private key in OpenSSH or PuTTY format.
pub fn load(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, KeyError> {
    if text.trim_start().starts_with("PuTTY-User-Key-File-") {
        return ppk::import(text, passphrase);
    }
    let key = PrivateKey::from_openssh(text)?;
    if !key.is_encrypted() {
        return Ok(key);
    }
    let passphrase = passphrase.ok_or(KeyError::Encrypted)?;
    key.decrypt(passphrase).map_err(|_| KeyError::BadPassphrase)
}
