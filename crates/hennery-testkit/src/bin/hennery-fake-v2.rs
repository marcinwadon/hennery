//! Small raw-wire fixture: prompt acceptance and foreground/background lifecycle
//! are deliberately separated. It never launches tools or models.
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

fn send(out: &Arc<Mutex<io::Stdout>>, value: Value) {
    let mut out = out.lock().unwrap();
    writeln!(out, "{value}").unwrap();
    out.flush().unwrap();
}
fn update(out: &Arc<Mutex<io::Stdout>>, value: Value) {
    send(
        out,
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture","update":value}}),
    );
}
fn main() {
    let out = Arc::new(Mutex::new(io::stdout()));
    let selected = std::env::var("FAKE_ACP_SELECTED")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(2);
    let mut wire_version = selected;
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or("");
        let id = message["id"].clone();
        let requested = message["params"]["protocolVersion"].as_u64().unwrap_or(2) as u16;
        if method == "initialize" {
            wire_version = selected.min(requested);
        }
        let selected = wire_version;
        if let Ok(path) = std::env::var("FAKE_ACP_REQUEST_LOG") {
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(log, "{method}:{selected}").unwrap();
        }
        if method == "initialize" {
            if let Ok(path) = std::env::var("FAKE_ACP_PROCESS_LOG") {
                let mut log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .unwrap();
                writeln!(log, "{}:{requested}", std::process::id()).unwrap();
            }
            if requested == 2 && std::env::var("FAKE_ACP_REJECT_V2").as_deref() == Ok("1") {
                send(
                    &out,
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32022,
                    "message":"Unsupported protocol version","data":{"requested":2,"supported":[1]}}}),
                );
                continue;
            }
        }
        let result = match method {
            "initialize" => {
                if selected == 1 {
                    json!({"protocolVersion":1,"agentCapabilities":{}})
                } else {
                    json!({"protocolVersion":2,"info":{"name":"fixture","version":"0"},"capabilities":{"session":{}},"_meta":{"steering":{"supported":std::env::var("FAKE_ACP_STEERING").as_deref()==Ok("1")}}})
                }
            }
            "session/new" | "session/resume" | "session/load" => json!({"sessionId":"fixture","configOptions":[]}),
            "session/prompt" => {
                if selected == 1 {
                    json!({"stopReason":"end_turn"})
                } else {
                    send(&out, json!({"jsonrpc":"2.0","id":id,"result":{"messageId":"m"}}));
                    let out = out.clone();
                    std::thread::spawn(move || {
                        update(
                            &out,
                            json!({"sessionUpdate":"state_update","state":"running","background":[]}),
                        );
                        std::thread::sleep(Duration::from_millis(100));
                        update(
                            &out,
                            json!({"sessionUpdate":"terminal_update","terminalId":"t","command":"fixture"}),
                        );
                        update(
                            &out,
                            json!({"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn","background":[{"type":"terminal","terminalId":"t"}]}),
                        );
                        std::thread::sleep(Duration::from_millis(450));
                        update(
                            &out,
                            json!({"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{"exitCode":0}}),
                        );
                        update(
                            &out,
                            json!({"sessionUpdate":"state_update","state":"idle","background":[]}),
                        );
                    });
                    continue;
                }
            }
            "session/close" => json!({}),
            "session/cancel" => continue,
            _ => {
                if !id.is_null() {
                    send(
                        &out,
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported fixture request"}}),
                    );
                }
                continue;
            }
        };
        send(&out, json!({"jsonrpc":"2.0","id":id,"result":result}));
    }
}
