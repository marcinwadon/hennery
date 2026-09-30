//! The fake adapter speaks ACP over stdio like a real one.

use hennery_testkit::{FakeAsk, FakeScript};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

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

#[test]
fn flood_streams_until_the_prompt_is_cancelled() {
    let out = exchange_until(r#"{"chunks":["x"],"flood":true}"#, &cancelled_prompt_requests(), 3);
    // It stops for the cancel, however many chunks it got out first.
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "cancelled", "{out:?}");
}

/// F1 (review round 1): `model_switch_chunks_first` gives a model switch's
/// own answer a known, deterministic backlog ahead of it — sent inline, no
/// sleep, so the notifications land on the wire strictly before the switch
/// answers, instead of a backlog whose size depends on racing another
/// task's own timing.
#[test]
fn a_model_switch_can_send_a_known_backlog_of_chunks_before_answering() {
    let script = config_script(json!({ "model_switch_chunks_first": 5 }));
    let mut requests = session_requests()[..2].to_vec();
    requests.push(set_config(3, "model", json!("large")));
    let out = exchange_until(&script, &requests, 3);
    let notifications: Vec<&Value> = out.iter().filter(|m| m["method"] == "session/update").collect();
    assert_eq!(notifications.len(), 5, "{out:?}");
    let answer_pos = out.iter().position(|m| m["id"] == json!(3)).unwrap();
    let last_notification_pos = out.iter().rposition(|m| m["method"] == "session/update").unwrap();
    assert!(last_notification_pos < answer_pos, "{out:?}");
    assert_eq!(
        out[answer_pos]["result"]["configOptions"][0]["currentValue"],
        json!("large"),
        "{out:?}"
    );
}

// Plan (2): the fake asks the client questions (ACP core §2.5, §4.6).

/// What the fake sent during one `converse`, and how it exited if it did.
struct Conversation {
    messages: Vec<Value>,
    exit: Option<i32>,
}

/// Drive the fake through `initialize` (advertising `capabilities`),
/// `session/new` and one prompt, playing the client: every request the fake
/// sends is handed to `on_request`, and whatever it returns is written back.
/// Ends when the prompt is answered or the fake exits; a watchdog kills a
/// fake still running after 20 s, so a question nobody answers fails the
/// test instead of hanging it.
fn converse(script: &FakeScript, capabilities: Value, on_request: impl FnMut(&Value) -> Vec<Value>) -> Conversation {
    let mut requests = session_requests();
    requests[0]["params"]["clientCapabilities"] = capabilities;
    converse_with(script, requests, on_request)
}

/// `converse` over any list of requests; it ends when the last one is
/// answered.
fn converse_with(
    script: &FakeScript,
    requests: Vec<Value>,
    mut on_request: impl FnMut(&Value) -> Vec<Value>,
) -> Conversation {
    let last = requests.last().unwrap()["id"].clone();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
        .env(hennery_testkit::SCRIPT_ENV, serde_json::to_string(script).unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let pid = child.id() as i32;
    let watchdog = std::thread::spawn(move || {
        if finished.recv_timeout(Duration::from_secs(20)).is_err() {
            // SAFETY: the child is not reaped before `done` is sent, so its
            // pid cannot have been recycled yet.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    for r in &requests {
        writeln!(stdin, "{r}").unwrap();
    }
    let mut messages = Vec::new();
    let mut answered = false;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if msg.get("method").is_some() && msg.get("id").is_some() {
            for reply in on_request(&msg) {
                writeln!(stdin, "{reply}").unwrap();
            }
        }
        answered = msg["id"] == last && msg.get("method").is_none();
        messages.push(msg);
        if answered {
            break;
        }
    }
    if answered {
        child.kill().ok();
    }
    let _ = done.send(());
    watchdog.join().unwrap();
    let status = child.wait().unwrap();
    Conversation {
        messages,
        exit: if answered { None } else { status.code() },
    }
}

/// The client's answer to one of the fake's requests.
fn result(request: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
}

/// The text of every `agent_message_chunk`, in order.
fn texts(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["method"] == "session/update")
        .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
        .map(str::to_string)
        .collect()
}

fn asking(asks: Vec<FakeAsk>) -> FakeScript {
    FakeScript {
        asks,
        ..FakeScript::default()
    }
}

#[test]
fn a_permission_ask_waits_for_the_clients_choice_and_the_agent_sees_it() {
    let mut asked = Vec::new();
    let talk = converse(&asking(vec![FakeAsk::Permission]), json!({}), |req| {
        asked.push(req.clone());
        vec![result(
            req,
            json!({"outcome": {"outcome": "selected", "optionId": "allow"}}),
        )]
    });
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["method"], "session/request_permission");
    assert_eq!(asked[0]["params"]["sessionId"], "fake-session-1");
    let options: Vec<&str> = asked[0]["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["optionId"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["allow", "reject"]);
    assert_eq!(texts(&talk.messages), ["permission:selected:allow", "Hello", " world"]);
    assert_eq!(talk.messages.last().unwrap()["result"]["stopReason"], "end_turn");
}

#[test]
fn an_elicitation_is_asked_only_of_a_client_that_advertises_form_elicitation() {
    let script = asking(vec![FakeAsk::Elicitation]);
    let talk = converse(&script, json!({"elicitation": {"form": {}}}), |req| {
        assert_eq!(
            (req["method"].as_str(), req["params"]["mode"].as_str()),
            (Some("elicitation/create"), Some("form"))
        );
        vec![result(
            req,
            json!({"action": "accept", "content": {"name": "notes.txt"}}),
        )]
    });
    assert_eq!(texts(&talk.messages)[0], r#"elicitation:accept:{"name":"notes.txt"}"#);
    // A boolean is not the capability (P-19): the agent asks nothing, as
    // if none were advertised.
    for caps in [json!({"elicitation": true}), json!({})] {
        let talk = converse(&script, caps.clone(), |req| {
            panic!("asked {req} of a client with {caps}")
        });
        assert_eq!(texts(&talk.messages)[0], "elicitation:unsupported", "{caps}");
    }
}

#[test]
fn a_prompt_cancelled_while_it_asks_stops_asking_and_ends_cancelled() {
    let mut asked = 0;
    let talk = converse(
        &asking(vec![FakeAsk::Permission, FakeAsk::Elicitation]),
        json!({"elicitation": {"form": {}}}),
        |req| {
            asked += 1;
            // A client cancels the turn, then answers what is pending as
            // cancelled, as ACP asks of it.
            vec![
                json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "fake-session-1"}}),
                result(req, json!({"outcome": {"outcome": "cancelled"}})),
            ]
        },
    );
    assert_eq!(asked, 1, "the agent kept asking after the cancel");
    assert_eq!(texts(&talk.messages), ["permission:cancelled"]);
    assert_eq!(talk.messages.last().unwrap()["result"]["stopReason"], "cancelled");
}

