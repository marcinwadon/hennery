//! Real operator HTTP contract for archival and retained shared workspaces.
use hennery_kernel::{hosts::Hosts, operator::Operator, secret::unix_now};
use hennery_sessions::{AppState, store::Store, workspaces::WorkspaceRegistration};
use hennery_testkit::{PUBLIC_URL, owner_phc};
use serde_json::Value;

#[tokio::test]
async fn archive_and_delete_release_only_their_own_workspace_membership() {
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
    let hat = state.hosts.hats().unwrap()[0].id.clone();
    state
        .store
        .register_workspace(WorkspaceRegistration {
            id: "workspace",
            host_id: "host",
            root: "/repo/worktree",
            repository: "/repo/.git",
            profile_source: "/repo",
            branch: "hennery/task",
            base_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        })
        .unwrap();
    for id in ["parent", "reviewer"] {
        state
            .store
            .create_session(id, "host", "fake", "/repo/worktree", &hat, None)
            .unwrap();
        state.store.close_now(id).unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(hennery_sessions::serve(listener, state.clone()));
    let client = reqwest::Client::new();
    let request = |method: &str, path: &str| {
        client
            .request(method.parse().unwrap(), format!("{url}{path}"))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("__Host-hennery_session={token}"))
    };
    assert_eq!(
        request("POST", "/api/sessions/parent/archive")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        state.store.find_session("parent").unwrap().unwrap().lifecycle,
        "archived"
    );
    let reconciliation = state
        .store
        .reconcile_host(
            "host",
            &[hennery_proto::frames::AttachedSession {
                session_id: "parent".into(),
                last_seq: 0,
                open_turn_id: None,
                ending: Some(false),
            }],
        )
        .unwrap();
    assert!(
        reconciliation.close.iter().any(|id| id == "parent"),
        "archived sessions must close stale adapters after reconnect"
    );
    let detail: Value = request("GET", "/api/sessions/parent/workspace")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["users"], 1);
    assert!(!state.store.claim_workspace_cleanup("workspace").unwrap());
    assert_eq!(
        request("DELETE", "/api/sessions/parent").send().await.unwrap().status(),
        200
    );
    assert_eq!(
        request("POST", "/api/sessions/reviewer/archive")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let inventory: Value = request("GET", "/api/workspaces")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inventory[0]["users"], 0);
    assert_eq!(inventory[0]["state"], "cleanup_pending");
    assert_eq!(
        request("POST", "/api/sessions/reviewer/resume")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        state.store.find_session("reviewer").unwrap().unwrap().lifecycle,
        "archived"
    );
    state.shutdown.cancel();
}
