mod support;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use hennery_kernel::{
    operator::{Operator, SetupOutcome},
    secret::unix_now,
};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn owner(world: &support::World) -> (hennery_gateway::api::GatewayState, String, String) {
    let operator = Arc::new(Operator::open(&world.db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator
        .set_up(&setup, "correct horse battery", "https://hennery.example", now)
        .unwrap()
    else {
        panic!("setup")
    };
    let fresh = operator.open_session("test", &phc, now).unwrap().unwrap();
    let stale = operator.open_session("test", &phc, now - 3600).unwrap().unwrap();
    let mut state = world.gateway();
    state.operator = operator;
    (state, fresh, stale)
}

#[tokio::test]
async fn unconfigured_owner_list_is_empty_and_does_not_create_control_file() {
    let world = support::World::new();
    let (state, fresh, _) = owner(&world);
    let response = hennery_gateway::api::router(state)
        .oneshot(
            Request::builder()
                .uri("/api/plugins/registrations")
                .header("origin", "https://hennery.example")
                .header("cookie", format!("__Host-hennery_session={fresh}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = axum::body::to_bytes(response.into_body(), 16384).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), serde_json::json!([]));
    assert!(!world.dir.path().join("plugin-control.db").exists());
}

#[tokio::test]
async fn owner_registration_requires_fresh_step_up_before_parsing_or_discovery() {
    let world = support::World::new();
    let (state, _, stale) = owner(&world);
    let response = hennery_gateway::api::router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugins/registrations")
                .header("origin", "https://hennery.example")
                .header("cookie", format!("__Host-hennery_session={stale}"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!world.dir.path().join("plugin-control.db").exists());
}

#[test]
fn machine_credentials_are_a_single_canonical_activity_bearer_and_never_browser_origin() {
    use axum::http::{HeaderMap, HeaderValue};
    use hennery_gateway::plugin_api::machine_token;
    let token = format!("hnry_activity_{}", "a".repeat(64));
    let mut headers = HeaderMap::new();
    assert!(machine_token(&headers).is_none());
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    assert_eq!(machine_token(&headers), Some(token.as_str()));
    for origin in ["", "null", "https://hennery.example"] {
        headers.insert("origin", HeaderValue::from_str(origin).unwrap());
        assert!(machine_token(&headers).is_none());
    }
    headers.remove("origin");
    headers.append("authorization", format!("Bearer {token}").parse().unwrap());
    assert!(machine_token(&headers).is_none());
    for auth in [
        format!("bearer {token}"),
        format!("Bearer  {token}"),
        format!("Bearer {token},Bearer {token}"),
        format!("Bearer {}", token.to_uppercase()),
        "Bearer hnry_session_abcd".into(),
    ] {
        headers.remove("authorization");
        headers.insert("authorization", auth.parse().unwrap());
        assert!(machine_token(&headers).is_none(), "{auth}");
    }
}

#[test]
fn query_decoding_never_accepts_duplicate_or_alternate_authority() {
    use hennery_gateway::plugin_api::machine_query;
    assert_eq!(machine_query(None).unwrap(), (None, 20));
    assert_eq!(
        machine_query(Some("after=opaque&limit=2")).unwrap(),
        (Some("opaque".into()), 2)
    );
    assert_eq!(
        machine_query(Some("%61fter=opaque&%6cimit=2")).unwrap(),
        (Some("opaque".into()), 2)
    );
    for query in [
        "after=a&after=b",
        "after=a&%61fter=b",
        "limit=1&limit=2",
        "Limit=1",
        "owner_id=victim",
        "hat_id=victim",
        "token=secret",
        "after=",
        "limit=0",
        "limit=21",
        "limit=-1",
        "limit=nan",
        "after=a&origin=operator",
    ] {
        assert!(machine_query(Some(query)).is_err(), "{query}");
    }
    assert!(machine_query(Some(&format!("after={}", "x".repeat(2049)))).is_err());
    assert!(machine_query(Some(&"x".repeat(4097))).is_err());
}

#[test]
fn hostile_and_differential_management_json_cannot_add_authority_or_override_fields() {
    use hennery_proto::plugin_management::{RegisterPluginRequest, RotatePluginRequest};
    let canonical = r#"{"connection_id":"one","lifecycle_credential":"secret","activity":true,"startup":false,"namespace_isolated":true}"#;
    assert!(serde_json::from_str::<RegisterPluginRequest>(canonical).is_ok());
    assert!(
        serde_json::from_str::<RegisterPluginRequest>(&canonical.replace("connection_id", r#"connection_\u0069d"#))
            .is_ok()
    );
    let mut cases = Vec::new();
    for key in [
        "owner_id",
        "Owner_Id",
        r#"\u006fwner_id"#,
        "hat_id",
        "origin",
        "provenance",
    ] {
        cases.push(canonical.replacen('{', &format!("{{\"{key}\":\"Operator\","), 1));
    }
    cases.push(canonical.replacen('{', "{\"activity\":false,", 1));
    cases.push(canonical.replacen('{', r#"{"\u0061ctivity":false,"#, 1));
    cases.push(canonical.replace("activity", "Activity"));
    cases.push(format!("\u{feff}{canonical}"));
    for case in cases {
        assert!(serde_json::from_str::<RegisterPluginRequest>(&case).is_err(), "{case}");
    }
    assert!(serde_json::from_str::<RotatePluginRequest>(r#"{"action":"prepare"}"#).is_ok());
    for case in [
        r#"{"action":"prepare","action":"finish"}"#,
        r#"{"action":"prepare","owner_id":"victim"}"#,
        r#"{"Action":"prepare"}"#,
    ] {
        assert!(serde_json::from_str::<RotatePluginRequest>(case).is_err(), "{case}");
    }
}

#[test]
fn machine_errors_classify_reset_budget_and_denial_without_echoing_private_text() {
    use hennery_gateway::plugin_api::machine_failure;
    for (message, status) in [
        ("reset_required", StatusCode::CONFLICT),
        ("plug-in budget exceeded", StatusCode::TOO_MANY_REQUESTS),
        ("private upstream body or SQL detail", StatusCode::NOT_FOUND),
    ] {
        let response = machine_failure(anyhow::anyhow!(message));
        assert_eq!(response.status(), status);
    }
}
