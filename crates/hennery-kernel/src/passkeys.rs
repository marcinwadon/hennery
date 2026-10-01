//! Passkeys (kernel spec §3.2, plan 3c): WebAuthn with `webauthn-rs`.
//!
//! - **The relying party is `public_url`** (decision 1): its host is the RP
//!   id and its origin the one origin accepted, with no subdomain and no
//!   other port. A `public_url` whose host is an IP address has no RP id,
//!   so passkeys are unavailable there.
//! - **The user handle is derived from the owner's id** (decision 3), not
//!   stored: an authenticator keeps it, and it names no one.

use crate::operator::PublicUrl;
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::{Url, Uuid, Webauthn, WebauthnBuilder};

/// How long a ceremony may take from its start to its finish, and the
/// timeout the browser is given (decision 5).
pub const CEREMONY_TTL_SECS: i64 = 5 * 60;

/// The relying party's name, which an authenticator may show.
const RP_NAME: &str = "hennery";

/// The relying party for `public_url`: `None` when its host is an IP
/// address (decision 1).
pub fn relying_party(public_url: &PublicUrl) -> Option<Webauthn> {
    let rp_id = public_url.rp_id()?;
    let origin = Url::parse(public_url.origin()).ok()?;
    WebauthnBuilder::new(rp_id, &origin)
        .ok()?
        .rp_name(RP_NAME)
        .allow_subdomains(false)
        .allow_any_port(false)
        .timeout(std::time::Duration::from_secs(CEREMONY_TTL_SECS as u64))
        .build()
        .ok()
}

/// The owner's WebAuthn user handle: the first 16 bytes of the SHA-256 of
/// their id. The same owner always gets the same handle.
pub fn user_handle(owner_id: &str) -> Uuid {
    let digest = Sha256::digest(owner_id.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}
