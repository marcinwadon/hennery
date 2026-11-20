//! `GET /api/capabilities` (kernel spec §8; plan 4b): which mode this
//! collector runs in, and which features it has switched on. The web UI
//! shows the views of its mode (frontend spec §2) and hides a screen whose
//! feature is off.
//!
//! Behind the session cookie: the shell asks for it first, so a 401 here is
//! what sends a signed-out browser to the login screen. The login and setup
//! screens never ask, so they render in either mode.

use crate::operator::Operator;
use axum::routing::get;
use axum::{Json, Router};
use hennery_proto::rest::{CapabilitiesResponse, DeploymentMode};
use std::sync::Arc;

/// The one place the mode is decided. Always the whole cockpit until the
/// standalone gateway (plan 8g) adds its switch here.
pub fn mode() -> DeploymentMode {
    DeploymentMode::Full
}

/// The one place features are switched on: each gateway part adds its name
/// when its API lands (`mcp_connections`, `mcp_stdio`, `mcp_oauth`,
/// `mcp_clients`). Plan 8a's connections API (`/api/mcp/connections`) is
/// served, alongside stdio servers and plan 8f's OAuth (`…/authorize`, the callback, the
/// pre-registered client, the probe).
pub fn features() -> Vec<String> {
    vec![
        "mcp_connections".to_owned(),
        "mcp_stdio".to_owned(),
        "mcp_oauth".to_owned(),
        "web_push".to_owned(),
        "session_defaults".to_owned(),
        "skills".to_owned(),
        "hook_sets".to_owned(),
    ]
}

/// What `GET /api/capabilities` answers.
pub fn current() -> CapabilitiesResponse {
    CapabilitiesResponse {
        mode: mode(),
        features: features(),
        collector_version: env!("CARGO_PKG_VERSION").to_owned(),
        source_commit: source_commit(option_env!("HENNERY_BUILD_SOURCE_SHA")),
    }
}

fn source_commit(recorded: Option<&str>) -> Option<String> {
    recorded
        .filter(|sha| matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
}

/// The route, behind the browser rules and the session cookie.
pub fn router(operator: Arc<Operator>) -> Router {
    crate::auth::operator_only(
        Router::new().route("/api/capabilities", get(|| async { Json(current()) })),
        operator,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_collector_is_the_whole_cockpit_with_the_gateways_connections() {
        let mut body = serde_json::to_value(current()).unwrap();
        let source = body.as_object_mut().unwrap().remove("source_commit").unwrap();
        assert!(
            source.is_null()
                || source.as_str().is_some_and(|sha| {
                    matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        );
        assert_eq!(
            body,
            serde_json::json!({"mode": "full", "features": ["mcp_connections", "mcp_stdio", "mcp_oauth", "web_push", "session_defaults", "skills", "hook_sets"],
                "collector_version": env!("CARGO_PKG_VERSION")})
        );
    }

    /// The other mode, as the web UI reads it (`lib/views.ts`).
    #[test]
    fn the_gateway_mode_is_named_gateway() {
        assert_eq!(
            serde_json::to_value(DeploymentMode::Gateway).unwrap(),
            serde_json::json!("gateway")
        );
    }

    #[test]
    fn source_identity_is_only_a_recorded_commit() {
        let sha = "abcdef1234567890abcdef1234567890abcdef12";
        assert_eq!(source_commit(Some(sha)).as_deref(), Some(sha));
        let sha256 = "a".repeat(64);
        assert_eq!(source_commit(Some(&sha256)).as_deref(), Some(sha256.as_str()));
        for value in [
            None,
            Some(""),
            Some("unrecorded (development build)"),
            Some("unrecorded (dirty source)"),
            Some("/source/checkout"),
            Some("not-a-commit00000000000000000000000000000"),
        ] {
            assert_eq!(source_commit(value), None);
        }
    }
}
