use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use hennery_gateway::{
    api::GatewayState, key::MasterKey, peers::PeerAccess, scope::ProxyStore, session::GatewayMcp, store::GatewayStore,
};
use hennery_kernel::{
    hats::HatChange,
    hosts::{Enrollment, Hosts},
    operator::Operator,
    secret::unix_now,
};
use hennery_proto::frames::{
    AgentIsolation, Capabilities, Capability, CollectorFrame, McpIsolation, McpServer, SessionBody,
};
use hennery_sessions::{
    AppState,
    store::{McpContext, Store},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct World {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    app: AppState,
    gateway: GatewayState,
    hat: String,
    other: String,
    _receivers: Vec<tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>>,
}
impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        for (host, key) in [
            (
                "host",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            ),
            (
                "host2",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            ),
        ] {
            hosts
                .register(
                    host,
                    &Enrollment {
                        public_key: key.into(),
                        name: host.into(),
                        host_version: "0".into(),
                        platform: "linux".into(),
                    },
                    1,
                )
                .unwrap();
            hosts
                .record_hello(host, "0", &Capabilities(vec![Capability::McpServers]), unix_now())
                .unwrap();
            hosts
                .record_mcp_isolation(
                    host,
                    &AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into()),
                )
                .unwrap();
        }
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let HatChange::Done(other) = hosts.create_hat("Other", None, 1).unwrap() else {
            panic!()
        };
        let operator = Arc::new(Operator::open(&db).unwrap());
        let token = operator.issue_setup_token(unix_now()).unwrap().unwrap();
        operator
            .set_up(&token, "correct horse battery", "https://hennery.example", unix_now())
            .unwrap();
        let gateway_store = Arc::new(GatewayStore::open(&db).unwrap());
        let gateway = GatewayState {
            runtime: Arc::new(hennery_gateway::runtime::Runtime::new(
                gateway_store.clone(),
                Arc::new(ProxyStore::open(&db).unwrap()),
                Arc::new(MasterKey::from_bytes([7; 32])),
                hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap(),
                Arc::new(hennery_gateway::notify::Silent),
            )),
            operator,
        };
        let app = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        app.store.set_session_mcp(Arc::new(GatewayMcp::for_collector(&gateway)));
        for h in [&hat, &other.id] {
            gateway_store.set_peer_policy(h, PeerAccess::Read).unwrap();
        }
        let mut receivers = Vec::new();
        for host in ["host", "host2"] {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let r = app
                .hub
                .register(
                    host,
                    tx,
                    Capabilities(vec![Capability::McpServers, Capability::Steering]),
                    AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into()),
                )
                .unwrap();
            app.hub.mark_ready(host, r.conn_id);
            receivers.push(rx);
        }
        Self {
            _dir: dir,
            db,
            app,
            gateway,
            hat,
            other: other.id,
            _receivers: receivers,
        }
    }
    fn start(&self, id: &str, host: &str, hat: &str, active: bool) -> String {
        let mcp = self
            .app
            .store
            .create_session_with_mcp(
                id,
                host,
                "claude",
                "/project",
                hat,
                None,
                McpContext {
                    capable: true,
                    isolation: McpIsolation::ClaudeStrict,
                    rules_name_other_hats: false,
                    source_policy: false,
                    native_sources: false,
                },
            )
            .unwrap()
            .unwrap();
        let token = mcp
            .servers
            .iter()
            .find_map(|s| match s {
                McpServer::Http { name, headers, .. } if name == "hennery_peers" => {
                    Some(headers[0].value.strip_prefix("Bearer ").unwrap().to_owned())
                }
                _ => None,
            })
            .unwrap();
        if active {
            let mut started = SessionBody::session_started("start", format!("agent-{id}"));
            if let SessionBody::SessionStarted { indexed, .. } = &mut started {
                indexed.steering_supported = true;
            }
            self.app.store.ingest(id, 1, &started).unwrap();
        }
        token
    }
    async fn rpc(&self, token: &str, method: &str, params: Value) -> (StatusCode, Value) {
        let response = hennery_sessions::builtin::router(self.app.clone(), self.gateway.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp/builtin/peers")
                    .header("content-type", "application/json")
                    .header("accept", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(
                        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    async fn tool(&self, token: &str, name: &str, args: Value) -> Value {
        let (status, body) = self
            .rpc(token, "tools/call", json!({"name":name,"arguments":args}))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"].clone()
    }
}

#[tokio::test]
async fn independent_cross_host_history_includes_retained_sessions_without_family_authority() {
    let w = World::new();
    let token = w.start("reader", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    w.start("secret", "host2", &w.other, true);
    assert!(
        !w.gateway
            .runtime
            .store
            .session_tools(&w.hat)
            .unwrap()
            .unwrap()
            .delegation_enabled
    );
    assert!(
        w.app
            .store
            .open_turn(
                "target",
                "turn",
                &[json!({"type":"text","text":"retained peer history"})]
            )
            .unwrap()
    );
    w.app
        .store
        .ingest(
            "target",
            2,
            &SessionBody::TurnStarted {
                request_id: "request".into(),
                turn_id: "turn".into(),
            },
        )
        .unwrap();
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    conn.execute(
        "UPDATE sessions SET lifecycle='closed',close_requested=1 WHERE owner_id=?1 AND id='target'",
        [w.app.store.owner_id()],
    )
    .unwrap();
    drop(conn);
    let list = w.tool(&token, "session_list", json!({})).await;
    assert_eq!(list["isError"], false);
    let text = list.to_string();
    assert!(text.contains("target"));
    assert!(!text.contains("secret"));
    let read = w.tool(&token, "session_read", json!({"session_id":"target"})).await;
    assert_eq!(read["isError"], false, "{read}");
    assert!(read.to_string().contains("retained peer history"), "{read}");
    for id in ["secret", "absent"] {
        let read = w.tool(&token, "session_read", json!({"session_id":id})).await;
        assert_eq!(read["structuredContent"]["code"], "not_found");
    }
    let denied = w
        .tool(
            &token,
            "session_close",
            json!({"child_id":"target","operation_key":"no"}),
        )
        .await;
    assert_eq!(denied["isError"], true);
}

#[tokio::test]
async fn policy_off_on_does_not_revive_old_peer_authority() {
    let w = World::new();
    let token = w.start("reader", "host", &w.hat, true);
    assert_eq!(w.tool(&token, "session_list", json!({})).await["isError"], false);
    let identity = w.gateway.runtime.store.authenticate_session(&token).unwrap().unwrap();
    let (watch, _) = w.gateway.runtime.store.watch_peers(&identity).unwrap().unwrap();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::Off)
        .unwrap();
    assert!(watch.token().is_cancelled());
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    assert_eq!(w.rpc(&token, "tools/list", json!({})).await.0, StatusCode::NOT_FOUND);
    assert!(w.gateway.runtime.store.watch_peers(&identity).unwrap().is_none());
}

#[tokio::test]
async fn peer_server_can_initialize_while_session_is_starting() {
    let w = World::new();
    let token = w.start("reader", "host", &w.hat, false);
    let (status, body) = w
        .rpc(
            &token,
            "initialize",
            json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        w.tool(&token, "session_list", json!({})).await["structuredContent"]["code"],
        "session_starting"
    );
}

#[tokio::test]
async fn peer_pages_are_bound_to_the_reader_and_large_items_are_reconstructible() {
    let w = World::new();
    let token = w.start("reader", "host", &w.hat, true);
    let other = w.start("reader2", "host2", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let first = w.tool(&token, "session_list", json!({"limit":1})).await;
    let cursor = first["structuredContent"]["next_cursor"].as_str().unwrap();
    assert_eq!(
        w.tool(&other, "session_list", json!({"limit":1,"cursor":cursor})).await["isError"],
        true
    );
    let second = w.tool(&token, "session_list", json!({"limit":1,"cursor":cursor})).await;
    assert_ne!(
        first["structuredContent"]["sessions"][0]["session_id"],
        second["structuredContent"]["sessions"][0]["session_id"]
    );
    let text = "peer-history-large-".repeat(1800);
    assert!(
        w.app
            .store
            .open_turn("target", "large", &[json!({"type":"text","text":text})])
            .unwrap()
    );
    w.app
        .store
        .ingest(
            "target",
            2,
            &SessionBody::TurnStarted {
                request_id: "request".into(),
                turn_id: "large".into(),
            },
        )
        .unwrap();
    let page = w.tool(&token, "session_read", json!({"session_id":"target"})).await;
    let item = page["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "user_turn")
        .unwrap();
    let mut offset = 0;
    let mut raw = String::new();
    loop {
        let chunk = w
            .tool(
                &token,
                "session_read",
                json!({"session_id":"target","item_id":item["id"],"version":item["version"],"offset":offset}),
            )
            .await;
        assert_eq!(chunk["isError"], false, "{chunk}");
        raw.push_str(chunk["structuredContent"]["item_json"].as_str().unwrap());
        if let Some(next) = chunk["structuredContent"]["next_offset"].as_u64() {
            offset = next;
        } else {
            break;
        }
    }
    let complete: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(complete["content"][0]["text"], text);
}

#[tokio::test]
async fn peer_messages_use_one_durable_receipt_and_real_peer_origin_on_another_host() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("recipient", "host2", &w.hat, true);
    let args = json!({"session_id":"recipient","request_key":"message-one","text":"Please inspect the shared change."});
    let first = w.tool(&token, "session_send", args.clone()).await;
    assert_eq!(first["isError"], false, "{first}");
    let repeat = w.tool(&token, "session_send", args.clone()).await;
    assert_eq!(
        first["structuredContent"]["operation_id"],
        repeat["structuredContent"]["operation_id"]
    );
    let conflict = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"recipient","request_key":"message-one","text":"Different message"}),
        )
        .await;
    assert_eq!(conflict["structuredContent"]["code"], "request_key_conflict");
    assert!(
        w.app
            .store
            .dispatch_operator_prompt("recipient", &w.app.hub, &mut None, None)
            .unwrap()
    );
    let CollectorFrame::Prompt { turn_id, content, .. } = w._receivers[1].try_recv().unwrap() else {
        panic!("expected prompt")
    };
    assert!(
        serde_json::to_string(&content)
            .unwrap()
            .contains("Message from peer session sender")
    );
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT input_origin FROM turns WHERE owner_id=?1 AND turn_id=?2",
            rusqlite::params![w.app.store.owner_id(), turn_id],
            |r| r.get(0),
        )
        .unwrap();
    let origin: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(origin["kind"], "peer");
    assert_eq!(origin["session_id"], "sender");
    assert_eq!(origin["operation_id"], first["structuredContent"]["operation_id"]);
    assert!(w._receivers[1].try_recv().is_err());
    let status = w
        .tool(&token, "message_status", json!({"request_key":"message-one"}))
        .await;
    assert_eq!(status["structuredContent"]["turn_id"], turn_id);
    let rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM peer_messages WHERE owner_id=?1",
            [w.app.store.owner_id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn read_access_cannot_send_and_same_hat_messages_do_not_cross_the_hat_boundary() {
    let w = World::new();
    let read = w.start("reader", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let args = json!({"session_id":"target","request_key":"read-only","text":"hello"});
    assert_eq!(
        w.tool(&read, "session_send", args).await["structuredContent"]["code"],
        "not_found"
    );
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("foreign", "host2", &w.other, true);
    for target in ["foreign", "absent"] {
        let result = w
            .tool(
                &token,
                "session_send",
                json!({"session_id":target,"request_key":target,"text":"hello"}),
            )
            .await;
        assert_eq!(result["structuredContent"]["code"], "not_found");
    }
    let forged = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"forged","text":"approval","origin":{"kind":"operator"}}),
        )
        .await;
    assert_eq!(forged["structuredContent"]["code"], "invalid_arguments");
}

