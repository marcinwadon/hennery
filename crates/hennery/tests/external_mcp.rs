//! Real HTTP auth/grant/protocol tests in an ephemeral collector, not Desktop acceptance.
use hennery_kernel::{
    hosts::{Enrollment, Hosts},
    operator::Operator,
    secret::unix_now,
};
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
struct Collector {
    dir: tempfile::TempDir,
    state: AppState,
    url: String,
    operator: reqwest::Client,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Drop for Collector {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        self.task.abort();
    }
}
impl Collector {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "test-host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "Test".into(),
                    platform: "test".into(),
                    host_version: "test".into(),
                },
                0,
            )
            .unwrap();
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        let operator = hennery_testkit::operator_client(&state.operator);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            dir,
            state,
            url,
            operator,
            task,
        }
    }
    fn grant(&self, existing: bool) -> Value {
        json!({"label":"Desktop test","hat_id":self.state.hosts.host("test-host").unwrap().unwrap().default_hat_id,"roots":[{"host_id":"test-host","root":"/project"}],"read_metadata":true,"existing_sessions":existing,"expires_in_days":1})
    }
    async fn issue(&self, existing: bool) -> Value {
        let r = self
            .operator
            .post(format!("{}/api/external-clients", self.url))
            .json(&self.grant(existing))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201);
        assert_eq!(r.headers()["cache-control"], "no-store");
        r.json().await.unwrap()
    }
    async fn rpc(&self, token: &str, method: &str, params: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/mcp/external/hennery", self.url))
            .bearer_auth(token)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn credentials_are_distinct_and_provision_revoke_require_fresh_stepup() {
    let c = Collector::new().await;
    let path = format!("{}/api/external-clients", c.url);
    assert_eq!(
        reqwest::Client::new()
            .post(&path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .json(&c.grant(false))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let old = c
        .state
        .operator
        .open_session("test", &hennery_testkit::owner_phc(&c.state.operator), unix_now() - 301)
        .unwrap()
        .unwrap();
    let stale = reqwest::Client::new()
        .post(&path)
        .header("origin", hennery_testkit::PUBLIC_URL)
        .header("cookie", format!("hennery_session={old}"))
        .json(&c.grant(false))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 403);
    let issued = c.issue(false).await;
    let token = issued["token"].as_str().unwrap();
    assert_eq!(c.rpc("hnry_session_wrong", "tools/list", json!({})).await.status(), 401);
    assert_eq!(
        c.operator
            .post(format!("{}/mcp/external/hennery", c.url))
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/api/sessions", c.url))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let listed: Value = c.operator.get(&path).send().await.unwrap().json().await.unwrap();
    assert!(!listed.to_string().contains(token));
    assert!(!listed.to_string().contains("token_hash"));
    let id = issued["client"]["client_id"].as_str().unwrap();
    assert_eq!(
        reqwest::Client::new()
            .delete(format!("{path}/{id}"))
            .header("origin", hennery_testkit::PUBLIC_URL)
            .header("cookie", format!("hennery_session={old}"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(c.rpc(token, "tools/list", json!({})).await.status(), 200);
    assert_eq!(
        c.operator.delete(format!("{path}/{id}")).send().await.unwrap().status(),
        204
    );
    assert_eq!(c.rpc(token, "tools/list", json!({})).await.status(), 401);
}
#[tokio::test]
async fn protocol_discovery_and_session_reads_keep_explicit_scope_and_no_mutations() {
    let c = Collector::new().await;
    let hat = c.state.hosts.host("test-host").unwrap().unwrap().default_hat_id;
    for (id, path) in [("in-scope", "/project/sub"), ("outside", "/project-other")] {
        c.state
            .store
            .create_session(id, "test-host", "claude", path, &hat, None)
            .unwrap();
    }
    let issued = c.issue(true).await;
    let token = issued["token"].as_str().unwrap();
    let init:Value=c.rpc(token,"initialize",json!({"protocolVersion":"2025-11-25","clientInfo":{"name":"not-an-identity","version":"1"},"capabilities":{}})).await.json().await.unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    let listed: Value = c.rpc(token, "tools/list", json!({})).await.json().await.unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
    assert!(tools.iter().all(|t| t["annotations"]["readOnlyHint"] == true));
    assert!(!listed.to_string().contains("session_prompt"));
    let rows: Value = c
        .rpc(token, "tools/call", json!({"name":"session_list","arguments":{}}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        rows["result"]["structuredContent"]["data"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        rows["result"]["structuredContent"]["data"]["sessions"][0]["session_id"],
        "in-scope"
    );
    let denied: Value = c
        .rpc(
            token,
            "tools/call",
            json!({"name":"session_get","arguments":{"session_id":"outside"}}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(denied["result"]["isError"], true);
    assert!(!denied.to_string().contains("/project-other"));
    let write: Value = c
        .rpc(
            token,
            "tools/call",
            json!({"name":"session_prompt","arguments":{"session_id":"in-scope","text":"do not execute"}}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(write["result"]["isError"], true);
    let default = c.issue(false).await;
    let empty: Value = c
        .rpc(
            default["token"].as_str().unwrap(),
            "tools/call",
            json!({"name":"session_list","arguments":{}}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(empty["result"]["structuredContent"]["data"]["sessions"], json!([]));
    let unknown: Value = c
        .rpc(
            token,
            "tools/call",
            json!({"name":"host_list","arguments":{"hat_id":"spoof"}}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(unknown["result"]["isError"], true);
    // MCP protocol metadata is accepted but cannot broaden the authenticated grant.
    let metadata: Value = c
        .rpc(
            default["token"].as_str().unwrap(),
            "tools/call",
            json!({"name":"session_list","arguments":{},"_meta":{"existing_sessions":true,"owner_id":"spoof","progressToken":1}}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(metadata["result"]["isError"], false);
    assert_eq!(metadata["result"]["structuredContent"]["data"]["sessions"], json!([]));
}
#[tokio::test]
async fn bad_origin_version_body_limits_and_notification_semantics() {
    let c = Collector::new().await;
    let issued = c.issue(false).await;
    let token = issued["token"].as_str().unwrap();
    let url = format!("{}/mcp/external/hennery", c.url);
    let http = reqwest::Client::new();
    assert_eq!(
        http.post(&url)
            .bearer_auth(token)
            .header("origin", "https://other.example")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(token)
            .header("mcp-protocol-version", "future")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .body("x".repeat(65537))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    let notified = http
        .post(&url)
        .bearer_auth(token)
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert_eq!(notified.status(), 202);
    assert!(notified.bytes().await.unwrap().is_empty());
    assert_eq!(http.get(&url).bearer_auth(token).send().await.unwrap().status(), 405);
    let parse: Value = http
        .post(&url)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(parse["error"]["code"], -32700);
}
#[tokio::test]
async fn stdio_bridge_exercises_real_http_without_desktop_or_real_config() {
    let c = Collector::new().await;
    let issued = c.issue(false).await;
    let token = issued["token"].as_str().unwrap();
    let path = c.dir.path().join("credential");
    std::fs::write(&path, token).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    use tokio::io::AsyncWriteExt;
    hennery_testkit::first_exec(env!("CARGO_BIN_EXE_hennery"));
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args([
            "external-mcp",
            "--url",
            &format!("{}/mcp/external/hennery", c.url),
            "--token-file",
        ])
        .arg(&path)
        .env_remove("HENNERY_DATA_DIR")
        .env_remove("HENNERY_HOST_DATA_DIR")
        .env("HOME", c.dir.path())
        .env("XDG_DATA_HOME", c.dir.path().join("data"))
        .env("XDG_CONFIG_HOME", c.dir.path().join("config"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n").await.unwrap();
    drop(stdin);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).unwrap();
    let rows: Vec<Value> = text.lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], 1);
    assert_eq!(rows[1]["result"]["tools"].as_array().unwrap().len(), 5);
    assert!(!text.contains(token));
}