#[test]
fn asks_at_once_are_all_open_before_the_first_answer() {
    let script = FakeScript {
        asks_at_once: true,
        ..asking(vec![FakeAsk::Permission, FakeAsk::Elicitation])
    };
    let mut open = Vec::new();
    let talk = converse(&script, json!({"elicitation": {"form": {}}}), |req| {
        open.push(req.clone());
        if open.len() < 2 {
            return vec![];
        }
        // Answered newest first: the echoes still follow the asks' order.
        vec![
            result(&open[1], json!({"action": "decline"})),
            result(
                &open[0],
                json!({"outcome": {"outcome": "selected", "optionId": "reject"}}),
            ),
        ]
    });
    assert_eq!(open.len(), 2);
    assert_eq!(
        texts(&talk.messages)[..2],
        ["permission:selected:reject", "elicitation:decline"]
    );
}

#[test]
fn a_request_no_client_serves_comes_back_with_the_clients_error() {
    let talk = converse(&asking(vec![FakeAsk::Unknown]), json!({}), |req| {
        assert_eq!(req["method"], "_fake/unknown");
        vec![json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": "Method not found"}})]
    });
    assert_eq!(texts(&talk.messages)[0], "unknown:error:-32601");
}

#[test]
fn ask_on_load_asks_before_the_load_is_answered() {
    let script = FakeScript {
        ask_on_load: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let requests = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"fake-session-1","cwd":"/tmp","mcpServers":[]}}),
    ];
    let talk = converse_with(&script, requests, |_| vec![]);
    let asked = talk
        .messages
        .iter()
        .position(|m| m["method"] == "session/request_permission")
        .expect("asked");
    let loaded = talk.messages.iter().position(|m| m["id"] == json!(2)).unwrap();
    assert!(asked < loaded, "{:?}", talk.messages);
}

#[test]
fn a_withdrawn_ask_is_cancelled_on_the_wire() {
    let script = FakeScript {
        withdraw_asks: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let mut asked = Vec::new();
    let talk = converse(&script, json!({}), |req| {
        asked.push(req.clone());
        vec![json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32800, "message": "Request cancelled"}})]
    });
    let withdrawn = talk
        .messages
        .iter()
        .find(|m| m["method"] == "$/cancel_request")
        .expect("the ask was withdrawn");
    assert_eq!(withdrawn["params"]["requestId"], asked[0]["id"]);
    assert_eq!(texts(&talk.messages)[0], "permission:error:-32800");
}

#[test]
fn crash_while_asking_exits_with_the_question_unanswered() {
    let script = FakeScript {
        crash_while_asking: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let mut asked = 0;
    let talk = converse(&script, json!({}), |_| {
        asked += 1;
        vec![]
    });
    assert_eq!(asked, 1);
    assert_eq!(talk.exit, Some(hennery_testkit::CRASH_EXIT_CODE));
    assert!(texts(&talk.messages).is_empty(), "{:?}", talk.messages);
}
