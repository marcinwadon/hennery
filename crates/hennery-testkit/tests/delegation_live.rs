//! Opt-in D4 provider acceptance. Normal tests never use credentials or infer.
//!
//! Supply HENNERY_D4_PINNED_ADAPTER_ROOT (the existing node_modules),
//! HENNERY_D4_CLAUDE_AUTH and HENNERY_D4_CODEX_AUTH (explicit source files),
//! HENNERY_D4_PARENT and HENNERY_D4_CHILD (claude/codex). Source credentials
//! are copied with mode 0600 into disposable homes; source homes are never
//! handed to an adapter. Run each pair with --ignored --exact --nocapture.
//! A passing smoke check is not a passing parent/child matrix or release gate.

use anyhow::{Context, Result, bail, ensure};
use hennery_gateway::{api::GatewayState, key::KeySource, session::GatewayMcp};
use hennery_host::{AgentCommand, HostConfig, identity::HostKey, profile::Profile};
use hennery_kernel::{
    egress::{Egress, Timeouts},
    hosts::{Enrollment, Hosts},
    operator::Operator,
};
use hennery_proto::{delegation_host::HostCatalog, frames::SessionConfig};
use hennery_sessions::{AppState, store::Store};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const HOST: &str = "live-acceptance";
const BOUND: Duration = Duration::from_secs(240);
const TOOLS: &[&str] = &[
    "session_capabilities",
    "session_spawn",
    "session_send",
    "session_answer",
    "session_status",
    "session_list",
    "session_close",
];

fn valid_tool_schemas(tools: &Value) -> bool {
    let Some(tools) = tools.as_array().filter(|tools| tools.len() == TOOLS.len()) else {
        return false;
    };
    hennery_proto::delegation::argument_schemas()
        .iter()
        .all(|(name, schema)| {
            tools.iter().filter(|tool| tool["name"] == *name).count() == 1
                && tools
                    .iter()
                    .any(|tool| tool["name"] == *name && tool["inputSchema"] == *schema)
        })
}

#[test]
fn discovery_probe_checks_input_schemas_not_only_names() {
    let mut tools = hennery_sessions::tool_args::discovery()["tools"].clone();
    assert!(valid_tool_schemas(&tools));
    let answer = tools
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| t["name"] == "session_answer")
        .unwrap();
    answer["inputSchema"]["properties"]["content"] = json!({"type":"string"});
    assert!(!valid_tool_schemas(&tools));
    assert!(!valid_tool_schemas(&json!([])));
}

fn runtime(variable: &str) -> Result<String> {
    let value = std::env::var(variable).with_context(|| format!("supply {variable}"))?;
    ensure!(matches!(value.as_str(), "claude" | "codex"), "invalid runtime");
    Ok(value)
}

fn spawn_diagnostics(conn: &rusqlite::Connection, owner: &str, parent: &str) -> Result<Vec<Value>> {
    // The operation survives an unavailable/detached child. Never require an
    // active family edge to find the child whose launch failed.
    let mut stmt = conn.prepare(
        "SELECT o.operation_id,o.child_id,o.state,o.code,c.agent,c.lifecycle,c.activity,c.failure_reason
         FROM session_operations o LEFT JOIN sessions c ON c.owner_id=?1 AND c.id=o.child_id
         WHERE o.owner_id=?1 AND o.caller_id=?2 AND o.tool='session_spawn' ORDER BY o.rowid LIMIT 32",
    )?;
    let mut rows = stmt
        .query_map(rusqlite::params![owner, parent], |r| {
            Ok(
                json!({"operation_id":r.get::<_,String>(0)?,"child_id":r.get::<_,Option<String>>(1)?,
            "operation_state":r.get::<_,String>(2)?,"operation_code":r.get::<_,Option<String>>(3)?,
            "child_runtime":r.get::<_,Option<String>>(4)?,"child_lifecycle":r.get::<_,Option<String>>(5)?,
            "child_activity":r.get::<_,Option<String>>(6)?,"child_failure_reason":r.get::<_,Option<String>>(7)?}),
            )
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in &mut rows {
        let mut events = vec![];
        if let Some(child) = row["child_id"].as_str() {
            let mut stmt = conn.prepare(
                "SELECT event_id,kind,body FROM events WHERE owner_id=?1 AND session_id=?2
                 AND kind IN ('start_failed','turn_ended','session_lost','session_closed') ORDER BY event_id DESC LIMIT 32",
            )?;
            for event in stmt.query_map(rusqlite::params![owner, child], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })? {
                let (id, kind, body) = event?;
                let body: Value = serde_json::from_str(&body)?;
                let mut safe = json!({"event_id":id,"kind":kind});
                for field in ["error", "message", "reason", "outcome"] {
                    if let Some(text) = body[field].as_str() {
                        safe[field] = json!(scrub_error(text)?);
                    }
                }
                events.push(safe);
            }
            events.reverse();
        }
        row["events"] = json!(events);
    }
    let mut safe = serde_json::from_str(&redact_credentials(&serde_json::to_string(&rows)?)?)?;
    scrub_capture(&mut safe);
    hennery_kernel::redact::value(&mut safe);
    Ok(serde_json::from_value(safe)?)
}

#[test]
fn failed_spawn_diagnostics_survive_without_an_attached_family() -> Result<()> {
    let conn = rusqlite::Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE TABLE session_operations(owner_id TEXT,caller_id TEXT,tool TEXT,operation_id TEXT,child_id TEXT,state TEXT,code TEXT);
         CREATE TABLE sessions(owner_id TEXT,id TEXT,agent TEXT,lifecycle TEXT,activity TEXT,failure_reason TEXT);
         CREATE TABLE events(owner_id TEXT,session_id TEXT,event_id INTEGER,kind TEXT,body TEXT);
         INSERT INTO session_operations VALUES('owner','parent','session_spawn','op','child','failed','start_failed');
         INSERT INTO sessions VALUES('owner','child','claude','failed',NULL,'Authentication required');
         INSERT INTO events VALUES('owner','child',1,'start_failed','{\"error\":\"Authentication required\"}');
         INSERT INTO events VALUES('owner','child',2,'turn_ended','{\"outcome\":\"failed\",\"error\":\"Bearer diagnostic-private\"}');
         INSERT INTO events VALUES('owner','child',3,'acp_update','{\"private_menu\":\"must not retain\"}');
         INSERT INTO session_operations VALUES('foreign','parent','session_spawn','other','other','failed','foreign-private');
         INSERT INTO sessions VALUES('foreign','child','claude','failed',NULL,'foreign-private');",
    )?;
    let rows = spawn_diagnostics(&conn, "owner", "parent")?;
    assert_eq!(rows.len(), 1, "retain failed spawn even without a family row");
    assert_eq!(rows[0]["operation_state"], "failed");
    assert_eq!(rows[0]["operation_code"], "start_failed");
    assert_eq!(rows[0]["child_lifecycle"], "failed");
    assert_eq!(rows[0]["child_failure_reason"], "Authentication required");
    assert_eq!(rows[0]["events"][0]["error"], "Authentication required");
    assert_eq!(rows[0]["events"][1]["outcome"], "failed");
    let text = serde_json::to_string(&rows)?;
    for private in ["diagnostic-private", "must not retain", "foreign-private"] {
        assert!(
            !text.contains(private),
            "diagnostics retain only scoped, scrubbed failure fields"
        );
    }
    conn.execute(
        "UPDATE session_operations SET child_id=NULL,state='failed',code='gone' WHERE owner_id='owner'",
        [],
    )?;
    let rows = spawn_diagnostics(&conn, "owner", "parent")?;
    assert_eq!(rows.len(), 1, "retain operation failure even without a surviving child");
    assert_eq!(rows[0]["operation_code"], "gone");
    assert!(rows[0]["child_id"].is_null());
    Ok(())
}

fn stage_auth(source_variable: &str, destination: &Path) -> Result<()> {
    let source = PathBuf::from(std::env::var(source_variable).with_context(|| format!("supply {source_variable}"))?);
    ensure!(source.is_file(), "{source_variable} must name a credential file");
    let bytes = std::fs::read(&source).context("read explicitly supplied credential file")?;
    let value: Value = serde_json::from_slice(&bytes).context("credential file must contain JSON")?;
    ensure!(value.is_object(), "credential file must contain a JSON object");
    ensure!(
        !bytes
            .windows(b"sk-hennery-discovery-fixture".len())
            .any(|part| part == b"sk-hennery-discovery-fixture"),
        "discovery fixture is not provider authentication"
    );
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    std::io::Write::write_all(&mut output, &bytes)?;
    Ok(())
}

