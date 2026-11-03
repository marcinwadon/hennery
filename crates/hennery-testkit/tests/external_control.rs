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
    assert_eq!(answer["result"]["isError"], true);
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

impl Harness {
    async fn terminal(&self, id: &Value) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let op = self.rpc("operation_get", json!({"operation_id":id})).await;
            if matches!(op["state"].as_str(), Some("completed" | "failed" | "cancelled")) {
                return op;
            }
            assert!(tokio::time::Instant::now() < deadline, "{op}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_config_clone_metadata_filters_and_usage_use_the_real_host() {
    let h = Harness::new(FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        ..Default::default()
    })
    .await;
    let catalog = h
        .rpc(
            "runtime_catalog",
            json!({"host_id":"host","cwd":h.root,"agent":"fake","refresh":true}),
        )
        .await;
    assert!(!catalog["config_options"].as_array().unwrap().is_empty(), "{catalog}");
    let session = h.create("original").await["session_id"].as_str().unwrap().to_owned();
    let changed = h
        .rpc(
            "session_config",
            json!({"request_key":"model","session_id":session,"config_id":"model","value":"large"}),
        )
        .await;
    let changed = h.done(changed["operation_id"].as_str().unwrap()).await;
    assert_eq!(changed["result"]["effective"]["model"], "large");
    assert_eq!(
        changed["result"]["requested"],
        json!({"config_id":"model","value":"large"})
    );
    let toggle = h
        .rpc(
            "session_config",
            json!({"request_key":"fast","session_id":session,"config_id":"fast","value":true}),
        )
        .await;
    let toggled = h.done(toggle["operation_id"].as_str().unwrap()).await;
    assert_eq!(toggled["result"]["requested"]["value"], true);
    let updated=h.rpc("session_update",json!({"request_key":"rename","session_id":session,"title":"MCP title","sidebar_pinned":true,"idle_pinned":true})).await;
    h.done(updated["operation_id"].as_str().unwrap()).await;
    let detail = h.rpc("session_get", json!({"session_id":session})).await;
    assert_eq!(detail["session"]["title"], "MCP title");
    assert_eq!(detail["detail"]["idle_pinned"], true);
    let list = h
        .rpc(
            "session_list",
            json!({"filters":{"query":"MCP title","sidebar_pinned":true,"agent":"fake"},"limit":1}),
        )
        .await;
    assert_eq!(list["sessions"][0]["session_id"], session);
    let empty = h
        .rpc("session_list", json!({"filters":{"query":"no such title"}}))
        .await;
    assert!(empty["sessions"].as_array().unwrap().is_empty());
    let clone = h
        .rpc(
            "session_clone",
            json!({"request_key":"clone","source_session_id":session}),
        )
        .await;
    let cloned = h.done(clone["operation_id"].as_str().unwrap()).await;
    let other = cloned["session_id"].as_str().unwrap();
    assert_ne!(other, session);
    let cloned_detail = h.rpc("session_get", json!({"session_id":other})).await;
    assert_eq!(cloned_detail["detail"]["catalog"]["model"], "large");
    assert_eq!(cloned_detail["detail"]["catalog"]["axes"]["fast"], true);
    // Each fake ACP process starts its native IDs at fake-session-1. A
    // fresh session/new, rather than native string inequality, proves cloning.
    assert_eq!(h.log("session/load"), 0);
    assert!(h.log("session/new") >= 2);
    let usage = h.rpc("session_usage", json!({"session_id":other})).await;
    assert!(usage.get("family").is_none());
    assert_eq!(usage["session"]["turns"], 0);
    h.action("session_close", "close-original", &session).await;
    h.action("session_close", "close-clone", other).await;
    h.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_answers_validate_schema_preserve_origin_and_do_not_approve_permissions() {
    let h = Harness::new(FakeScript {
        asks: vec![FakeAsk::Elicitation, FakeAsk::Permission],
        ..Default::default()
    })
    .await;
    let session = h.create("create").await["session_id"].as_str().unwrap().to_owned();
    h.rpc(
        "session_prompt",
        json!({"request_key":"prompt","session_id":session,"text":"ask"}),
    )
    .await;
    h.until(|| !h.state.store.open_pending(&session).unwrap().is_empty())
        .await;
    let pending = h.rpc("session_pending", json!({"session_id":session})).await;
    let id = pending["pending"][0]["pending_id"].clone();
    let invalid=h.rpc("session_answer",json!({"request_key":"invalid","session_id":session,"pending_id":id,"action":"accept","content":{"name":123}})).await;
    let invalid = h.terminal(&invalid["operation_id"]).await;
    assert_eq!(invalid["code"], "invalid_answer");
    let answer=h.rpc("session_answer",json!({"request_key":"answer","session_id":session,"pending_id":id,"action":"accept","content":{"name":"MCP"}})).await;
    h.done(answer["operation_id"].as_str().unwrap()).await;
    h.until(|| {
        h.state
            .store
            .open_pending(&session)
            .unwrap()
            .iter()
            .any(|p| p.kind == hennery_proto::frames::PendingKind::Permission)
    })
    .await;
    let pending = h.rpc("session_pending", json!({"session_id":session})).await;
    let permission = pending["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "permission")
        .unwrap();
    let denied=h.rpc("session_answer",json!({"request_key":"no-permission","session_id":session,"pending_id":permission["pending_id"],"action":"accept","content":{}})).await;
    let denied = h.terminal(&denied["operation_id"]).await;
    assert_eq!(denied["code"], "operator_required");
    let conn = rusqlite::Connection::open(h.dir.path().join("collector.db")).unwrap();
    let origin: String = conn
        .query_row(
            "SELECT input_origin FROM answer_queue WHERE pending_id=?1",
            [id.as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&origin).unwrap()["kind"],
        "external_client"
    );
    h.action("session_close", "close", &session).await;
    h.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_prompts_can_be_edited_and_withdrawn_without_replaying_original_keys() {
    let h = Harness::new(FakeScript {
        chunks: vec!["x".into(); 30],
        chunk_delay_ms: 100,
        ..Default::default()
    })
    .await;
    let session = h.create("create").await["session_id"].as_str().unwrap().to_owned();
    let first = h
        .rpc(
            "session_prompt",
            json!({"request_key":"first","session_id":session,"text":"first"}),
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
    let original = json!({"request_key":"second","session_id":session,"text":"old text"});
    let second = h.rpc("session_prompt", original.clone()).await;
    let third = h
        .rpc(
            "session_prompt",
            json!({"request_key":"third","session_id":session,"text":"withdraw"}),
        )
        .await;
    let edit=h.rpc("session_queue_update",json!({"request_key":"edit","session_id":session,"operation_id":second["operation_id"],"text":"edited text"})).await;
    h.done(edit["operation_id"].as_str().unwrap()).await;
    let cancel = h
        .rpc(
            "operation_cancel",
            json!({"request_key":"withdraw","session_id":session,"operation_id":third["operation_id"]}),
        )
        .await;
    h.done(cancel["operation_id"].as_str().unwrap()).await;
    let queue = h.rpc("session_queue", json!({"session_id":session})).await;
    assert!(
        queue["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["text"] == "edited text")
    );
    let retry = h.rpc("session_prompt", original).await;
    assert_eq!(retry["operation_id"], second["operation_id"]);
    h.done(first["operation_id"].as_str().unwrap()).await;
    h.done(second["operation_id"].as_str().unwrap()).await;
    assert_eq!(h.terminal(&third["operation_id"]).await["state"], "cancelled");
    assert_eq!(h.log("session/prompt"), 2);
    let read = h.rpc("session_read", json!({"session_id":session})).await.to_string();
    assert!(read.contains("edited text"));
    assert!(!read.contains("old text"));
    h.action("session_close", "close", &session).await;
    h.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunked_images_survive_retries_and_only_the_referencing_session_can_read_them() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let h = Harness::new(FakeScript::default()).await;
    let session = h.create("create").await["session_id"].as_str().unwrap().to_owned();
    let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
    let bytes = STANDARD.decode(data).unwrap();
    let hash = hennery_kernel::secret::sha256_hex(&bytes);
    let begin=h.rpc("attachment_upload_begin",json!({"request_key":"upload","session_id":session,"mime_type":"image/png","size":bytes.len(),"sha256":hash})).await;
    let upload = h.done(begin["operation_id"].as_str().unwrap()).await["result"]["upload_id"].clone();
    let chunk = json!({"session_id":session,"upload_id":upload,"offset":0,"data_base64":data});
    h.rpc("attachment_upload_chunk", chunk.clone()).await;
    h.rpc("attachment_upload_chunk", chunk.clone()).await;
    let finish = h
        .rpc(
            "attachment_upload_finish",
            json!({"request_key":"finish-upload","session_id":session,"upload_id":upload}),
        )
        .await;
    assert_eq!(
        h.done(finish["operation_id"].as_str().unwrap()).await["result"]["sealed"],
        true
    );
    h.rpc("attachment_upload_chunk", chunk).await;
    let prompt = h
        .rpc(
            "session_prompt",
            json!({"request_key":"image-prompt","session_id":session,"attachments":[upload]}),
        )
        .await;
    h.done(prompt["operation_id"].as_str().unwrap()).await;
    let image = h
        .rpc("attachment_get", json!({"session_id":session,"sha256":hash,"limit":10}))
        .await;
    assert_eq!(
        STANDARD.decode(image["data_base64"].as_str().unwrap()).unwrap(),
        bytes[..10]
    );
    assert_eq!(image["next_offset"], 10);
    let other = h.create("other").await["session_id"].as_str().unwrap().to_owned();
    let forbidden: Value = h
        .raw(&h.token, "attachment_get", json!({"session_id":other,"sha256":hash}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(forbidden["result"]["isError"], true);
    let aborted = h
        .rpc(
            "attachment_upload_abort",
            json!({"request_key":"abort","session_id":session,"upload_id":upload}),
        )
        .await;
    h.done(aborted["operation_id"].as_str().unwrap()).await;
    assert_eq!(
        h.rpc("attachment_get", json!({"session_id":session,"sha256":hash}))
            .await["data_base64"],
        data
    );
    h.action("session_close", "close", &session).await;
    h.action("session_close", "close-other", &other).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn image_chunks_enforce_offsets_content_ownership_and_reserved_byte_quotas() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let h = Harness::new(FakeScript::default()).await;
    let session = h.create("create").await["session_id"].as_str().unwrap().to_owned();
    // Use the same signature-based PNG fixture contract as content's unit
    // tests, large enough to require three HTTP chunks.
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(50_000, 1);
    let hash = hennery_kernel::secret::sha256_hex(&bytes);
    let args = json!({"request_key":"large-upload","session_id":session,"mime_type":"image/png","size":bytes.len(),"sha256":hash});
    let begin = h.rpc("attachment_upload_begin", args.clone()).await;
    let upload = h.done(begin["operation_id"].as_str().unwrap()).await["result"]["upload_id"].clone();
    assert_eq!(
        h.rpc("attachment_upload_begin", args).await["operation_id"],
        begin["operation_id"]
    );
    let misplaced:Value=h.raw(&h.token,"attachment_upload_chunk",json!({"session_id":session,"upload_id":upload,"offset":24576,"data_base64":STANDARD.encode(&bytes[24576..49152])})).await.json().await.unwrap();
    assert_eq!(misplaced["result"]["isError"], true);
    let other = h.grant(true, true, true);
    let denied: Value = h
        .raw(
            &other,
            "attachment_upload_status",
            json!({"session_id":session,"upload_id":upload}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(denied["result"]["isError"], true);
    for (offset, chunk) in bytes.chunks(24576).enumerate() {
        h.rpc(
            "attachment_upload_chunk",
            json!({"session_id":session,"upload_id":upload,"offset":offset*24576,"data_base64":STANDARD.encode(chunk)}),
        )
        .await;
    }
    let mut changed = bytes[..24576].to_vec();
    changed[100] = 2;
    let conflict: Value = h
        .raw(
            &h.token,
            "attachment_upload_chunk",
            json!({"session_id":session,"upload_id":upload,"offset":0,"data_base64":STANDARD.encode(changed)}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(conflict["result"]["isError"], true);
    let finish = h
        .rpc(
            "attachment_upload_finish",
            json!({"request_key":"seal-large","session_id":session,"upload_id":upload}),
        )
        .await;
    assert_eq!(
        h.done(finish["operation_id"].as_str().unwrap()).await["result"]["sealed"],
        true
    );
    let abort = h
        .rpc(
            "attachment_upload_abort",
            json!({"request_key":"abort-large","session_id":session,"upload_id":upload}),
        )
        .await;
    h.done(abort["operation_id"].as_str().unwrap()).await;
    for n in 0..5 {
        let begin=h.rpc("attachment_upload_begin",json!({"request_key":format!("quota-{n}"),"session_id":session,"mime_type":"image/png","size":5*1024*1024,"sha256":"a".repeat(64)})).await;
        let op = h.terminal(&begin["operation_id"]).await;
        if n < 4 {
            assert_eq!(op["state"], "completed");
        } else {
            assert_eq!(op["code"], "upload_limit");
        }
    }
    let caps = h.rpc("hennery_capabilities", json!({})).await;
    h.state
        .store
        .revoke_external_client(caps["client_id"].as_str().unwrap())
        .unwrap();
    let conn = rusqlite::Connection::open(h.dir.path().join("collector.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM external_uploads WHERE client_id=?1",
            [caps["client_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_advertises_the_complete_tool_surface_and_withheld_scopes_stay_withheld() {
    let h = Harness::new(FakeScript::default()).await;
    let session = h.create("create").await["session_id"].as_str().unwrap().to_owned();
    let list = |token: String| {
        let http = h.http.clone();
        let url = h.url.clone();
        async move {
            let body: Value = http
                .post(format!("{url}/mcp/external/hennery"))
                .bearer_auth(token)
                .header("MCP-Protocol-Version", "2025-11-25")
                .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            body["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        }
    };
    let all = list(h.token.clone()).await;
    assert_eq!(all.len(), 30);
    for name in [
        "session_pending",
        "session_answer",
        "runtime_catalog",
        "session_config",
        "session_queue",
        "operation_cancel",
        "session_queue_update",
        "session_usage",
        "session_update",
        "session_clone",
        "attachment_upload_begin",
        "attachment_upload_chunk",
        "attachment_upload_finish",
        "attachment_upload_abort",
        "attachment_upload_status",
        "attachment_get",
    ] {
        assert!(all.contains(name), "{name}");
    }
    let metadata = h.grant(true, false, false);
    let names = list(metadata.clone()).await;
    assert!(names.contains("runtime_catalog"));
    assert!(names.contains("session_usage"));
    assert!(!names.contains("session_answer"));
    assert!(!names.contains("session_pending"));
    assert!(!names.contains("attachment_get"));
    assert!(!names.contains("session_clone"));
    for (tool, args) in [
        (
            "session_answer",
            json!({"request_key":"denied","session_id":session,"pending_id":"guess","action":"cancel"}),
        ),
        (
            "session_config",
            json!({"request_key":"denied-config","session_id":session,"config_id":"model","value":"bypass"}),
        ),
        (
            "attachment_upload_chunk",
            json!({"session_id":session,"upload_id":"guess","offset":0,"data_base64":"AA=="}),
        ),
        (
            "runtime_catalog",
            json!({"host_id":"host","cwd":h.root,"agent":"fake","refresh":true}),
        ),
    ] {
        assert_eq!(h.raw(&metadata, tool, args).await.status(), 403, "{tool}");
    }
    assert_eq!(
        h.raw(&metadata, "session_pending", json!({"session_id":session}))
            .await
            .status(),
        403
    );
    let control = h.grant(true, true, false);
    let names = list(control.clone()).await;
    assert!(names.contains("session_answer"));
    assert!(!names.contains("attachment_get"));
    h.rpc_as(&metadata, "session_usage", json!({"session_id":session}))
        .await;
    h.action("session_close", "close", &session).await;
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_auto_resumes_a_parked_session_before_admission() {
    let h = Harness::new(FakeScript::default()).await;
    let session = h.create("create").await["session_id"].as_str().unwrap().to_owned();
    h.action("session_park", "park", &session).await;
    let prompt = h
        .rpc(
            "session_prompt",
            json!({"request_key":"resume-and-prompt","session_id":session,"text":"continue after parking"}),
        )
        .await;
    h.done(prompt["operation_id"].as_str().unwrap()).await;
    assert_eq!(h.log("session/load"), 1);
    assert_eq!(h.log("session/prompt"), 1);
    h.action("session_close", "close", &session).await;
    h.finish().await;
}
