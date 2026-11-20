//! Skills spec §8, phase 2: the host renders a session's delivered hook sets
//! per agent (Claude `_meta.claudeCode.options.settings.hooks`, Codex
//! `CODEX_CONFIG` plus the adapter's trust call), fails closed when it
//! cannot apply them, and acknowledges what it applied in
//! `Indexed.hook_sets_applied`.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_host::outbox::Outbox;
use hennery_host::profile::Profile;
use hennery_host::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_host::{HostConfig, run};
use hennery_proto::frames::{
    CollectorFrame, HookDelivery, HookSetApplied, HookSetDelivery, HostFrame, Indexed, SessionBody,
};
use hennery_proto::hooks::HookSetConfig;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_testkit::{FakeCodexHooks, FakeHookEntry, FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const KEY: &str = "/<session-flags>/config.toml:pre_tool_use:0:0";
const GUARD: &str = "/usr/local/bin/guard";
const OPERATOR: &str = "/usr/local/bin/operator";

/// The `currentHash` the fake lists for `command`.
fn hash(command: &str) -> String {
    use sha2::Digest;
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(command.as_bytes())))
}

fn set(id: &str, name: &str, hash: &str, hooks: Value) -> HookSetDelivery {
    HookSetDelivery {
        id: id.into(),
        name: name.into(),
        content_hash: hash.into(),
        hooks: serde_json::from_value::<HookSetConfig>(hooks).unwrap(),
    }
}

fn pre_tool_use() -> Value {
    json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": GUARD}]}]})
}

