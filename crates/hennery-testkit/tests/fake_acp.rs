//! The fake adapter speaks ACP over stdio like a real one.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn exchange(script: Option<&str>, requests: &[Value]) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"));
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    if let Some(s) = script {
        cmd.env(hennery_testkit::SCRIPT_ENV, s);
    }
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    let reader = BufReader::new(child.stdout.take().unwrap());
    let mut out = Vec::new();
    for line in reader.lines() {
        let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let done = msg["id"] == json!(3);
        out.push(msg);
        if done {
            break;
        }
    }
    drop(stdin);
    child.kill().ok();
    child.wait().ok();
    out
}

fn session_requests() -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}),
        json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"fake-session-1","prompt":[{"type":"text","text":"hi"}]}}),
    ]
}

#[test]
fn default_script_streams_two_chunks_then_ends_the_turn() {
    let out = exchange(None, &session_requests());
    let chunks: Vec<&str> = out
        .iter()
        .filter(|m| m["method"] == "session/update")
        .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
        .collect();
    assert_eq!(chunks, ["Hello", " world"]);
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "end_turn");
}

#[test]
fn script_env_controls_the_chunks() {
    let out = exchange(Some(r#"{"chunks":["x"]}"#), &session_requests());
    let updates = out.iter().filter(|m| m["method"] == "session/update").count();
    assert_eq!(updates, 1);
}

#[test]
fn exit_after_chunks_crashes_mid_turn_without_answering_the_prompt() {
    let script = r#"{"chunks":["a","b","c"],"exit_after_chunks":1,"stderr_lines":["using token sk-live-123"]}"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
        .env(hennery_testkit::SCRIPT_ENV, script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in session_requests() {
        writeln!(stdin, "{r}").unwrap();
    }
    // Read until EOF: the process exits, so stdout closes.
    let out: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(hennery_testkit::CRASH_EXIT_CODE));
    let updates = out.iter().filter(|m| m["method"] == "session/update").count();
    assert_eq!(updates, 1, "{out:?}");
    assert!(
        out.iter().all(|m| m["id"] != json!(3)),
        "the prompt must not be answered: {out:?}"
    );
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(
        stderr.contains("sk-live-123") && stderr.contains("crashing"),
        "{stderr}"
    );
}

/// Owns the fake adapter's `Child` and, on drop, SIGKILLs its whole process
/// group (adapter + its `sleep 600` grandchild) then reaps it. `Child` alone
/// does not kill on drop, so without this a failed assertion between spawn
/// and the manual cleanup at the end of the test would leak both processes
/// (the grandchild for up to ten minutes). Deref lets the test read the
/// child's id like a plain `Child`.
struct KillGroupOnDrop(std::process::Child);

impl std::ops::Deref for KillGroupOnDrop {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        // SAFETY: killpg on the process group this test spawned as leader
        // (`process_group(0)`); called before wait so the pid (== pgid while
        // the leader is unreaped) cannot be recycled in between.
        unsafe {
            libc::killpg(self.0.id() as i32, libc::SIGKILL);
        }
        self.0.wait().ok();
    }
}

#[test]
fn grandchild_pid_file_records_a_live_process_in_the_adapters_group() {
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = serde_json::to_string(&hennery_testkit::FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    let child = KillGroupOnDrop(
        Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
            .env(hennery_testkit::SCRIPT_ENV, script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.parse().ok()) {
            break pid;
        }
        assert!(std::time::Instant::now() < deadline, "no grandchild pid file");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(hennery_testkit::pid_alive(pid));
    assert_eq!(
        // SAFETY: getpgid on a process this test spawned.
        unsafe { libc::getpgid(pid) },
        child.id() as i32,
        "grandchild left the adapter's group"
    );
    // `child` drops here: SIGKILLs the group and reaps the adapter, on this
    // path and on any assertion failure above.
}

/// Send `requests`, read replies until the response with id `last`.
fn exchange_until(script: &str, requests: &[Value], last: i64) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
        .env(hennery_testkit::SCRIPT_ENV, script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    let mut out = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let done = msg["id"] == json!(last);
        out.push(msg);
        if done {
            break;
        }
    }
    drop(stdin);
    child.kill().ok();
    child.wait().ok();
    out
}

fn load_requests(session_id: &str) -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":session_id,"cwd":"/tmp","mcpServers":[]}}),
    ]
}

