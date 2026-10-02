//! `GET /api/capabilities` over HTTP (kernel spec §8; plan 4b). Its cookie
//! and browser rules are pinned by `auth.rs`'s route table.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::rest::{CapabilitiesResponse, DeploymentMode};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;

/// The signed-in owner learns the mode and the features switched on, and
/// nothing else: today the whole cockpit, with the gateway's connections.
#[tokio::test]
async fn the_owner_learns_the_mode_and_the_features() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(
        Store::open_in_memory().unwrap(),
        Hosts::open_in_memory().unwrap(),
        Operator::open_in_memory().unwrap(),
    );
    let client = hennery_testkit::operator_client(&state.operator);
    tokio::spawn(hennery_sessions::serve(listener, state));
    let resp = client
        .get(format!("http://{addr}/api/capabilities"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::json!({"mode": "full", "features": ["mcp_connections", "mcp_stdio"]})
    );
    let typed: CapabilitiesResponse = serde_json::from_value(body).unwrap();
    assert_eq!(typed.mode, DeploymentMode::Full);
}
