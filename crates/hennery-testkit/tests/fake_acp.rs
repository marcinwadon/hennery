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