#[test]
fn session_load_replays_the_script_in_order_before_answering() {
    let script = json!({
        "chunks": [],
        "replay": [
            {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "old"}},
            {"sessionUpdate": "available_commands_update", "availableCommands": []},
            {"sessionUpdate": "from_the_future", "x": 1}
        ]
    })
    .to_string();
    let out = exchange_until(&script, &load_requests("agent-7"), 2);
    assert_eq!(out[0]["result"]["agentCapabilities"]["loadSession"], true, "{out:?}");
    let kinds: Vec<&str> = out
        .iter()
        .filter(|m| m["method"] == "session/update")
        .map(|m| {
            assert_eq!(m["params"]["sessionId"], "agent-7");
            m["params"]["update"]["sessionUpdate"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        kinds,
        ["agent_message_chunk", "available_commands_update", "from_the_future"]
    );
    let answer = out.last().unwrap();
    assert_eq!(answer["id"], 2);
    assert!(answer.get("error").is_none(), "{answer}");
}

#[test]
fn scripted_errors_answer_session_load_and_session_new_with_their_codes() {
    let out = exchange_until(r#"{"chunks":[],"load_error":-32002}"#, &load_requests("agent-7"), 2);
    assert_eq!(out.last().unwrap()["error"]["code"], -32002, "{out:?}");
    let out = exchange_until(
        r#"{"chunks":[],"new_session_error":-32000}"#,
        &session_requests()[..2],
        2,
    );
    assert_eq!(out.last().unwrap()["error"]["code"], -32000, "{out:?}");
}

#[test]
fn no_load_session_withholds_the_capability() {
    let out = exchange_until(r#"{"chunks":[],"no_load_session":true}"#, &load_requests("a")[..1], 1);
    assert_eq!(out[0]["result"]["agentCapabilities"]["loadSession"], false, "{out:?}");
}

/// A prompt followed at once by `session/cancel`.
fn cancelled_prompt_requests() -> Vec<Value> {
    let mut requests = session_requests();
    requests.push(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"fake-session-1"}}));
    requests
}

#[test]
fn session_cancel_stops_the_prompt_and_answers_cancelled() {
    let script = r#"{"chunks":["a","b","c","d","e"],"chunk_delay_ms":300}"#;
    let out = exchange_until(script, &cancelled_prompt_requests(), 3);
    let chunks = out.iter().filter(|m| m["method"] == "session/update").count();
    assert!(chunks < 5, "the prompt ran to its end: {out:?}");
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "cancelled", "{out:?}");
    let script = r#"{"chunks":["a","b","c","d","e"],"chunk_delay_ms":300,"cancel_error":-32603}"#;
    let out = exchange_until(script, &cancelled_prompt_requests(), 3);
    assert_eq!(out.last().unwrap()["error"]["code"], -32603, "{out:?}");
}

#[test]
fn ignore_cancel_runs_the_prompt_to_its_end() {
    let script = r#"{"chunks":["a","b","c"],"chunk_delay_ms":50,"ignore_cancel":true}"#;
    let out = exchange_until(script, &cancelled_prompt_requests(), 3);
    let chunks = out.iter().filter(|m| m["method"] == "session/update").count();
    assert_eq!(chunks, 3, "{out:?}");
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "end_turn", "{out:?}");
}

// Plan B2b: config options.

fn config_script(extra: Value) -> String {
    let mut script = json!({ "chunks": [], "config_options": hennery_testkit::sample_config_options() });
    script
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    script.to_string()
}

/// `session/set_config_option`; a boolean value carries ACP's `type` tag.
fn set_config(id: i64, config_id: &str, value: Value) -> Value {
    let mut params = json!({"sessionId": "fake-session-1", "configId": config_id, "value": value});
    if value.is_boolean() {
        params["type"] = json!("boolean");
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "session/set_config_option", "params": params})
}

/// `requests` with an `initialize` that advertises boolean config options,
/// as the hennery host does.
fn with_booleans(mut requests: Vec<Value>) -> Vec<Value> {
    requests[0]["params"]["clientCapabilities"] = json!({"session": {"configOptions": {"boolean": {}}}});
    requests
}

