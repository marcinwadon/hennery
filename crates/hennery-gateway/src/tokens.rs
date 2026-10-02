//! Session tokens (gateway spec §3.1): one per session, minted at every
//! start and resume, revoked on park, close, adapter exit and host revoke,
//! and superseded by the next mint for the same session.
//!
//! The functions here take the caller's `rusqlite::Transaction` (lane L1):
//! the sessions store mints inside the transition that starts or resumes a
//! session, before it commits, so a failed mint rolls the transition back,
//! and revokes inside the transition that ends one. They touch only
//! `gw_session_tokens`, and every statement names the owner (lane L6).
//!
//! A token is `hnry_session_` and 64 lowercase hexadecimal digits: 32
//! random bytes behind a prefix a secret scanner can match (plan 8d
//! decision 1). Only its SHA-256 is stored; the plaintext exists in the
//! `SessionToken` the mint returns, and then only in the frame that carries
//! it to the session's host.

use anyhow::Result;
use hennery_kernel::secret::{random_bytes, sha256_hex};
use rusqlite::{Transaction, params};
use zeroize::Zeroizing;

/// What every session token starts with (plan 8d decision 1).
pub const SESSION_TOKEN_PREFIX: &str = "hnry_session_";

/// The random part's length, in hexadecimal digits (32 bytes).
const RANDOM_HEX: usize = 64;

/// A freshly minted session token. The plaintext is wiped from memory when
/// dropped and never shown by `Debug`.
#[derive(Clone)]
pub struct SessionToken(Zeroizing<String>);

impl SessionToken {
    /// The token, for the `Authorization: Bearer <token>` header of the
    /// session's `mcp_servers` entries (gateway spec §3.2) and nothing else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken(<redacted>)")
    }
}

/// Whether `token` has a session token's shape: the prefix, then exactly
/// 64 lowercase hexadecimal digits. Anything else is not looked up.
pub fn is_session_token(token: &str) -> bool {
    token
        .strip_prefix(SESSION_TOKEN_PREFIX)
        .is_some_and(|rest| rest.len() == RANDOM_HEX && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// How a token is stored and looked up: its SHA-256, in hexadecimal.
pub(crate) fn token_hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// Mint the token of `session_id` on `host_id` in `hat_id`, inside the
/// caller's transaction. It replaces the session's previous token, revoked
/// or not, which no longer resolves (superseded). The host and the hat
/// must be the owner's (their foreign keys), or the mint fails and the
/// caller's transaction should roll back.
pub fn mint_in(
    tx: &Transaction<'_>,
    owner_id: &str,
    session_id: &str,
    host_id: &str,
    hat_id: &str,
    now: i64,
) -> Result<SessionToken> {
    let random = Zeroizing::new(hex::encode(Zeroizing::new(random_bytes::<32>()).as_slice()));
    // Built at its final size, so no grown-out buffer keeps part of it.
    let mut token = Zeroizing::new(String::with_capacity(SESSION_TOKEN_PREFIX.len() + RANDOM_HEX));
    token.push_str(SESSION_TOKEN_PREFIX);
    token.push_str(&random);
    let changed = tx.execute(
        "INSERT INTO gw_session_tokens(session_id, owner_id, host_id, hat_id, token_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(session_id) DO UPDATE SET host_id = excluded.host_id, hat_id = excluded.hat_id,
             token_hash = excluded.token_hash, created_at = excluded.created_at, last_used_at = NULL,
             revoked_at = NULL
             WHERE gw_session_tokens.owner_id = excluded.owner_id",
        params![session_id, owner_id, host_id, hat_id, token_hash(&token), now],
    )?;
    // Another owner's row under this id: never taken over.
    anyhow::ensure!(changed == 1, "session {session_id} has a token of another owner's");
    Ok(SessionToken(token))
}

/// Revoke the token of `session_id`, inside the caller's transaction.
/// True if a live one was revoked; revoking twice, or a session that never
/// had one, is not an error.
pub fn revoke_in(tx: &Transaction<'_>, owner_id: &str, session_id: &str, now: i64) -> Result<bool> {
    let changed = tx.execute(
        "UPDATE gw_session_tokens SET revoked_at = ?3
         WHERE session_id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
        params![session_id, owner_id, now],
    )?;
    Ok(changed == 1)
}

/// Revoke every live token of `host_id` (a host revoke, ACP core §4.8),
/// inside the caller's transaction: how many were.
pub fn revoke_host_in(tx: &Transaction<'_>, owner_id: &str, host_id: &str, now: i64) -> Result<usize> {
    Ok(tx.execute(
        "UPDATE gw_session_tokens SET revoked_at = ?3
         WHERE host_id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
        params![host_id, owner_id, now],
    )?)
}

/// Delete every token of `hat_id`, revoked or not, inside the caller's
/// transaction: the gateway's part of a hat purge (gateway spec §2, kernel
/// spec §5.5), before the hat row goes. Idempotent.
pub fn purge_hat_in(tx: &Transaction<'_>, owner_id: &str, hat_id: &str) -> Result<usize> {
    Ok(tx.execute(
        "DELETE FROM gw_session_tokens WHERE hat_id = ?1 AND owner_id = ?2",
        params![hat_id, owner_id],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_is_the_prefix_and_64_lowercase_hex_digits() {
        let good = format!("{SESSION_TOKEN_PREFIX}{}", "a1".repeat(32));
        assert!(is_session_token(&good));
        for bad in [
            String::new(),
            SESSION_TOKEN_PREFIX.to_string(),
            format!("{SESSION_TOKEN_PREFIX}{}", "a1".repeat(31)),
            format!("{SESSION_TOKEN_PREFIX}{}0", "a1".repeat(32)),
            format!("{SESSION_TOKEN_PREFIX}{}", "A1".repeat(32)),
            format!("hnry_client_{}", "a1".repeat(32)),
            format!("{}{}", SESSION_TOKEN_PREFIX.to_uppercase(), "a1".repeat(32)),
        ] {
            assert!(!is_session_token(&bad), "{bad}");
        }
    }

    #[test]
    fn a_token_s_debug_shows_nothing_of_it() {
        let token = SessionToken(Zeroizing::new(format!("{SESSION_TOKEN_PREFIX}{}", "ab".repeat(32))));
        let shown = format!("{token:?} {token:#?}");
        assert!(!shown.contains("abab"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
