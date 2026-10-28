//! Seven D2 MCP handlers over real HTTP/WebSocket sockets and fake ACP
//! processes. No model provider calls; every adapter home and data dir is scratch.
use hennery_gateway::{api::GatewayState, key::KeySource, session::GatewayMcp};
use hennery_host::{AgentCommand, HostConfig, identity::HostKey, profile::Profile};
use hennery_kernel::{
    egress::{Egress, Timeouts},
    hosts::{Enrollment, Hosts},
    operator::Operator,
};
use hennery_sessions::{AppState, store::Store};
use hennery_testkit::{FakeAsk, FakeScript, SCRIPT_ENV, sample_config_options};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const HOST: &str = "delegation-host";
const BOUND: Duration = Duration::from_secs(30);

#[tokio::test]
async fn explicit_start_choices_never_become_agent_defaults_without_a_prior_probe() {
    let h = Harness::new(modes("default", &["default", "bypassPermissions"])).await;
    let (status, body) = h
        .post(
            "/api/sessions",
            json!({"host_id":HOST,"agent":"worker","cwd":h.cwd,"model":"large","mode":"bypassPermissions"}),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    let preview: Value = h
        .client
        .get(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["defaults"]["model"], "small");
    assert_eq!(preview["defaults"]["mode"], "default");
    assert_eq!(preview["config_options"][0]["currentValue"], "large");
    h.finish().await;
}

#[tokio::test]
async fn operator_catalog_reads_never_probe_and_real_start_resume_write_through() {
    let h = Harness::new(modes("default", &["default", "plan", "bypassPermissions"])).await;
    let path = format!("{}/api/hosts/{HOST}/runtime-catalog", h.url);
    let query = [("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())];
    let unknown: Value = h
        .client
        .get(&path)
        .query(&query)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unknown["freshness"], "unknown");
    let log = h.cwd.join("worker-sessions.jsonl");
    assert!(!log.exists(), "reading an absent catalog must never spawn an adapter");
    let refreshed: Value = h
        .client
        .post(&path)
        .query(&query)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["freshness"], "fresh");
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    for _ in 0..2 {
        let saved: Value = h
            .client
            .get(&path)
            .query(&query)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(saved["freshness"], "cached");
        assert!(saved["saved_at"].is_number());
    }
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "saved reads never probe"
    );
    let (status, body) = h
        .post(
            "/api/sessions",
            json!({"host_id":HOST,"agent":"worker","cwd":h.cwd,"model":"large","mode":"bypassPermissions"}),
        )
        .await;
    assert_eq!(status, 202, "{body}");
    let id = body["session_id"].as_str().unwrap();
    eventually("real start snapshot", async || {
        let value: Value = h
            .client
            .get(&path)
            .query(&query)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        (value["config_options"][0]["currentValue"] == "large").then(|| {
            assert_eq!(value["defaults"]["model"], "small");
            assert_eq!(value["defaults"]["mode"], "default");
        })
    })
    .await;
    let (status, body) = h.post(&format!("/api/sessions/{id}/park"), json!({})).await;
    assert_eq!(status, 202, "{body}");
    // Clear only this test database's preview, so a resume must repopulate it.
    rusqlite::Connection::open(h.cwd.join("hennery.db"))
        .unwrap()
        .execute("DELETE FROM runtime_catalog WHERE agent='worker'", [])
        .unwrap();
    let (status, body) = h.post(&format!("/api/sessions/{id}/resume"), json!({})).await;
    assert_eq!(status, 202, "{body}");
    eventually("real resume snapshot", async || {
        let value: Value = h
            .client
            .get(&path)
            .query(&query)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        (value["freshness"] == "cached").then(|| {
            assert_eq!(value["defaults"], json!({}), "resume cannot invent an agent baseline");
        })
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        3,
        "one explicit probe, one start, one resume"
    );
    h.finish().await;
}

#[tokio::test]
async fn aborted_operator_refresh_cancels_the_real_http_probe_early() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("refresh.pid");
    let command = AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), "echo $$ > \"$PROBE_PID_FILE\"; sleep 600 & wait".into()],
        env: vec![("PROBE_PID_FILE".into(), pid_file.display().to_string())],
    };
    let h = Harness::with_worker(script(), Some((command, Profile::Generic)), false).await;
    let client = h.client.clone();
    let path = format!("{}/api/hosts/{HOST}/runtime-catalog", h.url);
    let cwd = h.cwd.to_str().unwrap().to_owned();
    let refresh = tokio::spawn(async move {
        client
            .post(path)
            .query(&[("agent", "worker"), ("cwd", cwd.as_str())])
            .send()
            .await
    });
    let pid: i32 = eventually("HTTP refresh started an adapter", async || {
        std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
    })
    .await;
    // SAFETY: this test owns the live probe. Reading its pgid has no side effects.
    let group = unsafe { libc::getpgid(pid) };
    assert!(group > 0);
    struct Cleanup(i32);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            // SAFETY: clean up the test-owned group even if this regression fails.
            unsafe {
                libc::killpg(self.0, libc::SIGKILL);
            }
        }
    }
    let cleanup = Cleanup(group);
    refresh.abort();
    assert!(refresh.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // SAFETY: signal zero only checks the test-owned process group.
            if unsafe { libc::killpg(group, 0) } != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("aborted HTTP refresh must cancel before its 30-second probe timeout");
    std::mem::forget(cleanup);
    h.finish().await;
}

