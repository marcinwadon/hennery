//! Credentials at rest (gateway spec §6): XChaCha20-Poly1305, a random
//! 24-byte nonce per write, stored as `key_version ‖ nonce ‖ ciphertext`
//! (the version as 4 bytes, big-endian; the ciphertext ends in the 16-byte
//! tag).
//!
//! The AAD binds a blob to its row, its field and its key version:
//! `connection_id ‖ field ‖ key_version`, each part preceded by its length
//! as 4 bytes, big-endian (plan 8a decision 7), so no two (id, field) pairs
//! give one AAD. A blob moved to another row, read as another field (a
//! static token as an OAuth token, G-14) or relabelled with another version
//! does not open.

use crate::key::MasterKey;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

/// The nonce's length: XChaCha20's 192 bits.
pub const NONCE_LEN: usize = 24;

/// Poly1305's tag, at the end of every ciphertext.
const TAG_LEN: usize = 16;

/// The version prefix: a `u32`, big-endian.
const VERSION_LEN: usize = 4;

/// The AAD's field for a connection's static token (`cred_kind = static`).
/// Fields are named by their table too, so a later table's blobs (OAuth
/// clients, stdio servers' environments) never share one with these.
pub const STATIC_TOKEN: &str = "gw_credentials.static_token";

/// Why a blob did not open. None of them says more than this: a caller
/// names the row.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// Too short, or its version prefix is not the version its row says.
    #[error("the stored credential is malformed")]
    Malformed,
    /// Sealed under a key version this key is not.
    #[error("the stored credential was sealed under key version {stored}, not this master key's")]
    KeyVersion { stored: u32 },
    /// The tag does not check: another key, another row or field, or a
    /// changed byte.
    #[error("the stored credential does not open with this master key")]
    Refused,
}

/// The AAD's field for a stdio server's environment values (plan 8e): one
/// sealed JSON object per `gw_stdio_servers` row.
pub const STDIO_ENV: &str = "gw_stdio_servers.env";

fn aad(connection_id: &str, field: &str, key_version: u32) -> Vec<u8> {
    aad_of(&[connection_id.as_bytes(), field.as_bytes(), &key_version.to_be_bytes()])
}

/// Each part preceded by its length as 4 bytes, big-endian. `aad`'s three
/// parts are plan 8a's encoding, unchanged; a stdio server's (plan 8e, the
/// API review's R5) are its row id, its host, its hat, the field and the
/// version: five parts, so no stdio AAD reads as a connection's.
fn aad_of(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(parts.iter().map(|part| 4 + part.len()).sum());
    for part in parts {
        let len = u32::try_from(part.len()).expect("an AAD part under 4 GiB");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// A stdio server row's AAD: bound to the row, its host and its hat (R5),
/// so no `PUT` can move one hat's values into another hat or onto another
/// host.
fn stdio_aad(row_id: &str, host_id: &str, hat_id: &str, key_version: u32) -> Vec<u8> {
    aad_of(&[
        row_id.as_bytes(),
        host_id.as_bytes(),
        hat_id.as_bytes(),
        STDIO_ENV.as_bytes(),
        &key_version.to_be_bytes(),
    ])
}

fn cipher(key: &MasterKey) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key.bytes()).expect("a 32-byte key")
}

/// Seal `plaintext` for `connection_id`'s `field` under `key`, with a fresh
/// nonce from the operating system's generator.
pub fn seal(key: &MasterKey, connection_id: &str, field: &str, plaintext: &[u8]) -> Vec<u8> {
    seal_with(key, &aad(connection_id, field, key.version()), plaintext)
}

/// Seal a stdio server row's environment values (`STDIO_ENV`).
pub fn seal_stdio(key: &MasterKey, row_id: &str, host_id: &str, hat_id: &str, plaintext: &[u8]) -> Vec<u8> {
    seal_with(key, &stdio_aad(row_id, host_id, hat_id, key.version()), plaintext)
}

/// Open what `seal_stdio` stored for that row, host and hat.
pub fn open_stdio(
    key: &MasterKey,
    row_id: &str,
    host_id: &str,
    hat_id: &str,
    key_version: u32,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    open_with(key, key_version, blob, |version| {
        stdio_aad(row_id, host_id, hat_id, version)
    })
}

