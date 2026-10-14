//! PuTTY private key files (`.ppk`). Import (versions 2 and 3) comes from
//! `ssh-key`; export writes version 3, optionally encrypted with
//! Argon2id + AES-256-CBC as PuTTYgen does.

use aes::Aes256;
use aes::cipher::{BlockCipherEncrypt, KeyInit};
use argon2::{Argon2, Params};
use hmac::{Hmac, Mac};
use russh::keys::PrivateKey;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::KeyError;

const ARGON2_MEMORY_KIB: u32 = 8192;
const ARGON2_PASSES: u32 = 13;

/// Whether a `.ppk` file needs a passphrase.
pub fn is_encrypted(text: &str) -> bool {
    text.lines().any(|l| l.trim() == "Encryption: aes256-cbc")
}

/// Read a `.ppk` file.
pub fn import(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, KeyError> {
    if is_encrypted(text) && passphrase.is_none() {
        return Err(KeyError::Encrypted);
    }
    PrivateKey::from_ppk(text, passphrase.map(str::to_owned)).map_err(|e| {
        if passphrase.is_some() && is_encrypted(text) {
            KeyError::BadPassphrase
        } else {
            KeyError::Key(e)
        }
    })
}

/// Minimal SSH wire-format reader.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Result<u32, KeyError> {
        let (head, rest) = self
            .0
            .split_at_checked(4)
            .ok_or_else(|| KeyError::Unsupported("truncated key".into()))?;
        self.0 = rest;
        Ok(u32::from_be_bytes([head[0], head[1], head[2], head[3]]))
    }

    fn string(&mut self) -> Result<&'a [u8], KeyError> {
        let len = self.u32()? as usize;
        let (value, rest) = self
            .0
            .split_at_checked(len)
            .ok_or_else(|| KeyError::Unsupported("truncated key".into()))?;
        self.0 = rest;
        Ok(value)
    }
}

fn put_string(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

/// Public blob, private blob and comment, in PPK layout.
struct PpkParts {
    algorithm: String,
    public: Vec<u8>,
    private: Zeroizing<Vec<u8>>,
    comment: String,
}

/// Re-encode an OpenSSH key's fields the way PuTTY stores them.
fn parts(key: &PrivateKey) -> Result<PpkParts, KeyError> {
    if key.is_encrypted() {
        return Err(KeyError::Encrypted);
    }
    let bytes = key.to_bytes()?;
    let mut r = Reader(&bytes);
    let magic = b"openssh-key-v1\0";
    if !r.0.starts_with(magic) {
        return Err(KeyError::Unsupported("unexpected key encoding".into()));
    }
    r.0 = &r.0[magic.len()..];
    let (_cipher, _kdf, _kdf_options) = (r.string()?, r.string()?, r.string()?);
    if r.u32()? != 1 {
        return Err(KeyError::Unsupported("multiple keys".into()));
    }
    let public = r.string()?.to_vec();
    let mut s = Reader(r.string()?);
    let (_check1, _check2) = (s.u32()?, s.u32()?);
    let algorithm = String::from_utf8_lossy(s.string()?).into_owned();
    let mut private = Zeroizing::new(Vec::new());
    match algorithm.as_str() {
        "ssh-ed25519" => {
            let _public = s.string()?;
            let secret = s.string()?;
            // OpenSSH stores seed || public key; PuTTY stores the seed.
            let seed = secret
                .get(..32)
                .ok_or_else(|| KeyError::Unsupported("short Ed25519 key".into()))?;
            put_string(&mut private, seed);
        }
        "ssh-rsa" => {
            let (_n, _e) = (s.string()?, s.string()?);
            let (d, iqmp, p, q) = (s.string()?, s.string()?, s.string()?, s.string()?);
            for mpint in [d, p, q, iqmp] {
                put_string(&mut private, mpint);
            }
        }
        name if name.starts_with("ecdsa-sha2-") => {
            let (_curve, _point) = (s.string()?, s.string()?);
            put_string(&mut private, s.string()?);
        }
        other => return Err(KeyError::Unsupported(other.to_owned())),
    }
    let comment = String::from_utf8_lossy(s.string()?).into_owned();
    Ok(PpkParts {
        algorithm,
        public,
        private,
        comment,
    })
}

fn base64_lines(data: &[u8]) -> Vec<String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                encoded.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
        .as_bytes()
        .chunks(64)
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect()
}

/// AES-256-CBC without padding; `data` must be whole blocks.
fn encrypt_cbc(key: &[u8], iv: &[u8], data: &mut [u8]) {
    let cipher = Aes256::new_from_slice(key).expect("32-byte key");
    let mut previous = [0u8; 16];
    previous.copy_from_slice(iv);
    for block in data.as_chunks_mut::<16>().0 {
        for (byte, prev) in block.iter_mut().zip(previous) {
            *byte ^= prev;
        }
        cipher.encrypt_block(block.into());
        previous.copy_from_slice(block);
    }
}

