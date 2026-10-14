//! Generating key pairs and saving them in OpenSSH format.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use russh::keys::ssh_key::{Algorithm, EcdsaCurve, LineEnding, private::RsaKeypair};
use russh::keys::{PrivateKey, PublicKey};

use crate::KeyError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeyType {
    /// Recommended: small, fast and widely supported.
    #[default]
    Ed25519,
    EcdsaP256,
    Rsa3072,
    Rsa4096,
}

impl KeyType {
    pub const ALL: [KeyType; 4] = [Self::Ed25519, Self::EcdsaP256, Self::Rsa3072, Self::Rsa4096];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ed25519 => "Ed25519 (recommended)",
            Self::EcdsaP256 => "ECDSA P-256",
            Self::Rsa3072 => "RSA 3072",
            Self::Rsa4096 => "RSA 4096",
        }
    }

    /// Default file name in `~/.ssh`.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Ed25519 => "id_ed25519",
            Self::EcdsaP256 => "id_ecdsa",
            Self::Rsa3072 | Self::Rsa4096 => "id_rsa",
        }
    }
}

/// Generate a new key pair.
pub fn generate(kind: KeyType, comment: &str) -> Result<PrivateKey, KeyError> {
    let mut rng = rand::rng();
    let mut key = match kind {
        KeyType::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519)?,
        KeyType::EcdsaP256 => PrivateKey::random(
            &mut rng,
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
        )?,
        KeyType::Rsa3072 => PrivateKey::from(RsaKeypair::random(&mut rng, 3072)?),
        KeyType::Rsa4096 => PrivateKey::from(RsaKeypair::random(&mut rng, 4096)?),
    };
    key.set_comment(comment);
    Ok(key)
}

/// `algorithm base64 comment`, as in `authorized_keys`.
pub fn public_line(key: &PublicKey) -> String {
    key.to_openssh().unwrap_or_default()
}

/// Write `key` to `path` (readable only by the owner) and its public half to
/// `path.pub`. With a passphrase the private key is encrypted. Existing
/// files are not overwritten. Returns the public key path.
pub fn save_openssh(
    key: &PrivateKey,
    path: &Path,
    passphrase: Option<&str>,
) -> Result<PathBuf, KeyError> {
    let public_path = PathBuf::from(format!("{}.pub", path.display()));
    for target in [path, public_path.as_path()] {
        if target.exists() {
            return Err(KeyError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", target.display()),
            )));
        }
    }
    let private = match passphrase.filter(|p| !p.is_empty()) {
        Some(passphrase) => key.encrypt(&mut rand::rng(), passphrase)?,
        None => key.clone(),
    };
    let text = private.to_openssh(LineEnding::LF)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    write_private(path, text.as_bytes())?;
    fs::write(&public_path, format!("{}\n", public_line(key.public_key())))?;
    Ok(public_path)
}

/// Create a file only the owner can read.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_each_fast_type() {
        for kind in [KeyType::Ed25519, KeyType::EcdsaP256] {
            let key = generate(kind, "me@laptop").unwrap();
            assert_eq!(key.comment().as_str_lossy(), "me@laptop");
            let line = public_line(key.public_key());
            assert!(line.ends_with(" me@laptop"), "{line}");
        }
        assert_ne!(
            generate(KeyType::Ed25519, "").unwrap().public_key(),
            generate(KeyType::Ed25519, "").unwrap().public_key(),
            "keys are random"
        );
    }

    #[test]
    #[ignore = "RSA generation is slow in debug builds"]
    fn generates_rsa() {
        let key = generate(KeyType::Rsa3072, "rsa").unwrap();
        assert!(matches!(key.algorithm(), Algorithm::Rsa { .. }));
    }

    #[test]
    fn saves_plain_and_encrypted_keys() {
        let dir = tempfile::tempdir().unwrap();
        let key = generate(KeyType::Ed25519, "saved").unwrap();

        let path = dir.path().join("ssh").join("id_plain");
        let public = save_openssh(&key, &path, None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(
            crate::load(&text, None).unwrap().public_key(),
            key.public_key()
        );
        let pub_text = fs::read_to_string(&public).unwrap();
        assert_eq!(
            PublicKey::from_openssh(pub_text.trim()).unwrap(),
            *key.public_key()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let path = dir.path().join("id_secret");
        save_openssh(&key, &path, Some("hunter2")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(matches!(crate::load(&text, None), Err(KeyError::Encrypted)));
        assert!(matches!(
            crate::load(&text, Some("wrong")),
            Err(KeyError::BadPassphrase)
        ));
        assert_eq!(
            crate::load(&text, Some("hunter2")).unwrap().public_key(),
            key.public_key()
        );

        assert!(save_openssh(&key, &path, None).is_err(), "never overwrites");
    }
}