/// The current value of every option in a `configOptions` list.
fn current(options: &Value) -> Vec<(String, Value)> {
    options
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["id"].as_str().unwrap().to_string(), o["currentValue"].clone()))
        .collect()
}

#[test]
fn session_new_and_load_announce_the_scripted_config_options() {
    let script = config_script(json!({}));
    let out = exchange_until(&script, &with_booleans(session_requests()[..2].to_vec()), 2);
    let options = &out.last().unwrap()["result"]["configOptions"];
    assert_eq!(
        current(options),
        [
            ("model".to_string(), json!("small")),
            ("effort".to_string(), json!("low")),
            ("fast".to_string(), json!(false)),
            ("mode".to_string(), json!("default"))
        ]
    );
    let out = exchange_until(&script, &with_booleans(load_requests("agent-7")), 2);
    assert_eq!(out.last().unwrap()["result"]["configOptions"], *options);
    // A client that cannot show a boolean option gets an on/off select.
    let out = exchange_until(&script, &session_requests()[..2], 2);
    let fast = &out.last().unwrap()["result"]["configOptions"][2];
    assert_eq!(
        (&fast["type"], &fast["currentValue"]),
        (&json!("select"), &json!("off")),
        "{fast}"
    );
    // No scripted options: none announced, like an adapter without them.
    let out = exchange_until(r#"{"chunks":[]}"#, &session_requests()[..2], 2);
    assert!(out.last().unwrap()["result"].get("configOptions").is_none(), "{out:?}");
}

#[test]
fn set_config_option_switches_validates_and_clamps_the_mode_like_an_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let script = config_script(json!({ "model_switch_sets_mode": "default", "config_log": log }));
    let mut requests = with_booleans(session_requests()[..2].to_vec());
    requests.push(set_config(3, "mode", json!("plan")));
    requests.push(set_config(4, "model", json!("large")));
    requests.push(set_config(5, "fast", json!(true)));
    requests.push(set_config(6, "model", json!("huge")));
    requests.push(set_config(7, "nope", json!("x")));
    let out = exchange_until(&script, &requests, 7);
    let answer = |id: i64| out.iter().find(|m| m["id"] == json!(id)).unwrap();
    assert_eq!(current(&answer(3)["result"]["configOptions"])[3].1, json!("plan"));
    // The model switch resets the mode.
    assert_eq!(
        current(&answer(5)["result"]["configOptions"]),
        [
            ("model".to_string(), json!("large")),
            ("effort".to_string(), json!("low")),
            ("fast".to_string(), json!(true)),
            ("mode".to_string(), json!("default"))
        ]
    );
    assert_eq!(answer(6)["error"]["code"], -32602, "a value the option does not offer");
    assert_eq!(answer(7)["error"]["code"], -32602, "an unknown option");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "mode=plan\nmodel=large\nfast=true\nmodel=huge\nnope=x\n"
    );
}

#[test]
fn an_empty_read_back_still_applies_the_switch() {
    let script = config_script(json!({ "empty_config_read_back": true, "prompt_sets_mode": "bypass" }));
    let mut requests = with_booleans(session_requests()[..2].to_vec());
    requests.push(set_config(3, "model", json!("large")));
    requests.push(json!({"jsonrpc":"2.0","id":4,"method":"session/prompt",
                         "params":{"sessionId":"fake-session-1","prompt":[{"type":"text","text":"hi"}]}}));
    let out = exchange_until(&script, &requests, 4);
    let answer = out.iter().find(|m| m["id"] == json!(3)).unwrap();
    assert_eq!(answer["result"]["configOptions"], json!([]), "{answer}");
    // The prompt's own mode change shows the switch took effect.
    let update = out
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "config_option_update")
        .expect("a config_option_update");
    assert_eq!(
        current(&update["params"]["update"]["configOptions"]),
        [
            ("model".to_string(), json!("large")),
            ("effort".to_string(), json!("low")),
            ("fast".to_string(), json!(false)),
            ("mode".to_string(), json!("bypass"))
        ]
    );
}