fn command(root: &Path, runtime: &str, native_probe: bool) -> Result<(AgentCommand, Profile)> {
    let home = root.join(format!("{runtime}-home"));
    for leaf in [
        "",
        "config",
        "data",
        "cache",
        "state",
        "claude",
        "codex",
        "codex-sqlite",
    ] {
        std::fs::create_dir_all(home.join(leaf))?;
    }
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let adapters =
        PathBuf::from(std::env::var("HENNERY_D4_PINNED_ADAPTER_ROOT").context("supply pinned node_modules")?);
    let node = std::env::split_paths(&std::env::var_os("PATH").context("PATH missing")?)
        .map(|directory| directory.join("node"))
        .find(|file| file.is_file())
        .context("node missing from dev shell")?;
    hennery_testkit::first_exec(node.to_str().context("node path encoding")?);
    let mut env = vec![
        ("HOME".into(), home.display().to_string()),
        ("XDG_CONFIG_HOME".into(), home.join("config").display().to_string()),
        ("XDG_DATA_HOME".into(), home.join("data").display().to_string()),
        ("XDG_CACHE_HOME".into(), home.join("cache").display().to_string()),
        ("XDG_STATE_HOME".into(), home.join("state").display().to_string()),
        ("CLAUDE_CONFIG_DIR".into(), home.join("claude").display().to_string()),
        ("CODEX_HOME".into(), home.join("codex").display().to_string()),
        (
            "CODEX_SQLITE_HOME".into(),
            home.join("codex-sqlite").display().to_string(),
        ),
    ];
    // Prevent accidental provider/config inheritance. Only the explicitly staged
    // auth file supplies identity; neither discovery fixture keys nor runner
    // routing overrides can make this look like a live vendor acceptance.
    for variable in [
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HENNERY_DATA_DIR",
        "HENNERY_HOST_DATA_DIR",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENAI_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
    ] {
        env.push((variable.into(), String::new()));
    }
    let (package, version, profile) = if runtime == "claude" {
        stage_auth("HENNERY_D4_CLAUDE_AUTH", &home.join("claude/.credentials.json"))?;
        std::fs::write(home.join("claude.json"), br#"{"hasCompletedOnboarding":true}"#)?;
        ("claude-agent-acp", "0.93.0", Profile::Claude)
    } else {
        stage_auth("HENNERY_D4_CODEX_AUTH", &home.join("codex/auth.json"))?;
        let arch = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "darwin-arm64/node_modules/@openai/codex/vendor/aarch64-apple-darwin"
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "linux-x64/node_modules/@openai/codex/vendor/x86_64-unknown-linux-musl"
        } else {
            bail!("supply the supported pinned native Codex platform")
        };
        let binary = adapters.join(format!(".pnpm/@openai+codex@0.155.1-{arch}/bin/codex"));
        ensure!(binary.is_file(), "pinned native Codex 0.155.1 is missing");
        hennery_testkit::first_exec(binary.to_str().context("Codex path encoding")?);
        let program = if native_probe {
            let script = root.join("native-mcp-probe.mjs");
            std::fs::write(&script, include_str!("support/codex_mcp_probe.mjs"))?;
            std::fs::write(
                root.join("expected-mcp-schemas.json"),
                serde_json::to_vec(&hennery_proto::delegation::argument_schemas())?,
            )?;
            let launcher = root.join("native-mcp-probe");
            let quote = |path: &Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
            std::fs::write(
                &launcher,
                format!("#!/bin/sh\nexec {} {} \"$@\"\n", quote(&node), quote(&script)),
            )?;
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700))?;
            env.push(("HENNERY_D4_NATIVE_AUDIT_DIR".into(), root.display().to_string()));
            env.push(("HENNERY_D4_NATIVE_AUDIT_BINARY".into(), binary.display().to_string()));
            launcher
        } else {
            binary
        };
        env.push(("CODEX_PATH".into(), program.display().to_string()));
        ("codex-acp", "1.13.0", Profile::Codex)
    };
    let entry = adapters.join(format!("@agentclientprotocol/{package}"));
    let metadata: Value = serde_json::from_slice(&std::fs::read(entry.join("package.json"))?)?;
    ensure!(
        metadata["version"] == version,
        "adapter version differs from accepted pin"
    );
    Ok((
        AgentCommand {
            program: node.display().to_string(),
            args: vec![entry.join("dist/index.js").display().to_string()],
            env,
        },
        profile,
    ))
}

struct Harness {
    dir: tempfile::TempDir,
    state: AppState,
    client: reqwest::Client,
    url: String,
    parent: String,
    trace: Arc<Mutex<Vec<Value>>>,
    identities: Arc<Mutex<BTreeMap<String, hennery_gateway::identity::SessionIdentity>>>,
    probe_tokens: Arc<Mutex<BTreeMap<String, String>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    host: tokio::task::JoinHandle<anyhow::Result<()>>,
    server: tokio::task::JoinHandle<()>,
    approve_builtin: AtomicBool,
}

