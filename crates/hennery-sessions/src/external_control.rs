//! Scoped external orchestration. Durable admission outlives the HTTP caller.
use crate::{AppState, external_clients::ExternalIdentity, hub::RequestError};
use anyhow::{Result, ensure};
use axum::response::Response;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hennery_proto::{
    frames::{SessionBody, SessionConfig},
    plugin::InputOrigin,
    rest::{PromptRequest, StartSessionRequest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone, Debug)]
pub(crate) struct Command {
    pub operation_id: String,
    pub client_id: String,
    pub revision: i64,
    pub tool: String,
    pub session_id: String,
    pub turn_id: Option<String>,
}
impl Command {
    pub fn origin(&self) -> InputOrigin {
        InputOrigin::ExternalClient {
            client_id: self.client_id.clone(),
            operation_id: self.operation_id.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Receipt {
    pub operation_id: String,
    pub request_key_hash: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub state: String,
    pub code: Option<String>,
    pub result: Option<Value>,
    pub outcome: Option<String>,
}
pub(crate) struct Runtime {
    started: AtomicBool,
    flights: Mutex<HashSet<String>>,
    permits: Arc<Semaphore>,
    reads: Arc<Semaphore>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            started: AtomicBool::new(false),
            flights: Mutex::default(),
            permits: Arc::new(Semaphore::new(4)),
            reads: Arc::new(Semaphore::new(4)),
        }
    }
}
struct Flight {
    runtime: Arc<Runtime>,
    id: String,
}
impl Drop for Flight {
    fn drop(&mut self) {
        self.runtime
            .flights
            .lock()
            .expect("external operations")
            .remove(&self.id);
    }
}

/// Poll exactly the synchronous request admission/handoff while holding the
/// database gate, then release it before waiting for the host's fact. This is
/// the same gate used by consent revocation, hats and session transitions.
pub(crate) async fn guarded_request<F>(
    state: &AppState,
    command: Option<&Command>,
    transition: &str,
    request: &str,
    future: F,
) -> std::result::Result<SessionBody, RequestError>
where
    F: Future<Output = std::result::Result<SessionBody, RequestError>>,
{
    let Some(command) = command else { return future.await };
    let mut future = Box::pin(future);
    let mut polled = false;
    let first = std::future::poll_fn(|cx| {
        let conn = state.store.conn();
        let checked = (|| -> Result<()> {
            state.store.external_handoff_in(&conn, command, transition)?;
            state.store.external_request_in(&conn, command, request)
        })();
        Poll::Ready(match checked {
            Ok(()) => {
                polled = true;
                future.as_mut().poll(cx)
            }
            Err(error) => Poll::Ready(Err(RequestError::Rejected {
                code: error_code(&error),
                message: "external command authority changed".into(),
            })),
        })
    })
    .await;
    if !polled && let Err(error) = state.store.external_unsent(command, transition) {
        tracing::error!(%error, "external unsent transition cleanup failed");
    }
    match first {
        Poll::Ready(value) => value,
        Poll::Pending => future.await,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_key: String,
    host_id: String,
    cwd: String,
    agent: String,
    #[serde(default)]
    config: SessionConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prompt {
    request_key: String,
    session_id: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lifecycle {
    request_key: String,
    session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    request_key: String,
    session_id: String,
    turn_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    operation_id: Option<String>,
    request_key: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    operation_id: String,
    #[serde(default = "wait_default")]
    timeout_ms: u64,
}
fn wait_default() -> u64 {
    1000
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    session_id: String,
    item_id: Option<String>,
    offset: Option<usize>,
    version: Option<i64>,
    before_turn: Option<String>,
    cursor: Option<String>,
    #[serde(default = "items_default")]
    max_items: usize,
}
fn items_default() -> usize {
    10
}
#[derive(Deserialize, Serialize)]
struct ReadCursor {
    session_id: String,
    before_turn: Option<String>,
    epoch: String,
    revision: i64,
    end: usize,
}
fn decode<T: serde::de::DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|_| anyhow::anyhow!("invalid_arguments"))
}
fn short(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
pub(crate) fn is_control_tool(tool: &str) -> bool {
    matches!(
        tool,
        "session_create"
            | "session_prompt"
            | "session_cancel"
            | "session_park"
            | "session_resume"
            | "session_close"
            | "operation_get"
            | "session_wait"
            | "session_read"
    )
}

pub(crate) fn tools(identity: &ExternalIdentity) -> Vec<Value> {
    let id = json!({"type":"string","minLength":1,"maxLength":128});
    let mut rows = Vec::new();
    if identity.client.control_sessions {
        for (name, description, properties, required, read_only) in [
            (
                "session_create",
                "Create a standalone ACP session on a granted host/root. Returns a durable operation; poll operation_get for admission and launch outcome.",
                json!({"request_key":id,"host_id":id,"agent":id,"cwd":{"type":"string","maxLength":4096},"config":{"type":"object","properties":{"model":{"type":"string"},"mode":{"type":"string"},"axes":{"type":"object"},"cache_retention":{"enum":["inherit","5m","1h"]}},"additionalProperties":false}}),
                vec!["request_key", "host_id", "agent", "cwd"],
                false,
            ),
            (
                "session_prompt",
                "Send text to an authorized session. Busy sessions queue durably; parked sessions resume with current grant and source-policy checks. Acceptance is not completion.",
                json!({"request_key":id,"session_id":id,"text":{"type":"string","minLength":1,"maxLength":32768}}),
                vec!["request_key", "session_id", "text"],
                false,
            ),
            (
                "session_cancel",
                "Cancel only the explicitly named turn. Never cancels a newer turn; an already-ended intended turn returns its outcome.",
                json!({"request_key":id,"session_id":id,"turn_id":id}),
                vec!["request_key", "session_id", "turn_id"],
                false,
            ),
            (
                "session_park",
                "Park the session through its host; the durable operation reports confirmation or uncertain delivery.",
                json!({"request_key":id,"session_id":id}),
                vec!["request_key", "session_id"],
                false,
            ),
            (
                "session_resume",
                "Explicitly resume a stored session with current host path, hat and MCP-source policy checks.",
                json!({"request_key":id,"session_id":id}),
                vec!["request_key", "session_id"],
                false,
            ),
            (
                "session_close",
                "Explicitly close a session without deleting its transcript or other sessions.",
                json!({"request_key":id,"session_id":id}),
                vec!["request_key", "session_id"],
                false,
            ),
            (
                "operation_get",
                "Recover this client's operation by operation_id or request_key (exactly one). Returns current delivery/turn status without executing again.",
                json!({"operation_id":id,"request_key":id}),
                vec![],
                true,
            ),
            (
                "session_wait",
                "Wait at most ten seconds for a specific operation to complete, fail or need operator input. Disconnecting a wait never cancels accepted work.",
                json!({"operation_id":id,"timeout_ms":{"type":"integer","minimum":0,"maximum":10000}}),
                vec!["operation_id"],
                true,
            ),
        ] {
            rows.push(json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"idempotentHint":true,"openWorldHint":!read_only}}));
        }
    }
    if identity.client.read_transcript {
        rows.push(json!({"name":"session_read","description":"Read bounded canonical transcript excerpts, newest items first by page. Stable item IDs/versions and opaque continuation. For complete large items, pass item_id and offset; append item_json chunks until next_offset is null, keeping version fixed. content_truncated marks excerpts only. Does not acknowledge operator attention.","inputSchema":{"type":"object","properties":{"session_id":id,"before_turn":id,"item_id":{"type":"string","maxLength":1024},"offset":{"type":"integer","minimum":0},"version":{"type":"integer","minimum":0},"cursor":{"type":"string","maxLength":2048},"max_items":{"type":"integer","minimum":1,"maximum":10}},"required":["session_id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}));
    }
    for row in &mut rows {
        row["outputSchema"] =
            json!({"type":"object","properties":{"data":{}},"required":["data"],"additionalProperties":false});
    }
    rows
}

pub(crate) async fn invoke(state: &AppState, identity: &ExternalIdentity, tool: &str, args: Value) -> Result<Value> {
    state.store.external_roots(identity)?;
    if tool == "session_read" {
        ensure!(identity.client.read_transcript, "external_transcript_forbidden");
    } else {
        ensure!(identity.client.control_sessions, "external_control_forbidden");
    }
    after_startup(state);
    let data = match tool {
        "operation_get" => {
            let q: Operation = decode(args)?;
            ensure!(
                q.operation_id.as_deref().is_none_or(short) && q.request_key.as_deref().is_none_or(short),
                "invalid_arguments"
            );
            serde_json::to_value(state.store.external_operation(
                identity,
                q.operation_id.as_deref(),
                q.request_key.as_deref(),
            )?)?
        }
        "session_wait" => {
            let q: Wait = decode(args)?;
            ensure!(short(&q.operation_id) && q.timeout_ms <= 10_000, "invalid_arguments");
            let deadline = tokio::time::Instant::now() + Duration::from_millis(q.timeout_ms);
            let mut changes = state.hub.subscribe();
            loop {
                let receipt = state.store.external_operation(identity, Some(&q.operation_id), None)?;
                let waiting = !receipt.session_id.is_empty()
                    && (state
                        .store
                        .find_session(&receipt.session_id)?
                        .is_some_and(|s| s.activity.as_deref() == Some("blocked"))
                        || !state.store.open_pending(&receipt.session_id)?.is_empty());
                if waiting
                    || matches!(
                        receipt.state.as_str(),
                        "completed" | "failed" | "cancelled" | "not_delivered" | "delivery_unknown"
                    )
                    || tokio::time::Instant::now() >= deadline
                {
                    break json!({"operation":receipt,"waiting_on_operator":waiting});
                }
                tokio::select! {_=tokio::time::sleep_until(deadline)=>{},_=tokio::time::sleep(Duration::from_millis(250))=>{},_=changes.recv()=>{}}
            }
        }
        "session_read" => {
            let permit = state
                .external_commands
                .reads
                .clone()
                .try_acquire_owned()
                .map_err(|_| anyhow::anyhow!("operation_limit"))?;
            let (state_, identity, q) = (state.clone(), identity.clone(), decode(args)?);
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                read(&state_, &identity, q)
            })
            .await??
        }
        _ => {
            let key = match tool {
                "session_create" => {
                    let q: Create = decode(args.clone())?;
                    ensure!(
                        short(&q.host_id) && short(&q.agent) && q.cwd.len() <= 4096,
                        "invalid_arguments"
                    );
                    q.request_key
                }
                "session_prompt" => {
                    let q: Prompt = decode(args.clone())?;
                    ensure!(
                        short(&q.session_id) && !q.text.trim().is_empty() && q.text.len() <= 32 * 1024,
                        "invalid_arguments"
                    );
                    q.request_key
                }
                "session_cancel" => {
                    let q: Cancel = decode(args.clone())?;
                    ensure!(short(&q.session_id) && short(&q.turn_id), "invalid_arguments");
                    q.request_key
                }
                "session_park" | "session_resume" | "session_close" => {
                    let q: Lifecycle = decode(args.clone())?;
                    ensure!(short(&q.session_id), "invalid_arguments");
                    q.request_key
                }
                _ => anyhow::bail!("unknown_tool"),
            };
            serde_json::to_value(state.store.external_admit(identity, tool, &key, &args)?)?
        }
    };
    ensure!(serde_json::to_vec(&data)?.len() <= 256 * 1024, "result_too_large");
    // A long wait/read cannot publish data using an expired/revoked token.
    state.store.external_roots(identity)?;
    Ok(json!({"data":data}))
}

fn cut_value(value: &mut Value, budget: &mut usize, truncated: &mut bool) {
    match value {
        Value::String(text) => {
            let mut end = (*budget).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end < text.len() {
                text.truncate(end);
                *truncated = true;
            }
            *budget = budget.saturating_sub(end);
        }
        Value::Array(values) => {
            if values.len() > 64 {
                values.truncate(64);
                *truncated = true;
            }
            for value in values {
                cut_value(value, budget, truncated);
            }
        }
        Value::Object(values) => {
            let keys: Vec<_> = values.keys().cloned().collect();
            for (i, key) in keys.iter().enumerate() {
                if i >= 64 {
                    values.remove(key);
                    *truncated = true;
                } else if !matches!(
                    key.as_str(),
                    "id" | "turn_id" | "ts" | "kind" | "client_id" | "operation_id" | "sha256"
                ) {
                    cut_value(&mut values[key], budget, truncated);
                }
            }
        }
        _ => {}
    }
}
fn read(state: &AppState, identity: &ExternalIdentity, q: Read) -> Result<Value> {
    ensure!(
        short(&q.session_id) && q.before_turn.as_deref().is_none_or(short) && (1..=10).contains(&q.max_items),
        "invalid_arguments"
    );
    ensure!(q.cursor.is_none() || q.before_turn.is_none(), "invalid_arguments");
    ensure!(
        q.item_id.as_ref().is_none_or(|s| !s.is_empty() && s.len() <= 1024),
        "invalid_arguments"
    );
    ensure!(q.offset.is_none() || q.item_id.is_some(), "invalid_arguments");
    state.store.external_authorized_read(identity, &q.session_id, true)?;
    let cursor: Option<ReadCursor> = q
        .cursor
        .as_deref()
        .map(|raw| -> Result<_> {
            ensure!(raw.len() <= 2048, "invalid_arguments");
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(raw)
                    .map_err(|_| anyhow::anyhow!("invalid_arguments"))?,
            )
            .map_err(|_| anyhow::anyhow!("invalid_arguments"))
        })
        .transpose()?;
    if let Some(c) = &cursor {
        ensure!(
            c.session_id == q.session_id && c.before_turn.as_deref().is_none_or(short),
            "invalid_arguments"
        );
    }
    let before = cursor
        .as_ref()
        .and_then(|c| c.before_turn.as_deref())
        .or(q.before_turn.as_deref());
    let raw = crate::view::read_page(&state.store, &q.session_id, before, 1, 256 * 1024)?
        .ok_or_else(|| anyhow::anyhow!("invalid_arguments"))?;
    let page: hennery_view::api::ItemPage = serde_json::from_str(&raw)?;
    if let Some(item_id) = q.item_id.as_deref() {
        ensure!(q.cursor.is_none(), "invalid_arguments");
        let item = page
            .items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        let raw = serde_json::to_string(item)?;
        let offset = q.offset.unwrap_or(0);
        ensure!(
            offset <= raw.len() && raw.is_char_boundary(offset) && (offset == 0 || q.version.is_some()),
            "invalid_arguments"
        );
        ensure!(
            q.version.is_none_or(|version| version == item.version),
            "resync_required"
        );
        let mut end = (offset + 12 * 1024).min(raw.len());
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        state.store.external_authorized_read(identity, &q.session_id, true)?;
        return Ok(
            json!({"item_id":item.id,"version":item.version,"turn_id":item.turn_id,"epoch":page.epoch,"revision":page.revision,"item_json":&raw[offset..end],"offset":offset,"next_offset":if end<raw.len(){Some(end)}else{None}}),
        );
    }
    let end = if let Some(c) = &cursor {
        ensure!(c.epoch == page.epoch && c.revision == page.revision, "resync_required");
        ensure!(c.end <= page.items.len(), "invalid_arguments");
        c.end
    } else {
        page.items.len()
    };
    let start = end.saturating_sub(q.max_items);
    let mut items = Vec::new();
    let mut truncated = false;
    for item in &page.items[start..end] {
        let mut value = serde_json::to_value(item)?;
        cut_value(&mut value, &mut (12 * 1024), &mut truncated);
        items.push(value);
    }
    let next = if start > 0 {
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ReadCursor {
            session_id: q.session_id.clone(),
            before_turn: before.map(str::to_owned),
            epoch: page.epoch.clone(),
            revision: page.revision,
            end: start,
        })?))
    } else {
        None
    };
    state.store.external_authorized_read(identity, &q.session_id, true)?;
    Ok(
        json!({"items":items,"revision":page.revision,"epoch":page.epoch,"next_cursor":next,"older":page.older||start>0,"before_turn":page.items.first().and_then(|i|i.turn_id.as_deref()),"content_truncated":truncated}),
    )
}

