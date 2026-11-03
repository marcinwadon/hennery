//! External MCP orchestrates a real Hennery host and isolated fake ACP adapter.
use hennery_host::{AgentCommand, HostConfig, identity::HostKey};
use hennery_kernel::{
    hosts::{Enrollment, Hosts},
    operator::Operator,
};
use hennery_sessions::{
    AppState,
    external_clients::{NewClient, RootGrant},
    store::Store,
};
use hennery_testkit::{FakeAsk, FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::sync::oneshot;

struct Harness {
    dir: tempfile::TempDir,
    state: AppState,
    url: String,
    root: String,
    token: String,
    listener: std::net::TcpListener,
    server: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    host: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    stop: Option<oneshot::Sender<()>>,
    http: reqwest::Client,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
impl Harness {
    async fn new(script: FakeScript) -> Self {
        hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery-fake-acp"));
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("collector.db");
        let root = dir.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let hosts = Hosts::open(&db).unwrap();
        let key = HostKey::from_seed([77; 32]);
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: key.public_key_hex(),
                    name: "Isolated host".into(),
                    platform: "test".into(),
                    host_version: "test".into(),
                },
                0,
            )
            .unwrap();
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        let _operator = hennery_testkit::operator_client(&state.operator);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        // Reserve the address across test-only collector restarts. Otherwise
        // parallel clients can claim the released port for an ephemeral socket
        // between shutdown and bind. Collector state and host connections are
        // still recreated against the durable database.
        let listener = listener.into_std().unwrap();
        let serving = tokio::net::TcpListener::from_std(listener.try_clone().unwrap()).unwrap();
        let server = tokio::spawn(hennery_sessions::serve(serving, state.clone()));
        let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
        let mut script = script;
        script.session_log = Some(dir.path().join("agent.jsonl").to_string_lossy().into_owned());
        script.log_prompts = true;
        fake.env
            .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
        let mut cfg = HostConfig::new(
            format!("ws://{addr}/api/hosts/ws"),
            "host",
            key,
            dir.path().join("host"),
        );
        cfg.agents.insert("fake".into(), fake);
        cfg.workspace_roots = vec![PathBuf::from(&root)];
        cfg.home = Some(dir.path().join("home"));
        std::fs::create_dir(cfg.home.as_ref().unwrap()).unwrap();
        cfg.idle_timeout = Duration::ZERO;
        cfg.reconnect_min = Duration::from_millis(20);
        cfg.reconnect_max = Duration::from_millis(100);
        let (stop, stopped) = oneshot::channel();
        let host = tokio::spawn(hennery_host::run_until(cfg, async {
            let _ = stopped.await;
        }));
        let mut h = Self {
            dir,
            state,
            url,
            root,
            token: String::new(),
            listener,
            server: Some(server),
            host: Some(host),
            stop: Some(stop),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
        };
        h.until(|| h.state.hub.is_ready("host")).await;
        h.token = h.grant(false, true, true);
        h
    }
    fn grant(&self, existing: bool, control: bool, transcript: bool) -> String {
        self.state
            .store
            .issue_external_client(&NewClient {
                label: "External test".into(),
                hat_id: self.state.hosts.host("host").unwrap().unwrap().default_hat_id,
                roots: vec![RootGrant {
                    host_id: "host".into(),
                    root: self.root.clone(),
                }],
                read_metadata: true,
                existing_sessions: existing,
                read_transcript: transcript,
                control_sessions: control,
                expires_in_days: 1,
            })
            .unwrap()
            .1
    }
    async fn until(&self, condition: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(tokio::time::Instant::now() < deadline, "condition timed out");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    async fn raw(&self, token: &str, tool: &str, args: Value) -> reqwest::Response {
        self.http
            .post(format!("{}/mcp/external/hennery", self.url))
            .bearer_auth(token)
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}}))
            .send()
            .await
            .unwrap()
    }
    async fn rpc_as(&self, token: &str, tool: &str, args: Value) -> Value {
        let response = self.raw(token, tool, args).await;
        assert_eq!(response.status(), 200, "{tool}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["result"]["isError"], false, "{tool}: {body}");
        body["result"]["structuredContent"]["data"].clone()
    }
    async fn rpc(&self, tool: &str, args: Value) -> Value {
        self.rpc_as(&self.token, tool, args).await
    }
    fn create_args(&self, key: &str) -> Value {
        json!({"request_key":key,"host_id":"host","cwd":self.root,"agent":"fake"})
    }
    async fn done(&self, id: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let value = self
                .rpc("session_wait", json!({"operation_id":id,"timeout_ms":500}))
                .await;
            let op = &value["operation"];
            assert!(
                !matches!(
                    op["state"].as_str(),
                    Some("failed" | "cancelled" | "not_delivered" | "delivery_unknown")
                ),
                "operation failed: {value}"
            );
            if op["state"] == "completed" {
                return op.clone();
            }
            assert!(tokio::time::Instant::now() < deadline, "operation timed out: {value}");
        }
    }
    async fn create(&self, key: &str) -> Value {
        let op = self.rpc("session_create", self.create_args(key)).await;
        self.done(op["operation_id"].as_str().unwrap()).await
    }
    async fn action(&self, tool: &str, key: &str, session: &str) -> Value {
        let op = self.rpc(tool, json!({"request_key":key,"session_id":session})).await;
        self.done(op["operation_id"].as_str().unwrap()).await
    }
    fn log(&self, method: &str) -> usize {
        std::fs::read_to_string(self.dir.path().join("agent.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).is_ok_and(|v| v["method"] == method))
            .count()
    }
    async fn finish(mut self) {
        self.state.shutdown.cancel();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(host) = self.host.take() {
            host.await.unwrap().unwrap();
        }
        if let Some(server) = self.server.take() {
            server.await.unwrap().unwrap();
        }
    }
    async fn restart_collector(&mut self) {
        self.state.shutdown.cancel();
        self.server.take().unwrap().await.unwrap().unwrap();
        let listener = tokio::net::TcpListener::from_std(self.listener.try_clone().unwrap()).unwrap();
        let db = self.dir.path().join("collector.db");
        self.state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        self.server = Some(tokio::spawn(hennery_sessions::serve(listener, self.state.clone())));
        self.until(|| self.state.hub.is_ready("host")).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collector_restart_recovers_the_same_running_operation_without_resending() {
    let mut h = Harness::new(FakeScript {
        chunks: vec!["one".into(); 10],
        chunk_delay_ms: 150,
        ..Default::default()
    })
    .await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap().to_owned();
    let op = h
        .rpc(
            "session_prompt",
            json!({"request_key":"survive","session_id":session,"text":"Continue through reconnect"}),
        )
        .await;
    h.until(|| {
        h.state
            .store
            .turn_state(op["turn_id"].as_str().unwrap())
            .unwrap()
            .as_deref()
            == Some("started")
    })
    .await;
    h.restart_collector().await;
    let recovered = h.rpc("operation_get", json!({"request_key":"survive"})).await;
    assert_eq!(recovered["operation_id"], op["operation_id"]);
    assert_eq!(recovered["turn_id"], op["turn_id"]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let current = h.rpc("operation_get", json!({"operation_id":op["operation_id"]})).await;
        if current["state"] == "completed" {
            assert_eq!(current["outcome"], "completed");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{current}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let duplicate = h
        .rpc(
            "session_prompt",
            json!({"request_key":"survive","session_id":session,"text":"Continue through reconnect"}),
        )
        .await;
    assert_eq!(duplicate["operation_id"], op["operation_id"]);
    assert_eq!(h.log("session/new"), 1);
    assert_eq!(h.log("session/prompt"), 1);
    h.action("session_close", "close", &session).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_a_client_cancels_queued_work_and_erases_undelivered_payloads() {
    let h = Harness::new(FakeScript {
        chunks: vec!["one".into(); 6],
        chunk_delay_ms: 150,
        ..Default::default()
    })
    .await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap();
    let first = h
        .rpc(
            "session_prompt",
            json!({"request_key":"first","session_id":session,"text":"Already accepted"}),
        )
        .await;
    h.until(|| {
        h.state
            .store
            .turn_state(first["turn_id"].as_str().unwrap())
            .unwrap()
            .as_deref()
            == Some("started")
    })
    .await;
    let second = h
        .rpc(
            "session_prompt",
            json!({"request_key":"second","session_id":session,"text":"Must never be handed off"}),
        )
        .await;
    let capability = h.rpc("hennery_capabilities", json!({})).await;
    h.state
        .store
        .revoke_external_client(capability["client_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        h.raw(
            &h.token,
            "operation_get",
            json!({"operation_id":second["operation_id"]})
        )
        .await
        .status(),
        401
    );
    h.until(|| {
        h.state
            .store
            .turn_state(first["turn_id"].as_str().unwrap())
            .unwrap()
            .as_deref()
            == Some("ended")
    })
    .await;
    let conn = rusqlite::Connection::open(h.dir.path().join("collector.db")).unwrap();
    let (state, payload): (String, String) = conn
        .query_row(
            "SELECT state,args FROM external_operations WHERE id=?1",
            [second["operation_id"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "cancelled");
    assert_eq!(payload, "{}");
    assert_eq!(h.log("session/prompt"), 1);
    assert!(
        h.state
            .store
            .turn_state(second["turn_id"].as_str().unwrap())
            .unwrap()
            .is_none()
    );
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcript_pages_and_unicode_item_chunks_recover_full_long_responses() {
    let text = "🙂answer".repeat(5000);
    let h = Harness::new(FakeScript {
        chunks: vec![text.clone()],
        ..Default::default()
    })
    .await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap();
    let op = h
        .rpc(
            "session_prompt",
            json!({"request_key":"long","session_id":session,"text":"A long reply"}),
        )
        .await;
    h.done(op["operation_id"].as_str().unwrap()).await;
    let page = h.rpc("session_read", json!({"session_id":session})).await;
    assert_eq!(page["content_truncated"], true);
    let item = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["text"].is_string())
        .unwrap();
    let mut collected = String::new();
    let mut offset = 0;
    loop {
        let fragment = h
            .rpc(
                "session_read",
                json!({"session_id":session,"item_id":item["id"],"offset":offset,"version":item["version"]}),
            )
            .await;
        assert_eq!(fragment["offset"], offset);
        collected.push_str(fragment["item_json"].as_str().unwrap());
        match fragment["next_offset"].as_u64() {
            Some(next) => offset = next as usize,
            None => break,
        }
    }
    let whole: Value = serde_json::from_str(&collected).unwrap();
    assert_eq!(whole["text"], text);
    let mut ids = std::collections::BTreeSet::new();
    let mut cursor = None;
    loop {
        let mut args = json!({"session_id":session,"max_items":1});
        if let Some(c) = cursor {
            args["cursor"] = c;
        }
        let page = h.rpc("session_read", args).await;
        for i in page["items"].as_array().unwrap() {
            assert!(ids.insert(i["id"].as_str().unwrap().to_owned()));
        }
        cursor = page["next_cursor"].as_str().map(|s| json!(s));
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), page["items"].as_array().unwrap().len());
    h.action("session_close", "close", session).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_prompt_read_park_resume_and_close_are_real_scoped_operations() {
    let h = Harness::new(FakeScript::default()).await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap();
    let list = h.rpc("session_list", json!({})).await;
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(list["sessions"][0]["session_id"], session);
    let first = h
        .rpc(
            "session_prompt",
            json!({"request_key":"prompt","session_id":session,"text":"Please work"}),
        )
        .await;
    let result = h.done(first["operation_id"].as_str().unwrap()).await;
    assert_eq!(result["outcome"], "completed");
    let transcript = h.rpc("session_read", json!({"session_id":session})).await;
    assert!(transcript.to_string().contains("Hello world"), "{transcript}");
    let user = transcript["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "user_turn")
        .unwrap();
    assert_eq!(user["origin"]["kind"], "external_client");
    assert_eq!(user["origin"]["operation_id"], first["operation_id"]);
    h.action("session_park", "park", session).await;
    assert_eq!(
        h.state.store.find_session(session).unwrap().unwrap().lifecycle,
        "parked"
    );
    h.action("session_resume", "resume", session).await;
    assert_eq!(
        h.state.store.find_session(session).unwrap().unwrap().lifecycle,
        "active"
    );
    h.action("session_park", "park-again", session).await;
    let prompted = h
        .rpc(
            "session_prompt",
            json!({"request_key":"auto-resume","session_id":session,"text":"Continue"}),
        )
        .await;
    h.done(prompted["operation_id"].as_str().unwrap()).await;
    h.action("session_close", "close", session).await;
    assert_eq!(
        h.state.store.find_session(session).unwrap().unwrap().lifecycle,
        "closed"
    );
    let transcript = h.rpc("session_read", json!({"session_id":session})).await;
    assert!(!transcript["items"].as_array().unwrap().is_empty());
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_reports_permission_requests_without_granting_external_approval_authority() {
    let h = Harness::new(FakeScript {
        asks: vec![FakeAsk::Permission],
        ..Default::default()
    })
    .await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap();
    let prompt = h
        .rpc(
            "session_prompt",
            json!({"request_key":"prompt","session_id":session,"text":"Ask permission"}),
        )
        .await;
    h.until(|| !h.state.store.open_pending(session).unwrap().is_empty())
        .await;
    let waiting = h
        .rpc(
            "session_wait",
            json!({"operation_id":prompt["operation_id"],"timeout_ms":10000}),
        )
        .await;
    assert_eq!(waiting["waiting_on_operator"], true);
    assert_ne!(waiting["operation"]["state"], "completed");
    let answer: Value = h
        .raw(
            &h.token,
            "session_answer",
            json!({"session_id":session,"action":"allow"}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(answer["error"]["code"], -32602);
    assert_eq!(h.state.store.open_pending(session).unwrap().len(), 1);
    h.action("session_close", "close", session).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_retries_and_a_lost_response_never_create_or_prompt_twice() {
    let h = Harness::new(FakeScript::default()).await;
    drop(h.raw(&h.token, "session_create", h.create_args("lost-create")).await);
    let recovered = h.rpc("operation_get", json!({"request_key":"lost-create"})).await;
    let created = h.done(recovered["operation_id"].as_str().unwrap()).await;
    let session = created["session_id"].as_str().unwrap();
    let copies = futures::future::join_all((0..6).map(|_| h.rpc("session_create", h.create_args("lost-create")))).await;
    for copy in copies {
        assert_eq!(copy["operation_id"], recovered["operation_id"]);
        assert_eq!(copy["session_id"], session);
    }
    let args = json!({"request_key":"lost-prompt","session_id":session,"text":"Exactly once"});
    drop(h.raw(&h.token, "session_prompt", args.clone()).await);
    let results = futures::future::join_all((0..6).map(|_| h.rpc("session_prompt", args.clone()))).await;
    for result in &results {
        assert_eq!(result["operation_id"], results[0]["operation_id"]);
        assert_eq!(result["turn_id"], results[0]["turn_id"]);
    }
    h.done(results[0]["operation_id"].as_str().unwrap()).await;
    let conflict: Value = h
        .raw(
            &h.token,
            "session_prompt",
            json!({"request_key":"lost-prompt","session_id":session,"text":"Different request"}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(conflict["result"]["isError"], true);
    assert_eq!(conflict["result"]["content"][0]["text"], "request_key_conflict");
    assert_eq!(h.log("session/new"), 1);
    assert_eq!(h.log("session/prompt"), 1);
    h.action("session_close", "close", session).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_prompts_queue_and_cancel_never_hits_a_newer_turn() {
    let h = Harness::new(FakeScript {
        chunks: vec!["A".into(), "B".into(), "C".into()],
        chunk_delay_ms: 400,
        ..Default::default()
    })
    .await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap();
    let first = h
        .rpc(
            "session_prompt",
            json!({"request_key":"first","session_id":session,"text":"First"}),
        )
        .await;
    h.until(|| {
        h.state
            .store
            .find_session(session)
            .unwrap()
            .unwrap()
            .open_turn_id
            .is_some()
    })
    .await;
    let second = h
        .rpc(
            "session_prompt",
            json!({"request_key":"second","session_id":session,"text":"Second"}),
        )
        .await;
    assert_eq!(second["state"], "queued");
    let cancel = h
        .rpc(
            "session_cancel",
            json!({"request_key":"cancel-first","session_id":session,"turn_id":first["turn_id"]}),
        )
        .await;
    let cancelled = h.done(cancel["operation_id"].as_str().unwrap()).await;
    assert_eq!(cancelled["outcome"], "cancelled");
    h.until(|| {
        h.state
            .store
            .find_session(session)
            .unwrap()
            .unwrap()
            .open_turn_id
            .as_deref()
            == second["turn_id"].as_str()
    })
    .await;
    let stale = h
        .rpc(
            "session_cancel",
            json!({"request_key":"stale-cancel","session_id":session,"turn_id":first["turn_id"]}),
        )
        .await;
    let old = h.done(stale["operation_id"].as_str().unwrap()).await;
    assert_eq!(old["outcome"], "cancelled");
    let completed = h.done(second["operation_id"].as_str().unwrap()).await;
    assert_eq!(completed["outcome"], "completed");
    h.action("session_close", "close", session).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_cross_client_and_noncanonical_paths_cannot_gain_execution() {
    let h = Harness::new(FakeScript::default()).await;
    let created = h.create("create").await;
    let session = created["session_id"].as_str().unwrap();
    let read_only = h.grant(true, false, false);
    let response = h
        .raw(
            &read_only,
            "session_prompt",
            json!({"request_key":"forbidden","session_id":session,"text":"No"}),
        )
        .await;
    assert_eq!(response.status(), 403);
    assert!(
        response.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("hennery:sessions:control")
    );
    assert_eq!(
        h.raw(&read_only, "session_read", json!({"session_id":session}))
            .await
            .status(),
        403
    );
    let other = h.grant(false, true, true);
    for (tool, args) in [
        ("session_get", json!({"session_id":session})),
        ("session_read", json!({"session_id":session})),
        ("session_close", json!({"session_id":session,"request_key":"cross"})),
        ("operation_get", json!({"operation_id":created["operation_id"]})),
    ] {
        let body: Value = h.raw(&other, tool, args).await.json().await.unwrap();
        assert_eq!(body["result"]["isError"], true, "{tool}: {body}");
        assert_eq!(body["result"]["content"][0]["text"], "not_found");
    }
    let outside = h.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, PathBuf::from(&h.root).join("escape")).unwrap();
    let op = h
        .rpc(
            "session_create",
            json!({"request_key":"escape","host_id":"host","cwd":format!("{}/escape",h.root),"agent":"fake"}),
        )
        .await;
    h.until(|| {
        let conn = rusqlite::Connection::open(h.dir.path().join("collector.db")).unwrap();
        conn.query_row(
            "SELECT state='failed' FROM external_operations WHERE id=?1",
            [op["operation_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap()
    })
    .await;
    let failed = h.rpc("operation_get", json!({"operation_id":op["operation_id"]})).await;
    assert_eq!(failed["code"], "cwd_moved");
    assert!(
        h.state
            .store
            .find_session(op["session_id"].as_str().unwrap())
            .unwrap()
            .is_none()
    );
    h.action("session_close", "close", session).await;
    h.finish().await;
}
