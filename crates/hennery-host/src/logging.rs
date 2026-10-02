//! What the process's log never shows (plan 8c, ACP core §8). Some
//! libraries trace whole messages: at `trace` the ACP crate logs every
//! JSON-RPC line it sends (`session/new`, with a session's gateway token in
//! its MCP headers) and at `debug` the adapter's answers (an error that
//! quotes its config), and tungstenite logs every WebSocket message (the
//! `start_session` frame that carries the token, on both ends). Those
//! targets are held at `info`, whatever `RUST_LOG` says.

use tracing::Subscriber;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::{Layered, SubscriberExt};

/// The targets whose `debug` and `trace` events can carry a session's
/// secrets, and are therefore never shown.
pub const MESSAGE_TRACING_TARGETS: &[&str] = &["agent_client_protocol", "tungstenite", "tokio_tungstenite"];

/// Every target at any level, but `MESSAGE_TRACING_TARGETS` at `info` at
/// most.
pub fn secret_cap() -> Targets {
    MESSAGE_TRACING_TARGETS
        .iter()
        .fold(Targets::new().with_default(LevelFilter::TRACE), |targets, target| {
            targets.with_target(*target, LevelFilter::INFO)
        })
}

/// `subscriber` with `secret_cap` over it, as a global filter: an event it
/// refuses reaches no layer, so no `RUST_LOG` (`agent_client_protocol=trace`
/// included) lifts it. Every subscriber the process installs is wrapped in
/// this (`hennery`'s `log::init`).
pub fn capped<S>(subscriber: S) -> Layered<Targets, S>
where
    S: Subscriber,
{
    subscriber.with(secret_cap())
}