fn user_prompt_submit() -> Value {
    json!({"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "/usr/local/bin/log-prompt", "timeout": 5}]}]})
}

/// Two sets: one `PreToolUse` hook (Codex runs it), one `UserPromptSubmit`
/// hook (Codex does not).
fn delivery() -> HookDelivery {
    HookDelivery {
        sets: vec![
            set("hs-guard", "guard", "sha256:aaaa", pre_tool_use()),
            set("hs-prompts", "prompt-log", "sha256:bbbb", user_prompt_submit()),
        ],
    }
}

fn applied(sets: &[&HookSetDelivery]) -> Vec<HookSetApplied> {
    sets.iter()
        .map(|s| HookSetApplied {
            id: s.id.clone(),
            name: s.name.clone(),
            content_hash: s.content_hash.clone(),
        })
        .collect()
}

fn strict() -> Value {
    json!({"claudeCode": {"options": {"extraArgs": {"strict-mcp-config": ""}}}})
}

/// The fake lists the hooks of the `CODEX_CONFIG` it started with.
fn trusting(trusted: bool) -> Option<FakeCodexHooks> {
    Some(FakeCodexHooks {
        override_list: None,
        trusted,
    })
}

/// A session-flag `command` hook at `key`.
fn entry(key: &str, command: &str, matcher: Option<&str>) -> FakeHookEntry {
    FakeHookEntry {
        key: key.into(),
        handler_type: "command".into(),
        command: command.into(),
        matcher: matcher.map(str::to_string),
        timeout_sec: 600,
        current_hash: hash(command),
    }
}

/// The fake lists `entries`, whatever it started with.
fn listing(entries: Vec<FakeHookEntry>, trusted: bool) -> Option<FakeCodexHooks> {
    Some(FakeCodexHooks {
        override_list: Some(entries),
        trusted,
    })
}

/// Waits until the adapter whose `grandchild_pid_file` is `pid_file`, and
/// its process group, are gone.
async fn assert_adapter_gone(pid_file: &Path) {
    // The adapter leads its own process group: the group id is its pid.
    let adapter: i32 = std::fs::read_to_string(pid_file.with_extension("group"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let grandchild: i32 = std::fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
    assert!(!hennery_testkit::pid_alive(adapter), "the adapter still runs");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hennery_testkit::pid_alive(grandchild) {
        assert!(tokio::time::Instant::now() < deadline, "the adapter's group still runs");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn script_logging_to(path: &Path, codex_hooks: Option<FakeCodexHooks>) -> FakeScript {
    FakeScript {
        session_log: Some(path.to_string_lossy().into_owned()),
        codex_hooks,
        ..Default::default()
    }
}

fn fake(script: &FakeScript, env: &[(&str, &str)]) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    fake.env.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    fake
}

/// The session's first `session_started` or `start_failed`.
enum Outcome {
    Started(Box<Indexed>),
    Failed { code: String, message: String },
}

/// Launch one session actor with `hooks` and wait for its first
/// `session_started` or `start_failed`; the actor is closed before this
/// returns, so the adapter is gone before the log is read.
async fn attach(agent: AgentCommand, attach: Attach, profile: Profile, hooks: Option<HookDelivery>) -> Outcome {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        mcp_source_policy: None,
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach,
        config: Default::default(),
        agent,
        cwd: cwd(),
        profile,
        mcp_servers: vec![],
        hooks,
    };
    let options = SessionOptions {
        kill_grace: Duration::from_millis(200),
        ..Default::default()
    };
    let handle = session::launch(uplink.clone(), launch, options);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        let frames = uplink.pending().unwrap();
        let found = frames.into_iter().find_map(|frame| match frame {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(Outcome::Started(Box::new(indexed))),
            HostFrame::Session {
                body: SessionBody::StartFailed { code, message, .. },
                ..
            } => Some(Outcome::Failed { code, message }),
            _ => None,
        });
        if let Some(outcome) = found {
            break outcome;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let _ = handle.send(SessionCmd::Close {
        request_id: "r-close".into(),
    });
    handle.finished().await;
    outcome
}

fn started(outcome: Outcome) -> Indexed {
    match outcome {
        Outcome::Started(indexed) => *indexed,
        Outcome::Failed { code, message } => panic!("start failed: {code}: {message}"),
    }
}

fn failed(outcome: Outcome) -> (String, String) {
    match outcome {
        Outcome::Failed { code, message } => (code, message),
        Outcome::Started(indexed) => panic!("started: {indexed:?}"),
    }
}

fn cwd() -> std::path::PathBuf {
    std::fs::canonicalize(std::env::temp_dir()).unwrap()
}

/// Every line of the session log, as written.
fn logged(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn methods(lines: &[Value]) -> Vec<&str> {
    lines.iter().filter_map(|l| l["method"].as_str()).collect()
}

/// The `CODEX_CONFIG` the adapter ran with, parsed, if it had one.
fn codex_config(lines: &[Value]) -> Option<Value> {
    lines
        .iter()
        .find_map(|l| l["env"]["CODEX_CONFIG"].as_str())
        .map(|raw| serde_json::from_str(raw).unwrap())
}

fn params<'a>(lines: &'a [Value], method: &str) -> &'a Value {
    &lines
        .iter()
        .find(|l| l["method"] == method)
        .unwrap_or_else(|| panic!("no {method} in {lines:?}"))["params"]
}

#[tokio::test]
async fn claude_meta_keeps_strict_mcp_and_adds_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log, None);
    let indexed = started(attach(fake(&script, &[]), Attach::New, Profile::Claude, Some(delivery())).await);
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    started(attach(fake(&script, &[]), load, Profile::Claude, Some(delivery())).await);
    let delivery = delivery();
    assert_eq!(
        indexed.hook_sets_applied,
        applied(&[&delivery.sets[0], &delivery.sets[1]])
    );

    let lines = logged(&log);
    assert_eq!(methods(&lines), ["session/new", "session/load"], "{lines:?}");
    let mut merged = pre_tool_use();
    merged
        .as_object_mut()
        .unwrap()
        .extend(user_prompt_submit().as_object().unwrap().clone());
    for line in &lines {
        let meta = &line["params"]["_meta"];
        assert_eq!(
            meta["claudeCode"]["options"]["extraArgs"],
            strict()["claudeCode"]["options"]["extraArgs"]
        );
        assert_eq!(
            meta["claudeCode"]["options"]["settings"],
            json!({"hooks": merged}),
            "{meta}"
        );
        assert_eq!(
            meta["claudeCode"]["options"]["settings"]["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            GUARD
        );
    }
    assert_eq!(codex_config(&lines), None);
}

#[tokio::test]
async fn codex_gets_filtered_config_and_is_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log, trusting(true));
    let indexed = started(attach(fake(&script, &[]), Attach::New, Profile::Codex, Some(delivery())).await);
    let delivery = delivery();
    assert_eq!(indexed.hook_sets_applied, applied(&[&delivery.sets[0]]));

    let lines = logged(&log);
    assert_eq!(codex_config(&lines), Some(json!({"hooks": pre_tool_use()})));
    assert_eq!(
        methods(&lines),
        ["_codex/hooks/list", "_codex/hooks/trust", "session/new"],
        "{lines:?}"
    );
    let cwd = cwd().to_string_lossy().into_owned();
    assert_eq!(params(&lines, "_codex/hooks/list")["cwd"], cwd);
    assert_eq!(
        *params(&lines, "_codex/hooks/trust"),
        json!({"cwd": cwd, "hooks": [{"key": KEY, "currentHash": hash(GUARD)}]})
    );
    // Codex gets no Claude `_meta`.
    assert!(params(&lines, "session/new").get("_meta").is_none_or(Value::is_null));

    // A load is trusted the same way, before `session/load`.
    std::fs::remove_file(&log).unwrap();
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    started(attach(fake(&script, &[]), load, Profile::Codex, Some(delivery.clone())).await);
    assert_eq!(
        methods(&logged(&log)),
        ["_codex/hooks/list", "_codex/hooks/trust", "session/load"]
    );
}

#[tokio::test]
async fn an_operator_codex_config_is_merged_with_the_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log, trusting(true));
    let agent = fake(&script, &[("CODEX_CONFIG", r#"{"model":"x"}"#)]);
    started(attach(agent, Attach::New, Profile::Codex, Some(delivery())).await);
    assert_eq!(
        codex_config(&logged(&log)),
        Some(json!({"model": "x", "hooks": pre_tool_use()}))
    );

    // The operator's own hooks come first and are trusted with hennery's.
    std::fs::remove_file(&log).unwrap();
    let operator = json!({"hooks": [{"type": "command", "command": OPERATOR}]});
    let own = json!({"hooks": {"PreToolUse": [operator]}}).to_string();
    let agent = fake(&script, &[("CODEX_CONFIG", own.as_str())]);
    started(attach(agent, Attach::New, Profile::Codex, Some(delivery())).await);
    let lines = logged(&log);
    let mut groups = vec![operator];
    groups.extend(pre_tool_use()["PreToolUse"].as_array().unwrap().iter().cloned());
    assert_eq!(codex_config(&lines), Some(json!({"hooks": {"PreToolUse": groups}})));
    assert_eq!(
        params(&lines, "_codex/hooks/trust")["hooks"],
        json!([
            {"key": KEY, "currentHash": hash(OPERATOR)},
            {"key": "/<session-flags>/config.toml:pre_tool_use:1:0", "currentHash": hash(GUARD)}
        ])
    );

    std::fs::remove_file(&log).unwrap();
    let agent = fake(&script, &[("CODEX_CONFIG", "[]")]);
    let (code, message) = failed(attach(agent, Attach::New, Profile::Codex, Some(delivery())).await);
    assert_eq!(code, "hook_sets_refused", "{message}");
    // Refused before spawning: the fake never ran, so it logged nothing.
    assert_eq!(logged(&log), Vec::<Value>::new());
}

#[tokio::test]
async fn a_codex_trust_refusal_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let pid_file = dir.path().join("grandchild.pid");
    let mut script = script_logging_to(&log, trusting(false));
    script.grandchild_pid_file = Some(pid_file.to_string_lossy().into_owned());
    let (code, message) = failed(attach(fake(&script, &[]), Attach::New, Profile::Codex, Some(delivery())).await);
    assert_eq!(code, "hook_sets_refused", "{message}");
    assert_eq!(
        methods(&logged(&log)),
        ["_codex/hooks/list", "_codex/hooks/trust"],
        "no session was created"
    );
    assert_adapter_gone(&pid_file).await;
}

/// An agent command that sets its own `CODEX_CONFIG` (`/usr/bin/env
/// CODEX_CONFIG=… <cmd>`) replaces hennery's: the adapter lists other
/// hooks, and none of them is trusted.
#[tokio::test]
async fn a_listing_that_does_not_match_the_delivered_hooks_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let pid_file = dir.path().join("grandchild.pid");
    for keys in [
        vec![],
        vec!["/<session-flags>/config.toml:session_start:0:0"],
        vec![KEY, "/<session-flags>/config.toml:pre_tool_use:0:1"],
    ] {
        let _ = std::fs::remove_file(&log);
        let entries = keys.iter().map(|key| entry(key, GUARD, Some("Bash"))).collect();
        let mut script = script_logging_to(&log, listing(entries, true));
        script.grandchild_pid_file = Some(pid_file.to_string_lossy().into_owned());
        let (code, message) = failed(attach(fake(&script, &[]), Attach::New, Profile::Codex, Some(delivery())).await);
        assert_eq!(code, "hook_sets_refused", "{message}");
        assert!(message.contains("pre_tool_use:0:0"), "{message}");
        assert_eq!(
            methods(&logged(&log)),
            ["_codex/hooks/list"],
            "{keys:?}: nothing trusted"
        );
        assert_adapter_gone(&pid_file).await;
    }
}

