//! A stand-in for Codex 0.155.1's CLI as a forget runs it (plan 9d-ii,
//! decision 9): `--version`, and `app-server` speaking its JSON-RPC over
//! stdio, one JSON object per line with no `jsonrpc` field, answering as
//! 0.155.1 does (read at `rust-v0.155.1`, and seen from a live run of it):
//! - `initialize` → `{userAgent, codexHome, platformFamily, platformOs}`,
//!   then a `remoteControl/status/changed` notification nobody asked for;
//! - any request before it → `-32600 Not initialized`;
//! - a method it does not know → `-32600 Invalid request: unknown variant`;
//! - `thread/delete` → as `FakeCodex::delete` says.
//!
//! Every spawn and every line read is logged (`FakeCodex::log`), so a test
//! sees the exact frames the host sent, and whether it ran at all.

use hennery_testkit::{CODEX_SCRIPT_ENV, FakeCodex, FakeDelete, FakeInitialize};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    let script: FakeCodex = std::env::var(CODEX_SCRIPT_ENV)
        .ok()
        .map(|s| serde_json::from_str(&s).expect("valid fake codex script JSON"))
        .unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "-".into());
    log(
        &script,
        json!({ "spawn": {
            "args": args,
            "CODEX_HOME": var("CODEX_HOME"),
            "CODEX_SQLITE_HOME": var("CODEX_SQLITE_HOME"),
            "CLAUDE_CODE_PROJECT_DIR_NAME": var("CLAUDE_CODE_PROJECT_DIR_NAME"),
            "cwd": std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default(),
        }}),
    );
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--version"] if script.hang_version => {
            if let Some(path) = &script.pid_file {
                std::fs::write(path, std::process::id().to_string()).expect("write the pid file");
            }
            hang()
        }
        ["--version"] => println!("codex-cli {}", script.version),
        ["app-server"] => app_server(&script),
        other => {
            eprintln!("hennery-fake-codex: unexpected arguments {other:?}");
            std::process::exit(2);
        }
    }
}

fn log(script: &FakeCodex, line: Value) {
    if script.log.is_empty() {
        return;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&script.log)
        .expect("open the fake codex log");
    writeln!(file, "{line}").expect("write the fake codex log");
}

fn send(out: &mut impl Write, value: Value) {
    writeln!(out, "{value}").expect("write to the host");
    out.flush().expect("flush to the host");
}

fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() }, "id": id })
}

fn hang() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(600));
    }
}

fn app_server(script: &FakeCodex) {
    if let Some(path) = &script.pid_file {
        std::fs::write(path, std::process::id().to_string()).expect("write the pid file");
    }
    if script.ignore_term {
        // SAFETY: signal(2) with SIG_IGN, before any other thread exists.
        unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut initialized = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        log(script, json!({ "recv": line }));
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message.get("method").and_then(Value::as_str),
        ) else {
            // A notification (`initialized`, say) or an answer: only logged.
            continue;
        };
        match method {
            "initialize" => match script.initialize {
                FakeInitialize::Answer => {
                    initialized = true;
                    let home = script.codex_home.clone().unwrap_or_else(|| {
                        let home = std::env::var("CODEX_HOME").unwrap_or_default();
                        std::fs::canonicalize(&home)
                            .map(|p| p.display().to_string())
                            .unwrap_or(home)
                    });
                    send(
                        &mut out,
                        json!({ "id": id, "result": {
                            "userAgent": "hennery/0.155.1 (fake) unknown (hennery; 1)",
                            "codexHome": home,
                            "platformFamily": "unix",
                            "platformOs": std::env::consts::OS,
                        }}),
                    );
                    send(
                        &mut out,
                        json!({ "method": "remoteControl/status/changed",
                                "params": { "status": "disabled" }, "emittedAtMs": 0 }),
                    );
                }
                FakeInitialize::Error => send(
                    &mut out,
                    error(
                        &id,
                        -32600,
                        "Invalid clientInfo.name: ''. Must be a valid HTTP header value.",
                    ),
                ),
                FakeInitialize::Hang => hang(),
                FakeInitialize::Exit => std::process::exit(1),
            },
            _ if !initialized => send(&mut out, error(&id, -32600, "Not initialized")),
            "thread/delete" if script.delete != FakeDelete::UnknownMethod => {
                let thread = message["params"]["threadId"].as_str().unwrap_or_default().to_string();
                delete(script, &mut out, &id, &thread);
            }
            other => send(
                &mut out,
                error(
                    &id,
                    -32600,
                    format!("Invalid request: unknown variant `{other}`, expected one of `initialize`, `thread/start`"),
                ),
            ),
        }
    }
}

fn delete(script: &FakeCodex, out: &mut impl Write, id: &Value, thread: &str) {
    match script.delete {
        FakeDelete::Delete => {
            let home = PathBuf::from(std::env::var("CODEX_HOME").unwrap_or_default());
            let found = rollouts(&home, thread);
            if found.is_empty() {
                return send(
                    out,
                    error(id, -32600, format!("no rollout found for thread id {thread}")),
                );
            }
            for path in found {
                std::fs::remove_file(path).expect("remove a rollout");
            }
            send(out, json!({ "id": id, "result": {} }));
            send(
                out,
                json!({ "method": "thread/deleted", "params": { "threadId": thread }, "emittedAtMs": 0 }),
            );
        }
        FakeDelete::AnswerButKeep => send(out, json!({ "id": id, "result": {} })),
        FakeDelete::ForkedHistory => send(
            out,
            error(
                id,
                -32600,
                format!("cannot delete thread {thread}: forked history still references it"),
            ),
        ),
        FakeDelete::Ephemeral => send(
            out,
            error(
                id,
                -32600,
                format!("thread is not persisted and cannot be deleted: {thread}"),
            ),
        ),
        FakeDelete::LiveWorker => send(
            out,
            error(id, -32600, "live internal threads can only be removed by their owner"),
        ),
        FakeDelete::MethodNotFound => send(out, error(id, -32601, "thread/delete is not supported yet")),
        FakeDelete::Internal => send(
            out,
            error(
                id,
                -32603,
                format!("failed to delete thread: database is locked ({thread})"),
            ),
        ),
        FakeDelete::Hang => hang(),
        FakeDelete::Exit => std::process::exit(1),
        FakeDelete::UnknownMethod => unreachable!("answered as an unknown method"),
    }
}

/// The thread's rollouts as Codex finds them: under `sessions/` at most
/// three levels down, and at the top of `archived_sessions/`; regular files
/// only, never through a link.
fn rollouts(home: &Path, thread: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, thread: &str, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if meta.is_file() && hennery_testkit::names_rollout_of(&name, thread) {
                out.push(path);
            } else if meta.is_dir() && depth < 3 && hennery_testkit::is_codex_date_dir(&name, depth + 1) {
                walk(&path, depth + 1, thread, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&home.join("sessions"), 0, thread, &mut out);
    walk(&home.join("archived_sessions"), 3, thread, &mut out);
    out
}