#[tokio::test]
async fn queued_peer_message_is_invalidated_before_delivery_when_policy_changes() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let sent = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"queued","text":"queued"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::Read)
        .unwrap();
    assert!(
        !w.app
            .store
            .dispatch_operator_prompt("target", &w.app.hub, &mut None, None)
            .unwrap()
    );
    assert!(w._receivers[1].try_recv().is_err());
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    let code: String = conn
        .query_row(
            "SELECT code FROM session_operations WHERE owner_id=?1 AND operation_id=?2",
            rusqlite::params![
                w.app.store.owner_id(),
                sent["structuredContent"]["operation_id"].as_str().unwrap()
            ],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(code, "peer_access_changed");
}

#[tokio::test]
async fn peer_message_budget_is_per_sender_and_retries_do_not_consume_it() {
    let w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    for n in 0..32 {
        let result = w
            .tool(
                &token,
                "session_send",
                json!({"session_id":"target","request_key":format!("message-{n}"),"text":"data"}),
            )
            .await;
        assert_eq!(result["isError"], false, "{result}");
    }
    let repeat = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"message-0","text":"data"}),
        )
        .await;
    assert_eq!(repeat["isError"], false);
    let full = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"over-budget","text":"data"}),
        )
        .await;
    assert_eq!(full["structuredContent"]["code"], "peer_message_budget");
}