/// The documented way to give Codex its own `CODEX_CONFIG` is an `env(1)`
/// wrapper (`/usr/bin/env CODEX_CONFIG=… <cmd>`): the value in the argument
/// is the one the adapter runs with, so hennery's hooks are merged into
/// that argument, after the operator's.
#[tokio::test]
async fn an_env_wrapper_codex_config_is_merged_in_the_argument() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log, trusting(true));
    // The operator's hook sits where hennery's would without the merge.
    let operator = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": OPERATOR}]});
    let own = json!({"model": "x", "hooks": {"PreToolUse": [operator]}});
    let mut agent = fake(&script, &[]);
    agent.args = vec![format!("CODEX_CONFIG={own}"), agent.program.clone()];
    agent.program = "/usr/bin/env".into();
    let indexed = started(attach(agent, Attach::New, Profile::Codex, Some(delivery())).await);
    let delivery = delivery();
    assert_eq!(indexed.hook_sets_applied, applied(&[&delivery.sets[0]]));

    let lines = logged(&log);
    let mut groups = vec![operator];
    groups.extend(pre_tool_use()["PreToolUse"].as_array().unwrap().iter().cloned());
    assert_eq!(
        codex_config(&lines),
        Some(json!({"model": "x", "hooks": {"PreToolUse": groups}}))
    );
    assert_eq!(
        params(&lines, "_codex/hooks/trust")["hooks"],
        json!([
            {"key": KEY, "currentHash": hash(OPERATOR)},
            {"key": "/<session-flags>/config.toml:pre_tool_use:1:0", "currentHash": hash(GUARD)}
        ])
    );
}