pub fn after_startup(state: &AppState) {
    if state.external_commands.started.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = state.store.external_recover_operations() {
        state.external_commands.started.store(false, Ordering::Release);
        tracing::error!(code=%error_code(&error),"external operation recovery failed");
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {biased;()=state.shutdown.cancelled()=>break,_=tick.tick()=>{}}
            if let Err(error) = state.store.external_settle_operations() {
                tracing::warn!(code=%error_code(&error),"external operation settlement deferred");
            }
            let pending = match state.store.external_pending() {
                Ok(p) => p,
                Err(_) => continue,
            };
            for (command, args) in pending {
                let runtime = state.external_commands.clone();
                let Ok(permit) = runtime.permits.clone().try_acquire_owned() else {
                    break;
                };
                if !runtime
                    .flights
                    .lock()
                    .expect("external operations")
                    .insert(command.operation_id.clone())
                {
                    continue;
                }
                let state = state.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _flight = Flight {
                        runtime,
                        id: command.operation_id.clone(),
                    };
                    tokio::select! {biased;()=state.shutdown.cancelled()=>{},result=execute(&state,&command,args)=>{
                        if let Err(error)=result {let _=state.store.external_finish(&command,"failed",Some(&error_code(&error)),None);}
                    }}
                });
            }
        }
    });
}
async fn resolved(state: &AppState, host: &str, path: &str) -> Result<()> {
    let p = crate::resolve::resolve_on_host(state, host, path)
        .await
        .map_err(|_| anyhow::anyhow!("resolve_unavailable"))?;
    ensure!(p.is_dir && p.exists && p.canonical == path, "cwd_moved");
    Ok(())
}
async fn preflight(state: &AppState, command: &Command, args: &Value) -> Result<()> {
    let client = state.store.external_execution_client(command)?;
    let (host, cwd, hat) = if command.tool == "session_create" {
        (
            args["host_id"].as_str().unwrap_or_default().to_owned(),
            args["cwd"].as_str().unwrap_or_default().to_owned(),
            client.hat_id.clone(),
        )
    } else {
        let s = state
            .store
            .find_session(&command.session_id)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        (s.host_id, s.cwd, s.hat_id)
    };
    {
        let conn = state.store.conn();
        state.store.external_guard_in(
            &conn,
            command,
            if command.tool == "session_create" {
                Some((&host, &cwd, &hat))
            } else {
                None
            },
        )?;
    }
    if matches!(
        command.tool.as_str(),
        "session_create" | "session_prompt" | "session_resume"
    ) {
        let root = crate::external_control_store::root_for(&client, &host, &cwd)
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        resolved(state, &host, &cwd).await?;
        if root != cwd {
            resolved(state, &host, root).await?;
        }
    }
    Ok(())
}
async fn response_value(response: Response) -> Result<(bool, Value)> {
    let success = response.status().is_success();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await?;
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"code":"operation_failed"}));
    Ok((success, value))
}
async fn execute(state: &AppState, command: &Command, args: Value) -> Result<()> {
    if command.tool == "session_prompt" {
        let s = state
            .store
            .find_session(&command.session_id)?
            .ok_or_else(|| anyhow::anyhow!("not_found"))?;
        if s.lifecycle == "starting"
            || s.open_turn_id.is_some()
            || s.activity.as_deref() == Some("blocked")
            || !state.store.open_pending(&s.id)?.is_empty()
        {
            return Ok(());
        }
        if s.lifecycle == "active" && state.store.prompt_must_queue(&s.id)? {
            return Ok(());
        }
    }
    preflight(state, command, &args).await?;
    if command.tool == "session_prompt"
        && state
            .store
            .find_session(&command.session_id)?
            .is_some_and(|s| s.lifecycle == "parked")
    {
        let (success, value) = response_value(
            crate::api::resume_authorized(state.clone(), command.session_id.clone(), Some(command.clone())).await,
        )
        .await?;
        if !success {
            if value["code"] == "delivery_unknown" {
                return Ok(());
            }
            anyhow::bail!("resume_failed");
        }
    }
    if !state.store.external_begin(command)? {
        return Ok(());
    }
    let external = Some(command.clone());
    let state_ = state.clone();
    let id = command.session_id.clone();
    let response = match command.tool.as_str() {
        "session_create" => {
            let q: Create = decode(args)?;
            crate::api::start_in_hat(
                state_,
                StartSessionRequest {
                    host_id: q.host_id,
                    cwd: q.cwd,
                    agent: q.agent,
                    config: q.config,
                },
                None,
                external,
            )
            .await
        }
        "session_prompt" => {
            let q: Prompt = decode(args)?;
            crate::api::prompt_authorized(
                state_,
                id,
                PromptRequest {
                    content: vec![json!({"type":"text","text":q.text})],
                    request_key: Some(format!("external:{}", command.operation_id)),
                },
                external,
            )
            .await
        }
        "session_resume" => crate::api::resume_authorized(state_, id, external).await,
        "session_park" => crate::api::park_authorized(state_, id, external).await,
        "session_close" => crate::api::close_authorized(state_, id, external).await,
        "session_cancel" => crate::api::cancel_authorized(state_, id, external).await,
        _ => anyhow::bail!("unknown_tool"),
    };
    let (success, value) = response_value(response).await?;
    if value["external_queued"] == true {
        return state.store.external_finish(command, "queued", None, None);
    }
    let code = value["code"]
        .as_str()
        .filter(|s| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    let status = if success {
        if command.tool == "session_prompt" {
            "running"
        } else {
            "completed"
        }
    } else if code == Some("delivery_unknown") {
        "uncertain"
    } else {
        "failed"
    };
    state.store.external_finish(command, status, code, Some(&value))
}
fn error_code(error: &anyhow::Error) -> String {
    let s = error.to_string();
    if s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        s
    } else {
        "operation_failed".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        external_clients::{NewClient, RootGrant},
        store::Store,
    };
    use hennery_kernel::{
        hosts::{Enrollment, Hosts},
        operator::Operator,
    };
    fn world() -> (tempfile::TempDir, AppState, ExternalIdentity) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "test".into(),
                    platform: "test".into(),
                    host_version: "test".into(),
                },
                0,
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        state
            .store
            .create_session("session", "host", "fake", "/project", &hat, None)
            .unwrap();
        state
            .store
            .ingest("session", 1, &SessionBody::session_started("start", "agent"))
            .unwrap();
        let (_, token) = state
            .store
            .issue_external_client(&NewClient {
                label: "test".into(),
                hat_id: hat,
                roots: vec![RootGrant {
                    host_id: "host".into(),
                    root: "/project".into(),
                }],
                read_metadata: true,
                read_transcript: true,
                control_sessions: true,
                existing_sessions: true,
                expires_in_days: 1,
            })
            .unwrap();
        let identity = state.store.authenticate_external(&token).unwrap().unwrap();
        (dir, state, identity)
    }
    #[tokio::test]
    async fn revocation_at_final_handoff_does_not_send_or_leave_an_unsent_turn_open() {
        let (_dir, state, identity) = world();
        state
            .store
            .external_admit(
                &identity,
                "session_prompt",
                "prompt",
                &json!({"request_key":"prompt","session_id":"session","text":"test"}),
            )
            .unwrap();
        let (command, _) = state.store.external_pending().unwrap().remove(0);
        state.store.external_begin(&command).unwrap();
        let prompt = crate::content::check(vec![json!({"type":"text","text":"test"})]).unwrap();
        state
            .store
            .open_prompt_authorized(
                "session",
                command.turn_id.as_deref().unwrap(),
                &prompt,
                &mut None,
                None,
                Some(&command),
            )
            .unwrap();
        state.store.revoke_external_client(&command.client_id).unwrap();
        let polled = AtomicBool::new(false);
        let future = std::future::poll_fn(|_| {
            polled.store(true, Ordering::SeqCst);
            Poll::Ready(Err(RequestError::NotConnected))
        });
        assert!(
            guarded_request(&state, Some(&command), "prompt", "request", future)
                .await
                .is_err()
        );
        assert!(!polled.load(Ordering::SeqCst));
        assert!(
            state
                .store
                .find_session("session")
                .unwrap()
                .unwrap()
                .open_turn_id
                .is_none()
        );
    }

    fn admitted(state: &AppState, identity: &ExternalIdentity, tool: &str) -> Command {
        state
            .store
            .external_admit(
                identity,
                tool,
                tool,
                &json!({"request_key":tool,"session_id":"session"}),
            )
            .unwrap();
        let (command, _) = state
            .store
            .external_pending()
            .unwrap()
            .into_iter()
            .find(|(c, _)| c.tool == tool)
            .unwrap();
        state.store.external_begin(&command).unwrap();
        command
    }

    #[tokio::test]
    async fn unsent_attach_cleanup_is_fenced_against_a_newer_operator_attach() {
        for newer in [false, true] {
            let (_dir, state, identity) = world();
            let command = admitted(&state, &identity, "session_resume");
            {
                let conn = state.store.conn();
                conn.execute(
                    "UPDATE sessions SET lifecycle='starting' WHERE owner_id=?1 AND id='session'",
                    [state.store.owner_id()],
                )
                .unwrap();
                state.store.external_attach_in(&conn, &command).unwrap();
                if newer {
                    conn.execute(
                        "UPDATE sessions SET lifecycle='starting' WHERE owner_id=?1 AND id='session'",
                        [state.store.owner_id()],
                    )
                    .unwrap();
                }
            }
            state.store.revoke_external_client(&command.client_id).unwrap();
            let future = async { panic!("revoked attach must not be sent") };
            assert!(
                guarded_request(&state, Some(&command), "resume", "request", future)
                    .await
                    .is_err()
            );
            assert_eq!(
                state.store.find_session("session").unwrap().unwrap().lifecycle,
                if newer { "starting" } else { "failed" }
            );
        }
    }

    #[tokio::test]
    async fn unsent_close_clears_only_its_own_durable_intent() {
        for newer in [false, true] {
            let (_dir, state, identity) = world();
            let command = admitted(&state, &identity, "session_close");
            state.store.record_close_authorized("session", Some(&command)).unwrap();
            if newer {
                state.store.record_close_request("session").unwrap();
            }
            state.store.revoke_external_client(&command.client_id).unwrap();
            let future = async { panic!("revoked close must not be sent") };
            assert!(
                guarded_request(&state, Some(&command), "close", "request", future)
                    .await
                    .is_err()
            );
            let requested: bool = state
                .store
                .conn()
                .query_row(
                    "SELECT close_requested FROM sessions WHERE owner_id=?1 AND id='session'",
                    [state.store.owner_id()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(requested, newer);
        }
    }

    #[test]
    fn final_handoff_refuses_a_park_overtaken_by_operator_resume() {
        let (_dir, state, identity) = world();
        let command = admitted(&state, &identity, "session_park");
        state.store.record_park_authorized("session", Some(&command)).unwrap();
        let conn = state.store.conn();
        assert!(state.store.external_handoff_in(&conn, &command, "park").is_ok());
        conn.execute(
            "INSERT INTO events(owner_id,session_id,kind,body,ts) VALUES(?1,'session','operator_resumed','{}','now')",
            [state.store.owner_id()],
        )
        .unwrap();
        assert!(state.store.external_handoff_in(&conn, &command, "park").is_err());
    }

    #[test]
    fn scope_change_before_mutation_refuses_prompt_without_opening_a_turn() {
        let (_dir, state, identity) = world();
        let command = admitted(&state, &identity, "session_prompt");
        state
            .store
            .conn()
            .execute(
                "UPDATE sessions SET cwd='/outside' WHERE owner_id=?1 AND id='session'",
                [state.store.owner_id()],
            )
            .unwrap();
        let prompt = crate::content::check(vec![json!({"type":"text","text":"test"})]).unwrap();
        assert!(
            state
                .store
                .open_prompt_authorized(
                    "session",
                    command.turn_id.as_deref().unwrap(),
                    &prompt,
                    &mut None,
                    None,
                    Some(&command)
                )
                .is_err()
        );
        assert!(
            state
                .store
                .find_session("session")
                .unwrap()
                .unwrap()
                .open_turn_id
                .is_none()
        );
    }

    #[test]
    fn restart_clears_a_close_intent_that_never_reached_the_host_handoff() {
        let (_dir, state, identity) = world();
        let command = admitted(&state, &identity, "session_close");
        state.store.record_close_authorized("session", Some(&command)).unwrap();
        state.store.external_recover_operations().unwrap();
        let receipt = state
            .store
            .external_operation(&identity, Some(&command.operation_id), None)
            .unwrap();
        assert_eq!(receipt.state, "failed");
        assert_eq!(receipt.code.as_deref(), Some("not_delivered"));
        let requested: bool = state
            .store
            .conn()
            .query_row(
                "SELECT close_requested FROM sessions WHERE owner_id=?1 AND id='session'",
                [state.store.owner_id()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!requested);
    }

    #[test]
    fn idle_worker_needs_no_database_write_lock() {
        let (_dir, state, identity) = world();
        let command = admitted(&state, &identity, "session_close");
        state.store.external_finish(&command, "completed", None, None).unwrap();
        state.store.conn().execute_batch("PRAGMA query_only=ON").unwrap();
        state.store.external_recover_operations().unwrap();
        state.store.external_settle_operations().unwrap();
    }
}
