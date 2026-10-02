//! Credentials at rest (gateway spec §6): XChaCha20-Poly1305 with a random
//! nonce per write, AAD = connection id ‖ field ‖ key version, stored as
//! key version ‖ nonce ‖ ciphertext. A blob opens only for the row and the
//! field it was sealed for, under the key and the version it names.

use hennery_gateway::crypto::{CryptoError, NONCE_LEN, STATIC_TOKEN, open, seal};
use hennery_gateway::key::{KEY_VERSION, MasterKey};

fn key(seed: u8) -> MasterKey {
    MasterKey::from_bytes([seed; 32])
}

#[test]
fn a_sealed_secret_opens_again() {
    let key = key(1);
    let blob = seal(&key, "conn-1", STATIC_TOKEN, b"secret token");
    assert_eq!(&blob[..4], &KEY_VERSION.to_be_bytes());
    assert_eq!(blob.len(), 4 + NONCE_LEN + b"secret token".len() + 16);
    assert!(!blob.windows(12).any(|w| w == b"secret token"));
    let opened = open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &blob).unwrap();
    assert_eq!(opened.as_slice(), b"secret token");
}

#[test]
fn every_write_takes_a_fresh_nonce() {
    let key = key(1);
    let a = seal(&key, "conn-1", STATIC_TOKEN, b"same");
    let b = seal(&key, "conn-1", STATIC_TOKEN, b"same");
    assert_ne!(a[4..4 + NONCE_LEN], b[4..4 + NONCE_LEN]);
    assert_ne!(a, b);
}

#[test]
fn a_blob_opens_only_for_its_own_row_and_field() {
    let key = key(1);
    let blob = seal(&key, "conn-1", STATIC_TOKEN, b"secret");
    assert_eq!(
        open(&key, "conn-2", STATIC_TOKEN, KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
    assert_eq!(
        open(&key, "conn-1", "oauth_tokens", KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
    // The parts are length-prefixed: moving a character from the id to
    // the field is another AAD.
    let blob = seal(&key, "conn-1x", "y", b"secret");
    assert_eq!(
        open(&key, "conn-1", "xy", KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
}

#[test]
fn another_key_opens_nothing() {
    let blob = seal(&key(1), "conn-1", STATIC_TOKEN, b"secret");
    assert_eq!(
        open(&key(2), "conn-1", STATIC_TOKEN, KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
}

#[test]
fn a_changed_byte_or_version_is_refused() {
    let key = key(1);
    let blob = seal(&key, "conn-1", STATIC_TOKEN, b"secret");
    for at in [4, 4 + NONCE_LEN, blob.len() - 1] {
        let mut tampered = blob.clone();
        tampered[at] ^= 1;
        assert_eq!(
            open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &tampered).unwrap_err(),
            CryptoError::Refused,
            "byte {at}"
        );
    }
    // The blob's version must be the column's, and the key's.
    let mut relabelled = blob.clone();
    relabelled[..4].copy_from_slice(&2u32.to_be_bytes());
    assert_eq!(
        open(&key, "conn-1", STATIC_TOKEN, 2, &relabelled).unwrap_err(),
        CryptoError::KeyVersion { stored: 2 }
    );
    assert_eq!(
        open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &relabelled).unwrap_err(),
        CryptoError::Malformed
    );
    assert_eq!(
        open(&key, "conn-1", STATIC_TOKEN, 2, &blob).unwrap_err(),
        CryptoError::Malformed
    );
}

#[test]
fn a_short_blob_is_malformed() {
    let key = key(1);
    for len in [0, 3, 4 + NONCE_LEN, 4 + NONCE_LEN + 15] {
        // The right version, where there is room for it: only the length
        // is wrong.
        let mut blob = vec![0u8; len];
        if len >= 4 {
            blob[..4].copy_from_slice(&KEY_VERSION.to_be_bytes());
        }
        assert_eq!(
            open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &blob).unwrap_err(),
            CryptoError::Malformed,
            "{len}"
        );
    }
}