/// `env -i` drops the environment, hennery's `CODEX_CONFIG` with it: with
/// no `CODEX_CONFIG=` argument to merge into, the start is refused before
/// anything is spawned.
#[tokio::test]
async fn an_env_wrapper_that_drops_the_environment_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    for options in [vec!["-i"], vec!["-u", "CODEX_CONFIG"], vec!["--ignore-environment"]] {
        let mut args: Vec<String> = options.iter().map(|o| o.to_string()).collect();
        args.extend([
            "sh".into(),
            "-c".into(),
            format!("echo spawned >> {}", spawns.display()),
        ]);
        let agent = AgentCommand {
            program: "/usr/bin/env".into(),
            args,
            env: Vec::new(),
        };
        let (code, message) = failed(attach(agent, Attach::New, Profile::Codex, Some(delivery())).await);
        assert_eq!(code, "hook_sets_refused", "{options:?}: {message}");
        assert!(message.contains("environment"), "{message}");
    }
    assert!(!spawns.exists(), "an adapter was spawned");
}

/// An adapter that lists hooks at hennery's positions that are not the
/// ones it was given (another command, matcher or handler type) has
/// replaced them: nothing is trusted, the start fails, and the message
/// names the position, not the other command.
#[tokio::test]
async fn a_listing_with_other_commands_at_the_same_positions_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let pid_file = dir.path().join("grandchild.pid");
    let mut prompt = entry(KEY, GUARD, Some("Bash"));
    prompt.handler_type = "prompt".into();
    for (listed, what) in [
        (entry(KEY, "/usr/local/bin/other", Some("Bash")), "command"),
        (entry(KEY, GUARD, None), "matcher"),
        (entry(KEY, GUARD, Some("Edit")), "matcher"),
        (prompt, "handler type"),
    ] {
        let _ = std::fs::remove_file(&log);
        let mut script = script_logging_to(&log, listing(vec![listed], true));
        script.grandchild_pid_file = Some(pid_file.to_string_lossy().into_owned());
        let (code, message) = failed(attach(fake(&script, &[]), Attach::New, Profile::Codex, Some(delivery())).await);
        assert_eq!(code, "hook_sets_refused", "{message}");
        assert!(message.contains("pre_tool_use:0:0"), "{message}");
        assert!(message.contains(what), "{what}: {message}");
        assert!(!message.contains("/usr/local/bin/"), "{message}");
        assert_eq!(methods(&logged(&log)), ["_codex/hooks/list"], "{what}: nothing trusted");
        assert_adapter_gone(&pid_file).await;
    }
}

