//! What the process's log never shows (plan 8c, ACP core §8). Some
//! libraries trace whole messages: at `trace` the ACP crate logs every
//! JSON-RPC line it sends (`session/new`, with a session's gateway token in
//! its MCP headers) and at `debug` the adapter's answers (an error that
//! quotes its config), and tungstenite logs every WebSocket message (the
//! `start_session` frame that carries the token, on both ends). The ACP
//! crate also quotes a misbehaving adapter's raw stdout whole in its own
//! `warn` (`agent-client-protocol` 2.2.0: a line it cannot parse as
//! JSON-RPC becomes a parse-error `Error` whose `data` is that line, logged
//! at `warn`), so it cannot stop at `info` like the others — it is held at
//! `error`. `tungstenite` and `tokio_tungstenite` only ever trace whole
//! messages at `debug` and `trace`, so `info` is enough for them. Each
//! target is held to its own level, whatever `RUST_LOG` says.

use tracing::Subscriber;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::{Layered, SubscriberExt};

/// The targets whose events can carry a session's secrets, and the level
/// each is held to at most: `agent_client_protocol` quotes a misbehaving
/// adapter's stray stdout in its own `warn`, so it is held at `error`;
/// `tungstenite` and `tokio_tungstenite` only trace whole messages at
/// `debug`/`trace`, so `info` is enough.
pub const MESSAGE_TRACING_TARGETS: &[(&str, LevelFilter)] = &[
    ("agent_client_protocol", LevelFilter::ERROR),
    ("tungstenite", LevelFilter::INFO),
    ("tokio_tungstenite", LevelFilter::INFO),
];

/// Every target at any level, but each of `MESSAGE_TRACING_TARGETS` at its
/// own level at most.
pub fn secret_cap() -> Targets {
    MESSAGE_TRACING_TARGETS.iter().fold(
        Targets::new().with_default(LevelFilter::TRACE),
        |targets, (target, level)| targets.with_target(*target, *level),
    )
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
