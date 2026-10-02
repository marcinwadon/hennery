//! An upstream's `Mcp-Session-Id`, bound to the token that opened it
//! (plan 8e decision 13; gateway spec §5.2, §5.5).
//!
//! Every token on a connection sends upstream with the connection's one
//! credential, so the upstream's session id is all that separates two
//! sessions' upstream state. The proxy keeps no session table: the id the
//! client sees is `<upstream id>.<tag>`, `tag` the hex of an HMAC-SHA256,
//! under a key of the process's own, over the bearer token's hash, the
//! connection's id and the upstream id, each length-prefixed. A request's
//! id is verified and stripped before it goes up; one that fails is the
//! same 404 as an unknown token, and nothing is forwarded.
//!
//! The key is random per process: after a restart every id is refused, and
//! the client opens a new session (MCP's streamable HTTP: a 404 on a
//! request with a session id means "initialize again").

use crate::tokens::token_hash;
use axum::http::HeaderValue;
use hennery_kernel::secret::random_bytes;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// The tag's length in hex characters.
const TAG_HEX: usize = 64;

/// The process's session-id key. Clones share it.
#[derive(Clone)]
pub struct SessionIds {
    key: Arc<Zeroizing<[u8; 32]>>,
}

impl SessionIds {
    /// A fresh random key.
    pub fn new() -> Self {
        Self {
            key: Arc::new(Zeroizing::new(random_bytes::<32>())),
        }
    }

    fn mac(&self, token: &str, connection_id: &str, upstream: &[u8]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.key[..]).expect("HMAC takes a key of any length");
        for part in [token_hash(token).as_bytes(), connection_id.as_bytes(), upstream] {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part);
        }
        mac
    }

    /// The id the client sees for `upstream`, the id an upstream answered
    /// with on `connection_id` to a request carrying `token`.
    pub fn wrap(&self, token: &str, connection_id: &str, upstream: &HeaderValue) -> HeaderValue {
        let tag = hex::encode(
            self.mac(token, connection_id, upstream.as_bytes())
                .finalize()
                .into_bytes(),
        );
        let mut out = upstream.as_bytes().to_vec();
        out.push(b'.');
        out.extend_from_slice(tag.as_bytes());
        HeaderValue::from_bytes(&out).expect("a header value, a dot and hex are a header value")
    }

    /// The upstream id `downstream` wraps, if it was wrapped for `token` on
    /// `connection_id` by this process; else `None`. The tag is compared in
    /// constant time.
    pub fn unwrap(&self, token: &str, connection_id: &str, downstream: &HeaderValue) -> Option<HeaderValue> {
        let bytes = downstream.as_bytes();
        let dot = bytes.iter().rposition(|b| *b == b'.')?;
        let (upstream, tag) = (&bytes[..dot], &bytes[dot + 1..]);
        // Lowercase hex only: one spelling per id.
        if tag.len() != TAG_HEX || !tag.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let tag = hex::decode(tag).ok()?;
        self.mac(token, connection_id, upstream).verify_slice(&tag).ok()?;
        HeaderValue::from_bytes(upstream).ok()
    }
}

impl Default for SessionIds {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SessionIds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionIds(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "hnys_one";

    fn id(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn a_wrapped_id_unwraps_to_the_upstream_id() {
        let ids = SessionIds::new();
        let wrapped = ids.wrap(TOKEN, "c1", &id("up.with.dots-1"));
        assert!(wrapped.to_str().unwrap().starts_with("up.with.dots-1."));
        assert_eq!(ids.unwrap(TOKEN, "c1", &wrapped), Some(id("up.with.dots-1")));
    }

    #[test]
    fn another_token_connection_or_process_is_refused() {
        let ids = SessionIds::new();
        let wrapped = ids.wrap(TOKEN, "c1", &id("up-1"));
        assert_eq!(ids.unwrap("hnys_two", "c1", &wrapped), None, "another token");
        assert_eq!(ids.unwrap(TOKEN, "c2", &wrapped), None, "another connection");
        assert_eq!(SessionIds::new().unwrap(TOKEN, "c1", &wrapped), None, "another process");
        assert_eq!(
            ids.clone().unwrap(TOKEN, "c1", &wrapped),
            Some(id("up-1")),
            "a clone shares the key"
        );
    }

    #[test]
    fn a_bare_forged_or_respelled_id_is_refused() {
        let ids = SessionIds::new();
        let wrapped = ids.wrap(TOKEN, "c1", &id("up-1"));
        let good = wrapped.to_str().unwrap();
        let tag = &good["up-1.".len()..];
        let flipped = if tag.ends_with('0') { '1' } else { '0' };
        for bad in [
            "up-1".to_owned(),
            format!("up-2.{tag}"),
            format!("up-1.{}", tag.to_uppercase()),
            format!("up-1.{}", &tag[..62]),
            format!("up-1.{tag}00"),
            format!("{good}."),
            format!("up-1.{}{flipped}", &tag[..63]),
        ] {
            assert_eq!(ids.unwrap(TOKEN, "c1", &id(&bad)), None, "{bad}");
        }
    }

    /// The parts are length-prefixed: moving bytes between the connection
    /// id and the upstream id changes the tag.
    #[test]
    fn the_parts_cannot_be_shifted() {
        let ids = SessionIds::new();
        let a = ids.wrap(TOKEN, "c1", &id("2up"));
        let b = ids.wrap(TOKEN, "c12", &id("up"));
        // Unprefixed, both would MAC "c12up".
        assert_ne!(&a.as_bytes()[4..], &b.as_bytes()[3..]);
    }
}