#[tokio::test]
async fn an_adapter_without_the_hooks_extension_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let pid_file = dir.path().join("grandchild.pid");
    let mut script = script_logging_to(&log, None);
    script.grandchild_pid_file = Some(pid_file.to_string_lossy().into_owned());
    let (code, message) = failed(attach(fake(&script, &[]), Attach::New, Profile::Codex, Some(delivery())).await);
    assert_eq!(code, "hook_sets_refused", "{message}");
    assert_eq!(methods(&logged(&log)), Vec::<&str>::new(), "no session was created");
    assert_adapter_gone(&pid_file).await;
}

#[tokio::test]
async fn no_hooks_means_no_settings_and_no_trust_calls() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log, trusting(true));
    let indexed = started(attach(fake(&script, &[]), Attach::New, Profile::Claude, None).await);
    assert_eq!(indexed.hook_sets_applied, vec![]);
    let indexed = started(attach(fake(&script, &[]), Attach::New, Profile::Codex, None).await);
    assert_eq!(indexed.hook_sets_applied, vec![]);
    let lines = logged(&log);
    assert_eq!(methods(&lines), ["session/new", "session/new"], "{lines:?}");
    assert_eq!(lines[0]["params"]["_meta"], strict());
    assert!(lines[1]["params"].get("_meta").is_none_or(Value::is_null));
    assert_eq!(codex_config(&lines), None);
    assert!(lines.iter().all(|l| l.get("env").is_none()), "{lines:?}");
}

// Frame level: the connection's refusal and its hello.

/// The signature tungstenite's handshake callback requires.
#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
}

type ServerSink = futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>;
type ServerStream = futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>>;