#[tokio::test]
#[ignore = "requires pinned Claude adapter and copied provider credentials"]
async fn live_claude_haiku_plan_configuration_survives_late_updates() -> Result<()> {
    use hennery_host::{
        outbox::Outbox,
        session::{self, Attach, Launch, SessionCmd, SessionOptions},
        uplink::Uplink,
    };
    use hennery_proto::frames::{HostFrame, SessionBody};
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().canonicalize()?;
    let (agent, profile) = command(&cwd, "claude", false)?;
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory()?);
    let wanted = SessionConfig {
        model: Some("haiku".into()),
        mode: Some("plan".into()),
        axes: BTreeMap::new(),
    };
    let handle = session::launch_delegated_in(
        uplink.clone(),
        Launch {
            request_id: "live-config".into(),
            session_id: "live-config".into(),
            attach: Attach::New,
            config: wanted.clone(),
            agent,
            cwd: cwd.clone(),
            profile,
            mcp_servers: vec![],
        },
        SessionOptions::default(),
        1,
        vec![cwd],
    );
    let result = tokio::time::timeout(BOUND, async {
        loop {
            let frames = uplink.pending()?;
            if frames.iter().any(|f| {
                matches!(
                    f,
                    HostFrame::Session {
                        body: SessionBody::SessionStarted { .. },
                        ..
                    }
                )
            }) {
                break;
            }
            if frames.iter().any(|f| matches!(f, HostFrame::Error { .. })) {
                bail!(
                    "Claude configuration start failed: {}",
                    redact_credentials(&serde_json::to_string(&frames)?)?
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Observe post-start notifications too: the reported bug changed the
        // model in the next update, after a correct first read-back.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut count = 0;
        for frame in uplink.pending()? {
            if let HostFrame::Session {
                body:
                    SessionBody::SessionStarted { indexed, .. }
                    | SessionBody::AcpUpdate { indexed, .. }
                    | SessionBody::ConfigApplied { indexed, .. },
                ..
            } = frame
                && let Some(evidence) = indexed.config_evidence
            {
                ensure!(
                    evidence.actual.model == wanted.model && evidence.actual.mode == wanted.mode,
                    "requested haiku/plan, actual configuration differed: {}",
                    redact_credentials(&serde_json::to_string(&evidence.actual)?)?
                );
                count += 1;
            }
        }
        ensure!(count > 0, "no configuration evidence");
        println!("Claude 0.93.0: haiku + plan confirmed across {count} configuration updates");
        Ok::<(), anyhow::Error>(())
    })
    .await;
    handle.send(SessionCmd::Close {
        request_id: "live-close".into(),
    });
    tokio::time::timeout(Duration::from_secs(15), handle.finished()).await?;
    drop(handle);
    result??;
    Ok(())
}
impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.state.shutdown.cancel();
        self.server.abort();
    }
}
impl Harness {
    async fn new(parent_runtime: &str, child_runtime: Option<&str>) -> Result<Self> {
        Self::new_with_worker(parent_runtime, child_runtime, true).await
    }
    async fn new_with_worker(parent_runtime: &str, child_runtime: Option<&str>, deliver_brief: bool) -> Result<Self> {
        Self::with_probe(parent_runtime, child_runtime, deliver_brief, false).await
    }
    async fn with_probe(
        parent_runtime: &str,
        child_runtime: Option<&str>,
        deliver_brief: bool,
        native_probe: bool,
    ) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let db = root.join("hennery.db");
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let url = format!("http://{}", listener.local_addr()?);
        let key = HostKey::from_seed([23; 32]);
        let hosts = Hosts::open(&db)?;
        hosts.register(
            HOST,
            &Enrollment {
                public_key: key.public_key_hex(),
                name: "acceptance".into(),
                host_version: "acceptance".into(),
                platform: "acceptance".into(),
            },
            0,
        )?;
        let hat = hosts.host(HOST)?.context("registered host")?.default_hat_id;
        let state = AppState::new(Store::open(&db)?, hosts, Operator::open(&db)?);
        let now = hennery_kernel::secret::unix_now();
        let setup = state.operator.issue_setup_token(now)?.context("setup token")?;
        state
            .operator
            .set_up(&setup, hennery_testkit::OWNER_PASSWORD, &url, now)?;
        let phc = state
            .operator
            .verify_password(hennery_testkit::OWNER_PASSWORD)?
            .context("password")?;
        let session = state
            .operator
            .open_session("live-acceptance", &phc, now)?
            .context("operator session")?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::ORIGIN, url.parse()?);
        headers.insert(reqwest::header::COOKIE, format!("hennery_session={session}").parse()?);
        let client = reqwest::Client::builder().no_proxy().default_headers(headers).build()?;
        let keys = KeySource::from_vars(&root, Some("23".repeat(32).into()), None)?;
        let gateway: GatewayState = hennery_gateway::open(
            &db,
            &keys,
            state.operator.clone(),
            Egress::new(Timeouts::DEFAULT)?,
            Arc::new(hennery_gateway::notify::Silent),
        )?;
        state
            .store
            .set_session_mcp(Arc::new(GatewayMcp::for_collector(&gateway)));
        gateway.runtime.store.set_session_tools(&hat, true)?;
        let trace: Arc<Mutex<Vec<Value>>> = Arc::default();
        let observed = trace.clone();
        let identities: Arc<Mutex<BTreeMap<String, hennery_gateway::identity::SessionIdentity>>> = Arc::default();
        let authenticated = identities.clone();
        let probe_tokens: Arc<Mutex<BTreeMap<String, String>>> = Arc::default();
        let tokens = probe_tokens.clone();
        let auth = gateway.runtime.store.clone();
        let builtin = hennery_sessions::builtin::router(state.clone(), gateway).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = observed.clone(); let auth = auth.clone(); let authenticated = authenticated.clone(); let tokens=tokens.clone();
                async move {
                    let bearer=request.headers().get("authorization").and_then(|h|h.to_str().ok()).and_then(|h|h.strip_prefix("Bearer "));
                    let caller = bearer.and_then(|t| auth.authenticate_session(t).ok().flatten()).map(|i| {
                        let hennery_gateway::scope::PrincipalKind::Session { session_id, .. } = &i.principal.kind;
                        let session_id = session_id.clone();
                        authenticated.lock().unwrap().insert(session_id.clone(), i);
                        if native_probe { tokens.lock().unwrap().insert(session_id.clone(),bearer.unwrap().to_owned()); }
                        session_id
                    });
                    let (parts, body) = request.into_parts();
                    let bytes = axum::body::to_bytes(body, hennery_gateway::proxy::MAX_REQUEST_BODY).await.unwrap();
                    let input: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let response = next.run(axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes))).await;
                    let (parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                    let output: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    // Never persist bearer, arguments, free text, errors or cookies.
                    let method = input["method"].as_str().filter(|s| matches!(*s, "initialize" | "tools/list" | "tools/call"));
                    let tool = input["params"]["name"].as_str().filter(|s| TOOLS.contains(s));
                    let names = output["result"]["tools"].as_array().map(|tools| tools.iter().filter_map(|t| t["name"].as_str().filter(|s| TOOLS.contains(s))).collect::<Vec<_>>());
                    let schemas_valid = (method == Some("tools/list")).then(|| valid_tool_schemas(&output["result"]["tools"]));
                    observed.lock().unwrap().push(json!({"caller":caller,"method":method,"tool":tool,"status":parts.status.as_u16(),"is_error":output["result"]["isError"].as_bool(),"rpc_error":output["error"]["code"].as_i64(),"tools":names,"schemas_valid":schemas_valid,"fields":input.as_object().map(|object| object.keys().collect::<Vec<_>>()),"param_fields":input["params"].as_object().map(|object| object.keys().collect::<Vec<_>>()),"meta_fields":input["params"]["_meta"].as_object().map(|object| object.keys().collect::<Vec<_>>())}));
                    axum::response::Response::from_parts(parts, axum::body::Body::from(bytes))
                }
            }
        ));
        let router = hennery_sessions::router(state.clone()).merge(builtin);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let mut config = HostConfig::new(
            format!("{}/api/hosts/ws", url.replacen("http:", "ws:", 1)),
            HOST,
            key,
            root.join("host-data"),
        );
        config.workspace_roots = vec![root.clone()];
        config.git = None;
        for runtime in ["claude", "codex"]
            .into_iter()
            .filter(|runtime| *runtime == parent_runtime || Some(*runtime) == child_runtime)
        {
            let (command, profile) = command(&root, runtime, native_probe)?;
            config.agents.insert(runtime.into(), command);
            config.profiles.insert(runtime.into(), profile);
        }
        let (shutdown, signal) = tokio::sync::oneshot::channel();
        let host = tokio::spawn(hennery_host::run_until(config, async {
            let _ = signal.await;
        }));
        let mut h = Self {
            dir,
            state,
            client,
            url,
            parent: String::new(),
            trace,
            identities,
            probe_tokens,
            shutdown: Some(shutdown),
            host,
            server,
            approve_builtin: AtomicBool::new(true),
        };
        h.wait("host ready", || h.state.hub.is_ready(HOST).then_some(()))
            .await?;
        if deliver_brief {
            hennery_sessions::delegation_worker::after_startup(&h.state);
        }
        let catalog = h.catalog(parent_runtime).await?;
        println!(
            "{}",
            json!({"phase":"catalog","runtime":parent_runtime,"config_options":catalog.config_options,"defaults":catalog.defaults,"security":catalog.security})
        );
        // Bound paid parent turns with the same catalog-selected model/effort
        // as children, while keeping the parent in ordinary collaboration mode.
        let effort = match std::env::var("HENNERY_D4_CODEX_PARENT_EFFORT") {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => bail!("HENNERY_D4_CODEX_PARENT_EFFORT must be low or medium"),
        };
        let parent_config = parent_configuration(&catalog, effort.as_deref())?;
        ensure!(
            hennery_sessions::delegation_runtime::config_offered(&catalog, &parent_config),
            "chosen parent configuration absent from actual catalog"
        );
        let mut request = serde_json::to_value(&parent_config)?;
        request["host_id"] = json!(HOST);
        request["agent"] = json!(parent_runtime);
        request["cwd"] = json!(root);
        println!("{}", json!({"phase":"parent_requested_config","config":parent_config}));
        let response = h
            .client
            .post(format!("{}/api/sessions", h.url))
            .json(&request)
            .send()
            .await?;
        ensure!(
            response.status() == reqwest::StatusCode::ACCEPTED,
            "parent start HTTP {}",
            response.status()
        );
        h.parent = response.json::<Value>().await?["session_id"]
            .as_str()
            .context("parent session id")?
            .into();
        h.wait("parent attach", || {
            h.state
                .store
                .find_session(&h.parent)
                .ok()
                .flatten()
                .filter(|s| s.lifecycle == "active")
                .map(|_| ())
        })
        .await?;
        let actual = h
            .state
            .store
            .catalog(&h.parent)?
            .context("parent configuration readback")?
            .current;
        println!(
            "{}",
            json!({"case":"parent_configuration_comparison","requested":parent_config,"actual":actual})
        );
        ensure!(
            parent_config.model == actual.model
                && parent_config.mode == actual.mode
                && parent_config
                    .axes
                    .iter()
                    .all(|(id, value)| actual.axes.get(id) == Some(value)),
            "parent requested/actual configuration differs"
        );
        Ok(h)
    }
    async fn wait<T>(&self, phase: &str, mut predicate: impl FnMut() -> Option<T>) -> Result<T> {
        let result = tokio::time::timeout(BOUND, async {
            loop {
                if matches!(
                    phase,
                    "real parent spawn"
                        | "child configuration readback"
                        | "genuine child elicitation"
                        | "real parent answer delivered"
                        | "child provider continuation"
                ) {
                    let conn = hennery_kernel::db::open(&self.dir.path().join("hennery.db"))?;
                    let failed = spawn_diagnostics(&conn, self.state.store.owner_id(), &self.parent)?
                        .iter()
                        .any(|row| {
                            matches!(
                                row["operation_state"].as_str(),
                                Some("failed" | "cancelled" | "not_delivered")
                            ) || row["child_lifecycle"] == "failed"
                                || row["events"]
                                    .as_array()
                                    .is_some_and(|events| events.iter().any(|event| event["outcome"] == "failed"))
                        });
                    let parent_failed = phase == "real parent spawn"
                        && self
                            .state
                            .store
                            .events(&self.parent, 0, 1000)?
                            .iter()
                            .any(|event| event.kind == "turn_ended" && event.body["outcome"] == "failed");
                    if failed || parent_failed {
                        self.diagnostics(phase)?;
                        bail!(
                            "{phase}: provider or spawn failed; scrubbed durable operation/child diagnostics printed"
                        );
                    }
                }
                if let Some(value) = predicate() {
                    println!("{}", json!({"phase":phase}));
                    return Ok(value);
                }
                if self
                    .trace
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|row| row["method"] == "tools/call" && row["rpc_error"].is_number())
                {
                    self.diagnostics(phase)?;
                    bail!("actual adapter tools/call was refused at protocol parsing");
                }
                if phase == "genuine child elicitation"
                    && let Some(child) = self.child()
                {
                    let events = self.state.store.events(&child, 0, 1000)?;
                    if events.iter().any(|event| event.kind == "turn_ended")
                        && !events.iter().any(|event| event.kind == "pending_opened")
                    {
                        bail!("child completed without a genuine ACP elicitation; prose is not answer evidence");
                    }
                }
                self.approve_parent_tools().await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        match result {
            Ok(value) => value,
            Err(_) => {
                self.diagnostics(phase)?;
                bail!(
                    "{phase} exceeded {} seconds; scrubbed diagnostics printed",
                    BOUND.as_secs()
                )
            }
        }
    }
    async fn approve_parent_tools(&self) -> Result<()> {
        if self.parent.is_empty() || !self.approve_builtin.load(Ordering::SeqCst) {
            return Ok(());
        }
        for question in self.state.store.open_pending(&self.parent)? {
            if question.answered || question.kind != hennery_proto::frames::PendingKind::Permission {
                continue;
            }
            let updates: Vec<_> = self
                .state
                .store
                .events(&self.parent, 0, 1000)?
                .into_iter()
                .filter_map(|event| event.body.pointer("/payload/update").cloned())
                .collect();
            let allowed = builtin_permission(&question.payload, &updates);
            ensure!(
                allowed,
                "unexpected parent permission; harness authorizes only named built-in MCP tools"
            );
            let option = question.payload["options"]
                .as_array()
                .and_then(|options| options.iter().find(|option| option["kind"] == "allow_once"))
                .and_then(|option| option["optionId"].as_str())
                .context("parent MCP permission lacks allow_once")?;
            let response = self
                .client
                .post(format!(
                    "{}/api/sessions/{}/pending/{}/answer",
                    self.url, self.parent, question.pending_id
                ))
                .json(&json!({"option_id":option}))
                .send()
                .await?;
            ensure!(
                response.status() == reqwest::StatusCode::ACCEPTED,
                "parent MCP permission answer HTTP {}",
                response.status()
            );
        }
        Ok(())
    }
    fn diagnostics(&self, phase: &str) -> Result<()> {
        let events = if self.parent.is_empty() {
            vec![]
        } else {
            self.state.store.events(&self.parent, 0, 1000)?
        };
        let messages: Vec<_> = events
            .iter()
            .filter(|e| e.kind == "turn_ended" || e.kind == "start_failed")
            .filter_map(|e| {
                e.body
                    .get("error")
                    .or_else(|| e.body.get("message"))
                    .and_then(Value::as_str)
            })
            .map(scrub_error)
            .collect::<Result<_>>()?;
        let text = messages.join(" ").to_lowercase();
        let known = [
            "authentication_error",
            "invalid_api_key",
            "unauthorized",
            "rate_limit",
            "insufficient_quota",
            "token_expired",
            "authentication required",
            "certificate",
            "connection refused",
        ];
        let errors: Vec<_> = known.into_iter().filter(|error| text.contains(error)).collect();
        let kinds: Vec<_> = events.iter().map(|e| e.kind.as_str()).collect();
        println!(
            "{}",
            json!({"phase":phase,"error_classes":errors,"errors":messages,"event_kinds":kinds,"mcp":*self.trace.lock().unwrap()})
        );
        println!(
            "{}",
            json!({"phase":"parent_diagnostics","assistant":scrub_error(&assistant_text(&events))?})
        );
        let conn = hennery_kernel::db::open(&self.dir.path().join("hennery.db"))?;
        println!(
            "{}",
            json!({"phase":"spawn_diagnostics","operations":spawn_diagnostics(&conn,self.state.store.owner_id(),&self.parent)?})
        );
        if let Some(child) = self.child() {
            let events = self.state.store.events(&child, 0, 1000)?;
            println!(
                "{}",
                json!({"phase":"child_diagnostics","assistant":scrub_error(&assistant_text(&events))?,"event_kinds":events.iter().map(|event|event.kind.as_str()).collect::<Vec<_>>()})
            );
        }
        Ok(())
    }
    async fn catalog(&self, agent: &str) -> Result<HostCatalog> {
        let response = self
            .client
            .post(format!("{}/api/hosts/{HOST}/runtime-catalog", self.url))
            .query(&[
                ("agent", agent),
                ("cwd", self.dir.path().to_str().context("cwd encoding")?),
            ])
            .send()
            .await?;
        ensure!(response.status().is_success(), "catalog HTTP {}", response.status());
        Ok(response.json().await?)
    }
    async fn prompt(&self, id: &str, text: &str) -> Result<String> {
        let response = self
            .client
            .post(format!("{}/api/sessions/{id}/prompt", self.url))
            .json(&json!({"content":[{"type":"text","text":text}]}))
            .send()
            .await?;
        ensure!(
            response.status() == reqwest::StatusCode::ACCEPTED,
            "prompt HTTP {}",
            response.status()
        );
        Ok(response.json::<Value>().await?["turn_id"]
            .as_str()
            .context("turn id")?
            .into())
    }
    fn child(&self) -> Option<String> {
        hennery_kernel::db::open(&self.dir.path().join("hennery.db"))
            .ok()?
            .query_row(
                "SELECT child_id FROM session_families WHERE owner_id=?1 AND parent_id=?2 AND detached=0 LIMIT 1",
                rusqlite::params![self.state.store.owner_id(), self.parent],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }
    async fn finish(mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        tokio::time::timeout(Duration::from_secs(30), &mut self.host).await???;
        self.state.shutdown.cancel();
        Ok(())
    }
    async fn action(&self, session: &str, action: &str) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/api/sessions/{session}/{action}", self.url))
            .send()
            .await?;
        ensure!(response.status().is_success(), "{action} HTTP {}", response.status());
        Ok(())
    }
    async fn set_axis(&self, session: &str, id: &str, value: &str) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/api/sessions/{session}/config", self.url))
            .json(&json!({"config_id":id,"value":value}))
            .send()
            .await?;
        ensure!(
            response.status() == reqwest::StatusCode::ACCEPTED,
            "config switch HTTP {}",
            response.status()
        );
        ensure!(
            self.state
                .store
                .catalog(session)?
                .context("actual catalog after switch")?
                .current
                .axes
                .get(id)
                == Some(&hennery_proto::frames::ConfigValue::Id(value.into())),
            "config switch was not actually read back"
        );
        Ok(())
    }
    fn question(
        &self,
        session: &str,
        after: i64,
        kind: hennery_proto::frames::PendingKind,
    ) -> Option<hennery_proto::rest::PendingItem> {
        self.state
            .store
            .events(session, after, 1000)
            .ok()?
            .iter()
            .filter(|event| event.kind == "pending_opened")
            .filter_map(|event| event.body["pending_id"].as_str())
            .filter_map(|id| self.state.store.pending_item(id).ok().flatten())
            .find(|question| question.kind == kind)
    }
    async fn question_for_turn(
        &self,
        session: &str,
        turn: &str,
        after: i64,
        kind: hennery_proto::frames::PendingKind,
        phase: &str,
    ) -> Result<hennery_proto::rest::PendingItem> {
        self.wait(phase, || {
            if let Some(question) = self.question(session, after, kind) {
                return Some(Ok(question));
            }
            self.state.store.ended_turn_outcome(turn).ok().flatten().map(|outcome| {
                Err(anyhow::anyhow!(
                    "provider turn {outcome:?} without the required genuine question"
                ))
            })
        })
        .await?
    }
    async fn idle(&self, session: &str) -> Result<()> {
        self.wait("session idle",||{
            let row = self.state.store.find_session(session).ok().flatten()?;
            if row.lifecycle != "active" || row.activity.as_deref() != Some("idle") || row.open_turn_id.is_some() { return None; }
            let conn = hennery_kernel::db::open(&self.dir.path().join("hennery.db")).ok()?;
            let quiet: bool = conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM delegation_notices WHERE owner_id=?1 AND parent_id=?2 AND state IN ('parent_queued','waiting_config','awaiting_reconciliation'))",rusqlite::params![self.state.store.owner_id(),session],|row|row.get(0)).ok()?;
            quiet.then_some(())
        }).await
    }
    async fn answer_permission(
        &self,
        session: &str,
        question: &hennery_proto::rest::PendingItem,
        allow: bool,
    ) -> Result<()> {
        ensure!(
            question.kind == hennery_proto::frames::PendingKind::Permission,
            "permission only"
        );
        let kind = if allow { "allow_once" } else { "reject_once" };
        let option = question.payload["options"]
            .as_array()
            .and_then(|options| options.iter().find(|option| option["kind"] == kind))
            .and_then(|option| option["optionId"].as_str())
            .context("expected one-shot permission option")?;
        let response = self
            .client
            .post(format!(
                "{}/api/sessions/{session}/pending/{}/answer",
                self.url, question.pending_id
            ))
            .json(&json!({"option_id":option}))
            .send()
            .await?;
        ensure!(
            response.status() == reqwest::StatusCode::ACCEPTED,
            "operator permission HTTP {}",
            response.status()
        );
        self.wait("operator permission delivered", || {
            self.state
                .store
                .pending_item(&question.pending_id)
                .ok()
                .flatten()
                .filter(|pending| pending.delivered == Some(true))
        })
        .await?;
        Ok(())
    }
    fn token_hash(&self, session: &str) -> Result<Option<String>> {
        Ok(hennery_kernel::db::open(&self.dir.path().join("hennery.db"))?
            .query_row(
                "SELECT token_hash FROM gw_session_tokens WHERE owner_id=?1 AND session_id=?2 AND revoked_at IS NULL",
                rusqlite::params![self.state.store.owner_id(), session],
                |row| row.get(0),
            )
            .optional()?)
    }
    fn capture(&self, child: &str) -> Result<()> {
        let Ok(file) = std::env::var("HENNERY_D4_CAPTURE") else {
            return Ok(());
        };
        let events = scrub_events(self.state.store.events(child, 0, 1000)?);
        let mut text = String::new();
        for event in events {
            text.push_str("event: event\ndata: ");
            text.push_str(&serde_json::to_string(&event)?);
            text.push_str("\n\n");
        }
        let root = self.dir.path().canonicalize()?.display().to_string();
        text = redact_credentials(&text)?
            .replace(&root, "/tmp/smoke")
            .replace(&self.dir.path().display().to_string(), "/tmp/smoke")
            .replace(&root.replace('/', "-"), "-tmp-smoke")
            .replace(&self.dir.path().display().to_string().replace('/', "-"), "-tmp-smoke");
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(file)?;
        std::io::Write::write_all(&mut output, text.as_bytes())?;
        Ok(())
    }
    async fn extended(&self, child: &str, child_runtime: &str, child_pid: u32) -> Result<()> {
        use hennery_proto::frames::PendingKind;
        self.idle(child).await?;
        self.idle(&self.parent).await?;
        if child_runtime == "codex" {
            self.set_axis(child, "collaboration_mode", "default").await?;
        }
        let path = self.dir.path().canonicalize()?.join("permission-denied.txt");
        let after = self.state.store.max_event_id()?;
        let command = format!("printf test > '{}'", path.display());
        let (tool, execution) = if child_runtime == "claude" {
            ("Bash", json!({"command":command}))
        } else {
            (
                "exec_command",
                json!({"cmd":command,"sandbox_permissions":"require_escalated","justification":"D4 scratch operator-denial acceptance"}),
            )
        };
        let turn = self.prompt(child,&format!("New operator acceptance task, superseding the earlier no-shell restriction only for this scratch command: call {tool} with exactly these arguments: {execution}. You MUST request real operator approval through the tool; do not describe or simulate it. Do not change mode or policy or write by another means. The operator will deny this once. After denial do not retry and reply PERMISSION_DENIED_OK.")).await?;
        let permission = self
            .question_for_turn(child, &turn, after, PendingKind::Permission, "genuine child permission")
            .await?;
        self.wait("permission routes to operator", || {
            self.state
                .store
                .operator_attention(None, "")
                .ok()?
                .into_iter()
                .find(|attention| attention.pending_id.as_deref() == Some(&permission.pending_id))
        })
        .await?;
        ensure!(!permission.answered, "parent must not answer a permission");
        self.answer_permission(child, &permission, false).await?;
        self.wait("denied child turn ends", || {
            self.state.store.ended_turn_outcome(&turn).ok().flatten()
        })
        .await?;
        ensure!(!path.exists(), "operator denial must prevent the edit");
        println!("{}", json!({"case":"operator_permission_fallback","passed":true}));

        self.idle(child).await?;
        self.idle(&self.parent).await?;
        self.approve_builtin.store(false, Ordering::SeqCst);
        let after = self.state.store.max_event_id()?;
        let busy = self.prompt(&self.parent, busy_parent_prompt()).await?;
        let held = self
            .question_for_turn(
                &self.parent,
                &busy,
                after,
                PendingKind::Permission,
                "parent busy at real MCP approval",
            )
            .await?;
        let updates: Vec<_> = self
            .state
            .store
            .events(&self.parent, 0, 1000)?
            .into_iter()
            .filter_map(|event| event.body.pointer("/payload/update").cloned())
            .collect();
        ensure!(
            builtin_permission(&held.payload, &updates),
            "busy boundary must be a real known built-in call"
        );
        let report_turn = self
            .prompt(child, "Reply exactly CHILD_BUSY_REPORT without using tools.")
            .await?;
        self.wait("busy child report source", || {
            self.state.store.ended_turn_outcome(&report_turn).ok().flatten()
        })
        .await?;
        self.wait("report queued while parent busy",||{
            let conn = hennery_kernel::db::open(&self.dir.path().join("hennery.db")).ok()?;
            conn.query_row("SELECT EXISTS(SELECT 1 FROM delegation_notices WHERE owner_id=?1 AND child_id=?2 AND source_turn=?3 AND state='parent_queued')",rusqlite::params![self.state.store.owner_id(),child,report_turn],|row|row.get::<_,bool>(0)).ok()?.then_some(())
        }).await?;
        ensure!(
            self.state
                .store
                .find_session(&self.parent)?
                .context("parent")?
                .open_turn_id
                .as_deref()
                == Some(&busy),
            "notice must not overlap the active parent turn"
        );
        self.answer_permission(&self.parent, &held, true).await?;
        self.approve_builtin.store(true, Ordering::SeqCst);
        self.wait("busy report delivered after parent release",||{
            let conn = hennery_kernel::db::open(&self.dir.path().join("hennery.db")).ok()?;
            conn.query_row("SELECT EXISTS(SELECT 1 FROM delegation_notices WHERE owner_id=?1 AND child_id=?2 AND source_turn=?3 AND state='parent_delivered')",rusqlite::params![self.state.store.owner_id(),child,report_turn],|row|row.get::<_,bool>(0)).ok()?.then_some(())
        }).await?;
        println!("{}", json!({"case":"busy_parent_report_delivery","passed":true}));

        self.idle(child).await?;
        self.idle(&self.parent).await?;
        if child_runtime == "codex" {
            self.set_axis(child, "collaboration_mode", "plan").await?;
        }
        let token = self.token_hash(&self.parent)?.context("parent current token")?;
        self.action(&self.parent, "park").await?;
        ensure!(
            self.state
                .store
                .find_session(&self.parent)?
                .context("parked parent")?
                .lifecycle
                == "parked",
            "parent must be genuinely parked"
        );
        ensure!(
            self.token_hash(&self.parent)?.is_none(),
            "park must revoke the old token"
        );
        let after = self.state.store.max_event_id()?;
        let turn = self.prompt(child, question_brief(child_runtime)).await?;
        let question = self
            .question_for_turn(
                child,
                &turn,
                after,
                PendingKind::Elicitation,
                "parked-parent child elicitation",
            )
            .await?;
        self.wait("parked parent answers after automatic resume", || {
            self.state
                .store
                .pending_item(&question.pending_id)
                .ok()
                .flatten()
                .filter(|pending| pending.delivered == Some(true))
        })
        .await?;
        ensure!(
            self.token_hash(&self.parent)?.is_some_and(|rotated| rotated != token),
            "automatic resume must rotate the session token"
        );
        let resumed = self.state.store.events(&self.parent, after, 1000)?;
        let first_prompt = resumed
            .iter()
            .find(|event| event.kind == "user_turn")
            .context("resumed notice prompt")?;
        ensure!(
            resumed.iter().any(|event| event.event_id < first_prompt.event_id
                && event.body.pointer("/indexed/config_evidence/completeness") == Some(&json!("complete"))),
            "fresh resume config evidence must precede notice prompt"
        );
        println!(
            "{}",
            json!({"case":"parked_parent_resume_config_token_rotation","passed":true})
        );

        let old = self.state.hub.routable_conn(HOST).context("current host connection")?;
        ensure!(
            self.state.hub.disconnect_and_wait(HOST, Duration::from_secs(30)).await,
            "scratch collector socket did not close"
        );
        self.wait("collector-side reconnect and reconciliation", || {
            self.state
                .hub
                .routable_conn(HOST)
                .filter(|connection| *connection != old)
                .filter(|_| self.state.hub.is_ready(HOST))
        })
        .await?;
        println!(
            "{}",
            json!({"case":"collector_socket_reconnect","passed":true,"collector_process_restart":false})
        );

        self.idle(child).await?;
        self.idle(&self.parent).await?;
        ensure!(
            node_pids()?.contains(&child_pid),
            "child adapter PID must still belong directly to this test"
        );
        let after = self.state.store.max_event_id()?;
        // Only the node process started after this test's parent adapter was
        // recorded. Its supervisor exercises the real crash/loss/coalescing path.
        ensure!(
            unsafe { libc::kill(i32::try_from(child_pid)?, libc::SIGKILL) } == 0,
            "kill own child adapter"
        );
        self.wait("real child adapter failure", || {
            self.state
                .store
                .find_session(child)
                .ok()
                .flatten()
                .filter(|row| row.lifecycle == "parked" || row.lifecycle == "failed")
        })
        .await?;
        self.wait("child failure produces a durable notice",||{
            let conn = hennery_kernel::db::open(&self.dir.path().join("hennery.db")).ok()?;
            conn.query_row("SELECT COUNT(*) = 1 FROM delegation_notices WHERE owner_id=?1 AND child_id=?2 AND source_event_id>?3 AND kind='session_unavailable'",rusqlite::params![self.state.store.owner_id(),child,after],|row|row.get::<_,bool>(0)).ok()?.then_some(())
        }).await?;
        println!("{}", json!({"case":"actual_child_adapter_failure","passed":true}));

        self.idle(&self.parent).await?;
        self.action(&self.parent, "close").await?;
        ensure!(
            self.state
                .store
                .find_session(&self.parent)?
                .context("closed parent")?
                .lifecycle
                == "closed",
            "parent must actually close"
        );
        self.action(child, "resume").await?;
        self.idle(child).await?;
        let after = self.state.store.max_event_id()?;
        let turn = self.prompt(child, question_brief(child_runtime)).await?;
        let question = self
            .question_for_turn(
                child,
                &turn,
                after,
                PendingKind::Elicitation,
                "closed-parent child elicitation",
            )
            .await?;
        self.wait("closed-parent question falls back to operator", || {
            self.state
                .store
                .operator_attention(None, "")
                .ok()?
                .into_iter()
                .find(|attention| attention.pending_id.as_deref() == Some(&question.pending_id))
        })
        .await?;
        ensure!(
            self.state
                .store
                .find_session(&self.parent)?
                .context("closed parent after fallback")?
                .lifecycle
                == "closed",
            "fallback must not reopen the parent"
        );
        println!("{}", json!({"case":"closed_parent_operator_fallback","passed":true}));
        Ok(())
    }
}

