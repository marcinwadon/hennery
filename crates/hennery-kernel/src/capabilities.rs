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
/// `mcp_clients`). None has yet.
pub fn features() -> Vec<String> {
    Vec::new()
}

/// What `GET /api/capabilities` answers.
pub fn current() -> CapabilitiesResponse {
    CapabilitiesResponse {
        mode: mode(),
        features: features(),
    }
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
    fn the_collector_is_the_whole_cockpit_with_no_feature_on() {
        assert_eq!(
            serde_json::to_value(current()).unwrap(),
            serde_json::json!({"mode": "full", "features": []})
        );
    }
}