fn host(addr: std::net::SocketAddr, name: &str, program: AgentCommand) -> HostConfig {
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        HostKey::from_seed([1; 32]),
        std::env::temp_dir().join(format!(
            "hennery-hook-sets-host-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
    );
    cfg.reconnect_min = Duration::from_millis(50);
    cfg.reconnect_max = Duration::from_millis(200);
    cfg.agents.insert("fake".into(), program);
    cfg
}

/// Accept the host's connection and read its `hello`.
async fn accept(listener: &TcpListener) -> (ServerSink, ServerStream, HostFrame) {
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let ws = tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await.unwrap();
    let (sink, mut stream) = ws.split();
    let hello = read_until(&mut stream, |f| matches!(f, HostFrame::Hello { .. })).await;
    (sink, stream, hello)
}

async fn send_frame(sink: &mut ServerSink, frame: &CollectorFrame) {
    sink.send(Message::text(serde_json::to_string(frame).unwrap()))
        .await
        .unwrap();
}

async fn read_until(stream: &mut ServerStream, pred: impl Fn(&HostFrame) -> bool) -> HostFrame {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    let frame: HostFrame = serde_json::from_str(&text).unwrap();
                    if pred(&frame) {
                        return frame;
                    }
                }
                Some(Ok(_)) => {}
                other => panic!("connection ended while waiting: {other:?}"),
            }
        }
    })
    .await
    .expect("expected frame within 10s")
}

#[tokio::test]
async fn a_generic_profile_refuses_before_spawning() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    let counting = AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "echo spawned >> {}; exec {}",
                spawns.display(),
                env!("CARGO_BIN_EXE_hennery-fake-acp")
            ),
        ],
        env: Vec::new(),
    };
    tokio::spawn(run(host(listener.local_addr().unwrap(), "generic-refuse", counting)));
    let (mut sink, mut stream, _) = accept(&listener).await;
    send_frame(
        &mut sink,
        &CollectorFrame::HelloAck {
            protocol_version: PROTOCOL_VERSION.into(),
            collector_version: "test".into(),
            committed: BTreeMap::new(),
        },
    )
    .await;
    read_until(&mut stream, |f| matches!(f, HostFrame::ResendComplete)).await;
    let cwd = cwd().to_string_lossy().into_owned();
    for frame in [
        CollectorFrame::StartSession {
            request_id: "r1".into(),
            session_id: "s1".into(),
            committed_seq: 0,
            agent: "fake".into(),
            cwd: cwd.clone(),
            config: Default::default(),
            hat_id: String::new(),
            mcp: Default::default(),
            hooks: Some(delivery()),
        },
        CollectorFrame::ResumeSession {
            request_id: "r2".into(),
            session_id: "s2".into(),
            committed_seq: 0,
            agent: "fake".into(),
            cwd: cwd.clone(),
            agent_session_id: "fake-session-1".into(),
            config: Default::default(),
            hat_id: String::new(),
            mcp: Default::default(),
            hooks: Some(delivery()),
        },
    ] {
        let (CollectorFrame::StartSession { request_id, .. } | CollectorFrame::ResumeSession { request_id, .. }) =
            &frame
        else {
            unreachable!()
        };
        let request_id = request_id.clone();
        send_frame(&mut sink, &frame).await;
        match read_until(
            &mut stream,
            |f| matches!(f, HostFrame::Error { request_id: r, .. } if *r == request_id),
        )
        .await
        {
            HostFrame::Error { code, message, .. } => assert_eq!(code, "hook_sets_unsupported", "{message}"),
            other => panic!("{other:?}"),
        }
    }
    assert!(!spawns.exists(), "an adapter was spawned");
}

#[tokio::test]
async fn hello_announces_hook_sets() {
    use hennery_proto::frames::Capability;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let program = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    tokio::spawn(run(host(listener.local_addr().unwrap(), "hello", program)));
    let (_sink, _stream, hello) = accept(&listener).await;
    let HostFrame::Hello { capabilities, .. } = hello else {
        panic!("expected hello");
    };
    assert!(capabilities.has(Capability::HookSets), "{capabilities:?}");
}