#[tokio::test]
async fn running_peer_receives_automatic_steering_with_peer_provenance() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    assert!(
        w.app
            .store
            .open_turn("target", "busy", &[json!({"type":"text","text":"work"})])
            .unwrap()
    );
    w.app
        .store
        .ingest(
            "target",
            2,
            &SessionBody::TurnStarted {
                request_id: "busy-request".into(),
                turn_id: "busy".into(),
            },
        )
        .unwrap();
    let args = json!({"session_id":"target","request_key":"steer","text":"Peer information"});
    let sent = w.tool(&token, "session_send", args.clone()).await;
    assert_eq!(sent["isError"], false, "{sent}");
    let CollectorFrame::Steer {
        request_id, turn_id, ..
    } = w._receivers[1].try_recv().unwrap()
    else {
        panic!("expected steering")
    };
    assert_eq!(turn_id, "busy");
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT body FROM events WHERE session_id='target' AND kind='steering_attempt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let attempt: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(attempt["origin"]["kind"], "peer");
    assert_eq!(attempt["origin"]["session_id"], "sender");
    assert_eq!(attempt["automatic"], true);
    w.app
        .store
        .ingest(
            "target",
            3,
            &SessionBody::SteeringResult {
                request_id,
                outcome: hennery_proto::frames::SteeringOutcome::Injected,
            },
        )
        .unwrap();
    let status = w.tool(&token, "message_status", json!({"request_key":"steer"})).await;
    assert_eq!(status["structuredContent"]["state"], "delivered", "{status}");
    let repeat = w.tool(&token, "session_send", args).await;
    assert_eq!(
        repeat["structuredContent"]["operation_id"],
        sent["structuredContent"]["operation_id"]
    );
    assert!(w._receivers[1].try_recv().is_err());
}