fn seal_with(key: &MasterKey, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let nonce = hennery_kernel::secret::random_bytes::<NONCE_LEN>();
    let payload = Payload { msg: plaintext, aad };
    let sealed = cipher(key)
        .encrypt(&XNonce::from(nonce), payload)
        .expect("XChaCha20-Poly1305 seals any message under 256 GiB");
    let mut out = Vec::with_capacity(VERSION_LEN + NONCE_LEN + sealed.len());
    out.extend_from_slice(&key.version().to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// Open what `seal` stored for `connection_id`'s `field`. `key_version` is
/// the row's column: the blob's own prefix must agree with it, and both
/// must be `key`'s version.
pub fn open(
    key: &MasterKey,
    connection_id: &str,
    field: &str,
    key_version: u32,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    open_with(key, key_version, blob, |version| aad(connection_id, field, version))
}

fn open_with(
    key: &MasterKey,
    key_version: u32,
    blob: &[u8],
    aad: impl FnOnce(u32) -> Vec<u8>,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if blob.len() < VERSION_LEN + NONCE_LEN + TAG_LEN {
        return Err(CryptoError::Malformed);
    }
    let (version, rest) = blob.split_at(VERSION_LEN);
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let prefix = u32::from_be_bytes(version.try_into().expect("4 bytes"));
    if prefix != key_version {
        return Err(CryptoError::Malformed);
    }
    if key_version != key.version() {
        return Err(CryptoError::KeyVersion { stored: key_version });
    }
    let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("24 bytes");
    let payload = Payload {
        msg: sealed,
        aad: &aad(key_version),
    };
    cipher(key)
        .decrypt(&XNonce::from(nonce), payload)
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The review's R1: the key version is part of the AAD itself, not only
    /// checked beside it. Sealed for version 1, the same blob does not open
    /// for version 2 under the same key and nonce.
    #[test]
    fn the_key_version_is_bound_into_the_aad() {
        assert_ne!(aad("conn-1", STATIC_TOKEN, 1), aad("conn-1", STATIC_TOKEN, 2));
        let key = MasterKey::from_bytes([3; 32]);
        let nonce = XNonce::from([4u8; NONCE_LEN]);
        let sealed = cipher(&key)
            .encrypt(
                &nonce,
                Payload {
                    msg: b"secret",
                    aad: &aad("conn-1", STATIC_TOKEN, 1),
                },
            )
            .unwrap();
        let reopened = |version| {
            cipher(&key).decrypt(
                &nonce,
                Payload {
                    msg: &sealed,
                    aad: &aad("conn-1", STATIC_TOKEN, version),
                },
            )
        };
        assert_eq!(reopened(1).unwrap(), b"secret");
        assert!(reopened(2).is_err());
    }

    /// Plan 8e: generalising the AAD left plan 8a's encoding byte for byte.
    #[test]
    fn a_connections_aad_is_plan_8as_encoding() {
        let mut expected = Vec::new();
        for part in [b"conn-1".as_slice(), STATIC_TOKEN.as_bytes(), &1u32.to_be_bytes()] {
            expected.extend_from_slice(&(part.len() as u32).to_be_bytes());
            expected.extend_from_slice(part);
        }
        assert_eq!(aad("conn-1", STATIC_TOKEN, 1), expected);
    }

    /// The API review's R5: a stdio row's values are bound to its row, its
    /// host and its hat; another of any does not open them, nor does a
    /// connection's field.
    #[test]
    fn a_stdio_blob_opens_only_for_its_row_host_and_hat() {
        let key = MasterKey::from_bytes([5; 32]);
        let blob = seal_stdio(&key, "stdio-1", "host-a", "hat-a", b"{}");
        assert_eq!(
            &*open_stdio(&key, "stdio-1", "host-a", "hat-a", 1, &blob).unwrap(),
            b"{}"
        );
        for (row, host, hat) in [
            ("stdio-2", "host-a", "hat-a"),
            ("stdio-1", "host-b", "hat-a"),
            ("stdio-1", "host-a", "hat-b"),
        ] {
            assert_eq!(
                open_stdio(&key, row, host, hat, 1, &blob).unwrap_err(),
                CryptoError::Refused,
                "{row} {host} {hat}"
            );
        }
        assert_eq!(
            open(&key, "stdio-1", STATIC_TOKEN, 1, &blob).unwrap_err(),
            CryptoError::Refused
        );
    }
}