fn node_pids() -> Result<Vec<u32>> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()?;
    ensure!(output.status.success(), "inspect own adapter PIDs");
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent: u32 = fields.next()?.parse().ok()?;
            let binary = fields.next()?;
            (parent == std::process::id() && Path::new(binary).file_name()?.to_str()? == "node").then_some(pid)
        })
        .collect())
}

fn process_token_exposure() -> Result<Vec<Value>> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()?;
    ensure!(output.status.success(), "inspect own process tree");
    let rows: Vec<(u32, u32, String)> = String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.to_owned(),
            ))
        })
        .collect();
    let mut own = vec![std::process::id()];
    loop {
        let before = own.len();
        for (pid, parent, _) in &rows {
            if own.contains(parent) && !own.contains(pid) {
                own.push(*pid);
            }
        }
        if own.len() == before {
            break;
        }
    }
    let mut result = vec![];
    for (pid, _, binary) in rows {
        if !own.contains(&pid)
            || !Path::new(&binary)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "node" | "codex" | "claude"))
        {
            continue;
        }
        let output = std::process::Command::new("/bin/ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
            .output()?;
        if !output.status.success() {
            continue;
        }
        let arguments = String::from_utf8(output.stdout)?;
        let role = if arguments.contains("@agentclientprotocol/claude-agent-acp/") {
            "claude_adapter"
        } else if arguments.contains("@agentclientprotocol/codex-acp/") {
            "codex_adapter"
        } else {
            Path::new(&binary)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("runtime")
        };
        // Raw argv never leaves this function. Only presence booleans are evidence.
        result.push(json!({"process":role,"bearer_in_argv":arguments.to_ascii_lowercase().contains("bearer "),"authorization_header_in_argv":arguments.to_ascii_lowercase().contains("authorization")}));
    }
    Ok(result)
}

