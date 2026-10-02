//! The `Notifier` (gateway spec §7, umbrella §10.2): told when a
//! connection's status moves into or out of a problem, `needs_auth` or
//! `error`, and only then. A problem already there when the collector
//! starts is announced once (`announce_startup`); after that, only
//! transitions. Re-posting on every probe tick trains people to ignore it
//! (G-22).
//!
//! Full mode delivers through Web Push (`hennery_kernel::push::Push`, lane
//! L9); standalone mode (plan 8g) will log and call a webhook. The wording
//! is fixed here, and `error`'s never asks the operator to reconnect: an
//! outage is not fixed by a click (G-22).

use crate::model::{Status, StatusChange};
use hennery_kernel::push::{Notice, Push, Urgency};

/// What happened to a connection, for the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    /// The vendor refused the credential: sign in again.
    NeedsAuth,
    /// An outage or a transport failure.
    Failing,
    /// Back to `ok` from a problem.
    Recovered,
}

impl Alert {
    /// What the notification says after the connection's label (lane L9).
    pub fn body(self) -> &'static str {
        match self {
            Self::NeedsAuth => "needs sign-in again",
            Self::Failing => "is failing",
            Self::Recovered => "is working again",
        }
    }

    /// The alert a status change makes, if any: into `needs_auth` or
    /// `error` from anything else, or out of either into `ok`. Nothing else
    /// is a transition the operator hears of (`not_connected` → `ok` is a
    /// first Connect, which the operator just did).
    pub fn of(change: &StatusChange) -> Option<Self> {
        if change.from == change.to {
            return None;
        }
        match change.to {
            Status::NeedsAuth => Some(Self::NeedsAuth),
            Status::Error => Some(Self::Failing),
            Status::Ok if change.from.is_problem() => Some(Self::Recovered),
            Status::Ok | Status::NotConnected => None,
        }
    }
}

/// One alert about one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionAlert {
    pub connection_id: String,
    pub hat_id: String,
    pub label: String,
    pub alert: Alert,
}

/// Where alerts go (umbrella §10.2). Called from request handlers and the
/// probe's loop: it must not block.
pub trait Notifier: Send + Sync + 'static {
    fn notify(&self, alert: &ConnectionAlert);
}

/// Tell `notifier` of `change`, if it is a transition the operator hears
/// of.
pub fn announce(notifier: &dyn Notifier, change: &StatusChange) {
    if let Some(alert) = Alert::of(change) {
        tracing::info!(connection_id = %change.connection_id, alert = ?alert, "gateway connection status changed");
        notifier.notify(&ConnectionAlert {
            connection_id: change.connection_id.clone(),
            hat_id: change.hat_id.clone(),
            label: change.label.clone(),
            alert,
        });
    }
}

/// The generic title (lane L9): the hat's push policy may show it instead
/// of the label.
pub const GENERIC_TITLE: &str = "An MCP connection needs attention";

/// Full mode: Web Push, with the mapping agreed with the push lane (lane
/// L9). The kernel applies the hat's push policy.
impl Notifier for Push {
    fn notify(&self, alert: &ConnectionAlert) {
        Push::notify(self, notice(alert));
    }
}

/// The push notice for `alert` (lane L9).
pub fn notice(alert: &ConnectionAlert) -> Notice {
    Notice {
        hat_id: alert.hat_id.clone(),
        urgency: Urgency::Normal,
        title: alert.label.clone(),
        generic_title: GENERIC_TITLE.into(),
        body: alert.alert.body().into(),
        detail: None,
        url: "/mcp".into(),
        tag: format!("mcp-{}", alert.connection_id),
    }
}

/// Nobody is told: what tests that do not look use.
pub struct Silent;

impl Notifier for Silent {
    fn notify(&self, _alert: &ConnectionAlert) {}
}
