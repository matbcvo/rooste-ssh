//! Algorithm policy: modern algorithms by default, a small legacy set only
//! when a session explicitly opts in. SSH-1, DSA, DES/3DES and RC4 are never
//! offered (the russh `dsa` and `des` features are not enabled).

use std::borrow::Cow;

use russh::keys::{Algorithm, EcdsaCurve, HashAlg};
use russh::{Preferred, cipher, kex, mac};

const MODERN_KEX: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_GEX_SHA256,
    kex::DH_G18_SHA512,
    kex::DH_G17_SHA512,
    kex::DH_G16_SHA512,
    kex::DH_G15_SHA512,
    kex::DH_G14_SHA256,
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

const LEGACY_KEX: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_GEX_SHA256,
    kex::DH_G18_SHA512,
    kex::DH_G17_SHA512,
    kex::DH_G16_SHA512,
    kex::DH_G15_SHA512,
    kex::DH_G14_SHA256,
    kex::DH_GEX_SHA1,
    kex::DH_G14_SHA1,
    kex::DH_G1_SHA1,
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

const MODERN_KEYS: &[Algorithm] = &[
    Algorithm::Ed25519,
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP521,
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    },
];

const LEGACY_KEYS: &[Algorithm] = &[
    Algorithm::Ed25519,
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP521,
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    },
    // `ssh-rsa`: RSA with SHA-1 signatures.
    Algorithm::Rsa { hash: None },
];

const MODERN_CIPHERS: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    cipher::AES_128_GCM,
    cipher::AES_256_CTR,
    cipher::AES_192_CTR,
    cipher::AES_128_CTR,
];

const LEGACY_CIPHERS: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    cipher::AES_128_GCM,
    cipher::AES_256_CTR,
    cipher::AES_192_CTR,
    cipher::AES_128_CTR,
    cipher::AES_256_CBC,
    cipher::AES_192_CBC,
    cipher::AES_128_CBC,
];

const MODERN_MACS: &[mac::Name] = &[
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512,
    mac::HMAC_SHA256,
];

const LEGACY_MACS: &[mac::Name] = &[
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512,
    mac::HMAC_SHA256,
    mac::HMAC_SHA1_ETM,
    mac::HMAC_SHA1,
];

/// Algorithm preferences for a session.
pub fn preferred(allow_legacy: bool) -> Preferred {
    let (kex, key, cipher, mac) = if allow_legacy {
        (LEGACY_KEX, LEGACY_KEYS, LEGACY_CIPHERS, LEGACY_MACS)
    } else {
        (MODERN_KEX, MODERN_KEYS, MODERN_CIPHERS, MODERN_MACS)
    };
    Preferred {
        kex: Cow::Borrowed(kex),
        key: Cow::Borrowed(key),
        cipher: Cow::Borrowed(cipher),
        mac: Cow::Borrowed(mac),
        ..Preferred::DEFAULT
    }
}

/// Whether the named algorithm is only offered when legacy mode is enabled.
pub fn is_legacy(name: &str) -> bool {
    let modern = preferred(false);
    let legacy = preferred(true);
    let in_list = |p: &Preferred| {
        p.kex.iter().any(|n| n.as_ref() == name)
            || p.key.iter().any(|a| a.to_string() == name)
            || p.cipher.iter().any(|n| n.as_ref() == name)
            || p.mac.iter().any(|n| n.as_ref() == name)
    };
    in_list(&legacy) && !in_list(&modern)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &Preferred) -> Vec<String> {
        p.kex
            .iter()
            .map(|n| n.as_ref().to_owned())
            .chain(p.key.iter().map(|a| a.to_string()))
            .chain(p.cipher.iter().map(|n| n.as_ref().to_owned()))
            .chain(p.mac.iter().map(|n| n.as_ref().to_owned()))
            .collect()
    }

    #[test]
    fn modern_defaults_exclude_sha1_and_cbc() {
        for name in names(&preferred(false)) {
            assert!(!name.contains("sha1"), "{name} must not be a default");
            assert!(!name.contains("cbc"), "{name} must not be a default");
            assert_ne!(
                name, "ssh-rsa",
                "SHA-1 RSA signatures must not be a default"
            );
        }
    }

    #[test]
    fn legacy_is_a_superset_of_modern() {
        let legacy = names(&preferred(true));
        for name in names(&preferred(false)) {
            assert!(legacy.contains(&name), "{name} missing from legacy list");
        }
    }

    #[test]
    fn never_supported_algorithms_are_absent() {
        for name in names(&preferred(true)) {
            for banned in ["ssh-dss", "3des", "arcfour", "none"] {
                assert!(!name.contains(banned), "{name} must never be offered");
            }
        }
    }

    #[test]
    fn classifies_legacy_names() {
        assert!(is_legacy("diffie-hellman-group14-sha1"));
        assert!(is_legacy("aes128-cbc"));
        assert!(is_legacy("ssh-rsa"));
        assert!(!is_legacy("curve25519-sha256"));
        assert!(!is_legacy("rsa-sha2-512"));
        assert!(!is_legacy("ssh-dss"));
    }
}