fn scrub_capture(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let key = key.to_ascii_lowercase();
                if key.contains("authorization")
                    || key.contains("token")
                    || matches!(
                        key.as_str(),
                        "headers" | "http_headers" | "cookie" | "set-cookie" | "api_key"
                    )
                {
                    *value = json!("[redacted]");
                } else {
                    scrub_capture(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                scrub_capture(value);
            }
        }
        Value::String(text) if text.to_ascii_lowercase().contains("bearer") => *value = json!("[redacted bearer]"),
        _ => {}
    }
}

fn scrub_events(events: Vec<hennery_proto::rest::EventDto>) -> Vec<hennery_proto::rest::EventDto> {
    events
        .into_iter()
        .filter(|event| {
            event.body.pointer("/payload/update/sessionUpdate") != Some(&json!("available_commands_update"))
        })
        .map(|mut event| {
            scrub_capture(&mut event.body);
            event
        })
        .collect()
}

#[test]
fn captured_acp_removes_headers_and_bearers_before_retention() {
    let mut value = json!({"headers":[{"name":"Authorization","value":"private"}],"nested":{"Authorization":"private","access_token":"private","note":"Bearer private"},"safe":"CHILD_ACCEPTANCE_OK"});
    scrub_capture(&mut value);
    assert!(!value.to_string().contains("private"));
    assert_eq!(value["safe"], "CHILD_ACCEPTANCE_OK");
}

