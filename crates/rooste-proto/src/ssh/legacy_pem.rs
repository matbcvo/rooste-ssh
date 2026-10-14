//! Encrypted PEM keys in the old OpenSSL format (`Proc-Type: 4,ENCRYPTED`),
//! as written by PuTTYgen's "Export OpenSSH key" and older `ssh-keygen`.
//! russh only reads the AES-128 variant; this also handles 3DES, AES-192
//! and AES-256 by decrypting to a plain PEM key first.

use aes::cipher::{BlockCipherDecrypt, KeyInit};
use base64ct::{Base64, Encoding};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a valid encrypted PEM file, or an unsupported cipher.
    Format(String),
    /// The passphrase didn't decrypt the key.
    BadPassphrase,
}

/// Whether `text` is a PEM key in the old encrypted format.
pub fn is_encrypted(text: &str) -> bool {
    text.lines().any(|l| l.trim() == "Proc-Type: 4,ENCRYPTED")
}

/// Decrypt an old-format encrypted PEM key into an unencrypted one.
pub fn decrypt(text: &str, passphrase: &str) -> Result<Zeroizing<String>, Error> {
    let format = |m: &str| Error::Format(m.to_owned());
    let mut lines = text.lines().map(str::trim);
    let begin = lines
        .find(|l| l.starts_with("-----BEGIN ") && l.ends_with(" PRIVATE KEY-----"))
        .ok_or_else(|| format("no PEM private key found"))?;
    let label = &begin["-----BEGIN ".len()..begin.len() - "-----".len()];
    let mut dek_info = None;
    let mut body = String::new();
    for line in lines {
        if line.starts_with("-----END ") {
            break;
        }
        if let Some(value) = line.strip_prefix("DEK-Info:") {
            dek_info = Some(value.trim().to_owned());
        } else if !line.contains(':') {
            body.push_str(line);
        }
    }
    let dek_info = dek_info.ok_or_else(|| format("missing DEK-Info header"))?;
    let (cipher, iv) = dek_info
        .split_once(',')
        .ok_or_else(|| format("invalid DEK-Info header"))?;
    let iv = hex(iv.trim()).ok_or_else(|| format("invalid IV in DEK-Info header"))?;
    let mut data =
        Zeroizing::new(Base64::decode_vec(&body).map_err(|_| format("invalid base64 in key"))?);

    let (key_len, block_len) = match cipher {
        "DES-EDE3-CBC" => (24, 8),
        "AES-128-CBC" => (16, 16),
        "AES-192-CBC" => (24, 16),
        "AES-256-CBC" => (32, 16),
        other => return Err(Error::Format(format!("unsupported key cipher {other}"))),
    };
    if iv.len() != block_len || data.is_empty() || !data.len().is_multiple_of(block_len) {
        return Err(format("invalid encrypted key length"));
    }
    let key = derive_key(passphrase.as_bytes(), &iv[..8], key_len);
    match cipher {
        "DES-EDE3-CBC" => {
            let c = des::TdesEde3::new_from_slice(&key).expect("24-byte key");
            cbc_decrypt::<8>(&iv, &mut data, |b| c.decrypt_block(b.into()));
        }
        "AES-128-CBC" => {
            let c = aes::Aes128::new_from_slice(&key).expect("16-byte key");
            cbc_decrypt::<16>(&iv, &mut data, |b| c.decrypt_block(b.into()));
        }
        "AES-192-CBC" => {
            let c = aes::Aes192::new_from_slice(&key).expect("24-byte key");
            cbc_decrypt::<16>(&iv, &mut data, |b| c.decrypt_block(b.into()));
        }
        _ => {
            let c = aes::Aes256::new_from_slice(&key).expect("32-byte key");
            cbc_decrypt::<16>(&iv, &mut data, |b| c.decrypt_block(b.into()));
        }
    }

    // PKCS#7 padding; a wrong passphrase almost always breaks it, and the
    // result must be a DER sequence.
    let pad = usize::from(*data.last().expect("not empty"));
    let valid_pad = (1..=block_len).contains(&pad)
        && data[data.len() - pad..]
            .iter()
            .all(|&b| usize::from(b) == pad);
    if !valid_pad || data[0] != 0x30 {
        return Err(Error::BadPassphrase);
    }
    let der = &data[..data.len() - pad];

    let encoded = Zeroizing::new(Base64::encode_string(der));
    let mut pem = Zeroizing::new(format!("-----BEGIN {label}-----\n"));
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        pem.push('\n');
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    Ok(pem)
}

/// OpenSSL's `EVP_BytesToKey` with MD5 and one iteration.
fn derive_key(passphrase: &[u8], salt: &[u8], len: usize) -> Zeroizing<Vec<u8>> {
    let mut key = Zeroizing::new(Vec::with_capacity(len + 16));
    let mut previous: Vec<u8> = Vec::new();
    while key.len() < len {
        let mut input = Zeroizing::new(previous.clone());
        input.extend_from_slice(passphrase);
        input.extend_from_slice(salt);
        previous = md5::compute(&*input).0.to_vec();
        key.extend_from_slice(&previous);
    }
    key.truncate(len);
    key
}

/// CBC decryption in place; `data` must be whole blocks.
fn cbc_decrypt<const N: usize>(iv: &[u8], data: &mut [u8], decrypt: impl Fn(&mut [u8; N])) {
    let mut previous = [0u8; N];
    previous.copy_from_slice(iv);
    for block in data.as_chunks_mut::<N>().0 {
        let ciphertext = *block;
        decrypt(block);
        for (byte, prev) in block.iter_mut().zip(previous) {
            *byte ^= prev;
        }
        previous = ciphertext;
    }
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `openssl rsa -traditional -des3` (and `-aes256`) with the passphrase
    /// "secret", generated for these tests only.
    const DES3_KEY: &str = include_str!("../../tests/fixtures/rsa_des3.pem");
    const AES256_KEY: &str = include_str!("../../tests/fixtures/rsa_aes256.pem");

    #[test]
    fn decrypts_3des_and_aes256() {
        for text in [DES3_KEY, AES256_KEY] {
            assert!(is_encrypted(text));
            let pem = decrypt(text, "secret").unwrap();
            assert!(pem.starts_with("-----BEGIN RSA PRIVATE KEY-----\n"));
            let key = russh::keys::decode_secret_key(&pem, None).unwrap();
            assert_eq!(key.algorithm(), russh::keys::Algorithm::Rsa { hash: None });
        }
    }

    #[test]
    fn wrong_passphrase() {
        assert_eq!(
            decrypt(DES3_KEY, "wrong").unwrap_err(),
            Error::BadPassphrase
        );
    }

    #[test]
    fn key_derivation_matches_openssl() {
        // `openssl enc -des-ede3-cbc -pass pass:secret -S 0102030405060708 -P -md md5`
        let key = derive_key(b"secret", &[1, 2, 3, 4, 5, 6, 7, 8], 24);
        assert_eq!(
            hex("C9E5A1BD216DBE1317E230CEF48F38EE7F0E17AD64022144").unwrap(),
            *key
        );
    }
}
