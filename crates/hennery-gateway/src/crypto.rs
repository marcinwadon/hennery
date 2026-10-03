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

/// An OAuth connection's grant (`cred_kind = oauth_dcr | oauth_client`):
/// its access and refresh tokens, as one JSON object (plan 8f).
pub const OAUTH_TOKENS: &str = "gw_credentials.oauth_tokens";

/// An OAuth client's secret: the client the grant was made with.
pub const CLIENT_SECRET: &str = "gw_oauth_clients.client_secret";

/// The secret of a pre-registered client saved while a grant is live
/// (G-7): a field of its own, so it never opens as the active client's.
pub const PENDING_CLIENT_SECRET: &str = "gw_oauth_clients.pending_client_secret";

/// The field a connection's credential is sealed under, from its kind: a
/// row left under another kind does not open as this one's (G-14).
pub fn credential_field(kind: crate::model::CredKind) -> &'static str {
    match kind {
        crate::model::CredKind::OauthDcr | crate::model::CredKind::OauthClient => OAUTH_TOKENS,
        crate::model::CredKind::None | crate::model::CredKind::Static => STATIC_TOKEN,
    }
}

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

fn aad(connection_id: &str, field: &str, key_version: u32) -> Vec<u8> {
    let version = key_version.to_be_bytes();
    let mut out = Vec::with_capacity(12 + connection_id.len() + field.len() + version.len());
    for part in [connection_id.as_bytes(), field.as_bytes(), &version] {
        let len = u32::try_from(part.len()).expect("an AAD part under 4 GiB");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

fn cipher(key: &MasterKey) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key.bytes()).expect("a 32-byte key")
}

/// Seal `plaintext` for `connection_id`'s `field` under `key`, with a fresh
/// nonce from the operating system's generator.
pub fn seal(key: &MasterKey, connection_id: &str, field: &str, plaintext: &[u8]) -> Vec<u8> {
    let nonce = hennery_kernel::secret::random_bytes::<NONCE_LEN>();
    let payload = Payload {
        msg: plaintext,
        aad: &aad(connection_id, field, key.version()),
    };
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
        aad: &aad(connection_id, field, key_version),
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
}