#[test]
fn recorded_live_schema_answer_preserves_delivery_and_parent_provenance() {
    use hennery_proto::{
        frames::PendingKind,
        plugin::InputOrigin,
        rest::{EventDto, PendingState},
    };
    use hennery_view::{Body, Request};
    let events: Vec<EventDto> = include_str!("fixtures/delegation-codex-acp-1.13.0.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .all(|event| event.body.pointer("/payload/update/sessionUpdate")
                != Some(&json!("available_commands_update"))),
        "public fixture must exclude this machine's command menu"
    );
    let items = hennery_view::fold(&events, &|_: &str| false);
    let questions: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let Body::Question(question) = &item.body {
                Some(question)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(questions.len(), 1);
    let question = questions[0];
    assert_eq!(question.question_kind, PendingKind::Elicitation);
    assert_eq!(question.state, PendingState::Delivered);
    assert_eq!(question.delivered, Some(true));
    assert!(question.answered);
    assert!(!question.answerable);
    assert!(matches!(&question.request,Request::Elicitation{form_supported:true,fields,..} if !fields.is_empty()));
    for kind in ["user_turn", "answer_submitted"] {
        let event = events.iter().find(|event| event.kind == kind).unwrap();
        assert!(matches!(
            serde_json::from_value::<InputOrigin>(event.body["origin"].clone()).unwrap(),
            InputOrigin::Parent { .. }
        ));
    }
    assert!(assistant_text(&events).contains("CHILD_ACCEPTANCE_OK"));
    let mut menu = events[0].clone();
    menu.kind = "acp_update".into();
    menu.body = json!({"payload":{"update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"private personal command menu"}]}}});
    let public = scrub_events(vec![
        menu,
        events
            .iter()
            .find(|event| event.kind == "pending_opened")
            .unwrap()
            .clone(),
    ]);
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].kind, "pending_opened");
    assert!(
        !serde_json::to_string(&public)
            .unwrap()
            .contains("private personal command menu")
    );
}

fn question_brief(runtime: &str) -> &'static str {
    if runtime == "claude" {
        "This is an actual tool acceptance test. You MUST immediately call AskUserQuestion to ask one required single-choice question with options blue and green, named acceptance_color. You do not know the answer. Do not choose a color yourself, infer it, or ask in plain text. Wait for the real tool response before replying CHILD_ACCEPTANCE_OK followed by the returned color. If the question tool is unavailable, reply ELICITATION_UNAVAILABLE; never pretend you called it. Do not read/write files or run commands."
    } else {
        "This is an actual tool acceptance test. You are in Plan mode. You MUST immediately call the request_user_input tool to ask one required single-choice question named acceptance_color with options blue and green. You do not know the answer. Do not choose a color yourself, infer it, or ask in plain text. Wait for the real tool response before replying CHILD_ACCEPTANCE_OK followed by the returned color. If the question tool is unavailable, reply ELICITATION_UNAVAILABLE; never pretend you called it. Do not read/write files or run commands."
    }
}

fn scrub_error(message: &str) -> Result<String> {
    let text = redact_credentials(message)?;
    if text.to_ascii_lowercase().contains("bearer") {
        return Ok("[redacted bearer-bearing error]".into());
    }
    Ok(hennery_kernel::redact::text(&text)
        .unwrap_or(text)
        .chars()
        .take(2000)
        .collect())
}

fn redact_credentials(message: &str) -> Result<String> {
    fn strings(value: &Value, output: &mut Vec<String>) {
        match value {
            Value::String(text) if text.len() >= 8 => output.push(text.clone()),
            Value::Array(values) => {
                for value in values {
                    strings(value, output);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    strings(value, output);
                }
            }
            _ => {}
        }
    }
    let mut secrets = vec![];
    for variable in ["HENNERY_D4_CLAUDE_AUTH", "HENNERY_D4_CODEX_AUTH"] {
        if let Ok(path) = std::env::var(variable) {
            let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
            strings(&value, &mut secrets);
        }
    }
    secrets.sort_by_key(|text| std::cmp::Reverse(text.len()));
    let mut clean = message.to_owned();
    for secret in secrets {
        clean = clean.replace(&secret, "[credential]");
    }
    Ok(clean)
}

fn builtin_permission(payload: &Value, updates: &[Value]) -> bool {
    let claude = payload
        .pointer("/toolCall/_meta/claudeCode/toolName")
        .and_then(Value::as_str)
        .is_some_and(|name| TOOLS.iter().any(|tool| name == format!("mcp__hennery__{tool}")));
    let codex = payload.pointer("/_meta/is_mcp_tool_approval") == Some(&Value::Bool(true))
        && payload
            .pointer("/toolCall/toolCallId")
            .and_then(Value::as_str)
            .is_some_and(|id| {
                updates.iter().any(|update| {
                    update["toolCallId"] == id
                        && update["_meta"]["is_mcp_tool_call"] == true
                        && update["rawInput"]["server"] == "hennery"
                        && update["rawInput"]["tool"]
                            .as_str()
                            .is_some_and(|tool| TOOLS.contains(&tool))
                })
            });
    claude || codex
}