#[tokio::test]
async fn operator_limits_are_reported_by_session_capabilities_and_enforced_on_next_spawn() {
    let h = Harness::new(script()).await;
    for method in [reqwest::Method::GET, reqwest::Method::PATCH] {
        let response = h
            .raw
            .request(method, format!("{}/api/settings/delegation-limits", h.url))
            .bearer_auth(&h.token)
            .header("origin", &h.url)
            .json(&json!({"children":1}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(response.json::<Value>().await.unwrap()["code"], "unauthenticated");
    }
    let defaults = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    assert_eq!(
        defaults["limits"],
        json!({"children":8,"starts_per_operator_epoch":32,"automatic_wakes_per_operator_epoch":20,"queued_prompts":32,"queued_bytes":1048576,"message_bytes":65536})
    );
    let response = h
        .client
        .patch(format!("{}/api/settings/delegation-limits", h.url))
        .json(&json!({"children":2,"starts_per_operator_epoch":4,"automatic_wakes_per_operator_epoch":6}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let changed = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    assert_eq!(
        changed["limits"],
        json!({"children":2,"starts_per_operator_epoch":4,"automatic_wakes_per_operator_epoch":6,"queued_prompts":32,"queued_bytes":1048576,"message_bytes":65536})
    );
    h.spawn("first", json!({})).await;
    h.spawn("second", json!({})).await;
    let refused = h
        .call(
            "session_spawn",
            json!({"operation_key":"third","cwd":h.cwd,"agent":"worker","brief":"third child","config":{}}),
        )
        .await;
    assert_eq!(refused["structuredContent"]["code"], "child_limit", "{refused}");
    h.finish().await;
}

#[derive(Clone, Default)]
struct NoticeLogs(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for NoticeLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn operator_limits_dispatch_failure_warns_with_notice_id_without_content() {
    let h = Harness::with_worker(script(), None, false).await;
    // The model-facing capabilities omit `security`; dispatch takes the full
    // host catalog, which the operator endpoint returns.
    let catalog = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "claude"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json::<hennery_proto::delegation_host::HostCatalog>()
        .await
        .unwrap();
    let child = h.spawn("log-child", json!({})).await["child_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let conn = rusqlite::Connection::open(h.cwd.join("hennery.db")).unwrap();
    let owner = h.state.store.owner_id();
    let parent = h.state.store.find_session(&h.parent).unwrap().unwrap();
    conn.execute("INSERT INTO delegation_notices(owner_id,delivery_id,child_id,parent_id,root_id,hat_id,source_event_id,source_events,kind,incident,accepted_revision,created_at,deadline,required_config)
        SELECT ?1,'limits-error-notice',?2,?3,?3,?4,1,'[1]','report','log-incident',revision,?5,?6,'{}'
        FROM hat_session_tools WHERE owner_id=?1 AND hat_id=?4",rusqlite::params![owner,child,h.parent,parent.hat_id,hennery_kernel::secret::unix_now(),hennery_kernel::secret::unix_now()+300]).unwrap();
    conn.execute(
        "UPDATE session_attach_evidence SET evidence='PRIVATE_SENTINEL' WHERE owner_id=?1 AND session_id=?2",
        rusqlite::params![owner, h.parent],
    )
    .unwrap();
    let logs = NoticeLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer({
            let logs = logs.clone();
            move || logs.clone()
        })
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        for state in ["parent_queued", "waiting_config"] {
            conn.execute(
                "UPDATE delegation_notices SET state=?2 WHERE owner_id=?1 AND delivery_id='limits-error-notice'",
                rusqlite::params![owner, state],
            )
            .unwrap();
            assert!(h.state.store.notice_parents_after("").unwrap().contains(&h.parent));
            assert!(
                h.state
                    .store
                    .dispatch_child_notice(&h.parent, &catalog, &h.state.hub)
                    .is_err()
            );
        }
        assert_eq!(
            String::from_utf8(logs.0.lock().unwrap().clone())
                .unwrap()
                .matches("delegation notice dispatch failed")
                .count(),
            1,
            "retries for the same notice must be throttled"
        );
        conn.execute(
            "UPDATE delegation_notices SET state='obsolete' WHERE owner_id=?1 AND delivery_id='limits-error-notice'",
            [owner],
        )
        .unwrap();
        assert!(h.state.store.notice_parents_after("").unwrap().is_empty());
        // Reuse the fixture ID to prove settling removed its throttle entry.
        conn.execute(
            "UPDATE delegation_notices SET state='parent_queued' WHERE owner_id=?1 AND delivery_id='limits-error-notice'",
            [owner],
        )
        .unwrap();
        assert!(
            h.state
                .store
                .dispatch_child_notice(&h.parent, &catalog, &h.state.hub)
                .is_err()
        );
    });
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        output.contains("WARN")
            && output.contains("delegation notice dispatch failed")
            && output.contains("limits-error-notice")
    );
    assert!(!output.contains("PRIVATE_SENTINEL"));
    assert_eq!(output.matches("delegation notice dispatch failed").count(), 2);
    assert_eq!(output.matches("code=invalid_arguments").count(), 2);
    h.finish().await;
}

#[tokio::test]
async fn spawn_admission_catalog_is_reused_for_the_first_attach() {
    let h = Harness::with_worker(script(), None, false).await;
    let receipt = h.spawn("reuse-admission", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    eventually("child attached from its admission catalog", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "active")
            .map(|_| ())
    })
    .await;
    let log = std::fs::read_to_string(h.cwd.join("worker-sessions.jsonl")).unwrap();
    let starts = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["method"] == "session/new")
        .count();
    assert_eq!(
        starts, 2,
        "one discovery process plus the actual child; no second probe"
    );
    h.finish().await;
}

#[tokio::test]
async fn nested_spawn_is_refused_before_any_runtime_probe() {
    let mut worker = script();
    worker.new_session_error = Some(-32000);
    let h = Harness::with_worker(worker, None, false).await;
    // A previously delivered tool identity must still obey the permanent child bit.
    let conn = rusqlite::Connection::open(h.cwd.join("hennery.db")).unwrap();
    conn.execute("UPDATE sessions SET delegated_child=1 WHERE id=?1", [&h.parent])
        .unwrap();
    let result = h.call("session_spawn", json!({"operation_key":"nested","cwd":h.cwd,"agent":"worker","brief":"refuse before discovery","config":{}})).await;
    assert_eq!(result["structuredContent"]["code"], "generation_limit", "{result}");
    assert!(
        !h.cwd.join("worker-sessions.jsonl").exists(),
        "nested caller reached the runtime"
    );
    h.finish().await;
}

#[tokio::test]
async fn binary_delegation_unauthenticated_spawn_returns_auth_required() {
    let mut worker = script();
    worker.new_session_error = Some(-32000);
    let h = Harness::with_worker(worker, None, false).await;
    let result = h
        .call(
            "session_spawn",
            json!({"operation_key":"auth-required","cwd":h.cwd,"agent":"worker","brief":"must not run","config":{}}),
        )
        .await;
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["code"], "auth_required", "{result}");
    assert!(
        result["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("not signed in")
    );
    let response = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    let error: Value = response.json().await.unwrap();
    assert_eq!(error["code"], "auth_required");
    assert_eq!(error["message"], "worker on fixture: not signed in");
    assert!(
        h.state
            .store
            .family_page(&h.parent, "", 100)
            .unwrap()
            .unwrap()
            .children
            .is_empty()
    );
    h.finish().await;
}

#[tokio::test]
async fn binary_delegation_runtime_signed_in_after_enabling_uses_all_live_modes() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("signed-in");
    let wrapper = root.path().join("auth.cjs");
    std::fs::write(
        &wrapper,
        r#"
const {spawn}=require('node:child_process');
const fs=require('node:fs');
const script=JSON.parse(process.env.HENNERY_FAKE_ACP_SCRIPT);
if(!fs.existsSync(process.argv[3]))script.new_session_error=-32000;
else if(fs.readFileSync(process.argv[3],'utf8')!=='full')script.config_options.find(o=>o.id==='mode').options=[{value:'read-only',name:'Read only'}];
process.env.HENNERY_FAKE_ACP_SCRIPT=JSON.stringify(script);
const child=spawn(process.argv[2],[],{env:process.env,stdio:'inherit'});
child.on('exit',code=>process.exit(code??1));
"#,
    )
    .unwrap();
    let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("node"))
        .find(|p| p.is_file())
        .unwrap();
    hennery_testkit::first_exec(node.to_str().unwrap());
    let mut worker = script();
    let mode = worker.config_options.iter_mut().find(|o| o["id"] == "mode").unwrap();
    mode["currentValue"] = json!("read-only");
    mode["options"] =
        json!([{"value":"read-only","name":"Read only"},{"value":"agent-full-access","name":"Full access"}]);
    let mut command = fake(root.path(), "late-auth", worker);
    command.args = vec![
        wrapper.display().to_string(),
        command.program,
        marker.display().to_string(),
    ];
    command.program = node.display().to_string();
    let h = Harness::with_worker(script(), Some((command, Profile::Codex)), true).await;
    let hat = h.state.store.find_session(&h.parent).unwrap().unwrap().hat_id;
    let tools_url = format!("{}/api/hats/{hat}/session-tools", h.url);
    let before: Value = h.client.get(&tools_url).send().await.unwrap().json().await.unwrap();
    let caps = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    assert_eq!(caps["catalogs"][0]["freshness"], "unknown");
    assert!(caps["catalogs"][0].get("security").is_none());
    std::fs::write(&marker, "signed in").unwrap();
    let preview = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    assert_eq!(preview["catalogs"][0]["freshness"], "fresh");
    assert!(!preview.to_string().contains("agent-full-access"));
    // Simulate an adapter update after a successful cached preview. A spawn
    // re-probes rather than making the preview into an authorization snapshot.
    std::fs::write(&marker, "full").unwrap();
    let receipt = h.spawn("late-auth", json!({"mode":"agent-full-access"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    h.done(child, "late-auth").await;
    assert_eq!(h.status(child).await["actual"]["mode"], "agent-full-access");
    let after: Value = h.client.get(&tools_url).send().await.unwrap().json().await.unwrap();
    assert_eq!(before, after, "no re-toggle or grant write");
    h.finish().await;
}

#[tokio::test]
async fn binary_delegation_claude_catalog_allows_bypass_permissions_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    let mut worker = script();
    let mode = worker.config_options.iter_mut().find(|o| o["id"] == "mode").unwrap();
    mode["currentValue"] = json!("default");
    mode["options"] = json!([
        {"value":"default","name":"Default"},
        {"value":"plan","name":"Plan"},
        {"value":"bypassPermissions","name":"Bypass permissions"}
    ]);
    let command = fake(root.path(), "claude-bypass", worker);
    let h = Harness::with_worker(script(), Some((command, Profile::Claude)), true).await;
    let before: Value = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["defaults"]["mode"], "default");
    let caps = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    let catalog = &caps["catalogs"][0];
    assert_eq!(catalog["freshness"], "fresh");
    assert!(
        catalog.get("security").is_none(),
        "model-facing catalog leaked security metadata"
    );
    let mode = catalog["config_options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "mode")
        .unwrap();
    assert!(
        mode["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["value"] == "bypassPermissions")
    );
    let receipt = h.spawn("claude-bypass", json!({"mode":"bypassPermissions"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    h.done(child, "claude-bypass").await;
    let status = h.status(child).await;
    assert_eq!(status["requested"]["mode"], "bypassPermissions");
    assert_eq!(status["actual"]["mode"], "bypassPermissions");
    assert!(status["questions"].as_array().unwrap().is_empty());
    assert_eq!(status["last_report"], "child-result");
    let preview: Value = h
        .client
        .get(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        preview["defaults"]["mode"], "default",
        "a delegated bypass choice must not become the operator baseline"
    );
    assert_eq!(preview["defaults"], before["defaults"]);
    let baseline_source: Option<String> = rusqlite::Connection::open(h.cwd.join("hennery.db"))
        .unwrap()
        .query_row(
            "SELECT defaults_source_session_id FROM runtime_catalog WHERE agent='worker'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        baseline_source, None,
        "a delegated attachment cannot replace Refresh baseline provenance"
    );
    h.finish().await;
}

#[tokio::test]
async fn operator_catalog_preview_does_not_require_delegation_workspace_roots() {
    let h = Harness::with_worker_options(script(), None, false, hennery_host::session::IDLE_TIMEOUT, false).await;
    let response = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "{}",
        response.text().await.unwrap()
    );
    let error = h
        .state
        .catalogs
        .get(
            &h.state,
            HOST,
            &h.state.hosts.host(HOST).unwrap().unwrap().default_hat_id,
            h.cwd.to_str().unwrap(),
            "worker",
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "cwd_unavailable",
        "operator cache cannot authorize a delegated probe"
    );
}

#[tokio::test]
async fn operator_catalog_preserves_adapter_failure_and_cwd_reason_codes() {
    let mut broken = script();
    broken.new_session_error = Some(-32001);
    let h = Harness::with_worker_options(broken, None, false, hennery_host::session::IDLE_TIMEOUT, false).await;
    let response = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.json::<Value>().await.unwrap()["code"], "adapter_failed");
    let response = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.join("missing").to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.json::<Value>().await.unwrap()["code"], "cwd_unavailable");
}

#[tokio::test]
async fn d3b_deadline_worker_exposes_durable_attention_and_opens_the_child_card() {
    let mut worker = script();
    worker.asks = vec![FakeAsk::Elicitation];
    let mut h = Harness::with_worker(worker, None, false).await;
    let spawned = h.spawn("fallback", json!({})).await;
    let child = spawned["child_id"].as_str().unwrap();
    let question = eventually("child question", async || {
        hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
        h.state.store.open_pending(child).unwrap().into_iter().next()
    })
    .await;
    let db = h._dir.path().join("hennery.db");
    // Force only the persisted deadline; pending callbacks have no timeout.
    hennery_kernel::db::open(&db)
        .unwrap()
        .execute(
            "UPDATE delegation_notices SET deadline = 0 WHERE owner_id = ?1 AND child_id = ?2",
            rusqlite::params![h.state.store.owner_id(), child],
        )
        .unwrap();
    let (push, mut notices) = hennery_kernel::push::Push::new();
    h.state.push = push;
    hennery_sessions::delegation_worker::after_startup(&h.state);
    let notice = tokio::time::timeout(BOUND, notices.recv()).await.unwrap();
    assert!(notice.url.starts_with(&format!("/sessions/{child}?pending=")));
    assert!(notice.url.contains(&question.pending_id));
    assert!(notice.tag.starts_with("delegation:"));
    assert!((notices.guard())(&notice));
    assert!(h.state.store.still_open(child, &question.pending_id).unwrap());
    let response = h
        .client
        .get(format!("{}/api/delegation/attention", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let rows: Value = response.json().await.unwrap();
    assert_eq!(rows[0]["child_id"], child);
    assert_eq!(rows[0]["reason"], "handling_deadline");
    let denied = h
        .raw
        .get(format!("{}/api/delegation/attention", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    let id = rows[0]["id"].as_str().unwrap();
    let refused = h
        .client
        .post(format!("{}/api/delegation/attention/{id}/rearm", h.url))
        .json(&json!({"operator_turn_id":"unknown"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), reqwest::StatusCode::CONFLICT);
    let refused = h
        .client
        .post(format!("{}/api/delegation/attention/{id}/acknowledge", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        reqwest::StatusCode::CONFLICT,
        "a status acknowledgement cannot answer a question"
    );
    h.state.store.record_close_request(&h.parent).unwrap();
    let answered = h
        .client
        .post(format!(
            "{}/api/sessions/{child}/pending/{}/answer",
            h.url, question.pending_id
        ))
        .json(&json!({"action":"accept","content":{"name":"operator"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(answered.status(), reqwest::StatusCode::ACCEPTED);
    assert!(
        !(notices.guard())(&notice),
        "operator answers withdraw queued notification attempts"
    );
    eventually("answered child finishes after parent close", async || {
        h.state
            .store
            .events(child, 0, 100)
            .unwrap()
            .iter()
            .any(|event| event.kind == "turn_ended")
            .then_some(())
    })
    .await;
    assert!(h.state.store.operator_attention(None, "").unwrap().is_empty());
    h.finish().await;
}

struct LiveSse {
    response: reqwest::Response,
    buffered: Vec<u8>,
}
impl LiveSse {
    async fn open(h: &Harness, path: &str, after: Option<i64>) -> Self {
        let mut request = h.client.get(format!("{}{path}", h.url));
        if let Some(after) = after {
            request = request.header("last-event-id", after.to_string());
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        Self {
            response,
            buffered: Vec::new(),
        }
    }
    async fn next(&mut self) -> (String, Value) {
        tokio::time::timeout(BOUND, async {
            loop {
                while let Some(end) = self.buffered.windows(2).position(|bytes| bytes == b"\n\n") {
                    let block: Vec<_> = self.buffered.drain(..end + 2).collect();
                    let block = std::str::from_utf8(&block).unwrap();
                    let name = block
                        .lines()
                        .find_map(|line| line.strip_prefix("event:"))
                        .map(str::trim);
                    let data = block.lines().find_map(|line| line.strip_prefix("data:")).map(str::trim);
                    if let (Some(name), Some(data)) = (name, data) {
                        return (name.to_owned(), serde_json::from_str(data).unwrap());
                    }
                }
                self.buffered
                    .extend_from_slice(&self.response.chunk().await.unwrap().expect("live SSE ended"));
            }
        })
        .await
        .expect("live SSE message before deadline")
    }
    async fn matching(&mut self, name: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(BOUND, async {
            loop {
                let (kind, data) = self.next().await;
                if kind == name && predicate(&data) {
                    return data;
                }
            }
        })
        .await
        .expect("matching live SSE message before deadline")
    }
}
struct Harness {
    _dir: tempfile::TempDir,
    state: AppState,
    client: reqwest::Client,
    raw: reqwest::Client,
    url: String,
    parent: String,
    token: String,
    cwd: PathBuf,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    host_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    server_task: tokio::task::JoinHandle<()>,
    discovery: Arc<Mutex<Vec<Value>>>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.state.shutdown.cancel();
        self.server_task.abort();
    }
}
fn script() -> FakeScript {
    FakeScript {
        config_options: sample_config_options(),
        chunks: vec!["child-result".into()],
        ..Default::default()
    }
}

fn modes(default: &str, offered: &[&str]) -> FakeScript {
    let mut s = script();
    let mode = s.config_options.iter_mut().find(|o| o["id"] == "mode").unwrap();
    mode["currentValue"] = json!(default);
    // Labels intentionally carry no profile or mode semantics.
    mode["options"] = json!(
        offered
            .iter()
            .map(|id| json!({"value":id,"name":"Mode"}))
            .collect::<Vec<_>>()
    );
    s
}

async fn inherited_mode_case(
    parent_profile: Profile,
    parent_mode: &str,
    child_profile: Profile,
    same_runtime: bool,
    offered: &[&str],
    explicit: Option<&str>,
    expected: &str,
) {
    let root = tempfile::tempdir().unwrap();
    let worker = modes(offered[0], offered);
    let command = fake(root.path(), "inherit-worker", worker);
    let h = Harness::with_worker_and_parent_options(
        script(),
        Some((command, child_profile)),
        false,
        hennery_host::session::IDLE_TIMEOUT,
        true,
        (
            modes(
                "default",
                &["default", "plan", "bypassPermissions", "read-only", "agent-full-access"],
            ),
            parent_profile,
            Some(parent_mode),
        ),
    )
    .await;
    assert_eq!(
        h.state
            .store
            .find_session(&h.parent)
            .unwrap()
            .unwrap()
            .config
            .mode
            .as_deref(),
        Some(parent_mode)
    );
    let receipt = h
        .ok(
            "session_spawn",
            json!({
                "operation_key":"inherit-mode", "cwd":h.cwd, "brief":"child brief",
                "agent":if same_runtime { "claude" } else { "worker" }, "config":{"mode":explicit}
            }),
        )
        .await;
    let child = receipt["child_id"].as_str().unwrap();
    if !same_runtime {
        let log = std::fs::read_to_string(h.cwd.join("claude-sessions.jsonl")).unwrap();
        let starts = log
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|v| v["method"] == "session/new")
            .count();
        assert_eq!(starts, 1, "mode inheritance must not probe the parent runtime");
    }
    // Report delivery may independently need a parent catalog. Keep its worker
    // stopped until the spawn admission's no-parent-probe assertion is complete.
    hennery_sessions::delegation_worker::after_startup(&h.state);
    h.done(child, "inherit-mode").await;
    let status = h.status(child).await;
    assert_eq!(status["requested"]["mode"], expected);
    assert_eq!(status["actual"]["mode"], expected);
    assert_eq!(status["last_report"], "child-result");
    h.finish().await;
}

#[tokio::test]
async fn omitted_mode_inherits_same_runtime_claude_bypass() {
    inherited_mode_case(
        Profile::Claude,
        "bypassPermissions",
        Profile::Claude,
        true,
        &["default", "bypassPermissions"],
        None,
        "bypassPermissions",
    )
    .await;
}

#[tokio::test]
async fn omitted_mode_maps_claude_bypass_to_codex_full_access() {
    inherited_mode_case(
        Profile::Claude,
        "bypassPermissions",
        Profile::Codex,
        false,
        &["read-only", "agent-full-access"],
        None,
        "agent-full-access",
    )
    .await;
}

#[tokio::test]
async fn omitted_mode_maps_codex_full_access_to_claude_bypass() {
    inherited_mode_case(
        Profile::Codex,
        "agent-full-access",
        Profile::Claude,
        false,
        &["default", "bypassPermissions"],
        None,
        "bypassPermissions",
    )
    .await;
}

#[tokio::test]
async fn other_cross_runtime_modes_and_unverified_profiles_use_default() {
    for (profile, mode) in [
        (Profile::Claude, "default"),
        (Profile::Generic, "bypassPermissions"),
        (Profile::ClaudeOwnCli, "bypassPermissions"),
    ] {
        inherited_mode_case(
            profile,
            mode,
            Profile::Codex,
            false,
            &["read-only", "agent-full-access"],
            None,
            "read-only",
        )
        .await;
    }
    inherited_mode_case(
        Profile::Claude,
        "bypassPermissions",
        Profile::Generic,
        false,
        &["read-only", "agent-full-access"],
        None,
        "read-only",
    )
    .await;
}

#[tokio::test]
async fn explicit_mode_overrides_same_and_cross_runtime_inheritance() {
    inherited_mode_case(
        Profile::Claude,
        "bypassPermissions",
        Profile::Claude,
        true,
        &["default", "bypassPermissions"],
        Some("plan"),
        "plan",
    )
    .await;
    inherited_mode_case(
        Profile::Claude,
        "bypassPermissions",
        Profile::Codex,
        false,
        &["read-only", "agent-full-access"],
        Some("read-only"),
        "read-only",
    )
    .await;
}

#[tokio::test]
async fn inherited_mode_not_offered_uses_runtime_default() {
    inherited_mode_case(
        Profile::Claude,
        "bypassPermissions",
        Profile::Codex,
        false,
        &["read-only"],
        None,
        "read-only",
    )
    .await;
}

#[tokio::test]
async fn omitted_mode_follows_mid_session_operator_downgrade() {
    let h = Harness::with_worker_and_parent_options(
        script(),
        None,
        true,
        hennery_host::session::IDLE_TIMEOUT,
        true,
        (
            modes("default", &["default", "bypassPermissions"]),
            Profile::Claude,
            Some("bypassPermissions"),
        ),
    )
    .await;
    let response = h
        .client
        .post(format!("{}/api/sessions/{}/config", h.url, h.parent))
        .json(&json!({"config_id":"mode","value":"default"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(
        h.state
            .store
            .find_session(&h.parent)
            .unwrap()
            .unwrap()
            .config
            .mode
            .as_deref(),
        Some("default")
    );
    let receipt = h
        .ok(
            "session_spawn",
            json!({"operation_key":"downgraded","cwd":h.cwd,"agent":"claude","brief":"child brief","config":{}}),
        )
        .await;
    let child = receipt["child_id"].as_str().unwrap();
    h.done(child, "downgraded").await;
    assert_eq!(h.status(child).await["actual"]["mode"], "default");
    h.finish().await;
}

#[tokio::test]
async fn park_hold_pin_api_requires_cookie_origin_and_scopes_sessions_without_step_up() {
    let h = Harness::new(script()).await;
    let endpoint = format!("{}/api/sessions/{}/idle-hold", h.url, h.parent);
    let r = h.raw.get(&endpoint).send().await.unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
    let phc = h
        .state
        .operator
        .verify_password(hennery_testkit::OWNER_PASSWORD)
        .unwrap()
        .unwrap();
    let cookie = h
        .state
        .operator
        .open_session("idle-pin-stale", &phc, hennery_kernel::secret::unix_now() - 600)
        .unwrap()
        .unwrap();
    for origin in [None, Some("https://foreign.example")] {
        let mut request = h
            .raw
            .patch(&endpoint)
            .header("cookie", format!("hennery_session={cookie}"))
            .json(&json!({"pinned":true}));
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(request.send().await.unwrap().status(), reqwest::StatusCode::FORBIDDEN);
    }
    let r = h
        .raw
        .patch(&endpoint)
        .header("cookie", format!("hennery_session={cookie}"))
        .header("origin", &h.url)
        .json(&json!({"pinned":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        reqwest::StatusCode::OK,
        "a resource preference requires no step-up"
    );
    assert_eq!(
        r.json::<Value>().await.unwrap(),
        json!({"pinned":true,"family_active":false,"notices_pending":false,"held":true})
    );
    assert!(
        h.client
            .get(&endpoint)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["pinned"]
            .as_bool()
            .unwrap()
    );
    for body in [
        json!({"pinned":"true"}),
        json!({"pinned":true,"owner_id":"foreign"}),
        json!({}),
    ] {
        let r = h.client.patch(&endpoint).json(&body).send().await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    }
    let row = h.state.store.find_session(&h.parent).unwrap().unwrap();
    assert!(
        h.state
            .store
            .create_session(
                "foreign-pin",
                HOST,
                "worker",
                h.cwd.to_str().unwrap(),
                &row.hat_id,
                None
            )
            .unwrap()
    );
    let conn = hennery_kernel::db::open(&h._dir.path().join("hennery.db")).unwrap();
    conn.execute("UPDATE sessions SET owner_id='foreign' WHERE id='foreign-pin'", [])
        .unwrap();
    for id in ["missing", "foreign-pin"] {
        let endpoint = format!("{}/api/sessions/{id}/idle-hold", h.url);
        assert_eq!(
            h.client.get(&endpoint).send().await.unwrap().status(),
            reqwest::StatusCode::NOT_FOUND
        );
        assert_eq!(
            h.client
                .patch(&endpoint)
                .json(&json!({"pinned":true}))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
    let r = h
        .client
        .patch(&endpoint)
        .json(&json!({"pinned":false}))
        .send()
        .await
        .unwrap();
    assert!(!r.json::<Value>().await.unwrap()["held"].as_bool().unwrap());
    h.finish().await;
}

#[tokio::test]
async fn park_hold_real_child_keeps_parent_attached_then_releases_it() {
    let h = Harness::with_worker_idle(
        FakeScript {
            chunk_delay_ms: 6_000,
            ..script()
        },
        None,
        true,
        Duration::from_secs(2),
    )
    .await;
    let endpoint = format!("{}/api/sessions/{}/idle-hold", h.url, h.parent);
    assert_eq!(
        h.client
            .patch(&endpoint)
            .json(&json!({"pinned":true}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let spawn = h.spawn("idle-hold-work", json!({})).await;
    let child = spawn["child_id"].as_str().unwrap();
    eventually("child running", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.activity.as_deref() == Some("running"))
            .map(|_| ())
    })
    .await;
    let state = h
        .client
        .patch(&endpoint)
        .json(&json!({"pinned":false}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(state["family_active"], true);
    assert_eq!(state["held"], true);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        h.state.store.find_session(&h.parent).unwrap().unwrap().lifecycle,
        "active",
        "idle parent survives the window while its child works"
    );
    h.done(child, "idle-hold-work").await;
    eventually("child idle parked", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "parked")
            .map(|_| ())
    })
    .await;
    eventually("parent idle parked after family release", async || {
        h.state
            .store
            .find_session(&h.parent)
            .unwrap()
            .filter(|s| s.lifecycle == "parked")
            .map(|_| ())
    })
    .await;
    assert!(!h.state.store.idle_hold(&h.parent).unwrap().unwrap().held);
    h.finish().await;
}
fn fake(root: &Path, runtime: &str, mut script: FakeScript) -> AgentCommand {
    let home = root.join(format!("{runtime}-home"));
    std::fs::create_dir_all(&home).unwrap();
    script.session_log = Some(root.join(format!("{runtime}-sessions.jsonl")).display().to_string());
    AgentCommand {
        program: env!("CARGO_BIN_EXE_hennery-fake-acp").into(),
        args: vec![],
        env: vec![
            (SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()),
            ("HOME".into(), home.display().to_string()),
            ("XDG_CONFIG_HOME".into(), home.join("config").display().to_string()),
            ("XDG_DATA_HOME".into(), home.join("data").display().to_string()),
            ("XDG_CACHE_HOME".into(), home.join("cache").display().to_string()),
            ("SSL_CERT_FILE".into(), String::new()),
            ("SSL_CERT_DIR".into(), String::new()),
            ("HENNERY_DATA_DIR".into(), String::new()),
            ("HENNERY_HOST_DATA_DIR".into(), String::new()),
        ],
    }
}
async fn eventually<T>(what: &str, mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    tokio::time::timeout(BOUND, async {
        loop {
            if let Some(result) = probe().await {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out: {what}"))
}
impl Harness {
    async fn new(worker: FakeScript) -> Self {
        Self::with_worker(worker, None, true).await
    }
    async fn with_worker(worker: FakeScript, pinned: Option<(AgentCommand, Profile)>, dispatch: bool) -> Self {
        Self::with_worker_idle(worker, pinned, dispatch, hennery_host::session::IDLE_TIMEOUT).await
    }
    async fn with_worker_idle(
        worker: FakeScript,
        pinned: Option<(AgentCommand, Profile)>,
        dispatch: bool,
        idle: Duration,
    ) -> Self {
        Self::with_worker_options(worker, pinned, dispatch, idle, true).await
    }
    async fn with_worker_options(
        worker: FakeScript,
        pinned: Option<(AgentCommand, Profile)>,
        dispatch: bool,
        idle: Duration,
        workspace_roots: bool,
    ) -> Self {
        Self::with_worker_and_parent_options(
            worker,
            pinned,
            dispatch,
            idle,
            workspace_roots,
            (script(), Profile::Claude, None),
        )
        .await
    }
    async fn with_worker_and_parent_options(
        worker: FakeScript,
        mut pinned: Option<(AgentCommand, Profile)>,
        dispatch: bool,
        idle: Duration,
        workspace_roots: bool,
        parent: (FakeScript, Profile, Option<&str>),
    ) -> Self {
        let (parent_script, parent_profile, parent_mode) = parent;
        hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery-fake-acp"));
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let db = cwd.join("hennery.db");
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let key = HostKey::from_seed([9; 32]);
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                HOST,
                &Enrollment {
                    public_key: key.public_key_hex(),
                    name: "fixture".into(),
                    host_version: "fixture".into(),
                    platform: "fixture".into(),
                },
                0,
            )
            .unwrap();
        let hat = hosts.host(HOST).unwrap().unwrap().default_hat_id;
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        let url = format!("http://{addr}");
        let now = hennery_kernel::secret::unix_now();
        let setup = state.operator.issue_setup_token(now).unwrap().unwrap();
        state
            .operator
            .set_up(&setup, hennery_testkit::OWNER_PASSWORD, &url, now)
            .unwrap();
        let phc = state
            .operator
            .verify_password(hennery_testkit::OWNER_PASSWORD)
            .unwrap()
            .unwrap();
        let session = state.operator.open_session("integration", &phc, now).unwrap().unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::ORIGIN, url.parse().unwrap());
        headers.insert(
            reqwest::header::COOKIE,
            format!("hennery_session={session}").parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .default_headers(headers)
            .build()
            .unwrap();
        let keys = KeySource::from_vars(&cwd, Some("09".repeat(32).into()), None).unwrap();
        let gateway: GatewayState = hennery_gateway::open(
            &db,
            &keys,
            state.operator.clone(),
            Egress::new(Timeouts::DEFAULT).unwrap(),
            Arc::new(hennery_gateway::notify::Silent),
        )
        .unwrap();
        state
            .store
            .set_session_mcp(Arc::new(GatewayMcp::for_collector(&gateway)));
        gateway.runtime.store.set_session_tools(&hat, true).unwrap();
        let discovery: Arc<Mutex<Vec<Value>>> = Arc::default();
        let observed = discovery.clone();
        let trace_auth = gateway.runtime.store.clone();
        let builtin = hennery_sessions::builtin::router(state.clone(), gateway).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = observed.clone();
                let trace_auth = trace_auth.clone();
                async move {
                    let caller_session = request.headers().get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.strip_prefix("Bearer "))
                        .and_then(|token| trace_auth.authenticate_session(token).unwrap())
                        .map(|identity| {
                            let hennery_gateway::scope::PrincipalKind::Session { session_id, .. } = identity.principal.kind;
                            session_id
                        });
                    let (parts, body) = request.into_parts();
                    let bytes = axum::body::to_bytes(body, hennery_gateway::proxy::MAX_REQUEST_BODY).await.unwrap();
                    let message: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let response = next.run(axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes))).await;
                    if matches!(message["method"].as_str(), Some("initialize" | "tools/list")) {
                        let (parts, body) = response.into_parts();
                        let bytes = axum::body::to_bytes(body, 1024*1024).await.unwrap();
                        let answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                        observed.lock().unwrap().push(json!({"caller_session":caller_session,"method":message["method"],"status":parts.status.as_u16(),"tool_count":answer["result"]["tools"].as_array().map(Vec::len)}));
                        axum::response::Response::from_parts(parts, axum::body::Body::from(bytes))
                    } else { response }
                }
            }
        ));
        let router = hennery_sessions::router(state.clone()).merge(builtin);
        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        if dispatch {
            hennery_sessions::delegation_worker::after_startup(&state);
        }
        let mut cfg = HostConfig::new(format!("ws://{addr}/api/hosts/ws"), HOST, key, cwd.join("host-data"));
        cfg.idle_timeout = idle;
        cfg.workspace_roots = if workspace_roots { vec![cwd.clone()] } else { vec![] };
        cfg.git = None;
        for (runtime, s) in [("claude", parent_script), ("worker", worker)] {
            let (command, profile) = if runtime == "worker" {
                pinned
                    .take()
                    .unwrap_or_else(|| (fake(&cwd, runtime, s), Profile::Claude))
            } else {
                (fake(&cwd, runtime, s), parent_profile)
            };
            cfg.agents.insert(runtime.into(), command);
            cfg.profiles.insert(runtime.into(), profile);
        }
        let (shutdown, signal) = tokio::sync::oneshot::channel();
        let host_task = tokio::spawn(hennery_host::run_until(cfg, async {
            let _ = signal.await;
        }));
        eventually("ready host", async || state.hub.is_ready(HOST).then_some(())).await;
        let response = client
            .post(format!("{url}/api/sessions"))
            .json(&json!({"host_id":HOST,"agent":"claude","cwd":cwd,"mode":parent_mode}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{body}");
        let parent = body["session_id"].as_str().unwrap().to_owned();
        eventually("active parent", async || {
            state
                .store
                .find_session(&parent)
                .unwrap()
                .filter(|s| s.lifecycle == "active")
                .map(|_| ())
        })
        .await;
        let log = std::fs::read_to_string(cwd.join("claude-sessions.jsonl")).unwrap();
        let servers: Value =
            serde_json::from_str::<Value>(log.lines().last().unwrap()).unwrap()["params"]["mcpServers"].clone();
        let token = servers[0]["headers"][0]["value"]
            .as_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap()
            .to_owned();
        Self {
            _dir: dir,
            state,
            client,
            raw: reqwest::Client::builder().no_proxy().build().unwrap(),
            url,
            parent,
            token,
            cwd,
            shutdown: Some(shutdown),
            host_task,
            server_task,
            discovery,
        }
    }
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let response = self
            .raw
            .post(format!("{}/mcp/builtin/hennery", self.url))
            .bearer_auth(&self.token)
            .header("accept", "application/json")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.json::<Value>().await.unwrap()["result"].clone()
    }
    async fn call(&self, name: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({"name":name,"arguments":args})).await
    }
    async fn ok(&self, name: &str, args: Value) -> Value {
        let r = self.call(name, args).await;
        assert_eq!(r["isError"], false, "{name}: {r}");
        r["structuredContent"].clone()
    }
    async fn spawn(&self, key: &str, config: Value) -> Value {
        self.ok(
            "session_spawn",
            json!({"operation_key":key,"cwd":self.cwd,"agent":"worker","brief":"child brief","config":config}),
        )
        .await
    }
    async fn status(&self, child: &str) -> Value {
        self.ok("session_status", json!({"child_id":child})).await
    }
    async fn done(&self, child: &str, key: &str) {
        eventually("completed operation", async || {
            let s = self
                .ok("session_status", json!({"child_id":child,"operation_key":key}))
                .await;
            s["operations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["state"]["state"] == "completed" && r["child_id"] == child)
                .then_some(())
        })
        .await;
    }
    async fn post(&self, path: &str, body: Value) -> (reqwest::StatusCode, Value) {
        let r = self
            .client
            .post(format!("{}{path}", self.url))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status();
        (status, r.json().await.unwrap_or(Value::Null))
    }
    async fn finish(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.send(()).unwrap();
        }
        tokio::time::timeout(BOUND, &mut self.host_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.state.shutdown.cancel();
    }
}

#[tokio::test]
async fn removed_execution_policy_routes_are_not_found() {
    let h = Harness::with_worker(script(), None, false).await;
    let hat = h.state.store.find_session(&h.parent).unwrap().unwrap().hat_id;
    let endpoint = format!("{}/api/hats/{hat}/delegation-policy", h.url);
    assert_eq!(
        h.client.get(&endpoint).send().await.unwrap().status(),
        reqwest::StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.client
            .patch(&endpoint)
            .json(&json!({"allowed":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    h.finish().await;
}

#[tokio::test]
async fn seven_handlers_share_launcher_catalog_and_choose_runtime_and_model_without_substitute() {
    let h = Harness::new(script()).await;
    let tools = h.rpc("tools/list", json!({})).await;
    assert_eq!(tools["tools"].as_array().unwrap().len(), 7);
    let caps = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    assert_eq!(caps["runtimes"], json!(["claude", "worker"]));
    assert_eq!(caps["catalogs"][0]["defaults"]["model"], "small");
    let catalog = h
        .client
        .post(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap();
    assert_eq!(catalog.status(), reqwest::StatusCode::OK);
    let mut operator_catalog = catalog.json::<Value>().await.unwrap();
    assert!(operator_catalog["security"]["known"].as_bool().unwrap());
    assert!(operator_catalog.get("profile").is_none());
    assert!(caps["catalogs"][0].get("security").is_none());
    assert!(caps["catalogs"][0].get("profile").is_none());
    operator_catalog.as_object_mut().unwrap().remove("security");
    assert!(
        operator_catalog
            .as_object_mut()
            .unwrap()
            .remove("saved_at")
            .unwrap()
            .is_number()
    );
    assert_eq!(operator_catalog, caps["catalogs"][0]);
    let receipt = h.spawn("spawn-large", json!({"model":"large"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    h.done(child, "spawn-large").await;
    let s = h.status(child).await;
    assert_eq!(s["requested"]["model"], "large");
    assert_eq!(s["actual"]["model"], "large");
    assert_eq!(h.state.store.find_session(child).unwrap().unwrap().agent, "worker");
    let again = h.spawn("spawn-large", json!({"model":"large"})).await;
    assert_eq!(again["child_id"], child);
    let conflict = h
        .call("session_close", json!({"operation_key":"spawn-large","child_id":child}))
        .await;
    assert_eq!(conflict["structuredContent"]["code"], "operation_conflict");
    for (agent, model, code) in [
        ("absent", "small", "runtime_unavailable"),
        ("worker", "absent", "configuration_unavailable"),
    ] {
        let r=h.call("session_spawn",json!({"operation_key":format!("invalid-{agent}-{model}"),"cwd":h.cwd,"agent":agent,"brief":"never substitute","config":{"model":model}})).await;
        assert_eq!(r["structuredContent"]["code"], code);
    }
    let list = h.ok("session_list", json!({})).await;
    assert_eq!(list["children"].as_array().unwrap().len(), 1);
    h.finish().await;
}

#[tokio::test]
async fn durable_question_answer_and_report_complete_the_automatic_parent_child_loop() {
    let mut worker = script();
    worker.asks = vec![FakeAsk::Elicitation];
    let h = Harness::new(worker).await;
    let receipt = h.spawn("delivery-loop", json!({"model":"large"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    let question = eventually(
        "ordinary child question automatically delivered to parent",
        async || {
            h.state.store.events(&h.parent, 0, 100).unwrap().into_iter().find(|e| {
                e.kind == "user_turn"
                    && e.body["origin"]["kind"] == "child_report"
                    && e.body["content"][0]["text"]
                        .as_str()
                        .is_some_and(|s| s.contains("\"kind\":\"question\""))
            })
        },
    )
    .await;
    assert_eq!(question.body["origin"]["child_id"], child);
    let pending = h.state.store.open_pending(child).unwrap().pop().unwrap();
    h.ok("session_answer",json!({"operation_key":"loop-answer","child_id":child,"pending_id":pending.pending_id,"action":"accept","content":{"name":"parent"}})).await;
    h.done(child, "delivery-loop").await;
    let report = eventually("child completion automatically delivered to parent", async || {
        h.state.store.events(&h.parent, 0, 100).unwrap().into_iter().find(|e| {
            e.kind == "user_turn"
                && e.body["origin"]["kind"] == "child_report"
                && e.body["content"][0]["text"]
                    .as_str()
                    .is_some_and(|s| s.contains("\"kind\":\"turn_ended\""))
        })
    })
    .await;
    assert!(
        report.body["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("child-result")
    );
    assert_ne!(
        question.body["origin"]["delivery_id"],
        report.body["origin"]["delivery_id"]
    );
    assert_eq!(
        h.state
            .store
            .pending_item(&pending.pending_id)
            .unwrap()
            .unwrap()
            .delivered,
        Some(true)
    );
    h.finish().await;
}

#[tokio::test]
async fn child_completion_resumes_a_genuinely_parked_parent_and_delivers_its_report() {
    let mut worker = script();
    worker.asks = vec![FakeAsk::Elicitation];
    let h = Harness::new(worker).await;
    let receipt = h.spawn("resume-report", json!({"model":"large"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    let pending = eventually("child asks", async || h.state.store.open_pending(child).unwrap().pop()).await;
    eventually("parent handled its question notice", async || {
        let s = h.state.store.find_session(&h.parent).unwrap().unwrap();
        let received = h
            .state
            .store
            .events(&h.parent, 0, 100)
            .unwrap()
            .iter()
            .any(|e| e.kind == "user_turn" && e.body["origin"]["kind"] == "child_report");
        (received && s.open_turn_id.is_none()).then_some(())
    })
    .await;
    let (status, body) = h.post(&format!("/api/sessions/{}/park", h.parent), json!({})).await;
    assert!(status.is_success(), "{body}");
    eventually("parent genuinely parked", async || {
        h.state
            .store
            .find_session(&h.parent)
            .unwrap()
            .filter(|s| s.lifecycle == "parked" && !s.presumed_parked)
    })
    .await;
    let (status, body) = h
        .post(
            &format!("/api/sessions/{child}/pending/{}/answer", pending.pending_id),
            json!({"action":"accept","content":{"name":"operator"}}),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{body}");
    eventually("completion resumes parent and delivers report", async || {
        h.state.store.events(&h.parent, 0, 100).unwrap().into_iter().find(|e| {
            e.kind == "user_turn"
                && e.body["origin"]["kind"] == "child_report"
                && e.body["content"][0]["text"]
                    .as_str()
                    .is_some_and(|s| s.contains("\"kind\":\"turn_ended\""))
        })
    })
    .await;
    let log = std::fs::read_to_string(h.cwd.join("claude-sessions.jsonl")).unwrap();
    assert!(
        log.lines()
            .any(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == "session/load")
    );
    h.finish().await;
}

#[tokio::test]
async fn initial_brief_is_refused_when_authoritative_model_readback_differs() {
    let mut s = script();
    s.sticky_options = vec!["model".into()];
    let h = Harness::new(s).await;
    let receipt = h.spawn("held", json!({"model":"large"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    eventually("terminal configuration mismatch", async || {
        let s = h.status(child).await;
        (s["operations"][0]["state"]["state"] == "failed"
            && s["operations"][0]["state"]["code"] == "configuration_unavailable")
            .then_some(())
    })
    .await;
    // A later worker pass must not release the rejected brief.
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    let state = h.status(child).await;
    assert_eq!(state["requested"]["model"], "large");
    assert_eq!(state["summary"]["lifecycle"], "failed");
    assert_eq!(state["summary"]["queued"], 0);
    let events = h.state.store.events(child, 0, 1000).unwrap();
    assert!(events.iter().any(|event| {
        event.kind == "start_failed"
            && event.body["code"] == "configuration_unavailable"
            && event.body["message"]
                .as_str()
                .is_some_and(|text| text.contains("large") && text.contains("small"))
    }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind.as_str(), "user_turn" | "turn_started"))
    );
    assert!(state["last_outcome"].is_null());
    assert!(state["last_report"].is_null());
    assert!(
        h.state
            .store
            .find_session(child)
            .unwrap()
            .unwrap()
            .open_turn_id
            .is_none()
    );
    h.finish().await;
}

#[tokio::test]
async fn direct_children_only_and_close_does_not_cascade_or_reopen() {
    let h = Harness::new(script()).await;
    let a = h.spawn("a", json!({})).await;
    let a = a["child_id"].as_str().unwrap();
    h.done(a, "a").await;
    let b = h.spawn("b", json!({})).await;
    let b = b["child_id"].as_str().unwrap();
    h.done(b, "b").await;
    let (http, independent) = h
        .post("/api/sessions", json!({"host_id":HOST,"agent":"claude","cwd":h.cwd}))
        .await;
    assert_eq!(http, reqwest::StatusCode::ACCEPTED);
    let independent = independent["session_id"].as_str().unwrap();
    eventually("independent parent active", async || {
        h.state
            .store
            .find_session(independent)
            .unwrap()
            .filter(|s| s.lifecycle == "active")
            .map(|_| ())
    })
    .await;
    let independent_log = std::fs::read_to_string(h.cwd.join("claude-sessions.jsonl")).unwrap();
    let independent_session: Value = serde_json::from_str(independent_log.lines().last().unwrap()).unwrap();
    let independent_token = independent_session["params"]["mcpServers"][0]["headers"][0]["value"]
        .as_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap();
    let separate=h.raw.post(format!("{}/mcp/builtin/hennery",h.url)).bearer_auth(independent_token).header("accept","application/json")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_spawn","arguments":{"operation_key":"other-family","cwd":h.cwd,"agent":"worker","brief":"independent family","config":{}}}})).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(separate["result"]["isError"], false, "{separate}");
    let separate = separate["result"]["structuredContent"]["child_id"].as_str().unwrap();
    let hidden_child = h.call("session_status", json!({"child_id":separate})).await;
    assert_eq!(hidden_child["structuredContent"]["code"], "not_found");
    let hidden = h.call("session_status", json!({"child_id":independent})).await;
    assert_eq!(hidden["structuredContent"]["code"], "not_found");
    let log = std::fs::read_to_string(h.cwd.join("worker-sessions.jsonl")).unwrap();
    let child_token = log
        .lines()
        .rev()
        .find_map(|line| {
            let line: Value = serde_json::from_str(line).unwrap();
            line["params"]["mcpServers"][0]["headers"][0]["value"]
                .as_str()
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::to_owned)
        })
        .unwrap();
    let nested=h.raw.post(format!("{}/mcp/builtin/hennery",h.url)).bearer_auth(child_token).header("accept","application/json")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_spawn","arguments":{"operation_key":"grandchild","cwd":h.cwd,"brief":"never nest","config":{}}}})).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(nested["result"]["structuredContent"]["code"], "generation_limit");
    let unrelated = h.call("session_status", json!({"child_id":h.parent})).await;
    assert_eq!(unrelated["structuredContent"]["code"], "not_found");
    let close = h
        .ok("session_close", json!({"operation_key":"close-a","child_id":a}))
        .await;
    assert_eq!(close["child_id"], a);
    eventually("closed direct child", async || {
        h.state
            .store
            .find_session(a)
            .unwrap()
            .filter(|s| s.lifecycle == "closed")
            .map(|_| ())
    })
    .await;
    assert_eq!(h.state.store.find_session(b).unwrap().unwrap().lifecycle, "active");
    assert_eq!(
        h.state.store.find_session(&h.parent).unwrap().unwrap().lifecycle,
        "active"
    );
    let send = h
        .call(
            "session_send",
            json!({"operation_key":"no-reopen","child_id":a,"text":"do not reopen"}),
        )
        .await;
    assert_eq!(send["isError"], true);
    let full = h.ok("session_list", json!({"limit":100})).await;
    let ids: std::collections::BTreeSet<_> = full["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["child_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [a, b].into_iter().collect());
    let page = h.ok("session_list", json!({"limit":1})).await;
    assert_eq!(page["children"].as_array().unwrap().len(), 1);
    let cursor = page["next_cursor"].as_str().unwrap();
    let foreign_cursor=h.raw.post(format!("{}/mcp/builtin/hennery",h.url)).bearer_auth(independent_token).header("accept","application/json")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_list","arguments":{"cursor":cursor,"limit":1}}})).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(foreign_cursor["result"]["structuredContent"]["code"], "invalid_cursor");
    let rest = h.ok("session_list", json!({"cursor":cursor,"limit":1})).await;
    assert_eq!(rest["children"].as_array().unwrap().len(), 1);
    h.finish().await;
}

#[tokio::test]
async fn busy_send_is_durable_and_parked_followup_resumes_with_fresh_readback() {
    let mut s = script();
    s.asks = vec![FakeAsk::Elicitation];
    let h = Harness::new(s).await;
    let receipt = h.spawn("busy-start", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    let pending = eventually("child elicitation", async || {
        h.state
            .store
            .open_pending(child)
            .unwrap()
            .into_iter()
            .find(|p| !p.answered)
    })
    .await;
    let first_turn = h
        .state
        .store
        .find_session(child)
        .unwrap()
        .unwrap()
        .open_turn_id
        .unwrap();
    let follow = h
        .ok(
            "session_send",
            json!({"operation_key":"busy-follow","child_id":child,"text":"follow up"}),
        )
        .await;
    assert_eq!(follow["state"]["state"], "queued");
    assert_eq!(
        h.state
            .store
            .find_session(child)
            .unwrap()
            .unwrap()
            .open_turn_id
            .as_deref(),
        Some(first_turn.as_str())
    );
    let queued = h.status(child).await;
    assert_eq!(queued["summary"]["queued"], 1);
    h.ok("session_answer",json!({"operation_key":"release-first","child_id":child,"pending_id":pending.pending_id,"action":"accept","content":{"name":"first"}})).await;
    eventually("follow-up has its own turn", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .unwrap()
            .open_turn_id
            .filter(|turn| turn != &first_turn)
    })
    .await;
    h.finish().await;

    let h = Harness::new(script()).await;
    let receipt = h.spawn("park-start", json!({"model":"large"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    h.done(child, "park-start").await;
    let (status, body) = h.post(&format!("/api/sessions/{child}/park"), json!({})).await;
    assert!(status.is_success(), "{body}");
    eventually("genuinely parked child", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "parked" && !s.presumed_parked)
            .map(|_| ())
    })
    .await;
    h.ok(
        "session_send",
        json!({"operation_key":"resume-follow","child_id":child,"text":"continue after parking"}),
    )
    .await;
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    assert_eq!(h.state.store.find_session(child).unwrap().unwrap().lifecycle, "parked");
    let held = h
        .ok(
            "session_status",
            json!({"child_id":child,"operation_key":"resume-follow"}),
        )
        .await;
    assert_eq!(
        held["operations"][0]["state"]["state"], "queued",
        "operator Park holds delegated follow-ups too"
    );
    let (status, body) = h.post(&format!("/api/sessions/{child}/resume"), json!({})).await;
    assert!(status.is_success(), "{body}");
    h.done(child, "resume-follow").await;
    assert_eq!(h.status(child).await["actual"]["model"], "large");
    let log = std::fs::read_to_string(h.cwd.join("worker-sessions.jsonl")).unwrap();
    assert!(
        log.lines()
            .any(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == "session/load")
    );
    h.finish().await;
}

#[tokio::test]
async fn idle_parked_delegated_followup_auto_resumes_with_fresh_model_readback() {
    let h = Harness::with_worker_idle(script(), None, true, Duration::from_secs(1)).await;
    // Keep the caller attached while the child parks through the real idle reaper.
    let pinned = h
        .client
        .patch(format!("{}/api/sessions/{}/idle-hold", h.url, h.parent))
        .json(&json!({"pinned":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(pinned.status(), reqwest::StatusCode::OK);
    let receipt = h.spawn("idle-park-start", json!({"model":"large"})).await;
    let child = receipt["child_id"].as_str().unwrap();
    h.done(child, "idle-park-start").await;
    eventually("child idle parked", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "parked" && !s.presumed_parked)
            .map(|_| ())
    })
    .await;
    assert!(
        h.state
            .store
            .events(child, 0, 1000)
            .unwrap()
            .iter()
            .any(|event| event.kind == "session_parked" && event.body["reason"] == "idle")
    );
    h.ok(
        "session_send",
        json!({"operation_key":"idle-resume-follow","child_id":child,"text":"continue after idle parking"}),
    )
    .await;
    h.done(child, "idle-resume-follow").await;
    assert_eq!(h.status(child).await["actual"]["model"], "large");
    let log = std::fs::read_to_string(h.cwd.join("worker-sessions.jsonl")).unwrap();
    assert!(
        log.lines()
            .any(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == "session/load")
    );
    h.finish().await;
}

#[tokio::test]
async fn permission_questions_remain_operator_only_and_elicitation_has_one_answer_winner() {
    use hennery_proto::frames::PendingKind;
    let mut s = script();
    s.asks = vec![FakeAsk::Permission, FakeAsk::Elicitation];
    s.asks_at_once = true;
    let h = Harness::new(s).await;
    let receipt = h.spawn("questions", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    let pending = eventually("both canonical pending requests", async || {
        let p = h.state.store.open_pending(child).unwrap();
        (p.len() == 2).then_some(p)
    })
    .await;
    let permission = pending.iter().find(|p| p.kind == PendingKind::Permission).unwrap();
    let elicitation = pending.iter().find(|p| p.kind == PendingKind::Elicitation).unwrap();
    let status = h.status(child).await;
    let permission_card = status["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["pending_id"] == permission.pending_id)
        .unwrap();
    assert_eq!(permission_card["operator_required"], true);
    assert!(permission_card["question"].is_null());
    let refused=h.call("session_answer",json!({"operation_key":"not-permission","child_id":child,"pending_id":permission.pending_id,"action":"accept","content":{}})).await;
    assert_eq!(refused["structuredContent"]["code"], "operator_required");
    assert!(
        !h.state
            .store
            .pending_item(&permission.pending_id)
            .unwrap()
            .unwrap()
            .answered
    );
    let elicitation_path = format!("/api/sessions/{child}/pending/{}/answer", elicitation.pending_id);
    let (parent,operator)=tokio::join!(
        h.call("session_answer",json!({"operation_key":"parent-answer","child_id":child,"pending_id":elicitation.pending_id,"action":"accept","content":{"name":"parent"}})),
        h.post(&elicitation_path,json!({"action":"accept","content":{"name":"operator"}}))
    );
    let parent_won = parent["isError"] == false;
    let operator_won = operator.0 == reqwest::StatusCode::ACCEPTED;
    assert_ne!(parent_won, operator_won, "parent:{parent}, operator:{operator:?}");
    assert!(
        h.state
            .store
            .pending_item(&elicitation.pending_id)
            .unwrap()
            .unwrap()
            .answered
    );
    let (status, body) = h
        .post(
            &format!("/api/sessions/{child}/pending/{}/answer", permission.pending_id),
            json!({"option_id":"allow"}),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{body}");
    eventually("both answers delivered", async || {
        let permission = h.state.store.pending_item(&permission.pending_id).unwrap().unwrap();
        let elicitation = h.state.store.pending_item(&elicitation.pending_id).unwrap().unwrap();
        (permission.delivered == Some(true) && elicitation.delivered == Some(true)).then_some(())
    })
    .await;
    h.done(child, "questions").await;
    h.finish().await;
}

#[tokio::test]
async fn delegated_parked_close_is_attributed_and_reaches_existing_dashboard_stream() {
    let h = Harness::with_worker(script(), None, false).await;
    let receipt = h.spawn("park-close", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    eventually("completed child before parking", async || {
        hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
        (h.status(child).await["last_outcome"] == "completed").then_some(())
    })
    .await;
    let (status, body) = h.post(&format!("/api/sessions/{child}/park"), json!({})).await;
    assert!(status.is_success(), "{body}");
    eventually("idle genuinely parked child", async || {
        let session = h.state.store.find_session(child).unwrap().unwrap();
        (session.lifecycle == "parked" && !session.presumed_parked && session.open_turn_id.is_none()).then_some(())
    })
    .await;
    let page: Value = h
        .client
        .get(format!("{}/api/view/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut dashboard = LiveSse::open(
        &h,
        &format!(
            "/api/stream/sessions?after={}:{}",
            page["epoch"].as_str().unwrap(),
            page["revision"].as_i64().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(dashboard.next().await.0, "waiting_changed");
    let before = h.state.store.max_event_id().unwrap();
    let arguments = json!({"operation_key":"close-parked","child_id":child});
    let closed = h.ok("session_close", arguments.clone()).await;
    let events = h.state.store.events(child, before, 100).unwrap();
    let close: Vec<_> = events.iter().filter(|event| event.kind == "operator_closed").collect();
    assert_eq!(close.len(), 1, "one durable close marker for an idle parked child");
    assert_eq!(
        close[0].body["origin"],
        json!({"kind":"parent","session_id":h.parent,"operation_id":closed["operation_id"]})
    );
    let summary = dashboard
        .matching("session_upsert", |summary| {
            summary["session_id"] == child && summary["lifecycle"] == "closed"
        })
        .await;
    assert_eq!(summary["question_waits"], false);
    h.ok("session_close", arguments).await;
    assert_eq!(h.state.store.events(child, before, 100).unwrap().len(), events.len());
    h.finish().await;
}

#[tokio::test]
async fn queued_parent_answer_reaches_existing_session_stream_before_dispatch() {
    let mut worker = script();
    worker.asks = vec![FakeAsk::Elicitation];
    let h = Harness::with_worker(worker, None, false).await;
    let receipt = h.spawn("answer-stream", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    let pending = eventually("ordinary child question", async || {
        hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
        h.state.store.open_pending(child).unwrap().into_iter().next()
    })
    .await;
    let last = h.state.store.events(child, 0, 100).unwrap().last().unwrap().event_id;
    let mut stream = LiveSse::open(&h, &format!("/api/stream/sessions/{child}"), Some(last - 1)).await;
    stream.matching("event", |event| event["event_id"] == last).await;
    let mut published = h.state.hub.subscribe();
    let arguments = json!({"operation_key":"answer-live","child_id":child,"pending_id":pending.pending_id,"action":"accept","content":{"name":"parent"}});
    let accepted = h.ok("session_answer", arguments.clone()).await;
    let event = stream
        .matching("event", |event| event["kind"] == "answer_submitted")
        .await;
    assert_eq!(
        event["body"]["origin"],
        json!({"kind":"parent","session_id":h.parent,"operation_id":accepted["operation_id"]})
    );
    let changed = stream
        .matching("pending_changed", |item| item["pending_id"] == pending.pending_id)
        .await;
    assert_eq!(changed["answered"], true);
    assert!(changed["delivered"].is_null(), "publication precedes host handoff");
    let committed = h.state.store.events(child, last, 100).unwrap();
    assert_eq!(committed.len(), 1);
    let child_events: Vec<_> = std::iter::from_fn(|| published.try_recv().ok())
        .filter(|event| event.session_id == child)
        .collect();
    assert_eq!(
        child_events, committed,
        "the child publishes exactly its committed answer; parent reports have their own stream"
    );
    h.ok("session_answer", arguments).await;
    assert!(
        !std::iter::from_fn(|| published.try_recv().ok()).any(|event| event.session_id == child),
        "retry must not republish the child's answer"
    );
    h.finish().await;
}

/// Local acceptance only: adapters are supplied explicitly, never installed by
/// the test. The dispatch worker is disabled and drained exactly once to attach
/// the accepted child; its saved brief is never submitted to the adapter.
#[tokio::test]
#[ignore = "requires explicitly supplied pinned adapter; starts no provider turn"]
async fn pinned_adapter_discovers_seven_tools_and_attaches_one_child_without_inference() {
    let adapters = PathBuf::from(std::env::var("HENNERY_D2_PINNED_ADAPTER_ROOT").expect("pinned adapter root"));
    let runtime = std::env::var("HENNERY_D2_RUNTIME").unwrap_or_else(|_| "claude".into());
    assert!(matches!(runtime.as_str(), "claude" | "codex"));
    let node = std::env::var("HENNERY_D2_NODE").unwrap_or_else(|_| {
        std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("node"))
            .find(|file| file.is_file())
            .expect("node in dev shell")
            .display()
            .to_string()
    });
    hennery_testkit::first_exec(&node);
    let home = tempfile::tempdir().unwrap();
    for leaf in ["config", "data", "cache", "claude", "codex", "codex-sqlite"] {
        std::fs::create_dir(home.path().join(leaf)).unwrap();
    }
    let entry = if runtime == "codex" {
        "@agentclientprotocol/codex-acp/dist/index.js"
    } else {
        "@agentclientprotocol/claude-agent-acp/dist/index.js"
    };
    let command = AgentCommand {
        program: node,
        args: vec![adapters.join(entry).display().to_string()],
        env: vec![
            ("HOME".into(), home.path().display().to_string()),
            (
                "XDG_CONFIG_HOME".into(),
                home.path().join("config").display().to_string(),
            ),
            ("XDG_DATA_HOME".into(), home.path().join("data").display().to_string()),
            ("XDG_CACHE_HOME".into(), home.path().join("cache").display().to_string()),
            (
                "CLAUDE_CONFIG_DIR".into(),
                home.path().join("claude").display().to_string(),
            ),
            ("CODEX_HOME".into(), home.path().join("codex").display().to_string()),
            (
                "CODEX_SQLITE_HOME".into(),
                home.path().join("codex-sqlite").display().to_string(),
            ),
            ("SSL_CERT_FILE".into(), String::new()),
            ("SSL_CERT_DIR".into(), String::new()),
            ("CLAUDECODE".into(), String::new()),
        ],
    };
    let mut command = command;
    let profile = if runtime == "codex" {
        std::fs::write(
            home.path().join("codex/auth.json"),
            serde_json::to_vec(&json!({"OPENAI_API_KEY":"sk-hennery-discovery-fixture"})).unwrap(),
        )
        .unwrap();
        let binary = adapters.join(
            ".pnpm/@openai+codex@0.155.1-darwin-arm64/node_modules/@openai/codex/vendor/aarch64-apple-darwin/bin/codex",
        );
        hennery_testkit::first_exec(binary.to_str().unwrap());
        command.env.push(("CODEX_PATH".into(), binary.display().to_string()));
        command
            .env
            .push(("OPENAI_API_KEY".into(), "sk-hennery-discovery-fixture".into()));
        Profile::Codex
    } else {
        Profile::Claude
    };
    let h = Harness::with_worker(script(), Some((command, profile)), false).await;
    let caps = h.ok("session_capabilities", json!({"agent":"worker"})).await;
    assert_eq!(caps["catalogs"][0]["freshness"], "fresh");
    assert_eq!(caps["catalogs"][0]["agent"], "worker");
    assert!(caps["catalogs"][0].get("security").is_none());
    let config = if runtime == "codex" {
        json!({"mode":"read-only"})
    } else {
        json!({})
    };
    let receipt = h.spawn("pinned-startup", config).await;
    let child = receipt["child_id"].as_str().unwrap();
    // This pass sends only DelegatedAttach. With no periodic worker running,
    // no pass can dispatch the brief after SessionStarted/config evidence.
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    eventually("pinned child active after startup only", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "active")
            .map(|_| ())
    })
    .await;
    let delivery = h.state.store.mcp_delivery(child).unwrap().unwrap();
    assert_eq!(delivery.servers, 1);
    assert_eq!(
        delivery.mode,
        if runtime == "codex" {
            hennery_proto::rest::McpSessionDeliveryMode::Unisolated
        } else {
            hennery_proto::rest::McpSessionDeliveryMode::Isolated
        }
    );
    assert_eq!(
        h.state.hub.mcp_isolation(HOST, "worker").unwrap().1,
        profile.mcp_isolation()
    );
    let trace = eventually("pinned MCP seven-tool discovery", async || {
        let trace: Vec<_> = h
            .discovery
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t["caller_session"] == child)
            .cloned()
            .collect();
        trace
            .iter()
            .any(|t| t["method"] == "tools/list" && t["status"] == 200 && t["tool_count"] == 7)
            .then_some(trace)
    })
    .await;
    assert!(trace.iter().any(|t| t["method"] == "initialize" && t["status"] == 200));
    assert!(
        h.state
            .store
            .find_session(child)
            .unwrap()
            .unwrap()
            .open_turn_id
            .is_none()
    );
    let status = h.status(child).await;
    assert!(status["last_outcome"].is_null());
    assert!(status["last_report"].is_null());
    println!(
        "{}",
        json!({"runtime":runtime,"child_id":child,"child_attached":true,"brief_dispatched":false,"catalog_agent":caps["catalogs"][0]["agent"],"catalog_freshness":caps["catalogs"][0]["freshness"],"mcp_delivery":delivery,"mcp_isolation":profile.mcp_isolation(),"mcp":trace})
    );
    h.finish().await;
}

// Probe adapters receive no MCP entries; attached children receive the built-in
// entry. This wrapper removes only the disposable probe's catalog response,
// leaving the real fake ACP process and attached-child readback unchanged.
const COLD_CATALOG_ADAPTER: &str = r#"
const {spawn}=require('node:child_process');
const readline=require('node:readline');
const child=spawn(process.argv[2],[],{env:process.env,stdio:['pipe','pipe','inherit']});
const probes=new Set();
readline.createInterface({input:process.stdin}).on('line',line=>{
 const request=JSON.parse(line);
 if(request.method==='session/new' && request.params.mcpServers.length===0)probes.add(JSON.stringify(request.id));
 child.stdin.write(line+'\n');
}).on('close',()=>child.stdin.end());
readline.createInterface({input:child.stdout}).on('line',line=>{
 const response=JSON.parse(line);
 if(probes.has(JSON.stringify(response.id)) && response.result)response.result.configOptions=[];
 process.stdout.write(JSON.stringify(response)+'\n');
});
child.on('exit',code=>process.exit(code??1));
"#;

#[tokio::test]
async fn unavailable_prestart_catalog_refuses_spawn_before_allocating_a_child() {
    for clamped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let wrapper = root.path().join("cold-catalog.cjs");
        std::fs::write(&wrapper, COLD_CATALOG_ADAPTER).unwrap();
        let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("node"))
            .find(|p| p.is_file())
            .unwrap();
        hennery_testkit::first_exec(node.to_str().unwrap());
        let mut s = script();
        if clamped {
            s.sticky_options = vec!["model".into()];
        }
        let mut command = fake(root.path(), "cold", s);
        command.args = vec![wrapper.display().to_string(), command.program];
        command.program = node.display().to_string();
        let h = Harness::with_worker(script(), Some((command, Profile::Claude)), true).await;
        let caps = h.ok("session_capabilities", json!({"agent":"worker"})).await;
        assert_eq!(caps["catalogs"][0]["freshness"], "unknown");
        assert!(caps["catalogs"][0]["defaults"].get("model").is_none());
        let result = h.call("session_spawn",json!({"operation_key":"cold-explicit","cwd":h.cwd,"agent":"worker","brief":"must not run","config":{"model":"large"}})).await;
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["code"], "configuration_unavailable");
        assert!(
            h.state
                .store
                .family_page(&h.parent, "", 100)
                .unwrap()
                .unwrap()
                .children
                .is_empty()
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn background_worker_advances_past_offline_page_and_wraps_to_repaired_earlier_target() {
    use hennery_proto::frames::McpIsolation;
    use hennery_sessions::store::McpContext;
    use rusqlite::params;
    let h = Harness::with_worker(script(), None, false).await;
    let later = h.spawn("later-page", json!({})).await;
    let later = later["child_id"].as_str().unwrap();
    // Attach once before adding global pressure. Its brief stays queued,
    // so one later page visit can complete it without first needing a wrap.
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    eventually("later child ready for one dispatch", async || {
        let s = h.status(later).await;
        (s["actual"]["model"] == "small").then_some(())
    })
    .await;
    let db = h.cwd.join("hennery.db");
    let parent = h.state.store.find_session(&h.parent).unwrap().unwrap();
    let gateway = hennery_gateway::store::GatewayStore::open(&db).unwrap();
    let identity = gateway.authenticate_session(&h.token).unwrap().unwrap();
    let revision = identity.delivered_delegation_revision().unwrap();
    let earlier = "000-earlier-child";
    let earlier_cwd = h.cwd.join("repairable-workspace");
    std::fs::create_dir(&earlier_cwd).unwrap();
    h.state
        .store
        .create_delegated_session(
            &identity,
            revision,
            earlier,
            "worker",
            earlier_cwd.to_str().unwrap(),
            McpContext {
                source_policy: false,
                native_sources: false,
                capable: true,
                isolation: McpIsolation::ClaudeStrict,
                rules_name_other_hats: false,
            },
            0,
        )
        .unwrap()
        .unwrap();
    // A directory can disappear after acceptance and before host admission.
    // Recreating this same path later makes the held target eligible again.
    std::fs::remove_dir(&earlier_cwd).unwrap();
    h.state
        .hosts
        .register(
            "offline-backlog",
            &Enrollment {
                public_key: HostKey::from_seed([11; 32]).public_key_hex(),
                name: "offline fixture".into(),
                host_version: "fixture".into(),
                platform: "fixture".into(),
            },
            0,
        )
        .unwrap();
    let mut conn = hennery_kernel::db::open(&db).unwrap();
    {
        let tx = conn.transaction().unwrap();
        let owner = h.state.store.owner_id();
        let required = r#"{"model":"small","mode":"default"}"#;
        tx.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES (?1,?2,'earlier','earlier-operation','fixture','session_spawn',?3,'queued',?4)",params![owner,h.parent,earlier,revision]).unwrap();
        tx.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES (?1,'earlier-operation',?2,?3,?4,'earlier brief',?5)",params![owner,h.parent,earlier,revision,required]).unwrap();
        tx.execute("INSERT INTO session_attach_evidence(owner_id,session_id,attach_generation,requested_config) VALUES (?1,?2,1,?3)",params![owner,earlier,required]).unwrap();
        // Global pressure is fixture data, bypassing per-parent admission caps;
        // offline rows are skipped before family/launch validation. The two
        // live targets retain real family grants, host actors and ACP traffic.
        for i in 0..129 {
            let id = format!("000-offline-{i:04}");
            let operation = format!("backlog-{i:04}");
            tx.execute("INSERT INTO sessions(id,owner_id,host_id,hat_id,agent,cwd,lifecycle,created_at,last_event_at) VALUES (?1,?2,'offline-backlog',?3,'worker',?4,'starting','fixture','fixture')",params![id,owner,parent.hat_id,h.cwd.to_str().unwrap()]).unwrap();
            tx.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,state,accepted_revision) VALUES (?1,?2,?3,?3,'fixture','session_send',?4,'queued',?5)",params![owner,h.parent,operation,id,revision]).unwrap();
            tx.execute("INSERT INTO session_dispatch_queue(owner_id,operation_id,source_id,target_id,accepted_revision,payload,required_config) VALUES (?1,?2,?3,?4,?5,'offline brief',?6)",params![owner,operation,h.parent,id,revision,required]).unwrap();
        }
        tx.commit().unwrap();
    }
    let first_page = h.state.store.delegation_targets_after("").unwrap();
    assert_eq!(first_page.len(), 128);
    assert_eq!(first_page[0], earlier);
    assert!(!first_page.iter().any(|id| id == later));
    hennery_sessions::delegation_worker::after_startup(&h.state);
    eventually("later ready child beyond the offline first page", async || {
        let s = h.status(later).await;
        s["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["state"]["state"] == "completed")
            .then_some(())
    })
    .await;
    assert_eq!(
        h.state.store.find_session(earlier).unwrap().unwrap().lifecycle,
        "starting"
    );
    std::fs::create_dir(&earlier_cwd).unwrap();
    eventually("earlier repaired child after cursor wrap", async || {
        let s = h.status(earlier).await;
        s["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["state"]["state"] == "completed")
            .then_some(())
    })
    .await;
    assert_eq!(h.status(earlier).await["last_outcome"], "completed");
    h.finish().await;
}

#[tokio::test]
async fn background_answer_worker_advances_past_held_page_and_wraps_to_repaired_earlier_answer() {
    use rusqlite::params;
    let mut worker = script();
    worker.asks = vec![FakeAsk::Elicitation];
    let h = Harness::with_worker(worker, None, false).await;
    let earlier = h.spawn("answer-earlier", json!({})).await;
    let earlier = earlier["child_id"].as_str().unwrap();
    let later = h.spawn("answer-later", json!({})).await;
    let later = later["child_id"].as_str().unwrap();
    let (earlier_pending, later_pending) = eventually("both children asking", async || {
        hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
        let earlier = h
            .state
            .store
            .open_pending(earlier)
            .unwrap()
            .into_iter()
            .find(|p| !p.answered)?;
        let later = h
            .state
            .store
            .open_pending(later)
            .unwrap()
            .into_iter()
            .find(|p| !p.answered)?;
        Some((earlier, later))
    })
    .await;
    h.ok("session_answer", json!({"operation_key":"earlier-answer","child_id":earlier,"pending_id":earlier_pending.pending_id,"action":"accept","content":{"name":"answer"}})).await;
    let mut conn = hennery_kernel::db::open(&h.cwd.join("hennery.db")).unwrap();
    let revision: i64;
    {
        let tx = conn.transaction().unwrap();
        let owner = h.state.store.owner_id();
        revision = tx.query_row("SELECT accepted_revision FROM session_operations WHERE owner_id=?1 AND caller_id=?2 AND operation_key='earlier-answer'",params![owner,h.parent],|r|r.get(0)).unwrap();
        // These accepted rows are held by a changed delegation grant. Fixture
        // pressure bypasses admission limits; both live answers still use real
        // parent grants, pending records, host actors and ACP traffic.
        tx.execute("UPDATE session_operations SET operation_id='000-earlier-answer',accepted_revision=?3 WHERE owner_id=?1 AND caller_id=?2 AND operation_key='earlier-answer'",params![owner,h.parent,revision-1]).unwrap();
        for i in 0..127 {
            let pending = format!("held-answer-question-{i:04}");
            let operation = format!("100-held-answer-{i:04}");
            tx.execute("INSERT INTO pending(pending_id,session_id,turn_id,kind,option_ids,payload,state,opened_at,owner_id) SELECT ?1,session_id,turn_id,kind,option_ids,payload,'open',opened_at,owner_id FROM pending WHERE owner_id=?2 AND pending_id=?3",params![pending,owner,earlier_pending.pending_id]).unwrap();
            tx.execute("INSERT INTO answer_queue(owner_id,pending_id,session_id,request_id,answer,submitted_at,delegation_handoff) SELECT owner_id,?1,session_id,?2,answer,submitted_at,0 FROM answer_queue WHERE owner_id=?3 AND pending_id=?4",params![pending,operation,owner,earlier_pending.pending_id]).unwrap();
            tx.execute("INSERT INTO session_operations(owner_id,caller_id,operation_key,operation_id,digest,tool,child_id,pending_id,request_id,state,accepted_revision) VALUES (?1,?2,?3,?3,'fixture','session_answer',?4,?5,?3,'queued',?6)",params![owner,h.parent,operation,earlier,pending,revision-1]).unwrap();
        }
        tx.commit().unwrap();
    }
    h.ok("session_answer", json!({"operation_key":"later-answer","child_id":later,"pending_id":later_pending.pending_id,"action":"accept","content":{"name":"answer"}})).await;
    conn.execute("UPDATE session_operations SET operation_id='999-later-answer' WHERE owner_id=?1 AND caller_id=?2 AND operation_key='later-answer'",params![h.state.store.owner_id(),h.parent]).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM session_operations WHERE owner_id=?1 AND tool='session_answer' AND state='queued' AND operation_id < '999-later-answer'",[h.state.store.owner_id()],|r|r.get::<_,i64>(0)).unwrap(),128);
    hennery_sessions::delegation_worker::after_startup(&h.state);
    eventually("eligible answer beyond 128 held answers", async || {
        (h.state
            .store
            .pending_item(&later_pending.pending_id)
            .unwrap()
            .unwrap()
            .delivered
            == Some(true))
        .then_some(())
    })
    .await;
    assert_eq!(
        h.state
            .store
            .pending_item(&earlier_pending.pending_id)
            .unwrap()
            .unwrap()
            .delivered,
        None
    );
    conn.execute("UPDATE session_operations SET accepted_revision=?3 WHERE owner_id=?1 AND caller_id=?2 AND operation_key='earlier-answer'",params![h.state.store.owner_id(),h.parent,revision]).unwrap();
    eventually("repaired earlier answer after cursor wrap", async || {
        (h.state
            .store
            .pending_item(&earlier_pending.pending_id)
            .unwrap()
            .unwrap()
            .delivered
            == Some(true))
        .then_some(())
    })
    .await;
    h.finish().await;
}

async fn signout_worker() -> (Harness, tempfile::TempDir, PathBuf) {
    signout_worker_with_idle(hennery_host::session::IDLE_TIMEOUT).await
}

async fn signout_worker_with_idle(idle: Duration) -> (Harness, tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("signed-out");
    let wrapper = root.path().join("auth.cjs");
    std::fs::write(
        &wrapper,
        r#"
const {spawn}=require('node:child_process');
const fs=require('node:fs');
const script=JSON.parse(process.env.HENNERY_FAKE_ACP_SCRIPT);
if(fs.existsSync(process.argv[3])) { if(fs.readFileSync(process.argv[3],"utf8")==="unknown")script.config_options=[]; else script.new_session_error=-32000; }
process.env.HENNERY_FAKE_ACP_SCRIPT=JSON.stringify(script);
const child=spawn(process.argv[2],[],{env:process.env,stdio:'inherit'});
child.on('exit',code=>process.exit(code??1));
"#,
    )
    .unwrap();
    let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("node"))
        .find(|p| p.is_file())
        .unwrap();
    hennery_testkit::first_exec(node.to_str().unwrap());
    let mut command = fake(root.path(), "signout", script());
    command.args = vec![
        wrapper.display().to_string(),
        command.program,
        marker.display().to_string(),
    ];
    command.program = node.display().to_string();
    let h = Harness::with_worker_idle(script(), Some((command, Profile::Claude)), false, idle).await;
    (h, root, marker)
}

#[tokio::test]
async fn accepted_start_signout_fails_operation_and_child_without_reprobing() {
    let (h, _root, marker) = signout_worker().await;
    let receipt = h.spawn("signout-start", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    std::fs::write(marker, "signed out").unwrap();
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    // Admission can supply the attach catalog. Sign-out is then reported by
    // the actual child attach rather than an extra disposable probe.
    eventually("accepted start sign-in refusal is ingested", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "failed")
            .map(|_| ())
    })
    .await;
    assert!(
        h.state.store.delegation_targets().unwrap().is_empty(),
        "failed queue cleared before any tool read"
    );
    let status = h
        .ok(
            "session_status",
            json!({"child_id":child,"operation_key":"signout-start"}),
        )
        .await;
    assert_eq!(
        status["operations"][0]["state"],
        json!({"state":"failed","code":"auth_required"}),
        "{status}"
    );
    let session = h.state.store.find_session(child).unwrap().unwrap();
    assert_eq!(session.lifecycle, "failed");
    assert_eq!(session.failure_reason.as_deref(), Some("agent_not_logged_in"));
    let conn = rusqlite::Connection::open(h.cwd.join("hennery.db")).unwrap();
    assert!(
        conn.query_row(
            "SELECT revoked_at IS NOT NULL FROM gw_session_tokens WHERE session_id=?1",
            [child],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert!(h.state.store.delegation_targets().unwrap().is_empty());
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    assert_eq!(
        h.state.store.find_session(&h.parent).unwrap().unwrap().lifecycle,
        "active"
    );
    h.finish().await;
}

#[tokio::test]
async fn parked_followup_signout_fails_operation_without_failing_existing_session() {
    let (h, _root, marker) = signout_worker_with_idle(Duration::from_secs(1)).await;
    // Keep the caller attached while waiting for the child's real idle park.
    let pinned = h
        .client
        .patch(format!("{}/api/sessions/{}/idle-hold", h.url, h.parent))
        .json(&json!({"pinned":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(pinned.status(), reqwest::StatusCode::OK);
    let receipt = h.spawn("before-signout", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    eventually("initial child completes", async || {
        hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
        let status = h
            .ok(
                "session_status",
                json!({"child_id":child,"operation_key":"before-signout"}),
            )
            .await;
        (status["operations"][0]["state"]["state"] == "completed").then_some(())
    })
    .await;
    // Automatic parking allows background follow-ups; explicit operator Park
    // instead holds the shared FIFO until Resume or a new operator Send.
    eventually("child idle parked", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "parked" && !s.presumed_parked)
            .map(|_| ())
    })
    .await;
    assert!(
        h.state
            .store
            .events(child, 0, 1000)
            .unwrap()
            .iter()
            .any(|event| event.kind == "session_parked" && event.body["reason"] == "idle")
    );
    h.ok(
        "session_send",
        json!({"operation_key":"after-signout","child_id":child,"text":"continue"}),
    )
    .await;
    std::fs::write(marker, "signed out").unwrap();
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    assert!(
        h.state.store.delegation_targets().unwrap().is_empty(),
        "failed queue cleared before any tool read"
    );
    let status = h
        .ok(
            "session_status",
            json!({"child_id":child,"operation_key":"after-signout"}),
        )
        .await;
    assert_eq!(
        status["operations"][0]["state"],
        json!({"state":"failed","code":"auth_required"}),
        "{status}"
    );
    let session = h.state.store.find_session(child).unwrap().unwrap();
    assert_eq!(session.lifecycle, "parked");
    assert_eq!(session.failure_reason, None);
    assert!(h.state.store.delegation_targets().unwrap().is_empty());
    h.finish().await;
}

#[tokio::test]
async fn parent_notice_signout_requires_operator_without_failing_parent_session() {
    let (mut h, root, marker) = signout_worker().await;
    let (status, body) = h
        .post("/api/sessions", json!({"host_id":HOST,"agent":"worker","cwd":h.cwd}))
        .await;
    assert!(status.is_success(), "{body}");
    h.parent = body["session_id"].as_str().unwrap().to_owned();
    eventually("worker parent active", async || {
        h.state
            .store
            .find_session(&h.parent)
            .unwrap()
            .filter(|s| s.lifecycle == "active")
            .map(|_| ())
    })
    .await;
    let log = std::fs::read_to_string(root.path().join("signout-sessions.jsonl")).unwrap();
    let record: Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    h.token = record["params"]["mcpServers"][0]["headers"][0]["value"]
        .as_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap()
        .to_owned();
    let receipt = h.spawn("notice-signout", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    eventually("child attached", async || {
        h.state
            .store
            .find_session(child)
            .unwrap()
            .filter(|s| s.lifecycle == "active")
            .map(|_| ())
    })
    .await;
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    h.done(child, "notice-signout").await;
    let (status, body) = h.post(&format!("/api/sessions/{}/park", h.parent), json!({})).await;
    assert!(status.is_success(), "{body}");
    eventually("notice parent parked", async || {
        h.state
            .store
            .find_session(&h.parent)
            .unwrap()
            .filter(|s| s.lifecycle == "parked" && !s.presumed_parked)
            .map(|_| ())
    })
    .await;
    std::fs::write(marker, "signed out").unwrap();
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    let conn = rusqlite::Connection::open(h.cwd.join("hennery.db")).unwrap();
    let disposition: (String, Option<String>) = conn
        .query_row(
            "SELECT state,reason FROM delegation_notices WHERE child_id=?1 AND kind='turn_ended'",
            [child],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(disposition, ("operator_required".into(), Some("auth_required".into())));
    let session = h.state.store.find_session(&h.parent).unwrap().unwrap();
    assert_eq!(session.lifecycle, "parked");
    assert_eq!(session.failure_reason, None);
    h.finish().await;
}

#[tokio::test]
async fn transient_catalog_loss_retries_then_recovers_without_losing_the_start() {
    let (mut h, _root, marker) = signout_worker().await;
    let receipt = h.spawn("lost-catalog", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    // A cold collector has no admission cache, as after a restart.
    h.state.catalogs = Arc::default();
    std::fs::write(&marker, "unknown").unwrap();
    hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
    assert_eq!(
        h.state.store.find_session(child).unwrap().unwrap().lifecycle,
        "starting"
    );
    assert_eq!(h.state.store.delegation_queue_depth(child).unwrap().0, 1);
    let status = h
        .ok(
            "session_status",
            json!({"child_id":child,"operation_key":"lost-catalog"}),
        )
        .await;
    assert_eq!(status["operations"][0]["state"], json!({"state":"queued"}));
    std::fs::remove_file(marker).unwrap();
    eventually("transient catalog recovers and original brief completes", async || {
        hennery_sessions::delegation_worker::drain(&h.state).await.unwrap();
        let status = h
            .ok(
                "session_status",
                json!({"child_id":child,"operation_key":"lost-catalog"}),
            )
            .await;
        (status["operations"][0]["state"]["state"] == "completed").then_some(())
    })
    .await;
    assert_eq!(h.state.store.find_session(child).unwrap().unwrap().failure_reason, None);
    h.finish().await;
}

#[tokio::test]
async fn delegated_host_signin_failure_retains_session_login_reason_and_tool_auth_code() {
    let h = Harness::with_worker(script(), None, false).await;
    let receipt = h.spawn("host-signin", json!({})).await;
    let child = receipt["child_id"].as_str().unwrap();
    let facts = h
        .state
        .store
        .ingest(
            child,
            1,
            &hennery_proto::frames::SessionBody::StartFailed {
                request_id: "attach-request".into(),
                code: "agent_not_logged_in".into(),
                message: "Sign in using the runtime login command".into(),
            },
        )
        .unwrap();
    assert_eq!(facts[0].body["message"], "Sign in using the runtime login command");
    let session = h.state.store.find_session(child).unwrap().unwrap();
    assert_eq!(session.failure_reason.as_deref(), Some("agent_not_logged_in"));
    let status = h
        .ok(
            "session_status",
            json!({"child_id":child,"operation_key":"host-signin"}),
        )
        .await;
    assert_eq!(
        status["operations"][0]["state"],
        json!({"state":"failed","code":"auth_required"}),
        "{status}"
    );
    h.finish().await;
}

#[tokio::test]
async fn review_real_adapter_catalog_limit_must_not_disconnect_host() {
    let mut worker = script();
    while worker.config_options.len() < 129 {
        worker.config_options.push(json!({
            "id": format!("setting-{}", worker.config_options.len()), "name": "Review setting",
            "type": "select", "currentValue": "enabled", "options": [{"value":"enabled","name":"Enabled"}]
        }));
    }
    let h = Harness::new(worker).await;
    let (status, body) = h
        .post("/api/sessions", json!({"host_id":HOST,"agent":"worker","cwd":h.cwd}))
        .await;
    let state = h.state.clone();
    h.finish().await;
    assert_eq!(status, 202, "valid real adapter catalog disconnected the host: {body}");
    let id = body["session_id"].as_str().unwrap();
    assert_eq!(state.store.find_session(id).unwrap().unwrap().lifecycle, "active");
    assert_eq!(state.store.catalog(id).unwrap().unwrap().config_options.len(), 129);
}

#[tokio::test]
async fn review_real_session_delete_must_forget_derived_preview() {
    let h = Harness::new(script()).await;
    let (status, body) = h
        .post("/api/sessions", json!({"host_id":HOST,"agent":"worker","cwd":h.cwd}))
        .await;
    assert_eq!(status, 202, "{body}");
    let id = body["session_id"].as_str().unwrap();
    let deletion = h
        .client
        .delete(format!("{}/api/sessions/{id}", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(deletion.status(), 200);
    let remaining: Value = h
        .client
        .get(format!("{}/api/hosts/{HOST}/runtime-catalog", h.url))
        .query(&[("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    h.finish().await;
    assert_eq!(
        remaining["freshness"], "unknown",
        "deleted real session preview remains available: {remaining}"
    );
}
#[tokio::test]
async fn rereview_delete_old_session_preserves_later_sessions_http_preview() {
    let h = Harness::new(script()).await;
    let (status, first) = h
        .post(
            "/api/sessions",
            json!({"host_id":HOST,"agent":"worker","cwd":h.cwd,"model":"small"}),
        )
        .await;
    assert_eq!(status, 202, "{first}");
    let first_id = first["session_id"].as_str().unwrap();
    let (status, second) = h
        .post(
            "/api/sessions",
            json!({"host_id":HOST,"agent":"worker","cwd":h.cwd,"model":"large"}),
        )
        .await;
    assert_eq!(status, 202, "{second}");
    let second_id = second["session_id"].as_str().unwrap();
    let path = format!("{}/api/hosts/{HOST}/runtime-catalog", h.url);
    let query = [("agent", "worker"), ("cwd", h.cwd.to_str().unwrap())];
    let before: Value = h
        .client
        .get(&path)
        .query(&query)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["freshness"], "cached");
    assert_eq!(before["defaults"]["model"], "small");
    assert_eq!(before["config_options"][0]["currentValue"], "large");
    let deletion = h
        .client
        .delete(format!("{}/api/sessions/{first_id}", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(deletion.status(), 200);
    let kept = h.state.store.find_session(second_id).unwrap().unwrap();
    assert_eq!(kept.lifecycle, "active");
    assert_eq!(kept.config.model.as_deref(), Some("large"));
    let after: Value = h
        .client
        .get(&path)
        .query(&query)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    h.finish().await;
    assert_eq!(
        after["freshness"], "cached",
        "deleting the earlier session removed the later session's preview: {after}"
    );
    assert_eq!(
        after, before,
        "the newer session's full preview and age must remain unchanged"
    );
}
