//! Single-flight refresh (gateway spec §4.4, §4.5).
//!
//! - **One lock per connection** (`Runtime::lock`). Under it the stored
//!   grant is read again: if its access token already differs from the one
//!   that failed, that one is used, and nothing is sent (G-13's other half).
//!   So concurrent 401s share one refresh.
//! - **A started refresh runs to completion and persists**, whatever the
//!   caller does: it runs as a task of its own, which the caller awaits; a
//!   caller that goes away drops the wait, not the task. It is bounded by
//!   `Runtime::refresh_timeout` (20 s) at the vendor (G-11).
//! - **The caller's cancellation never sets `needs_auth`** (G-12): only the
//!   task does, and only on the vendor's refusal.
//! - **Outcomes are distinct** (`Refreshed`): a token to retry with; nothing
//!   to refresh; the vendor refused (`needs_auth`); the vendor could not be
//!   reached; the result could not be stored (502, no `needs_auth`: a click
//!   cannot fix a disk).
//! - **The write is a compare-and-swap** on the blob read
//!   (`GatewayStore::store_refreshed`), so an edit that deleted the grant
//!   meanwhile is never undone.
//! - **`resource`** is sent on refresh as the grant recorded it (G-10); a
//!   server answering `invalid_target` is asked once more without it, and
//!   that is recorded (`resource_param_accepted`).

use crate::model::GrantTokens;
use crate::oauth::{self, Grant, TokenError};
use crate::runtime::Runtime;
use crate::store::RefreshedGrant;
use hennery_kernel::secret::unix_now;
use std::sync::Arc;
use zeroize::Zeroizing;

/// How close to its expiry an access token is refreshed before use
/// (gateway spec §4.4: 5 minutes).
pub const PROACTIVE: i64 = 5 * 60;

/// What a refresh came to.
#[derive(Clone, PartialEq, Eq)]
pub enum Refreshed {
    /// An access token to use: a fresh one, or one another refresh stored
    /// meanwhile.
    Retry(Zeroizing<String>),
    /// Nothing to refresh: no grant, or one without a refresh token.
    NotRefreshable,
    /// The vendor refused the refresh: the connection is `needs_auth`.
    Refused,
    /// The vendor could not be reached, or did not answer in time.
    Unavailable,
    /// The vendor answered, but the result could not be stored.
    Unsaved,
}

impl std::fmt::Debug for Refreshed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Retry(_) => "Retry(<redacted>)",
            Self::NotRefreshable => "NotRefreshable",
            Self::Refused => "Refused",
            Self::Unavailable => "Unavailable",
            Self::Unsaved => "Unsaved",
        })
    }
}

/// Whether an access token expiring at `expires_at` is to be refreshed
/// before it is used, at `now`.
pub fn due(expires_at: Option<i64>, now: i64) -> bool {
    expires_at.is_some_and(|at| at - now <= PROACTIVE)
}

/// Refresh `connection_id`'s grant, single-flight. `failed` is the access
/// token the upstream refused (a 401), or `None` for a refresh before use
/// (it is then done only if still due).
pub async fn refresh(runtime: Arc<Runtime>, connection_id: String, failed: Option<Zeroizing<String>>) -> Refreshed {
    let id = connection_id.clone();
    let task =
        tokio::spawn(async move { locked(&runtime, &connection_id, failed.as_deref().map(String::as_str)).await });
    match task.await {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::error!(connection_id = %id, error = %err, "gateway: a refresh failed");
            Refreshed::Unavailable
        }
    }
}

async fn locked(runtime: &Runtime, id: &str, failed: Option<&str>) -> Refreshed {
    let _guard = runtime.lock(id).await;
    let now = unix_now();
    let current = match runtime.store.oauth_credential(id, &runtime.key) {
        Ok(Some(current)) => current,
        Ok(None) => return Refreshed::NotRefreshable,
        Err(err) => {
            tracing::error!(connection_id = %id, error = %err, "gateway: a grant could not be read");
            return Refreshed::Unsaved;
        }
    };
    match failed {
        Some(failed) if current.tokens.access_token.as_str() != failed => {
            return Refreshed::Retry(current.tokens.access_token.clone());
        }
        None if !due(current.expires_at, now) => return Refreshed::Retry(current.tokens.access_token.clone()),
        _ => {}
    }
    let Some(refresh_token) = current.tokens.refresh_token.clone() else {
        return Refreshed::NotRefreshable;
    };
    let client = runtime.client(current.internal_network);
    let resource = if current.resource_param_accepted {
        current.resource.as_deref()
    } else {
        None
    };
    let grant = Grant::Refresh {
        refresh_token: &refresh_token,
        scopes: &current.scopes,
    };
    let sent = tokio::time::timeout(runtime.refresh_timeout, async {
        match oauth::token(&client, &current.client, &grant, resource).await {
            Err(TokenError::InvalidTarget) if resource.is_some() => {
                (oauth::token(&client, &current.client, &grant, None).await, false)
            }
            other => (other, current.resource_param_accepted),
        }
    })
    .await;
    let (issued, accepted) = match sent {
        Ok(sent) => sent,
        Err(_) => {
            tracing::warn!(connection_id = %id, "gateway: a refresh timed out");
            return Refreshed::Unavailable;
        }
    };
    match issued {
        Ok(issued) => {
            let expires_at = oauth::expires_at(unix_now(), issued.expires_in);
            // A response without a refresh token keeps the old one (§4.4).
            let tokens = GrantTokens {
                access_token: issued.access_token,
                refresh_token: issued.refresh_token.or(Some(refresh_token)),
            };
            let refreshed = RefreshedGrant {
                tokens: &tokens,
                expires_at,
                resource_param_accepted: accepted,
            };
            match runtime
                .store
                .store_refreshed(id, &current, &refreshed, &runtime.key, unix_now())
            {
                Ok(true) => {
                    tracing::info!(connection_id = %id, "gateway: a grant was refreshed");
                    Refreshed::Retry(tokens.access_token.clone())
                }
                // Changed meanwhile (an edit deleted it): use what is there.
                Ok(false) => match runtime.store.oauth_credential(id, &runtime.key) {
                    Ok(Some(now)) => Refreshed::Retry(now.tokens.access_token.clone()),
                    _ => Refreshed::NotRefreshable,
                },
                Err(err) => {
                    tracing::error!(connection_id = %id, error = %err, "gateway: a refreshed grant could not be stored");
                    Refreshed::Unsaved
                }
            }
        }
        Err(err @ (TokenError::Refused { .. } | TokenError::InvalidTarget)) => {
            tracing::warn!(connection_id = %id, why = %err.message(), "gateway: the vendor refused a refresh");
            runtime.announce(runtime.statuses.mark_needs_auth(
                id,
                &current.url,
                "the authorization server refused to refresh the grant",
                unix_now(),
            ));
            Refreshed::Refused
        }
        Err(err @ (TokenError::Unavailable | TokenError::EgressRefused(_) | TokenError::Invalid)) => {
            tracing::warn!(connection_id = %id, why = %err.message(), "gateway: a refresh did not reach the vendor");
            Refreshed::Unavailable
        }
    }
}