fn assistant_text(events: &[hennery_proto::rest::EventDto]) -> String {
    events
        .iter()
        .filter_map(|event| event.body.pointer("/payload/update"))
        .filter(|update| update["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|update| update.pointer("/content/text").and_then(Value::as_str))
        .collect()
}

#[test]
fn parent_operator_approval_is_limited_to_correlated_builtin_tools() {
    let call = json!({"toolCallId":"call","_meta":{"is_mcp_tool_call":true},"rawInput":{"server":"hennery","tool":"session_spawn"}});
    let permission = json!({"toolCall":{"toolCallId":"call"},"_meta":{"is_mcp_tool_approval":true}});
    assert!(builtin_permission(&permission, std::slice::from_ref(&call)));
    assert!(builtin_permission(
        &json!({"toolCall":{"_meta":{"claudeCode":{"toolName":"mcp__hennery__session_spawn"}}}}),
        &[]
    ));
    for denied in [
        json!({"toolCall":{"toolCallId":"other"},"_meta":{"is_mcp_tool_approval":true}}),
        json!({"toolCall":{"toolCallId":"call"}}),
        json!({"toolCall":{"title":"mcp__hennery__session_spawn","_meta":{"claudeCode":{"toolName":"Bash"}}}}),
    ] {
        assert!(!builtin_permission(&denied, std::slice::from_ref(&call)));
    }
    for denied in [
        json!({"toolCallId":"call","_meta":{"is_mcp_tool_call":true},"rawInput":{"server":"foreign","tool":"session_spawn"}}),
        json!({"toolCallId":"call","_meta":{"is_mcp_tool_call":true},"rawInput":{"server":"hennery","tool":"unknown"}}),
        json!({"toolCallId":"call","rawInput":{"server":"hennery","tool":"session_spawn"}}),
    ] {
        assert!(!builtin_permission(&permission, &[denied]));
    }
}

#[tokio::test]
#[ignore = "requires explicit pinned adapters and provider credentials; performs inference"]
async fn pinned_runtime_provider_authentication() -> Result<()> {
    let runtime = runtime("HENNERY_D4_PARENT")?;
    let h = Harness::new(&runtime, None).await?;
    let result = async {
        let turn = h
            .prompt(
                &h.parent,
                "Reply with exactly HENNERY_PROVIDER_OK. Do not call any tools.",
            )
            .await?;
        let outcome = h
            .wait("provider authentication", || {
                h.state.store.ended_turn_outcome(&turn).ok().flatten()
            })
            .await?;
        if outcome != hennery_proto::frames::TurnOutcome::Completed {
            h.diagnostics("provider failed")?;
            bail!("provider turn outcome {outcome:?}");
        }
        let events = h.state.store.events(&h.parent, 0, 1000)?;
        ensure!(
            assistant_text(&events).contains("HENNERY_PROVIDER_OK"),
            "provider did not return the acceptance marker"
        );
        println!(
            "{}",
            json!({"runtime":runtime,"provider_authentication":true,"outcome":outcome})
        );
        Ok(())
    }
    .await;
    let cleanup = h.finish().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires pinned Claude adapter and copied provider credentials; performs one inference"]
async fn live_operator_model_switch_allows_queued_and_subsequent_prompts() -> Result<()> {
    let h = Harness::new_with_worker("claude", None, false).await?;
    let result = async {
        let before = h.state.store.catalog(&h.parent)?.context("initial catalog")?;
        let model = before.config_options.iter()
            .find(|option| option["category"] == "model").context("model option")?;
        let id = model["id"].as_str().context("model option id")?;
        let target = model["options"].as_array().context("model choices")?.iter()
            .filter_map(|option| option["value"].as_str())
            .find(|value| Some(*value) != before.current.model.as_deref() && matches!(*value, "haiku" | "sonnet"))
            .context("an alternative supported low-cost model")?;
        let response = h.client.post(format!("{}/api/sessions/{}/config", h.url, h.parent))
            .json(&json!({"config_id":id,"value":target})).send().await?;
        ensure!(response.status() == reqwest::StatusCode::ACCEPTED, "model switch HTTP {}", response.status());
        let after = h.state.store.catalog(&h.parent)?.context("switched catalog")?;
        ensure!(after.current.model.as_deref() == Some(target), "adapter did not confirm selected model");
        let checked = hennery_sessions::content::check(vec![json!({"type":"text","text":"queued model-switch probe"})])
            .map_err(|_| anyhow::anyhow!("invalid synthetic queue probe"))?;
        let queue = h.state.store.enqueue_operator_prompt(&h.parent, &checked)?;
        h.state.store.refresh_delegation_attention(&h.state.hub, hennery_kernel::secret::unix_now() + 31)?;
        ensure!(h.state.store.find_session(&h.parent)?.context("session")?.configuration_error.is_none(), "live switch left a stale configuration mismatch");
        ensure!(h.state.store.operator_prompt_queue(&h.parent)?[0].state == "queued", "queued prompt failed after model switch");
        ensure!(h.state.store.remove_operator_prompt(&h.parent, &queue)?, "remove inference-free queue probe");
        let turn = h.prompt(&h.parent, "Reply with exactly HENNERY_MODEL_SWITCH_OK. Do not call tools.").await?;
        let outcome = h.wait("model-switch provider turn", || h.state.store.ended_turn_outcome(&turn).ok().flatten()).await?;
        ensure!(outcome == hennery_proto::frames::TurnOutcome::Completed, "provider turn failed");
        ensure!(assistant_text(&h.state.store.events(&h.parent, 0, 1000)?).contains("HENNERY_MODEL_SWITCH_OK"), "provider did not return the marker");
        println!("{}", json!({"case":"operator_model_switch","before":before.current,"after":after.current,"queue_preserved":true,"provider_turn_completed":true}));
        Ok(())
    }.await;
    if result.is_err() {
        h.diagnostics("operator model switch failed")?;
    }
    let cleanup = h.finish().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires explicit pinned adapter and staged auth; starts no provider turn"]
async fn pinned_runtime_process_token_exposure_without_inference() -> Result<()> {
    let runtime = runtime("HENNERY_D4_PARENT")?;
    let h = Harness::new(&runtime, None).await?;
    let result=async {
        ensure!(h.state.store.find_session(&h.parent)?.context("attached session")?.open_turn_id.is_none(),"exposure inspection must start no turn");
        ensure!(!h.state.store.events(&h.parent,0,1000)?.iter().any(|event|event.kind=="user_turn"),"exposure inspection must submit no prompt");
        h.wait("pinned inputSchema discovery", || h.trace.lock().unwrap().iter().any(|r|r["caller"]==h.parent && r["method"]=="tools/list").then_some(())).await?;
        ensure!(h.trace.lock().unwrap().iter().filter(|r|r["method"]=="tools/list").all(|r|r["schemas_valid"]==true), "pinned adapter received an invalid inputSchema contract");
        println!("{}",json!({"case":"process_token_exposure","runtime":runtime,"advertised_input_schemas_valid":true,"provider_inference":false,"processes":process_token_exposure()?}));
        Ok(())
    }.await;
    let cleanup = h.finish().await;
    result.and(cleanup)
}

fn requested(catalog: &HostCatalog) -> SessionConfig {
    let mut config = catalog.defaults.clone();
    // A probe's default model can advertise axes absent on the chosen model.
    // Request only the cost controls selected below, never unrelated defaults.
    config.axes.clear();
    if let Some(model) = catalog
        .config_options
        .iter()
        .find(|option| option["category"] == "model")
        .and_then(|option| option["options"].as_array())
        .and_then(|options| {
            options
                .iter()
                .filter_map(|option| option["value"].as_str())
                .find(|model| matches!(*model, "sonnet" | "gpt-6-sol"))
        })
    {
        config.model = Some(model.into());
    }
    // Independent selection comes exclusively from this child's actual catalog.
    // Exercise a non-default effort when it is offered; never invent an axis.
    for option in &catalog.config_options {
        let Some(id) = option["id"].as_str() else { continue };
        if (id.contains("effort") || id.contains("reasoning"))
            && let Some(value) = option["options"].as_array().and_then(|options| {
                options
                    .iter()
                    .find_map(|o| o["value"].as_str().filter(|v| matches!(*v, "low" | "medium")))
            })
        {
            config
                .axes
                .insert(id.into(), hennery_proto::frames::ConfigValue::Id(value.into()));
        }
    }
    if catalog.agent == "codex" {
        config.mode = Some("read-only".into());
        config.axes.insert(
            "collaboration_mode".into(),
            hennery_proto::frames::ConfigValue::Id("plan".into()),
        );
    }
    config
}

fn parent_configuration(catalog: &HostCatalog, effort: Option<&str>) -> Result<SessionConfig> {
    let mut config = requested(catalog);
    if catalog.agent == "codex" {
        config.axes.insert(
            "collaboration_mode".into(),
            hennery_proto::frames::ConfigValue::Id("default".into()),
        );
    }
    if let Some(effort) = effort {
        ensure!(
            catalog.agent == "codex" && matches!(effort, "low" | "medium"),
            "parent effort override requires Codex and low or medium"
        );
        ensure!(
            catalog
                .config_options
                .iter()
                .any(|option| option["id"] == "reasoning_effort"
                    && option["options"]
                        .as_array()
                        .is_some_and(|options| options.iter().any(|option| option["value"] == effort))),
            "parent effort is absent from pinned catalog"
        );
        config.axes.insert(
            "reasoning_effort".into(),
            hennery_proto::frames::ConfigValue::Id(effort.into()),
        );
    }
    Ok(config)
}

fn busy_parent_prompt() -> &'static str {
    "New operator acceptance task. This supersedes earlier instructions to stop after spawning or avoid polling. Call the session_list tool from the hennery MCP server exactly once with arguments {}. Make an actual MCP tool call. If approval is requested, wait for the operator's response. Do not simulate tool output or finish before the actual call returns. Then reply exactly BUSY_RELEASED. Do not call any other tools."
}

#[test]
fn busy_parent_prompt_specifies_new_task_tool_arguments_and_real_approval() {
    let prompt = busy_parent_prompt();
    for text in ["supersedes", "session_list", "{}", "approval", "BUSY_RELEASED"] {
        assert!(prompt.contains(text), "missing {text}");
    }
}

#[test]
fn codex_parent_medium_is_explicit_catalog_checked_and_does_not_raise_child_effort() {
    let catalog = HostCatalog {
        saved_at: None,
        agent: "codex".into(),
        adapter_revision: String::new(),
        config_options: vec![
            json!({"category":"model","options":[{"value":"gpt-6-sol"}]}),
            json!({"id":"reasoning_effort","options":[{"value":"low"},{"value":"medium"}]}),
        ],
        defaults: SessionConfig {
            model: Some("gpt-6-sol".into()),
            mode: Some("read-only".into()),
            axes: Default::default(),
        },
        security: hennery_proto::delegation_host::SecurityClassification {
            known: true,
            axes: Default::default(),
        },
        freshness: hennery_proto::delegation_host::CatalogFreshness::Fresh,
    };
    let medium = parent_configuration(&catalog, Some("medium")).unwrap();
    assert_eq!(
        medium.axes["reasoning_effort"],
        hennery_proto::frames::ConfigValue::Id("medium".into())
    );
    assert_eq!(medium.model.as_deref(), Some("gpt-6-sol"));
    assert_eq!(
        requested(&catalog).axes["reasoning_effort"],
        hennery_proto::frames::ConfigValue::Id("low".into())
    );
    assert!(parent_configuration(&catalog, Some("ultra")).is_err());
    let mut unavailable = catalog;
    unavailable.config_options[1]["options"] = json!([{"value":"low"}]);
    assert!(parent_configuration(&unavailable, Some("medium")).is_err());
}

#[tokio::test]
#[ignore = "requires pinned Codex; adapter-local /mcp only, audited zero provider turns"]
async fn pinned_codex_busy_tools_without_inference() -> Result<()> {
    let h = Harness::with_probe("codex", None, false, true).await?;
    let pause = h.dir.path().join("pause-mcp-status");
    let result=async {
        std::fs::write(&pause,[])?;
        h.approve_builtin.store(false,Ordering::SeqCst);
        let turn=h.prompt(&h.parent,"/mcp").await?;
        let held=h.wait("native local MCP status held during busy turn",||{
            if h.state.store.ended_turn_outcome(&turn).ok().flatten().is_some() {return Some(Err(anyhow::anyhow!("local /mcp ended without the audited native status request")));}
            let row=h.state.store.find_session(&h.parent).ok().flatten()?;
            if row.open_turn_id.as_deref()!=Some(&turn) {return None;}
            std::fs::read(h.dir.path().join("held-mcp-status.json")).ok().and_then(|bytes|serde_json::from_slice::<Value>(&bytes).ok()).map(Ok)
        }).await??;
        ensure!(held["thread_scoped"]==true && held["schemas_valid"]==true,"native busy MCP status has different schemas: {held}");
        ensure!(held["names"].as_array().is_some_and(|names|names.iter().any(|name|name=="session_list")),"session_list absent during busy native command");
        let token=h.probe_tokens.lock().unwrap().get(&h.parent).context("probe-only in-memory bearer")?.clone();
        let response=h.client.post(format!("{}/mcp/builtin/hennery",h.url)).bearer_auth(token)
            .header("accept","application/json, text/event-stream")
            .header("mcp-protocol-version","2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).send().await?;
        ensure!(response.status()==reqwest::StatusCode::OK,"busy discovery refused: {}",response.status());
        ensure!(valid_tool_schemas(&response.json::<Value>().await?["result"]["tools"]),"busy discovery schema changed");
        ensure!(h.state.store.open_pending(&h.parent)?.is_empty(),"local status must request no provider permission");
        std::fs::remove_file(&pause)?;
        h.wait("local status turn completes",||h.state.store.ended_turn_outcome(&turn).ok().flatten()).await?;
        let methods:Vec<String>=serde_json::from_slice(&std::fs::read(h.dir.path().join("native-methods.json"))?)?;
        ensure!(methods.iter().any(|method|method=="mcpServerStatus/list"),"native status not queried");
        ensure!(!methods.iter().any(|method|matches!(method.as_str(),"turn/start"|"review/start")),"probe invoked provider inference");
        println!("{}",json!({"case":"native_busy_tools_without_inference","native_tools":held["names"],"advertised_input_schemas_valid":true,"provider_turn_starts":0,"busy_tools_available":true,"automatic_approval_enabled":false}));
        Ok(())
    }.await;
    if pause.exists() {
        std::fs::remove_file(pause)?;
    }
    let cleanup = h.finish().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires explicit pinned Claude adapter and scratch auth; performs no inference"]
async fn pinned_claude_child_configuration_without_inference() -> Result<()> {
    claude_child_configuration_without_inference(false).await
}

#[tokio::test]
#[ignore = "requires explicit pinned Claude adapter and scratch auth; performs no inference"]
async fn pinned_claude_unsupported_fast_without_inference() -> Result<()> {
    claude_child_configuration_without_inference(true).await
}

async fn claude_child_configuration_without_inference(explicit_fast: bool) -> Result<()> {
    let h = Harness::new_with_worker("claude", Some("claude"), false).await?;
    let result = async {
        let catalog = h.catalog("claude").await?;
        let mut config = requested(&catalog);
        if explicit_fast { config.axes.insert("fast".into(),hennery_proto::frames::ConfigValue::Bool(false)); }
        let identity = h.wait("parent discovery identity", || h.identities.lock().unwrap().get(&h.parent).cloned()).await?;
        let mcp = || -> Result<hennery_sessions::store::McpContext> {
            let (capable,isolation) = h.state.hub.mcp_isolation(HOST,"claude").context("Claude isolation")?;
            Ok(hennery_sessions::store::McpContext { capable,isolation,rules_name_other_hats:h.state.hosts.rules_name_other_hats(HOST)? })
        };
        let receipt: hennery_proto::delegation::OperationReceipt = serde_json::from_value(
            hennery_sessions::session_tools::execute(&h.state,&identity,identity.delivered_delegation_revision().context("delivered grant")?,"session_spawn",
                json!({"operation_key":"config-only-probe","agent":"claude","cwd":h.dir.path().canonicalize()?,"brief":"Configuration probe: this brief must never be dispatched","name":"configuration probe","config":config})
            ).await?
        )?;
        let child = receipt.child_id.context("allocated child")?;
        ensure!(h.state.store.attach_delegated(&child,mcp()?,&catalog,&h.state.hub)?,"attach handed off");
        let evidence = h.wait("configuration-only child readback", || {
            if let Some(row) = h.state.store.find_session(&child).ok().flatten().filter(|row|row.lifecycle=="failed") {
                return Some(Err(row.failure_reason.unwrap_or_default()));
            }
            let conn = hennery_kernel::db::open(&h.dir.path().join("hennery.db")).ok()?;
            conn.query_row("SELECT evidence FROM session_attach_evidence WHERE owner_id=?1 AND session_id=?2 AND evidence IS NOT NULL",rusqlite::params![h.state.store.owner_id(),child],|r|r.get::<_,String>(0)).optional().ok().flatten().map(Ok)
        }).await?;
        if let Err(reason) = evidence {
            ensure!(explicit_fast && reason=="configuration_unavailable","unexpected configuration refusal: {}",scrub_error(&reason)?);
            let page = h.state.store.family_page(&h.parent,"",100)?.context("operator family")?;
            let failed = page.children.iter().find(|row|row.summary.session.session_id==child).context("operator child failure")?;
            ensure!(failed.operations.iter().any(|op|op.state=="failed" && op.code.as_deref()==Some("configuration_unavailable")),"parent/operator must see failed receipt");
            ensure!(!h.state.store.events(&child,0,1000)?.iter().any(|e|matches!(e.kind.as_str(),"user_turn"|"turn_started")),"refused child must run no brief");
            h.diagnostics("explicit fast refused before brief")?;
            println!("{}",json!({"case":"unsupported_fast_refused_without_inference","requested":config,"failure_reason":reason,"parent_and_operator_receipt_failed":true,"provider_inference":false}));
            return Ok(());
        }
        let evidence = evidence.map_err(|reason|anyhow::anyhow!(reason))?;
        let evidence: hennery_proto::delegation_host::ConfigEvidence = serde_json::from_str(&evidence)?;
        let actual_catalog = h.state.store.catalog(&child)?.context("child catalog")?;
        let events = h.state.store.events(&child,0,1000)?;
        println!("{}",json!({"case":"configuration_without_inference","requested":config,"actual":evidence.actual,"actual_config_options":actual_catalog.config_options,"host_notes":events.iter().filter(|e|e.kind=="host_note").map(|e|e.body.clone()).collect::<Vec<_>>(),"provider_inference":false}));
        ensure!(!explicit_fast,"unsupported explicit fast must be refused");
        ensure!(!events.iter().any(|e|matches!(e.kind.as_str(),"user_turn"|"turn_started")),"configuration probe must dispatch no brief");
        ensure!(config.axes.iter().all(|(key,value)|evidence.actual.axes.get(key)==Some(value)),"requested versus actual child axes differ");
        Ok(())
    }.await;
    if result.is_err() {
        h.diagnostics("configuration-only failed")?;
    }
    let cleanup = h.finish().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires explicit pinned parent/child adapters and provider credentials; performs inference"]
async fn pinned_parent_child_provider_matrix() -> Result<()> {
    let parent = runtime("HENNERY_D4_PARENT")?;
    let child_runtime = runtime("HENNERY_D4_CHILD")?;
    let h = Harness::new(&parent, Some(&child_runtime)).await?;
    let result = async {
        let catalog = h.catalog(&child_runtime).await?;
        let config = requested(&catalog);
        ensure!(hennery_sessions::delegation_runtime::config_offered(&catalog, &config), "chosen child configuration absent from actual catalog");
        let brief = question_brief(&child_runtime);
        let parent_nodes = node_pids()?;
        let arguments = json!({"operation_key":"d4-live-spawn","cwd":h.dir.path().canonicalize()?,"agent":child_runtime,"brief":brief,"config":config,"name":"acceptance child"});
        let prompt = format!("Use your hennery MCP tools for this acceptance. First call session_capabilities with agent={child_runtime}. Then call session_spawn EXACTLY ONCE with this JSON: {arguments}. Do not use another subagent facility. If a child ordinary question arrives, answer it with session_answer: action accept and content matching its schema, selecting blue. For an ordinary question the acceptance color is blue. Do not answer permission requests, run shell commands, or read/write files. After spawning, end your turn with SPAWN_ACCEPTED; the durable system will deliver the child's question. Do not poll or close the child.");
        h.prompt(&h.parent, &prompt).await?;
        let child = h.wait("real parent spawn", || h.child()).await?;
        let evidence = h.wait("child configuration readback", || {
            let conn = hennery_kernel::db::open(&h.dir.path().join("hennery.db")).ok()?;
            conn.query_row("SELECT evidence FROM session_attach_evidence WHERE owner_id=?1 AND session_id=?2 AND evidence IS NOT NULL", rusqlite::params![h.state.store.owner_id(),child], |r| r.get::<_,String>(0)).optional().ok().flatten()
        }).await?;
        let evidence: hennery_proto::delegation_host::ConfigEvidence = serde_json::from_str(&evidence)?;
        println!("{}", json!({"case":"child_configuration_comparison","requested":config,"actual":evidence.actual,"completeness":evidence.completeness}));
        ensure!(evidence.completeness == hennery_proto::delegation_host::ConfigCompleteness::Complete, "child configuration not completely read back");
        ensure!(config.model.as_ref().is_none_or(|v| evidence.actual.model.as_ref() == Some(v)), "child model readback differs");
        ensure!(config.mode.as_ref().is_none_or(|v| evidence.actual.mode.as_ref() == Some(v)), "child permission mode readback differs");
        ensure!(config.axes.iter().all(|(k,v)| evidence.actual.axes.get(k) == Some(v)), "child effort readback differs");
        let child_pid = h.wait("own child adapter PID", || { let candidates:Vec<_> = node_pids().ok()?.into_iter().filter(|pid|!parent_nodes.contains(pid)).collect(); (candidates.len() == 1).then(||candidates[0]) }).await?;
        let pending = h.wait("genuine child elicitation", || h.question(&child,0,hennery_proto::frames::PendingKind::Elicitation)).await?;
        h.wait("real parent answer delivered", || h.state.store.pending_item(&pending.pending_id).ok().flatten().filter(|p| p.delivered == Some(true))).await?;
        h.wait("child provider continuation", || {
            let events = h.state.store.events(&child, 0, 1000).ok()?;
            assistant_text(&events).contains("CHILD_ACCEPTANCE_OK").then_some(())
        }).await?;
        let events = h.state.store.events(&child, 0, 1000)?;
        let brief = events.iter().find(|event| event.kind == "user_turn").context("child brief event")?;
        ensure!(events.iter().any(|event| event.event_id < brief.event_id && event.body.pointer("/indexed/config_evidence/completeness") == Some(&json!("complete"))), "actual complete configuration readback did not precede the brief");
        let conn = hennery_kernel::db::open(&h.dir.path().join("hennery.db"))?;
        let origin: String = conn.query_row("SELECT input_origin FROM answer_queue WHERE owner_id=?1 AND pending_id=?2 AND delivered=1", rusqlite::params![h.state.store.owner_id(),pending.pending_id], |row| row.get(0))?;
        ensure!(matches!(serde_json::from_str::<hennery_proto::plugin::InputOrigin>(&origin)?, hennery_proto::plugin::InputOrigin::Parent {session_id,..} if session_id == h.parent), "delivered schema answer did not originate from the actual parent");
        let trace = h.trace.lock().unwrap().clone();
        for id in [&h.parent, &child] {
            ensure!(trace.iter().any(|r| r["caller"] == *id && r["method"] == "tools/list" && r["tools"].as_array().is_some_and(|t| t.len() == 7) && r["schemas_valid"] == true), "actual participant did not discover all seven precise inputSchemas");
        }
        for tool in ["session_capabilities", "session_spawn", "session_answer"] {
            ensure!(trace.iter().any(|r| r["caller"] == h.parent && r["tool"] == tool && r["is_error"] == false), "real parent did not successfully call {tool}");
        }
        println!("{}", json!({"case":"base_parent_child_loop","parent_runtime":parent,"child_runtime":child_runtime,"spawn":true,"real_elicitation":true,"parent_answer_delivered":true,"child_continued":true,"requested_config":config,"actual_config":evidence,"mcp":trace}));
        h.capture(&child)?;
        println!("{}",json!({"case":"process_token_exposure","processes":process_token_exposure()?}));
        h.extended(&child,&child_runtime,child_pid).await?;
        println!("{}",json!({"parent_runtime":parent,"child_runtime":child_runtime,"base_loop":true,"extended_lifecycle_acceptance":true}));
        Ok(())
    }.await;
    if result.is_err() {
        h.diagnostics("acceptance failed")?;
    }
    let cleanup = h.finish().await;
    result.and(cleanup)
}