#[tokio::test]
async fn replacement_token_cannot_inherit_the_previous_peer_grant() {
    let w = World::new();
    let token = w.start("reader", "host", &w.hat, true);
    let mut conn = hennery_kernel::db::open(&w.db).unwrap();
    let tx = conn.transaction().unwrap();
    let replacement =
        hennery_gateway::tokens::mint_in(&tx, w.app.store.owner_id(), "reader", "host", &w.hat, unix_now()).unwrap();
    tx.commit().unwrap();
    assert_eq!(w.rpc(&token, "tools/list", json!({})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        w.rpc(replacement.expose(), "tools/list", json!({})).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn moved_recipient_invalidates_queued_peer_intent() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let result = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"move","text":"message"}),
        )
        .await;
    assert_eq!(result["isError"], false, "{result}");
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    conn.execute("UPDATE sessions SET hat_id=?1 WHERE id='target'", [&w.other])
        .unwrap();
    assert!(
        !w.app
            .store
            .dispatch_operator_prompt("target", &w.app.hub, &mut None, None)
            .unwrap()
    );
    assert!(w._receivers[1].try_recv().is_err());
    assert_eq!(
        w.tool(&token, "session_read", json!({"session_id":"target"})).await["structuredContent"]["code"],
        "not_found"
    );
}

#[tokio::test]
async fn peer_input_cannot_release_an_operator_queue_hold() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    conn.execute("UPDATE sessions SET prompt_queue_held=1 WHERE id='target'", [])
        .unwrap();
    let sent = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"held","text":"information"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    let held: i64 = conn
        .query_row("SELECT prompt_queue_held FROM sessions WHERE id='target'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(held, 1);
    assert!(
        !w.app
            .store
            .dispatch_operator_prompt("target", &w.app.hub, &mut None, None)
            .unwrap()
    );
    assert!(w._receivers[1].try_recv().is_err());
}