#[test]
fn a_sticky_option_accepts_a_switch_and_keeps_its_value() {
    let script = config_script(json!({ "sticky_options": ["effort"] }));
    let mut requests = session_requests()[..2].to_vec();
    requests.push(set_config(3, "effort", json!("high")));
    let out = exchange_until(&script, &requests, 3);
    let answer = out.last().unwrap();
    assert_eq!(
        current(&answer["result"]["configOptions"])[1].1,
        json!("low"),
        "{answer}"
    );
}

#[test]
fn config_in_update_only_announces_the_options_before_the_answer() {
    let script = config_script(json!({ "config_in_update_only": true }));
    let out = exchange_until(&script, &session_requests()[..2], 2);
    let answer = out.last().unwrap();
    assert!(answer["result"].get("configOptions").is_none(), "{answer}");
    let update = out
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "config_option_update")
        .expect("a config_option_update before the answer");
    assert_eq!(
        current(&update["params"]["update"]["configOptions"])[0].1,
        json!("small")
    );
}

/// The real adapters' SDK handles requests concurrently: a slow model
/// switch does not hold back the mode switch sent after it, and its mode
/// clamp lands after its own answer.
#[test]
fn a_slow_model_switch_is_answered_after_a_later_switch_and_clamps_after_answering() {
    let script = config_script(json!({ "slow_model_switch_ms": 300, "model_switch_sets_mode": "default" }));
    let mut requests = session_requests()[..2].to_vec();
    requests.push(set_config(3, "model", json!("large")));
    requests.push(set_config(4, "mode", json!("plan")));
    let out = exchange_until(&script, &requests, 3);
    let order: Vec<i64> = out
        .iter()
        .filter_map(|m| m["id"].as_i64())
        .filter(|id| *id >= 3)
        .collect();
    assert_eq!(order, [4, 3], "{out:?}");
    // Its answer still shows the mode the later switch set: the clamp came after.
    let model = out.last().unwrap();
    assert_eq!(
        (
            current(&model["result"]["configOptions"])[0].1.clone(),
            current(&model["result"]["configOptions"])[3].1.clone()
        ),
        (json!("large"), json!("plan"))
    );
}

/// A hung switch must never answer, and must not hold up other traffic: the
/// framework handles requests concurrently, so a prompt sent right after it
/// still gets its answer.
#[test]
fn a_hung_config_switch_never_answers_while_other_traffic_is_served() {
    use std::os::unix::process::CommandExt;
    let script = config_script(json!({ "hang_config": true }));
    let mut child = KillGroupOnDrop(
        Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
            .env(hennery_testkit::SCRIPT_ENV, &script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let mut stdin = child.0.stdin.take().unwrap();
    let mut requests = session_requests()[..2].to_vec();
    requests.push(set_config(3, "model", json!("large")));
    requests.push(json!({"jsonrpc":"2.0","id":4,"method":"session/prompt",
                         "params":{"sessionId":"fake-session-1","prompt":[{"type":"text","text":"hi"}]}}));
    for r in &requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);

    // Read replies on a background thread so the main thread can enforce a
    // wall-clock window on the switch without also timing out the process's
    // own (unrelated, and locally observed to vary by hundreds of ms) start-up
    // latency: blocking on a plain iterator (like `exchange_until` does)
    // would hang the test forever if the hang branch regressed and the
    // switch never gets an id 4 to unblock on.
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                break;
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    // Other traffic sent right after the hung switch must still be served.
    // Wait generously for it (this leg is dominated by process start-up, not
    // by the switch), while watching that id 3 never sneaks in alongside it.
    let mut out = Vec::new();
    loop {
        let msg = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the prompt was never answered while the switch hung");
        assert_ne!(msg["id"], json!(3), "the hung switch answered: {msg}");
        let done = msg["id"] == json!(4);
        out.push(msg);
        if done {
            break;
        }
    }
    assert_eq!(out.last().map(|m| &m["id"]), Some(&json!(4)), "{out:?}");
    // The prompt has already answered, so the process is fully up and
    // running: a further short, quiet window now means the switch is truly
    // hung, not merely slower than the prompt.
    if let Ok(msg) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
        panic!("the hung switch answered after all: {msg}");
    }
    // `child` drops here: SIGKILLs the group and reaps it (the hung switch's
    // task included), on this path and on any assertion failure above.
}
