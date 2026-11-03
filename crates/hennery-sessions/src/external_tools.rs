//! Additional external tools. Every snapshot and local mutation shares the
//! same owner gate as grant revocation; no operator or agent identity is forged.
use crate::{
    AppState,
    external_clients::{self, ExternalIdentity},
    external_control::Command,
    external_control_store::{placement, session_scope},
    store::Store,
};
use anyhow::{Result, bail, ensure};
use hennery_proto::{
    frames::{ConfigValue, ElicitationAction},
    rest::{AnswerRequest, ConfigRequest},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionArg {
    pub session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    request_key: String,
    session_id: String,
    pending_id: String,
    action: ElicitationAction,
    content: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    request_key: String,
    session_id: String,
    config_id: String,
    value: ConfigValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    request_key: String,
    session_id: String,
    title: Option<String>,
    #[serde(default)]
    reset_title: bool,
    sidebar_pinned: Option<bool>,
    idle_pinned: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueChange {
    request_key: String,
    session_id: String,
    operation_id: String,
    text: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Queue {
    session_id: String,
    #[serde(default)]
    after: i64,
    #[serde(default = "queue_limit")]
    limit: u32,
}
fn queue_limit() -> u32 {
    25
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    host_id: String,
    cwd: String,
    agent: String,
    #[serde(default)]
    refresh: bool,
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| anyhow::anyhow!("invalid_arguments"))
}
fn id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "session_pending"
            | "session_answer"
            | "runtime_catalog"
            | "session_config"
            | "session_queue"
            | "operation_cancel"
            | "session_queue_update"
            | "session_usage"
            | "session_update"
    )
}
pub(crate) fn tools(identity: &ExternalIdentity) -> Vec<Value> {
    let id = json!({"type":"string","minLength":1,"maxLength":128});
    let mut rows = Vec::new();
    for (name, description, props, required, control, transcript) in [
        (
            "session_pending",
            "Read ordinary questions and permission notices. Permissions can only be answered in the operator UI.",
            json!({"session_id":id}),
            vec!["session_id"],
            false,
            true,
        ),
        (
            "session_usage",
            "Read runtime-reported token, cache, context and cost usage for this session. Unknown usage is not estimated; no family data is exposed.",
            json!({"session_id":id}),
            vec!["session_id"],
            false,
            false,
        ),
        (
            "session_queue",
            "Inspect this client's queued/running operations for one session. Prompt text requires transcript consent. Cursor is the last position.",
            json!({"session_id":id,"after":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}}),
            vec!["session_id"],
            true,
            false,
        ),
        (
            "runtime_catalog",
            "Read the saved runtime model/mode/effort catalog for a granted host and canonical directory. refresh probes the host and requires session control consent.",
            json!({"host_id":id,"cwd":{"type":"string","maxLength":4096},"agent":id,"refresh":{"type":"boolean"}}),
            vec!["host_id", "cwd", "agent"],
            false,
            false,
        ),
        (
            "session_answer",
            "Answer an ordinary elicitation after validating its current schema. Returns a durable operation and later delivery verdict; never grants permissions.",
            json!({"request_key":id,"session_id":id,"pending_id":id,"action":{"enum":["accept","decline","cancel"]},"content":{"type":["object","null"]}}),
            vec!["request_key", "session_id", "pending_id", "action"],
            true,
            false,
        ),
        (
            "session_config",
            "Set one attached-session option using its catalog config_id and value. Receipt contains the confirmed effective catalog, or delivery_unknown.",
            json!({"request_key":id,"session_id":id,"config_id":id,"value":{"type":["string","boolean"],"maxLength":4096}}),
            vec!["request_key", "session_id", "config_id", "value"],
            true,
            false,
        ),
        (
            "session_update",
            "Rename, reset the title override, set sidebar pin or idle hold. Does not change the hat or session identity.",
            json!({"request_key":id,"session_id":id,"title":{"type":"string","maxLength":4096},"reset_title":{"type":"boolean"},"sidebar_pinned":{"type":"boolean"},"idle_pinned":{"type":"boolean"}}),
            vec!["request_key", "session_id"],
            true,
            false,
        ),
        (
            "operation_cancel",
            "Withdraw this client's queued prompt or command only. Once running, a command cannot be recalled; use exact-turn session_cancel for an active turn.",
            json!({"request_key":id,"session_id":id,"operation_id":id}),
            vec!["request_key", "session_id", "operation_id"],
            true,
            false,
        ),
        (
            "session_queue_update",
            "Edit the text of this client's still queued prompt, preserving its FIFO position and original idempotency key. The edit has its own durable receipt.",
            json!({"request_key":id,"session_id":id,"operation_id":id,"text":{"type":"string","minLength":1,"maxLength":32768}}),
            vec!["request_key", "session_id", "operation_id", "text"],
            true,
            false,
        ),
    ] {
        if (!control || identity.client.control_sessions)
            && (!transcript || identity.client.read_transcript)
            && identity.client.read_metadata
        {
            rows.push(json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":props,"required":required,"additionalProperties":false},"outputSchema":{"type":"object","properties":{"data":{}},"required":["data"],"additionalProperties":false},"annotations":{"readOnlyHint":!control&&(name!="runtime_catalog"||!identity.client.control_sessions),"destructiveHint":control,"idempotentHint":true,"openWorldHint":control||(name=="runtime_catalog"&&identity.client.control_sessions)}}));
        }
    }
    rows
}
impl Store {
    pub(crate) fn external_read_in(
        &self,
        conn: &Connection,
        identity: &ExternalIdentity,
        session: &str,
        transcript: bool,
    ) -> Result<()> {
        ensure!(
            external_clients::live(conn, self.owner_id(), identity)?,
            "external_unauthenticated"
        );
        ensure!(identity.client.read_metadata, "external_forbidden");
        ensure!(
            !transcript || identity.client.read_transcript,
            "external_transcript_forbidden"
        );
        session_scope(conn, self.owner_id(), &identity.client, session, false)
    }
    pub(crate) fn external_detail(&self, identity: &ExternalIdentity, session: &str) -> Result<Value> {
        let conn = self.conn();
        self.external_read_in(&conn, identity, session, false)?;
        let catalog = self.catalog_in(&conn, session)?;
        let (native,created,last,pin,idle,branch):(Option<String>,String,Option<String>,bool,bool,Option<String>)=conn.query_row("SELECT agent_session_id,created_at,last_event_at,sidebar_pinned,idle_pinned,git_branch FROM sessions WHERE owner_id = ?1 AND id=?2",params![self.owner_id(),session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        let pending = {
            let mut stmt=conn.prepare("SELECT p.pending_id,p.kind,EXISTS(SELECT 1 FROM answer_queue q WHERE q.owner_id=p.owner_id AND q.pending_id=p.pending_id) FROM pending p WHERE p.owner_id = ?1 AND p.session_id=?2 AND p.state='open' ORDER BY p.rowid LIMIT 51")?;
            stmt.query_map(params![self.owner_id(),session],|r|{let kind=r.get::<_,String>(1)?;Ok(json!({"pending_id":r.get::<_,String>(0)?,"kind":kind,"answered":r.get::<_,bool>(2)?,"operator_required":kind=="permission"}))})?.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let pending_truncated = pending.len() > 50;
        let pending: Vec<_> = pending.into_iter().take(50).collect();
        let queued:i64=conn.query_row("SELECT count(*) FROM external_operations WHERE owner_id = ?1 AND client_id=?2 AND session_id=?3 AND state='queued'",params![self.owner_id(),identity.client.client_id,session],|r|r.get(0))?;
        Ok(
            json!({"agent_session_id":native,"created_at":created,"last_event_at":last,"branch":branch,"sidebar_pinned":pin,"idle_pinned":idle,"catalog":catalog,"pending":pending,"pending_limit":50,"pending_truncated":pending_truncated,"own_queued_operations":queued}),
        )
    }
    pub(crate) fn external_arguments(&self, command: &Command) -> Result<Value> {
        let conn = self.conn();
        let raw: String = conn.query_row(
            "SELECT args FROM external_operations WHERE owner_id = ?1 AND id=?2 AND state='running'",
            params![self.owner_id(), command.operation_id],
            |r| r.get(0),
        )?;
        let args: Value = serde_json::from_str(&raw)?;
        if matches!(command.tool.as_str(), "session_create" | "session_clone") {
            let client = self.external_execution_client_unlocked(&conn, command)?;
            self.external_guard_in(
                &conn,
                command,
                Some((
                    args["host_id"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("invalid_arguments"))?,
                    args["cwd"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("invalid_arguments"))?,
                    &client.hat_id,
                )),
            )?;
        } else {
            self.external_guard_in(&conn, command, None)?;
        }
        Ok(args)
    }
}
pub(crate) async fn invoke(state: &AppState, identity: &ExternalIdentity, name: &str, args: Value) -> Result<Value> {
    state.store.external_roots(identity)?;
    let value = match name {
        "session_pending" => {
            let q: SessionArg = decode(args)?;
            let conn = state.store.conn();
            state.store.external_read_in(&conn, identity, &q.session_id, true)?;
            let mut stmt=conn.prepare("SELECT p.pending_id,p.kind,p.turn_id,CASE WHEN length(CAST(p.payload AS BLOB))<=4096 THEN p.payload END,EXISTS(SELECT 1 FROM answer_queue q WHERE q.owner_id=p.owner_id AND q.pending_id=p.pending_id) FROM pending p WHERE p.owner_id = ?1 AND p.session_id=?2 AND p.state='open' ORDER BY p.rowid LIMIT 51")?;
            let pending = stmt
                .query_map(params![state.store.owner_id(), q.session_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, bool>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let truncated = pending.len() > 50;
            let pending=pending.into_iter().take(50).map(|(id,kind,turn,payload,answered)|->Result<Value>{let payload=payload.map(|s|serde_json::from_str::<Value>(&s)).transpose()?;Ok(json!({"pending_id":id,"kind":kind,"turn_id":turn,"state":"open","answered":answered,"operator_required":kind=="permission","payload_truncated":payload.is_none(),"payload":payload}))}).collect::<Result<Vec<_>>>()?;
            json!({"pending":pending,"truncated":truncated,"permissions":"operator_ui"})
        }
        "session_usage" => {
            let q: SessionArg = decode(args)?;
            let conn = state.store.conn();
            state.store.external_read_in(&conn, identity, &q.session_id, false)?;
            let totals = crate::usage::totals_in(&conn, state.store.owner_id(), &q.session_id)?;
            let context: Option<String> = conn.query_row(
                "SELECT usage_context FROM sessions WHERE owner_id = ?1 AND id=?2",
                params![state.store.owner_id(), q.session_id],
                |r| r.get(0),
            )?;
            json!({"session_id":q.session_id,"session":totals,"context":context.map(|s|serde_json::from_str::<Value>(&s)).transpose()?})
        }
        "session_queue" => {
            let q: Queue = decode(args)?;
            ensure!((1..=50).contains(&q.limit) && q.after >= 0, "invalid_arguments");
            ensure!(identity.client.control_sessions, "external_control_forbidden");
            let conn = state.store.conn();
            state.store.external_read_in(&conn, identity, &q.session_id, false)?;
            let mut stmt=conn.prepare("SELECT position,id,tool,state,turn_id,args,request_id IS NULL FROM external_operations WHERE owner_id = ?1 AND client_id=?2 AND session_id=?3 AND position>?4 AND state IN ('queued','running','uncertain') ORDER BY position LIMIT ?5")?;
            let rows = stmt
                .query_map(
                    params![
                        state.store.owner_id(),
                        identity.client.client_id,
                        q.session_id,
                        q.after,
                        q.limit
                    ],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, bool>(6)?,
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let next = rows.last().filter(|_| rows.len() == q.limit as usize).map(|r| r.0);
            let rows=rows.into_iter().map(|(position,id,tool,status,turn,args,unhanded)| { let args:Value=serde_json::from_str(&args).unwrap_or(Value::Null); {let full=args["text"].as_str();let excerpt=full.map(|s|{let mut end=s.len().min(512);while !s.is_char_boundary(end){end-=1;} &s[..end]});json!({"position":position,"operation_id":id,"tool":tool,"state":status,"turn_id":turn,"withdrawable":status=="queued"&&unhanded,"text":if identity.client.read_transcript {json!(excerpt)}else{Value::Null},"text_truncated":identity.client.read_transcript&&full.is_some_and(|s|s.len()>512)})}}).collect::<Vec<_>>();
            json!({"operations":rows,"next_cursor":next})
        }
        "runtime_catalog" => {
            let q: Catalog = decode(args)?;
            ensure!(
                id(&q.host_id) && id(&q.agent) && q.cwd.len() <= 4096,
                "invalid_arguments"
            );
            ensure!(
                !q.refresh || identity.client.control_sessions,
                "external_control_forbidden"
            );
            let check = || -> Result<()> {
                let conn = state.store.conn();
                ensure!(
                    external_clients::live(&conn, state.store.owner_id(), identity)?,
                    "external_unauthenticated"
                );
                ensure!(identity.client.read_metadata, "external_forbidden");
                placement(
                    &conn,
                    state.store.owner_id(),
                    &identity.client,
                    &q.host_id,
                    &q.cwd,
                    &identity.client.hat_id,
                )
            };
            check()?;
            let catalog = if q.refresh {
                let resolved = crate::resolve::resolve_on_host(state, &q.host_id, &q.cwd)
                    .await
                    .map_err(|_| anyhow::anyhow!("cwd_unavailable"))?;
                ensure!(
                    resolved.exists && resolved.is_dir && resolved.canonical == q.cwd,
                    "cwd_unavailable"
                );
                check()?;
                state
                    .catalogs
                    .get_external(state, identity, &q.host_id, &q.cwd, &q.agent)
                    .await?
            } else {
                state
                    .store
                    .saved_runtime_catalog(&q.host_id, &identity.client.hat_id, &q.cwd, &q.agent)?
            };
            check()?;
            serde_json::to_value(catalog)?
        }
        _ => {
            ensure!(identity.client.control_sessions, "external_control_forbidden");
            let (key, session) = match name {
                "session_answer" => {
                    let q: Answer = decode(args.clone())?;
                    ensure!(id(&q.pending_id), "invalid_arguments");
                    ensure!(q.content.as_ref().is_none_or(Value::is_object), "invalid_arguments");
                    (q.request_key, q.session_id)
                }
                "session_config" => {
                    let q: Config = decode(args.clone())?;
                    ensure!(
                        id(&q.config_id) && serde_json::to_vec(&q.value)?.len() <= 8192,
                        "invalid_arguments"
                    );
                    (q.request_key, q.session_id)
                }
                "session_update" => {
                    let q: Update = decode(args.clone())?;
                    ensure!(
                        !(q.reset_title && q.title.is_some())
                            && q.title.as_ref().is_none_or(|s| s.len() <= 4096)
                            && (q.reset_title
                                || q.title.is_some()
                                || q.sidebar_pinned.is_some()
                                || q.idle_pinned.is_some()),
                        "invalid_arguments"
                    );
                    (q.request_key, q.session_id)
                }
                "operation_cancel" | "session_queue_update" => {
                    let q: QueueChange = decode(args.clone())?;
                    ensure!(id(&q.operation_id), "invalid_arguments");
                    ensure!(
                        if name == "operation_cancel" {
                            q.text.is_none()
                        } else {
                            q.text
                                .as_ref()
                                .is_some_and(|t| !t.trim().is_empty() && t.len() <= 32768)
                        },
                        "invalid_arguments"
                    );
                    (q.request_key, q.session_id)
                }
                _ => bail!("unknown_tool"),
            };
            ensure!(id(&session), "invalid_arguments");
            crate::external_control::after_startup(state);
            serde_json::to_value(state.store.external_admit(identity, name, &key, &args)?)?
        }
    };
    ensure!(serde_json::to_vec(&value)?.len() <= 256 * 1024, "result_too_large");
    state.store.external_roots(identity)?;
    Ok(json!({"data":value}))
}

pub(crate) async fn execute(state: &AppState, command: &Command, args: Value) -> Result<Option<Value>> {
    match command.tool.as_str() {
        "session_config" => {
            let q: Config = decode(args)?;
            let requested = json!({"config_id":q.config_id,"value":q.value});
            let response = crate::api::set_config_authorized(
                state.clone(),
                q.session_id,
                ConfigRequest {
                    config_id: q.config_id,
                    value: q.value,
                },
                Some(command.clone()),
            )
            .await;
            let (ok, value) = crate::external_control::response_value(response).await?;
            ensure!(ok, "{}", value["code"].as_str().unwrap_or("operation_failed"));
            Ok(Some(json!({"requested":requested,"effective":value})))
        }
        "session_answer" => {
            let q: Answer = decode(args)?;
            let state = state.clone();
            let command = command.clone();
            tokio::task::spawn_blocking(move || queue_answer(&state, &command, q)).await??;
            Ok(None)
        }
        "session_update" => {
            let q: Update = decode(args)?;
            let mut conn = state.store.conn();
            let tx = conn.transaction()?;
            state.store.external_guard_in(&tx, command, None)?;
            if q.title.is_some() || q.reset_title {
                let title = q.title.as_deref().and_then(|s| {
                    crate::store::one_line(
                        s,
                        hennery_proto::rest::TITLE_MAX_CHARS,
                        hennery_proto::rest::TITLE_MAX_JSON_BYTES,
                    )
                });
                tx.execute("UPDATE sessions SET title=COALESCE(?3,agent_title,prompt_title),title_source=CASE WHEN ?3 IS NOT NULL THEN 'operator' WHEN agent_title IS NOT NULL THEN 'agent' WHEN prompt_title IS NOT NULL THEN 'prompt' END WHERE owner_id = ?1 AND id=?2",params![state.store.owner_id(),command.session_id,title])?;
            }
            if let Some(pin) = q.sidebar_pinned {
                tx.execute(
                    "UPDATE sessions SET sidebar_pinned=?3 WHERE owner_id = ?1 AND id=?2",
                    params![state.store.owner_id(), command.session_id, pin],
                )?;
            }
            if let Some(pin) = q.idle_pinned {
                tx.execute(
                    "UPDATE sessions SET idle_pinned=?3 WHERE owner_id = ?1 AND id=?2",
                    params![state.store.owner_id(), command.session_id, pin],
                )?;
            }
            let mut events = Vec::new();
            for (changed, kind) in [
                (q.title.is_some() || q.reset_title, "session_renamed"),
                (q.sidebar_pinned.is_some(), "sidebar_pin_changed"),
            ] {
                if changed {
                    events.push(crate::store::collector_event(
                        &tx,
                        state.store.owner_id(),
                        &command.session_id,
                        kind,
                        json!({"origin":command.origin()}),
                        &crate::store::stamp_ago(std::time::Duration::ZERO),
                    )?);
                }
            }
            let result = json!({"updated":true});
            finish_in(&tx, state.store.owner_id(), command, &result)?;
            tx.commit()?;
            drop(conn);
            for event in events {
                state.hub.publish(event);
            }
            Ok(Some(result))
        }
        "operation_cancel" | "session_queue_update" => {
            let q: QueueChange = decode(args)?;
            let mut conn = state.store.conn();
            let tx = conn.transaction()?;
            state.store.external_guard_in(&tx, command, None)?;
            let old:Option<(String,String)>=tx.query_row("SELECT tool,args FROM external_operations WHERE owner_id = ?1 AND client_id=?2 AND session_id=?3 AND id=?4 AND state='queued' AND request_id IS NULL",params![state.store.owner_id(),command.client_id,command.session_id,q.operation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (tool, args) = old.ok_or_else(|| anyhow::anyhow!("not_queued"))?;
            if command.tool == "operation_cancel" {
                tx.execute("UPDATE external_operations SET state='cancelled',code='client_cancelled',args=json_object() WHERE owner_id = ?1 AND id=?2",params![state.store.owner_id(),q.operation_id])?;
            } else {
                ensure!(tool == "session_prompt", "not_editable");
                let mut args: Value = serde_json::from_str(&args)?;
                args["text"] = json!(q.text);
                tx.execute(
                    "UPDATE external_operations SET args=?3 WHERE owner_id = ?1 AND id=?2",
                    params![state.store.owner_id(), q.operation_id, serde_json::to_string(&args)?],
                )?;
            }
            let result = json!({"operation_id":q.operation_id,"state":if command.tool=="operation_cancel"{"cancelled"}else{"queued"}});
            finish_in(&tx, state.store.owner_id(), command, &result)?;
            tx.commit()?;
            Ok(Some(result))
        }
        _ => bail!("unknown_tool"),
    }
}
fn finish_in(conn: &Connection, owner: &str, command: &Command, result: &Value) -> Result<()> {
    conn.execute(
        "UPDATE external_operations SET state='completed',args=json_object(),result=?3 WHERE owner_id = ?1 AND id=?2",
        params![owner, command.operation_id, serde_json::to_string(result)?],
    )?;
    Ok(())
}
fn form_in(conn: &Connection, owner: &str, session: &str, pending: &str) -> Result<String> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT kind,state,payload FROM pending WHERE owner_id = ?1 AND session_id=?2 AND pending_id=?3",
            params![owner, session, pending],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (kind, state, payload) = row.ok_or_else(|| anyhow::anyhow!("not_found"))?;
    ensure!(kind != "permission", "operator_required");
    ensure!(kind == "elicitation" && state == "open", "not_open");
    Ok(payload)
}
fn queue_answer(state: &AppState, command: &Command, q: Answer) -> Result<()> {
    let payload = {
        let conn = state.store.conn();
        state.store.external_guard_in(&conn, command, None)?;
        form_in(&conn, state.store.owner_id(), &q.session_id, &q.pending_id)?
    };
    let answer = AnswerRequest::Elicitation {
        action: q.action,
        content: q.content,
    };
    crate::delegation_form::validate_with_hook(&serde_json::from_str(&payload)?, &answer, || {})
        .map_err(|_| anyhow::anyhow!("invalid_answer"))?;
    let mut conn = state.store.conn();
    let tx = conn.transaction()?;
    state.store.external_guard_in(&tx, command, None)?;
    ensure!(
        form_in(&tx, state.store.owner_id(), &q.session_id, &q.pending_id)? == payload,
        "form_changed"
    );
    let crate::store::AnswerSubmission::Queued(queued) =
        state
            .store
            .submit_answer_in(&tx, &q.session_id, &q.pending_id, &answer, &command.origin())?
    else {
        bail!("already_answered")
    };
    tx.execute(
        "UPDATE answer_queue SET delegation_handoff=0 WHERE owner_id = ?1 AND request_id=?2",
        params![state.store.owner_id(), queued.request_id],
    )?;
    tx.execute(
        "UPDATE external_operations SET args=json_object(),result=?3 WHERE owner_id = ?1 AND id=?2",
        params![
            state.store.owner_id(),
            command.operation_id,
            serde_json::to_string(&json!({"pending_id":q.pending_id,"request_id":queued.request_id}))?
        ],
    )?;
    tx.commit()?;
    drop(conn);
    state.hub.publish(queued.event);
    Ok(())
}

/// Durable ordinary-answer outbox. Reconnect never routes this through the
/// operator queue: every send is fenced by current external authority.
pub(crate) fn dispatch_answers(state: &AppState) -> Result<()> {
    let mut conn = state.store.conn();
    let rows = {
        let mut stmt=conn.prepare("SELECT o.id,o.client_id,o.revision,o.tool,o.session_id,o.turn_id,q.pending_id,q.request_id,q.answer,q.delivered,s.host_id,o.request_id FROM external_operations o JOIN answer_queue q ON q.owner_id=o.owner_id AND json_extract(q.input_origin,'$.operation_id')=o.id AND json_extract(q.input_origin,'$.client_id')=o.client_id AND json_extract(q.input_origin,'$.kind')='external_client' AND q.session_id=o.session_id JOIN sessions s ON s.owner_id=o.owner_id AND s.id=o.session_id WHERE o.owner_id = ?1 AND o.tool='session_answer' AND o.state IN ('running','uncertain') LIMIT 32")?;
        stmt.query_map([state.store.owner_id()], |r| {
            Ok((
                Command {
                    operation_id: r.get(0)?,
                    client_id: r.get(1)?,
                    revision: r.get(2)?,
                    tool: r.get(3)?,
                    session_id: r.get(4)?,
                    turn_id: r.get(5)?,
                },
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<bool>>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, Option<String>>(11)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    state
        .external_commands
        .answer_connections
        .lock()
        .expect("answer connections")
        .retain(|id, _| rows.iter().any(|r| r.0.operation_id == *id));
    for (command, pending, request, answer, delivered, host, handed) in rows {
        if let Some(delivered) = delivered {
            conn.execute(
                "UPDATE external_operations SET state=?3,code=?4,args=json_object() WHERE owner_id = ?1 AND id=?2",
                params![
                    state.store.owner_id(),
                    command.operation_id,
                    if delivered { "completed" } else { "failed" },
                    if delivered { None } else { Some("not_delivered") }
                ],
            )?;
            continue;
        }
        let authorized = state
            .store
            .external_guard_in(&conn, &command, None)
            .and_then(|()| form_in(&conn, state.store.owner_id(), &command.session_id, &pending).map(|_| ()));
        if authorized.is_err() {
            if handed.is_none() {
                let tx = conn.transaction()?;
                tx.execute("DELETE FROM answer_queue WHERE owner_id = ?1 AND pending_id=?2 AND json_extract(input_origin,'$.operation_id')=?3",params![state.store.owner_id(),pending,command.operation_id])?;
                let event = crate::store::collector_event(
                    &tx,
                    state.store.owner_id(),
                    &command.session_id,
                    "answer_cancelled",
                    json!({"pending_id":pending,"reason":"external_authority_changed","origin":command.origin()}),
                    &crate::store::stamp_ago(std::time::Duration::ZERO),
                )?;
                tx.execute("UPDATE external_operations SET state='cancelled',code='grant_changed',args=json_object() WHERE owner_id = ?1 AND id=?2",params![state.store.owner_id(),command.operation_id])?;
                tx.commit()?;
                state.hub.publish(event);
            } else {
                conn.execute("UPDATE external_operations SET state='uncertain',code='delivery_unknown' WHERE owner_id = ?1 AND id=?2 AND state='running'",params![state.store.owner_id(),command.operation_id])?;
            }
            continue;
        }
        let Some(connection) = state.hub.ready_connection(&host) else {
            continue;
        };
        if state
            .external_commands
            .answer_connections
            .lock()
            .expect("answer connections")
            .get(&command.operation_id)
            == Some(&connection)
        {
            continue;
        }
        // A second send after reconnect is safe: host dedupes pending_id. The
        // same request ID is retained, and every retry rechecks the grant.
        state.store.external_request_in(&conn, &command, &request)?;
        if state.hub.notify(
            &host,
            crate::store::answer_frame(request, &command.session_id, &pending, serde_json::from_str(&answer)?),
        ) {
            state
                .external_commands
                .answer_connections
                .lock()
                .expect("answer connections")
                .insert(command.operation_id.clone(), connection);
        } else if handed.is_none() {
            conn.execute(
                "UPDATE external_operations SET request_id=NULL WHERE owner_id = ?1 AND id=?2",
                params![state.store.owner_id(), command.operation_id],
            )?;
        }
    }
    Ok(())
}

/// Final synchronous catalog-probe handoff, fenced by the external principal.
/// Poll only Hub::probe under the gate; completion processing may use Store.
pub(crate) async fn guarded_catalog<F>(
    state: &AppState,
    identity: Option<&ExternalIdentity>,
    host: &str,
    cwd: &str,
    future: F,
) -> std::result::Result<hennery_proto::frames::HostFrame, crate::hub::RequestError>
where
    F: std::future::Future<Output = std::result::Result<hennery_proto::frames::HostFrame, crate::hub::RequestError>>,
{
    let Some(identity) = identity else {
        return future.await;
    };
    let mut future = Box::pin(future);
    let first = std::future::poll_fn(|cx| {
        let conn = state.store.conn();
        let checked = (|| -> Result<()> {
            ensure!(
                external_clients::live(&conn, state.store.owner_id(), identity)?,
                "external_unauthenticated"
            );
            ensure!(identity.client.control_sessions, "external_control_forbidden");
            placement(
                &conn,
                state.store.owner_id(),
                &identity.client,
                host,
                cwd,
                &identity.client.hat_id,
            )
        })();
        std::task::Poll::Ready(match checked {
            Ok(()) => future.as_mut().poll(cx),
            Err(_) => std::task::Poll::Ready(Err(crate::hub::RequestError::Rejected {
                code: "external_unauthenticated".into(),
                message: "catalog authority changed".into(),
            })),
        })
    })
    .await;
    match first {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => future.await,
    }
}