#[tokio::test]
async fn revocation_is_rechecked_in_the_resume_transaction_after_discovery() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    w.app
        .store
        .ingest(
            "target",
            2,
            &SessionBody::SessionParked {
                reason: hennery_proto::frames::ParkReason::Idle,
            },
        )
        .unwrap();
    let sent = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"resume","text":"message"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    // Model policy changing while catalog/path discovery was awaiting I/O.
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::Read)
        .unwrap();
    let catalog = serde_json::from_value(json!({"agent":"claude","adapter_revision":"test","config_options":[],"defaults":{},"security":{"known":false,"axes":{}},"freshness":"fresh"})).unwrap();
    let mcp = McpContext {
        capable: true,
        isolation: McpIsolation::ClaudeStrict,
        rules_name_other_hats: false,
        source_policy: false,
        native_sources: false,
    };
    assert!(
        !w.app
            .store
            .attach_delegated("target", mcp, None, &catalog, &w.app.hub)
            .unwrap()
    );
    assert!(w._receivers[1].try_recv().is_err());
    assert_eq!(w.app.store.find_session("target").unwrap().unwrap().lifecycle, "parked");
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    let code: String = conn
        .query_row(
            "SELECT code FROM session_operations WHERE operation_id=?1",
            [sent["structuredContent"]["operation_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(code, "peer_access_changed");
}

#[tokio::test]
async fn peers_cannot_reopen_closed_recipients_or_read_another_senders_receipts() {
    let w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    let other = w.start("other-sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let sent = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"private-receipt","text":"message"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    let denied = w
        .tool(&other, "message_status", json!({"request_key":"private-receipt"}))
        .await;
    assert_eq!(denied["structuredContent"]["code"], "not_found");
    w.app.store.close_now("target").unwrap();
    let closed = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"closed","text":"reopen"}),
        )
        .await;
    assert_eq!(closed["structuredContent"]["code"], "queue_unavailable");
}

#[tokio::test]
async fn deleting_either_endpoint_scrubs_peer_receipts_and_queued_input_only_for_that_endpoint() {
    for deleted in ["sender", "target"] {
        let w = World::new();
        w.gateway
            .runtime
            .store
            .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
            .unwrap();
        let token = w.start("sender", "host", &w.hat, true);
        w.start("target", "host2", &w.hat, true);
        let kept = w.start("kept-sender", "host", &w.hat, true);
        w.start("kept-target", "host2", &w.hat, true);
        let sent = w
            .tool(
                &token,
                "session_send",
                json!({"session_id":"target","request_key":"erase","text":"private message"}),
            )
            .await;
        assert_eq!(sent["isError"], false, "{sent}");
        let retained = w
            .tool(
                &kept,
                "session_send",
                json!({"session_id":"kept-target","request_key":"keep","text":"retained message"}),
            )
            .await;
        assert_eq!(retained["isError"], false, "{retained}");
        w.app.store.close_now(deleted).unwrap();
        w.app.store.delete_session(deleted, None).unwrap();
        let conn = hennery_kernel::db::open(&w.db).unwrap();
        for table in ["peer_messages", "session_dispatch_queue", "session_operations"] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE operation_id=?1"),
                    [sent["structuredContent"]["operation_id"].as_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{table}: {deleted}");
            let kept_count: i64 = conn
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE operation_id=?1"),
                    [retained["structuredContent"]["operation_id"].as_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(kept_count, 1, "{table}: unrelated message");
        }
        assert_eq!(
            w.tool(&kept, "session_read", json!({"session_id":deleted})).await["structuredContent"]["code"],
            "not_found"
        );
    }
}

#[tokio::test]
async fn operator_edited_input_survives_deletion_of_its_original_peer_sender() {
    let mut w = World::new();
    w.gateway
        .runtime
        .store
        .set_peer_policy(&w.hat, PeerAccess::ReadWrite)
        .unwrap();
    let token = w.start("sender", "host", &w.hat, true);
    w.start("target", "host2", &w.hat, true);
    let sent = w
        .tool(
            &token,
            "session_send",
            json!({"session_id":"target","request_key":"edited","text":"peer suggestion"}),
        )
        .await;
    let id = sent["structuredContent"]["operation_id"].as_str().unwrap();
    w.app
        .store
        .edit_operator_prompt("target", id, "Operator follow-up")
        .unwrap();
    w.app.store.close_now("sender").unwrap();
    w.app.store.delete_session("sender", None).unwrap();
    assert!(
        w.app
            .store
            .dispatch_operator_prompt("target", &w.app.hub, &mut None, None)
            .unwrap()
    );
    let CollectorFrame::Prompt { turn_id, content, .. } = w._receivers[1].try_recv().unwrap() else {
        panic!("expected operator prompt")
    };
    assert!(serde_json::to_string(&content).unwrap().contains("Operator follow-up"));
    let conn = hennery_kernel::db::open(&w.db).unwrap();
    let raw: String = conn
        .query_row("SELECT input_origin FROM turns WHERE turn_id=?1", [turn_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&raw).unwrap()["kind"], "operator");
}