/// Write `key` as a version 3 `.ppk` file, encrypted when a non-empty
/// passphrase is given.
pub fn export(key: &PrivateKey, passphrase: Option<&str>) -> Result<String, KeyError> {
    let parts = parts(key)?;
    let passphrase = passphrase.filter(|p| !p.is_empty());
    let mut private = parts.private.clone();
    let mut kdf_lines = Vec::new();
    let (encryption, mac_key) = match passphrase {
        None => ("none", Zeroizing::new(Vec::new())),
        Some(passphrase) => {
            let salt: [u8; 16] = rand::random();
            let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_PASSES, 1, Some(80))
                .map_err(|e| KeyError::Unsupported(e.to_string()))?;
            let mut derived = Zeroizing::new([0u8; 80]);
            Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
                .hash_password_into(passphrase.as_bytes(), &salt, derived.as_mut())
                .map_err(|e| KeyError::Unsupported(e.to_string()))?;
            // Pad to whole cipher blocks before computing the MAC.
            while private.len() % 16 != 0 {
                private.push(rand::random());
            }
            let mac_key = Zeroizing::new(derived[48..80].to_vec());
            kdf_lines = vec![
                "Key-Derivation: Argon2id".to_owned(),
                format!("Argon2-Memory: {ARGON2_MEMORY_KIB}"),
                format!("Argon2-Passes: {ARGON2_PASSES}"),
                "Argon2-Parallelism: 1".to_owned(),
                format!("Argon2-Salt: {}", hex::encode(salt)),
            ];
            let mac_input = mac_input(&parts, "aes256-cbc", &private);
            let mac = hmac_sha256(&mac_key, &mac_input);
            encrypt_cbc(&derived[..32], &derived[32..48], &mut private);
            return Ok(render(&parts, "aes256-cbc", &kdf_lines, &private, &mac));
        }
    };
    let mac = hmac_sha256(&mac_key, &mac_input(&parts, encryption, &private));
    Ok(render(&parts, encryption, &kdf_lines, &private, &mac))
}

fn mac_input(parts: &PpkParts, encryption: &str, private: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    put_string(&mut data, parts.algorithm.as_bytes());
    put_string(&mut data, encryption.as_bytes());
    put_string(&mut data, parts.comment.as_bytes());
    put_string(&mut data, &parts.public);
    put_string(&mut data, private);
    data
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn render(
    parts: &PpkParts,
    encryption: &str,
    kdf_lines: &[String],
    private: &[u8],
    mac: &[u8],
) -> String {
    let public_lines = base64_lines(&parts.public);
    let private_lines = base64_lines(private);
    let mut out = vec![
        format!("PuTTY-User-Key-File-3: {}", parts.algorithm),
        format!("Encryption: {encryption}"),
        format!("Comment: {}", parts.comment),
        format!("Public-Lines: {}", public_lines.len()),
    ];
    out.extend(public_lines);
    out.extend(kdf_lines.iter().cloned());
    out.push(format!("Private-Lines: {}", private_lines.len()));
    out.extend(private_lines);
    out.push(format!("Private-MAC: {}", hex::encode(mac)));
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{KeyType, generate};

    #[test]
    fn base64_matches_the_standard() {
        let lines = |d: &[u8]| base64_lines(d).concat();
        assert_eq!(lines(b""), "");
        assert_eq!(lines(b"f"), "Zg==");
        assert_eq!(lines(b"fo"), "Zm8=");
        assert_eq!(lines(b"foo"), "Zm9v");
        assert_eq!(lines(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_lines(&[0u8; 100]).len(), 3, "64 characters per line");
    }

    #[test]
    fn export_and_import_round_trip() {
        for kind in [KeyType::Ed25519, KeyType::EcdsaP256] {
            let key = generate(kind, "round trip").unwrap();
            let plain = export(&key, None).unwrap();
            assert!(plain.starts_with("PuTTY-User-Key-File-3: "));
            assert!(!is_encrypted(&plain));
            assert_eq!(import(&plain, None).unwrap().public_key(), key.public_key());

            let secret = export(&key, Some("passw0rd")).unwrap();
            assert!(is_encrypted(&secret));
            assert!(secret.contains("Key-Derivation: Argon2id"));
            assert!(matches!(import(&secret, None), Err(KeyError::Encrypted)));
            assert!(matches!(
                import(&secret, Some("nope")),
                Err(KeyError::BadPassphrase)
            ));
            let back = import(&secret, Some("passw0rd")).unwrap();
            assert_eq!(back.public_key(), key.public_key());
            assert_eq!(back.comment().as_str_lossy(), "round trip");
        }
    }
}
