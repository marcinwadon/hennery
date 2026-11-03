use axum::Router;
use hennery_gateway::api::GatewayState;
use hennery_gateway::key::MasterKey;
use hennery_gateway::scope::ProxyStore;
use hennery_gateway::session::GatewayMcp;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use hennery_proto::frames::{AgentIsolation, Capabilities, Capability, McpIsolation, McpServer};
use hennery_sessions::{
    AppState,
    store::{McpContext, Store},
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;
const ORIGIN: &str = "https://hennery.example";

#[tokio::test]
async fn invalid_answer_content_is_an_actionable_tool_result_for_the_parent() {
    let h = Harness::new().await;
    h.state
        .store
        .ingest(
            "session",
            1,
            &hennery_proto::frames::SessionBody::session_started("r", "agent"),
        )
        .unwrap();
    let response = h.rpc(json!({"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"session_answer","arguments":{"operation_key":"retryable","child_id":"child","pending_id":"question","action":"accept","content":"not JSON"}}})).await;
    assert!(
        response.get("error").is_none(),
        "tool failure must be delivered as an MCP result"
    );
    assert_eq!(response["id"], 21);
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["code"], "invalid_arguments");
    assert!(
        result["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("content must be a JSON object")
    );
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        result["structuredContent"]
    );
}

#[tokio::test]
async fn answer_objects_and_encoded_objects_share_strict_validation_and_retry_receipts() {
    use hennery_proto::frames::{Indexed, PendingExtract, PendingKind, SessionBody};
    let h = Harness::new().await;
    h.state
        .store
        .ingest("session", 1, &SessionBody::session_started("r", "parent"))
        .unwrap();
    h.state
        .store
        .create_session_with_mcp(
            "child",
            "host",
            "claude",
            "/project",
            &h.hat,
            None,
            McpContext {
                source_policy: false,
                native_sources: false,
                capable: true,
                isolation: McpIsolation::ClaudeStrict,
                rules_name_other_hats: false,
            },
        )
        .unwrap()
        .unwrap();
    let identity = h.gateway.runtime.store.authenticate_session(&h.token).unwrap().unwrap();
    let mut conn = hennery_kernel::db::open(&h._dir.path().join("hennery.db")).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(h.state.store.bind_family_in(&tx, &identity, 1, "child", 0).unwrap());
    tx.commit().unwrap();
    h.state
        .store
        .ingest("child", 1, &SessionBody::session_started("r", "child"))
        .unwrap();
    let schema = json!({"type":"object","properties":{"color":{"type":"string","enum":["blue","green"]}},"required":["color"],"additionalProperties":false});
    h.state
        .store
        .ingest(
            "child",
            2,
            &SessionBody::PendingOpened {
                pending_id: "question".into(),
                indexed: Indexed {
                    pending: Some(Box::new(PendingExtract {
                        id: "question".into(),
                        kind: PendingKind::Elicitation,
                        option_ids: None,
                        title: None,
                    })),
                    ..Default::default()
                },
                payload: json!({"mode":"form","message":"Choose","requestedSchema":schema}),
            },
        )
        .unwrap();
    let call = |content: Value, child: &str| json!({"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"session_answer","arguments":{"operation_key":"answer","child_id":child,"pending_id":"question","action":"accept","content":content}}});
    for object in [
        json!({"color":"red"}),
        json!({"color":true}),
        json!({}),
        json!({"color":"blue","extra":true}),
    ] {
        for content in [object.clone(), json!(object.to_string())] {
            let response = h.rpc(call(content, "child")).await;
            let result = &response["result"];
            assert_eq!(result["isError"], true);
            assert_eq!(result["structuredContent"]["code"], "invalid_arguments");
            assert!(
                result["structuredContent"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(&schema.to_string())
            );
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
                result["structuredContent"]
            );
        }
    }
    let foreign = h.rpc(call(json!({"color":"red"}), "foreign")).await;
    assert_eq!(foreign["result"]["structuredContent"]["code"], "not_found");
    assert!(
        !foreign.to_string().contains("additionalProperties"),
        "do not expose another child's schema"
    );
    let object = json!({"color":"blue"});
    let accepted = h.rpc(call(object.clone(), "child")).await;
    assert_eq!(accepted["result"]["isError"], false, "{accepted}");
    let retry = h.rpc(call(json!(object.to_string()), "child")).await;
    assert_eq!(accepted, retry, "the normalized object has the same durable receipt");
    assert_eq!(
        h.state.store.open_pending("child").unwrap()[0].payload["requestedSchema"],
        schema
    );
}

#[tokio::test]
async fn pinned_codex_tool_metadata_does_not_change_caller_authority() {
    let h = Harness::new().await;
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"session_spawn","arguments":{},"_meta":{"progressToken":1,"callId":"call",
            "threadId":"thread","itemId":"item","x-codex-turn-metadata":{"turn_id":"turn"}}}});
    let actual = h.rpc(call.clone()).await;
    assert_eq!(actual["result"]["structuredContent"]["code"], "session_starting");
    h.state
        .store
        .ingest(
            "session",
            1,
            &hennery_proto::frames::SessionBody::session_started("r", "agent"),
        )
        .unwrap();
    let actual = h.rpc(call.clone()).await;
    assert_eq!(actual["result"]["structuredContent"]["code"], "invalid_arguments");
    for meta in [json!([]), json!(null), json!("x"), json!({"large":"x".repeat(16*1024)})] {
        let mut wrong = call.clone();
        wrong["params"]["_meta"] = meta;
        let response = h.post(serde_json::to_vec(&wrong).unwrap()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>().await.unwrap()["error"]["code"], -32602);
    }
    // Keys decode exactly once, including escapes, and duplicate keys remain
    // refused by the transport before metadata can reach a handler.
    let bytes = serde_json::to_string(&call).unwrap();
    let escaped = bytes.replace("\"_meta\"", "\"\\u005fmeta\"");
    assert_eq!(h.post(escaped).await.status(), StatusCode::OK);
    let duplicate = bytes.replace("\"_meta\":", "\"_meta\":{},\"\\u005fmeta\":");
    assert_eq!(h.post(duplicate).await.status(), StatusCode::BAD_REQUEST);
    let wrong_case = bytes.replace("\"_meta\"", "\"_META\"");
    assert_eq!(h.post(wrong_case).await.status(), StatusCode::BAD_REQUEST);
    let mut forged = call;
    forged["params"]["caller"] = "other".into();
    assert_eq!(
        h.post(serde_json::to_vec(&forged).unwrap()).await.status(),
        StatusCode::BAD_REQUEST
    );
}
struct Harness {
    _dir: tempfile::TempDir,
    state: AppState,
    gateway: GatewayState,
    task: tokio::task::JoinHandle<()>,
    url: String,
    token: String,
    hat: String,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        Self::with_limits(Default::default()).await
    }
    async fn with_limits(limits: hennery_gateway::proxy::Limits) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "test".into(),
                    host_version: "0".into(),
                    platform: "linux".into(),
                },
                1,
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let operator = Operator::open(&db).unwrap();
        let setup = operator.issue_setup_token(unix_now()).unwrap().unwrap();
        assert!(matches!(
            operator
                .set_up(&setup, "correct horse battery", ORIGIN, unix_now())
                .unwrap(),
            SetupOutcome::Done { .. }
        ));
        hosts
            .record_hello("host", "0", &Capabilities(vec![Capability::McpServers]), unix_now())
            .unwrap();
        hosts
            .record_mcp_isolation(
                "host",
                &AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
            )
            .unwrap();
        let state = AppState::new(Store::open(&db).unwrap(), hosts, operator);
        let gateway_store = Arc::new(GatewayStore::open(&db).unwrap());
        let runtime = Arc::new(hennery_gateway::runtime::Runtime::new(
            gateway_store.clone(),
            Arc::new(ProxyStore::open(&db).unwrap()),
            Arc::new(MasterKey::from_bytes([7; 32])),
            hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap(),
            Arc::new(hennery_gateway::notify::Silent),
        ));
        let gateway = GatewayState {
            runtime,
            operator: state.operator.clone(),
        };
        state
            .store
            .set_session_mcp(Arc::new(GatewayMcp::for_collector(&gateway)));
        gateway_store.set_session_tools(&hat, true).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let registered = state
            .hub
            .register(
                "host",
                tx,
                Capabilities(vec![Capability::McpServers]),
                AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
            )
            .unwrap();
        state.hub.mark_ready("host", registered.conn_id);
        let given = state
            .store
            .create_session_with_mcp(
                "session",
                "host",
                "claude",
                "/project",
                &hat,
                None,
                McpContext {
                    source_policy: false,
                    native_sources: false,
                    capable: true,
                    isolation: McpIsolation::ClaudeStrict,
                    rules_name_other_hats: false,
                },
            )
            .unwrap()
            .unwrap();
        let McpServer::Http { headers, .. } = &given.servers[0] else {
            panic!()
        };
        let token = headers[0].value.strip_prefix("Bearer ").unwrap().into();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp/builtin/hennery", listener.local_addr().unwrap());
        let proxy = hennery_gateway::proxy::ProxyState::for_sessions(gateway.runtime.clone(), Default::default());
        let app: Router = hennery_sessions::router(state.clone())
            .merge(hennery_sessions::builtin::router_with_limits(
                state.clone(),
                gateway.clone(),
                limits,
            ))
            .merge(hennery_gateway::proxy::router(proxy));
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            _dir: dir,
            state,
            gateway,
            task,
            url,
            token,
            hat,
        }
    }
    async fn post(&self, body: impl Into<Vec<u8>>) -> reqwest::Response {
        Client::new()
            .post(&self.url)
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body.into())
            .send()
            .await
            .unwrap()
    }
    async fn rpc(&self, message: Value) -> Value {
        let response = self.post(serde_json::to_vec(&message).unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(!response.headers().contains_key("mcp-session-id"));
        assert!(!response.headers().contains_key("access-control-allow-origin"));
        response.json().await.unwrap()
    }
}
#[tokio::test]
async fn starting_discovery_stateless_protocol_and_seven_control_tools() {
    let h = Harness::new().await;
    for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
        let response = h.rpc(json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture", "version":"1"}}})).await;
        assert_eq!(response["result"]["protocolVersion"], version);
        assert_eq!(response["result"]["capabilities"], json!({"tools":{}}));
    }
    let initialized = h
        .post(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_vec())
        .await;
    assert_eq!(initialized.status(), StatusCode::ACCEPTED);
    assert!(initialized.bytes().await.unwrap().is_empty());
    let discovery = h.rpc(json!({"jsonrpc":"2.0","id":"list","method":"tools/list"})).await;
    let names: std::collections::BTreeSet<_> = discovery["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "session_capabilities",
            "session_spawn",
            "session_send",
            "session_status",
            "session_list",
            "session_close",
            "session_answer"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        h.rpc(json!({"jsonrpc":"2.0","id":2,"method":"ping"})).await["result"],
        json!({})
    );
    let call = h
        .rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/call", "params":{"name":"session_spawn","arguments":{}}}))
        .await;
    assert_eq!(call["result"]["isError"], true);
    assert_eq!(call["result"]["structuredContent"]["code"], "session_starting");
    h.state
        .store
        .ingest(
            "session",
            1,
            &hennery_proto::frames::SessionBody::session_started("r", "agent"),
        )
        .unwrap();
    let call = h
        .rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/call", "params":{"name":"session_spawn","arguments":{}}}))
        .await;
    assert_eq!(call["result"]["structuredContent"]["code"], "invalid_arguments");
    assert_eq!(
        serde_json::from_str::<Value>(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap(),
        call["result"]["structuredContent"]
    );
    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        assert_eq!(
            Client::new()
                .request(method, &h.url)
                .bearer_auth(&h.token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}
#[tokio::test]
async fn authority_and_origin_are_header_only_and_uniformly_refused() {
    let h = Harness::new().await;
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    for auth in [
        None,
        Some("Bearer bad"),
        Some("Bearer hnry_client_bad"),
        Some("Basic xxx"),
    ] {
        let mut req = Client::new()
            .post(&h.url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(auth) = auth {
            req = req.header("authorization", auth);
        }
        assert_eq!(req.send().await.unwrap().status(), StatusCode::NOT_FOUND);
    }
    let repeated = Client::new()
        .post(&h.url)
        .header("authorization", format!("Bearer {}", h.token))
        .header("authorization", format!("Bearer {}", h.token))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        Client::new()
            .post(&h.url)
            .header("cookie", format!("__Host-hennery_session={}", h.token))
            .body(body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    for origin in [
        "https://foreign.example",
        "null",
        "https://hennery.example/path",
        "garbage",
    ] {
        assert_eq!(
            Client::new()
                .post(&h.url)
                .bearer_auth(&h.token)
                .header("origin", origin)
                .body(body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        Client::new()
            .post(&h.url)
            .bearer_auth(&h.token)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        Client::new()
            .post(format!("{}?caller=session", h.url))
            .bearer_auth(&h.token)
            .body(body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = h.post(body.as_bytes().to_vec()).await.text().await.unwrap();
    assert!(!response.contains(&h.token));
    h.gateway.runtime.store.set_session_tools(&h.hat, false).unwrap();
    assert_eq!(h.post(body.as_bytes().to_vec()).await.status(), StatusCode::NOT_FOUND);
    h.gateway.runtime.store.set_session_tools(&h.hat, true).unwrap();
    assert_eq!(h.post(body.as_bytes().to_vec()).await.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn strict_dispatch_has_deterministic_differential_refusals_and_positive_escapes() {
    let h = Harness::new().await;
    for body in [
        r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"ping","method":"tools/call"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"ping","\u006dethod":"tools/call"}"#,
        r#"{"jsonrpc":"2.0","id":1,"Method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_spawn","arguments":{"owner_id":"foreign"},"caller":"spoof"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_spawn","arguments":[]}}"#,
        r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
        "\u{feff}{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}",
    ] {
        let response = h.post(body.as_bytes().to_vec()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        let answer: Value = response.json().await.unwrap();
        assert!(answer["error"]["code"].as_i64().is_some());
    }
    assert_eq!(h.post(vec![0xff, 0xfe]).await.status(), StatusCode::BAD_REQUEST);
    for body in [
        r#"{ "jsonrpc" : "2.0", "id" : 1, "\u006dethod" : "ping" }"#,
        "\r\n\t{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\t",
    ] {
        assert_eq!(h.post(body.as_bytes().to_vec()).await.status(), StatusCode::OK);
    }
    assert_eq!(
        Client::new()
            .post(&h.url)
            .bearer_auth(&h.token)
            .header("mcp-protocol-version", "unknown")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let unknown = h.rpc(json!({"jsonrpc":"2.0","id":1,"method":"resources/list"})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(
        Client::new()
            .get(h.url.replace("/mcp/builtin/hennery", "/mcp/builtin/other"))
            .bearer_auth(&h.token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn race_disable_or_revoke_cuts_body_read_and_keeps_external_identity() {
    for revoke in [false, true] {
        let h = Harness::new().await;
        let (send, receive) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(2);
        send.send(Ok(axum::body::Bytes::from_static(b"{"))).await.unwrap();
        let stream = futures::stream::unfold(receive, |mut receive| async move {
            receive.recv().await.map(|chunk| (chunk, receive))
        });
        let client = Client::new();
        let request = client
            .post(&h.url)
            .bearer_auth(&h.token)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(reqwest::Body::wrap_stream(stream))
            .send();
        tokio::pin!(request);
        let wait = async {
            while h.gateway.runtime.revocations.watched() == 0 {
                tokio::task::yield_now().await;
            }
        };
        tokio::select! { result = &mut request => panic!("request ended before cut: {result:?}"), result = tokio::time::timeout(std::time::Duration::from_secs(5), wait) => result.unwrap() }
        if revoke {
            use hennery_gateway::session::SessionMcp;
            let mcp = GatewayMcp::for_collector(&h.gateway);
            let mut conn = hennery_kernel::db::open(&h._dir.path().join("hennery.db")).unwrap();
            let tx = conn.transaction().unwrap();
            let cut = mcp.revoke_in(&tx, "session").unwrap();
            tx.commit().unwrap();
            mcp.cut(cut);
        } else {
            h.gateway.runtime.store.set_session_tools(&h.hat, false).unwrap();
        }
        drop(send);
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response.text().await.unwrap().contains(&h.token));
        assert_eq!(
            h.gateway
                .runtime
                .store
                .authenticate_session(&h.token)
                .unwrap()
                .is_some(),
            !revoke
        );
    }
}

#[tokio::test]
async fn response_ids_are_redacted_after_assembly() {
    let h = Harness::new().await;
    h.state
        .store
        .ingest(
            "session",
            1,
            &hennery_proto::frames::SessionBody::session_started("r", "agent"),
        )
        .unwrap();
    let id = h.token.clone();
    let result = h.rpc(json!({"jsonrpc":"2.0","id":id,"method":"ping"})).await;
    assert!(!result.to_string().contains(&h.token));
}

#[tokio::test]
async fn repeated_origin_null_notification_and_nested_duplicates_are_refused() {
    let h = Harness::new().await;
    let response = Client::new()
        .post(&h.url)
        .bearer_auth(&h.token)
        .header("origin", ORIGIN)
        .header("origin", ORIGIN)
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for raw in [
        r#"{"jsonrpc":"2.0","id":null,"method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":{"x":1,"\u0078":2}}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":[],"clientInfo":{"name":"x","version":"1"}}}"#,
    ] {
        assert_eq!(
            h.post(raw.as_bytes().to_vec()).await.status(),
            StatusCode::BAD_REQUEST,
            "{raw}"
        );
    }
}

#[tokio::test]
async fn policy_management_requires_browser_auth_origin_and_fresh_step_up() {
    let h = Harness::new().await;
    let endpoint = h
        .url
        .replace("/mcp/builtin/hennery", &format!("/api/hats/{}/session-tools", h.hat));
    let phc = h
        .state
        .operator
        .verify_password("correct horse battery")
        .unwrap()
        .unwrap();
    let fresh = h
        .state
        .operator
        .open_session("test", &phc, unix_now())
        .unwrap()
        .unwrap();
    let stale = h
        .state
        .operator
        .open_session("test", &phc, unix_now() - 600)
        .unwrap()
        .unwrap();
    let client = Client::new();
    for (session, origin, status) in [
        (Some(stale.as_str()), Some(ORIGIN), StatusCode::FORBIDDEN),
        (None, Some(ORIGIN), StatusCode::UNAUTHORIZED),
        (Some(fresh.as_str()), None, StatusCode::FORBIDDEN),
        (
            Some(fresh.as_str()),
            Some("https://foreign.example"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = client.patch(&endpoint).json(&json!({"delegation_enabled":false}));
        if let Some(session) = session {
            request = request.header("cookie", format!("__Host-hennery_session={session}"));
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(request.send().await.unwrap().status(), status);
    }
    let get = client
        .get(&endpoint)
        .header("cookie", format!("__Host-hennery_session={stale}"))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        get.json::<Value>().await.unwrap(),
        json!({"delegation_enabled":true,"revision":1})
    );
    for body in [
        r#"{"delegation_enabled":false,"delegation_enabled":true}"#,
        r#"{"delegation_enabled":false,"owner_id":"foreign"}"#,
    ] {
        assert!(
            !client
                .patch(&endpoint)
                .header("cookie", format!("__Host-hennery_session={fresh}"))
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    let changed = client
        .patch(&endpoint)
        .header("cookie", format!("__Host-hennery_session={fresh}"))
        .header("origin", ORIGIN)
        .json(&json!({"delegation_enabled":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(
        changed.json::<Value>().await.unwrap(),
        json!({"delegation_enabled":false,"revision":2})
    );
    assert_eq!(
        h.post(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn body_caps_deadlines_content_types_and_request_permits_are_bounded() {
    use hennery_gateway::proxy::Limits;
    use std::time::Duration;
    let h = Harness::with_limits(Limits::new(64, 32, Duration::from_secs(1), Duration::from_millis(50))).await;
    assert_eq!(
        h.post(vec![b' '; hennery_gateway::proxy::MAX_REQUEST_BODY + 1])
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let (send, receive) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(1);
    let stream = futures::stream::unfold(receive, |mut receive| async move {
        receive.recv().await.map(|v| (v, receive))
    });
    let stalled = Client::new()
        .post(&h.url)
        .bearer_auth(&h.token)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(reqwest::Body::wrap_stream(stream))
        .send();
    tokio::pin!(stalled);
    let response = tokio::time::timeout(Duration::from_secs(2), &mut stalled)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    drop(send);
    for (content, accept, status) in [
        ("text/plain", "application/json", StatusCode::UNSUPPORTED_MEDIA_TYPE),
        ("application/json", "text/event-stream", StatusCode::NOT_ACCEPTABLE),
    ] {
        assert_eq!(
            Client::new()
                .post(&h.url)
                .bearer_auth(&h.token)
                .header("content-type", content)
                .header("accept", accept)
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            status
        );
    }
    let busy = Harness::with_limits(Limits::new(0, 32, Duration::from_secs(1), Duration::from_secs(1))).await;
    assert_eq!(
        busy.post(b"{}".to_vec()).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        h.post(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec())
            .await
            .status(),
        StatusCode::OK
    );
}

#[path = "../../hennery-gateway/tests/support/differential.rs"]
mod differential;
#[tokio::test]
async fn differential_decoders_cannot_change_the_builtin_dispatch() {
    use differential::{DECODERS, Node};
    let h = Harness::new().await;
    for (raw, accepted) in [
        (r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, true),
        (r#"{ "jsonrpc" : "2.0", "id":1, "\u006dethod" : "ping" }"#, true),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","method":"tools/list"}"#,
            false,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","Method":"tools/list"}"#,
            false,
        ),
        (r#"{"jsonrpc":"2.0","id":1,"METHOD":"ping"}"#, false),
        (r#"{"jsonrpc":"2.0","id":1,"me_thod":"ping"}"#, false),
        (r#"{"jsonrpc":"2.0","id":1,"method\u0000x":"ping"}"#, false),
    ] {
        let tree: Node = serde_json::from_slice(raw.as_bytes()).unwrap();
        let decoded: Vec<_> = DECODERS.iter().map(|d| d.str(d.get(&tree, "method"))).collect();
        let response = h.post(raw.as_bytes().to_vec()).await;
        if accepted {
            assert!(
                decoded.iter().all(|m| m.as_deref() == Some("ping")),
                "{raw}: {decoded:?}"
            );
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.json::<Value>().await.unwrap()["result"], json!({}));
        } else {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{raw}: {decoded:?}");
        }
    }
}

#[tokio::test]
async fn exact_builtin_route_preserves_external_builtin_slug_and_rejects_deeper_paths() {
    let h = Harness::new().await;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}/mcp", upstream.local_addr().unwrap());
    let app = Router::new().route(
        "/mcp",
        axum::routing::post(|| async { axum::Json(json!({"jsonrpc":"2.0","id":1,"result":{"external":true}})) }),
    );
    let upstream_task = tokio::spawn(async move { axum::serve(upstream, app).await.unwrap() });
    let new = hennery_gateway::model::NewConnection {
        slug: "builtin".into(),
        label: "external".into(),
        url: upstream_url,
        hat_id: h.hat.clone(),
        cred_kind: hennery_gateway::model::CredKind::None,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: true,
    };
    let hennery_gateway::model::Change::Done(connection) = h.gateway.runtime.store.create(&new, unix_now()).unwrap()
    else {
        panic!()
    };
    h.gateway
        .runtime
        .store
        .replace_mounts(&connection.id, &["host".into()])
        .unwrap();
    let external = h.url.replace("/mcp/builtin/hennery", "/mcp/builtin");
    for disabled in [false, true] {
        if disabled {
            h.gateway.runtime.store.set_session_tools(&h.hat, false).unwrap();
        }
        let response = Client::new()
            .post(&external)
            .bearer_auth(&h.token)
            .header("content-type", "application/json")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap()["result"]["external"], true);
        assert_eq!(
            h.post(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec())
                .await
                .status(),
            if disabled {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            }
        );
    }
    for path in ["/mcp/builtin/hennery/other", "/mcp/builtin/other", "/mcp/other/deeper"] {
        assert_eq!(
            Client::new()
                .post(h.url.replace("/mcp/builtin/hennery", path))
                .bearer_auth(&h.token)
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    upstream_task.abort();
}

#[tokio::test]
async fn current_host_capability_and_isolation_are_rechecked_after_reconnect() {
    let h = Harness::new().await;
    for (capable, isolation, status) in [
        (false, McpIsolation::ClaudeStrict, StatusCode::NOT_FOUND),
        (true, McpIsolation::None, StatusCode::NOT_FOUND),
        (true, McpIsolation::ClaudeStrict, StatusCode::OK),
    ] {
        if let Some(connection) = h.state.hub.routable_conn("host") {
            h.state.hub.unregister("host", connection);
        }
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let registration = h
            .state
            .hub
            .register(
                "host",
                tx,
                Capabilities(if capable { vec![Capability::McpServers] } else { vec![] }),
                AgentIsolation([("claude".into(), isolation)].into_iter().collect()),
            )
            .unwrap();
        h.state.hub.mark_ready("host", registration.conn_id);
        assert_eq!(
            h.post(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec())
                .await
                .status(),
            status
        );
    }
}

#[tokio::test]
async fn every_protocol_error_verdict_is_positive_and_never_echoes_the_token() {
    let h = Harness::new().await;
    for (raw, code, status) in [
        ("not-json", -32700, StatusCode::BAD_REQUEST),
        (r#"[]"#, -32600, StatusCode::BAD_REQUEST),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":[]}}"#,
            -32602,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
            -32601,
            StatusCode::OK,
        ),
    ] {
        let response = h.post(raw.as_bytes().to_vec()).await;
        assert_eq!(response.status(), status);
        assert_eq!(response.json::<Value>().await.unwrap()["error"]["code"], code);
    }
    let answer = h
        .rpc(json!({"jsonrpc":"2.0","id":h.token,"method":"resources/list"}))
        .await;
    assert!(!answer.to_string().contains(&h.token));
}

#[tokio::test]
async fn capability_guard_is_independent_of_isolation_when_old_delivery_was_unisolated() {
    let h = Harness::new().await;
    let conn = hennery_kernel::db::open(&h._dir.path().join("hennery.db")).unwrap();
    conn.execute(
        "UPDATE sessions SET mcp_delivery_mode = 'unisolated' WHERE id = 'session'",
        [],
    )
    .unwrap();
    if let Some(id) = h.state.hub.routable_conn("host") {
        h.state.hub.unregister("host", id);
    }
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let reg = h
        .state
        .hub
        .register("host", tx, Capabilities(vec![]), AgentIsolation::default())
        .unwrap();
    h.state.hub.mark_ready("host", reg.conn_id);
    assert_eq!(
        h.post(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn pinned_claude_implementation_metadata_uses_standard_optional_fields() {
    let h = Harness::new().await;
    let result=h.rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claude-code","title":"Claude Code","version":"fixture","description":"Pinned SDK metadata","websiteUrl":"https://example.com"}}})).await;
    assert_eq!(result["result"]["protocolVersion"], "2025-11-25");
}

#[tokio::test]
async fn origin_scope_and_stateless_method_refusals_precede_the_busy_limit() {
    use std::time::Duration;
    let h = Harness::with_limits(hennery_gateway::proxy::Limits::new(
        0,
        32,
        Duration::from_secs(1),
        Duration::from_secs(1),
    ))
    .await;
    assert_eq!(
        Client::new()
            .post(&h.url)
            .header("origin", "https://foreign.example")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        Client::new().post(&h.url).body("{}").send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        Client::new()
            .get(&h.url)
            .bearer_auth(&h.token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(h.post(b"{}".to_vec()).await.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn positional_json_arrays_cannot_become_messages_or_typed_parameter_objects() {
    let h = Harness::new().await;
    for raw in [
        r#"["2.0",1,"ping",null]"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":["x",{}]}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":["2025-03-26",{},{"name":"x","version":"1"}]}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":["x","1"]}}"#,
    ] {
        assert_eq!(
            h.post(raw.as_bytes().to_vec()).await.status(),
            StatusCode::BAD_REQUEST,
            "{raw}"
        );
    }
}

#[tokio::test]
async fn policy_parse_refusals_do_not_echo_first_party_tokens() {
    let h = Harness::new().await;
    let phc = h
        .state
        .operator
        .verify_password("correct horse battery")
        .unwrap()
        .unwrap();
    let session = h
        .state
        .operator
        .open_session("test", &phc, unix_now())
        .unwrap()
        .unwrap();
    let endpoint = h
        .url
        .replace("/mcp/builtin/hennery", &format!("/api/hats/{}/session-tools", h.hat));
    let raw = format!("{{\"{}\":true}}", h.token);
    let response = Client::new()
        .patch(endpoint)
        .header("cookie", format!("__Host-hennery_session={session}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
    assert!(!response.text().await.unwrap().contains(&h.token));
}
#[tokio::test]
async fn standard_client_icons_are_typed_data_and_positional_icons_are_refused() {
    let h = Harness::new().await;
    for theme in ["light", "dark"] {
        let response=h.rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1","icons":[{"src":"https://example.com/icon.png","mimeType":"image/png","sizes":["16x16"],"theme":theme}]}}})).await;
        assert!(response.get("result").is_some());
    }
    let bad = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1","icons":[["https://example.com/icon.png"]]}}});
    assert_eq!(
        h.post(serde_json::to_vec(&bad).unwrap()).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn tool_deadline_bounds_an_unanswered_host_probe_and_reclaims_request_permit() {
    use hennery_proto::frames::CollectorFrame;
    use std::time::Duration;
    let h = Harness::with_limits(
        hennery_gateway::proxy::Limits::new(1, 32, Duration::from_secs(1), Duration::from_millis(50))
            .with_answer_timeout(Duration::from_millis(50)),
    )
    .await;
    let capabilities = Capabilities(vec![Capability::McpServers, Capability::ResolvePath]);
    h.state
        .hosts
        .record_hello("host", "fixture", &capabilities, unix_now())
        .unwrap();
    let previous = h.state.hub.routable_conn("host").unwrap();
    h.state.hub.unregister("host", previous);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registered = h
        .state
        .hub
        .register(
            "host",
            tx,
            capabilities,
            AgentIsolation([("claude".into(), McpIsolation::ClaudeStrict)].into_iter().collect()),
        )
        .unwrap();
    h.state.hub.mark_ready("host", registered.conn_id);
    h.state
        .store
        .ingest(
            "session",
            1,
            &hennery_proto::frames::SessionBody::session_started("start", "adapter"),
        )
        .unwrap();
    let (probe,call)=tokio::join!(
        tokio::time::timeout(Duration::from_secs(2),rx.recv()),
        tokio::time::timeout(Duration::from_secs(2),h.rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_capabilities","arguments":{}}})))
    );
    assert!(matches!(probe.unwrap().unwrap(), CollectorFrame::ResolvePath { .. }));
    let call = call.expect("tool deadline must bound a stalled host probe");
    assert_eq!(call["result"]["isError"], true);
    assert_eq!(call["result"]["structuredContent"]["code"], "request_timeout");
    let ping = h.rpc(json!({"jsonrpc":"2.0","id":2,"method":"ping"})).await;
    assert_eq!(ping["result"], json!({}));
}
