//! A revoke ends what is open on the token (plan 8e decision 12, the
//! fleet parent's ruling of 2026-10-02): for a token's revoke, scope that
//! is checked only when a request arrives is not revocation. (A change to a
//! connection, an unmount, delete or edit, is refused at request time only,
//! gateway spec §3.2.) Every request the proxy serves on a
//! session token watches that token here, from before the token is
//! resolved until the answer's body ends; a revoke, once its transaction
//! has committed, cuts every watch on the tokens it invalidated.
//!
//! The order closes the race with a revoke in flight: a revoke that
//! commits before the proxy resolves the token makes the resolve fail (the
//! same 404 as ever); one that commits after finds the watch already
//! registered, and cuts it. Cutting before the commit would race a
//! rollback, which would leave the token working with its streams cut.
//!
//! Tokens are named by their SHA-256 (`tokens::token_hash`), as stored: a
//! supersession at resume cuts the old token's watches and not the new
//! one's, which no request has presented yet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// The tokens a transaction invalidated, by hash: revoked, superseded or
/// deleted. Hand it to `Revocations::cut` once that transaction has
/// committed; dropping it unused leaves their open streams running.
#[must_use = "cut it after the transaction commits, or the revoked tokens' streams stay open"]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cut(pub(crate) Vec<String>);

impl Cut {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many tokens it names.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// This cut and `other`'s, for a transaction that invalidates in more
    /// than one place.
    pub fn and(mut self, other: Cut) -> Cut {
        self.0.extend(other.0);
        self
    }
}

#[derive(Default)]
struct Entry {
    cancel: CancellationToken,
    watchers: usize,
}

/// Every open watch, by token hash. Cheap to clone: the proxy and the
/// sessions' side of the gateway (`session::GatewayMcp`) share one.
#[derive(Clone, Default)]
pub struct Revocations {
    open: Arc<Mutex<HashMap<String, Entry>>>,
}

impl Revocations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Watch the token with `hash` until the returned `Watch` is dropped.
    pub fn watch(&self, hash: &str) -> Watch {
        let mut open = self.open.lock().expect("revocations lock");
        let entry = open.entry(hash.to_string()).or_default();
        entry.watchers += 1;
        Watch {
            revocations: self.clone(),
            hash: hash.to_string(),
            cancel: entry.cancel.clone(),
        }
    }

    /// End every watch on the tokens `cut` names. Call it only after the
    /// transaction that invalidated them has committed.
    pub fn cut(&self, cut: Cut) {
        let mut open = self.open.lock().expect("revocations lock");
        for hash in cut.0 {
            if let Some(entry) = open.remove(&hash) {
                entry.cancel.cancel();
            }
        }
    }

    /// How many tokens have an open watch (for tests).
    pub fn watched(&self) -> usize {
        self.open.lock().expect("revocations lock").len()
    }
}

/// One request's watch on its token.
pub struct Watch {
    revocations: Revocations,
    hash: String,
    cancel: CancellationToken,
}

impl Watch {
    /// Cancelled when the token is cut.
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut open = self.revocations.open.lock().expect("revocations lock");
        // A cut watch's entry is gone already, and an entry for the same
        // hash now is a newer one's: left alone. An uncut watch's entry is
        // still its own (only a cut, or its last watch, removes one).
        if !self.cancel.is_cancelled()
            && let Some(entry) = open.get_mut(&self.hash)
        {
            entry.watchers -= 1;
            if entry.watchers == 0 {
                open.remove(&self.hash);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_cancels_every_watch_on_its_tokens_and_no_other() {
        let revocations = Revocations::new();
        let a1 = revocations.watch("a");
        let a2 = revocations.watch("a");
        let b = revocations.watch("b");
        revocations.cut(Cut(vec!["a".into()]));
        assert!(a1.token().is_cancelled() && a2.token().is_cancelled());
        assert!(!b.token().is_cancelled());
    }

    #[test]
    fn a_watch_made_after_a_cut_is_not_cut_by_it() {
        let revocations = Revocations::new();
        let old = revocations.watch("a");
        revocations.cut(Cut(vec!["a".into()]));
        let new = revocations.watch("a");
        drop(old);
        assert!(!new.token().is_cancelled());
        assert_eq!(revocations.watched(), 1);
        drop(new);
        assert_eq!(revocations.watched(), 0);
    }

    #[test]
    fn the_last_watch_dropped_forgets_its_token() {
        let revocations = Revocations::new();
        let one = revocations.watch("a");
        let two = revocations.watch("a");
        drop(one);
        assert_eq!(revocations.watched(), 1);
        drop(two);
        assert_eq!(revocations.watched(), 0);
    }
}
