use hennery_kernel::{hosts::Hosts, operator::Operator, secret::unix_now};
use hennery_proto::frames::{Indexed, SessionBody};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{PUBLIC_URL, owner_phc};
use serde_json::{Value, json};

#[tokio::test]
async fn rename_is_operator_only_and_null_restores_agent_title_without_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let state = AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        Operator::open(&db).unwrap(),
    );
    hennery_testkit::operator_client(&state.operator);
    let token = state
        .operator
        .open_session("test", &owner_phc(&state.operator), unix_now())
        .unwrap()
        .unwrap();
    state.store.create_session("s", "h", "fake", "/tmp", "", None).unwrap();
    state
        .store
        .ingest("s", 1, &SessionBody::session_started("r", "a"))
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/sessions/s", listener.local_addr().unwrap());
    let server = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .patch(&url)
            .header("origin", PUBLIC_URL)
            .json(&json!({"title":"No"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let patch = |body: Value| {
        client
            .patch(&url)
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("__Host-hennery_session={token}"))
            .json(&body)
    };
    let response = patch(json!({"title":"Chosen"})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let detail: Value = response.json().await.unwrap();
    assert_eq!(detail["title"], "Chosen");
    assert_eq!(detail["title_source"], "operator");
    state
        .store
        .ingest(
            "s",
            2,
            &SessionBody::AcpUpdate {
                indexed: Indexed {
                    title: Some("Agent title".into()),
                    ..Default::default()
                },
                payload: json!({}),
            },
        )
        .unwrap();
    assert_eq!(
        patch(json!({})).send().await.unwrap().json::<Value>().await.unwrap()["title"],
        "Chosen"
    );
    let detail = patch(json!({"title":null}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(detail["title"], "Agent title");
    assert_eq!(detail["title_source"], "agent");
    assert_eq!(patch(json!({"title":"  "})).send().await.unwrap().status(), 400);
    server.abort();
}

#[tokio::test(start_paused = true)]
async fn configuration_error_is_visible_and_refused_prompt_remains_a_draft() {
    configuration_error_case(false).await;
}

#[tokio::test(start_paused = true)]
async fn matching_evidence_does_not_clear_a_presumed_parked_configuration_error() {
    configuration_error_case(true).await;
}

async fn configuration_error_case(presume_parked: bool) {
    use hennery_proto::{
        delegation_host::{ConfigCompleteness, ConfigEvidence},
        frames::SessionConfig,
    };
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let state = AppState::new(
        Store::open(&db).unwrap(),
        Hosts::open(&db).unwrap(),
        Operator::open(&db).unwrap(),
    );
    hennery_testkit::operator_client(&state.operator);
    let token = state
        .operator
        .open_session("test", &owner_phc(&state.operator), unix_now())
        .unwrap()
        .unwrap();
    state.store.create_session("s", "h", "fake", "/tmp", "", None).unwrap();
    let wanted = SessionConfig {
        model: Some("haiku".into()),
        mode: Some("plan".into()),
        ..Default::default()
    };
    state.store.expect_initial_configuration("s", &wanted).unwrap();
    let actual = SessionConfig {
        model: Some("sonnet".into()),
        ..wanted.clone()
    };
    let indexed = |actual, revision| Indexed {
        config_evidence: Some(Box::new(ConfigEvidence {
            attach_generation: 0,
            config_revision: revision,
            completeness: ConfigCompleteness::Complete,
            actual,
        })),
        ..Default::default()
    };
    state
        .store
        .ingest(
            "s",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
                indexed: indexed(actual, 1),
                agent_home: None,
            },
        )
        .unwrap();
    let at = unix_now();
    let started = tokio::time::Instant::now();
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    state
        .store
        .refresh_delegation_attention(&state.hub, at + started.elapsed().as_secs() as i64)
        .unwrap();
    // Only the configuration deadline needs virtual time. HTTP I/O with a
    // paused clock can auto-advance the offline watcher and park this session.
    tokio::time::resume();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
    let client = reqwest::Client::new();
    let request = |method: reqwest::Method, path: &str| {
        client
            .request(method, format!("{url}{path}"))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("__Host-hennery_session={token}"))
    };
    let detail = request(reqwest::Method::GET, "/api/sessions/s")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let reason = detail["configuration_error"].as_str().unwrap();
    assert!(reason.contains("haiku") && reason.contains("sonnet") && reason.contains("plan"));
    let summaries = request(reqwest::Method::GET, "/api/view/sessions")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(summaries["sessions"][0]["configuration_error"], reason);
    let response = request(reqwest::Method::POST, "/api/sessions/s/prompt")
        .json(&json!({"content":[{"type":"text","text":"Draft prompt"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "configuration_mismatch"
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM turns", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let before_evidence = state.store.find_session("s").unwrap().unwrap();
    assert_eq!(
        before_evidence.lifecycle, "active",
        "presumed_parked={}",
        before_evidence.presumed_parked
    );
    assert!(!before_evidence.presumed_parked);
    if presume_parked {
        assert_eq!(state.store.presume_parked("h").unwrap().len(), 1);
        let parked = state.store.find_session("s").unwrap().unwrap();
        assert_eq!(parked.lifecycle, "parked");
        assert!(parked.presumed_parked);
    }
    state
        .store
        .ingest(
            "s",
            2,
            &SessionBody::AcpUpdate {
                indexed: indexed(wanted, 2),
                payload: json!({}),
            },
        )
        .unwrap();
    let after_evidence = state.store.find_session("s").unwrap().unwrap();
    if presume_parked {
        assert_eq!(after_evidence.configuration_error.as_deref(), Some(reason));
        assert_eq!(after_evidence.lifecycle, "parked");
        assert!(after_evidence.presumed_parked);
    } else {
        assert!(after_evidence.configuration_error.is_none());
        assert!(state.store.operator_attention(None, "").unwrap().is_empty());
    }
    server.abort();
}
