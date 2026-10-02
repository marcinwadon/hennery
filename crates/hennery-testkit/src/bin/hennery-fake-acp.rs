//! A scripted ACP agent for tests. Speaks ACP over stdio via the
//! `agent-client-protocol` crate, so the host is tested against the same
//! wire format real adapters use.

use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk, InitializeRequest,
    InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    PromptCapabilities, PromptRequest, PromptResponse, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelect, SessionConfigSelectOption,
    SessionConfigSelectOptions, SessionConfigValueId, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, SentRequest, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeAsk, FakeScript, SCRIPT_ENV};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// The fake's config options, as the switches so far have left them.
type Catalogue = Arc<Mutex<Vec<SessionConfigOption>>>;

#[tokio::main]
async fn main() -> agent_client_protocol::Result<()> {
    let script: FakeScript = std::env::var(SCRIPT_ENV)
        .ok()
        .map(|s| serde_json::from_str(&s).expect("valid fake script JSON"))
        .unwrap_or_default();

    for line in &script.stderr_lines {
        eprintln!("{line}");
    }
    if let Some(path) = &script.grandchild_pid_file {
        // Detached from our stdio so it cannot hold the ACP pipes open; it
        // stays in our process group, like an agent CLI's own subprocess.
        // Never waited on by design: it must outlive us unless the host kills
        // the whole group.
        #[allow(clippy::zombie_processes)]
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn grandchild");
        std::fs::write(path, child.id().to_string()).expect("write grandchild pid");
    }

    let load_session = !script.no_load_session;
    let images = !script.no_images;
    let catalogue: Catalogue = Arc::new(Mutex::new(
        serde_json::from_value(serde_json::Value::Array(script.config_options.clone()))
            .expect("valid config options in the fake script"),
    ));
    // Absent, not empty, when the script has none: like an adapter without
    // config options.
    let announced = {
        let catalogue = catalogue.clone();
        move || {
            let options = catalogue.lock().unwrap().clone();
            (!options.is_empty()).then_some(options)
        }
    };
    // `session/cancel` for the prompt in flight: set by the notification,
    // cleared when a prompt starts. Handlers run in arrival order, so a
    // cancel sent right after its prompt is never cleared by that prompt.
    let cancel = Arc::new(watch::channel(false).0);
    // The client advertised form elicitation in `initialize`.
    let forms = Arc::new(AtomicBool::new(false));
    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            {
                let catalogue = catalogue.clone();
                let forms = forms.clone();
                async move |req: InitializeRequest, responder, _cx| {
                    // Typed, like the real adapters' schema validation: a
                    // boolean `elicitation` does not parse, so it counts as
                    // not advertised (P-19).
                    let advertised = req
                        .client_capabilities
                        .elicitation
                        .as_ref()
                        .is_some_and(|elicitation| elicitation.form.is_some());
                    forms.store(advertised, Ordering::SeqCst);
                    // A boolean option is announced as one only to a client
                    // that says it can show one; any other gets an on/off
                    // select, as the real adapters do.
                    let booleans = req
                        .client_capabilities
                        .session
                        .as_ref()
                        .and_then(|session| session.config_options.as_ref())
                        .is_some_and(|options| options.boolean.is_some());
                    if !booleans {
                        booleans_as_selects(&mut catalogue.lock().unwrap());
                    }
                    responder.respond(
                        InitializeResponse::new(req.protocol_version).agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(load_session)
                                .prompt_capabilities(PromptCapabilities::new().image(images)),
                        ),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let announced = announced.clone();
                async move |_req: NewSessionRequest, responder, cx| match script.new_session_error {
                    Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                    None if script.config_in_update_only => {
                        if let Some(options) = announced() {
                            cx.send_notification(SessionNotification::new(
                                "fake-session-1",
                                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                            ))?;
                        }
                        responder.respond(NewSessionResponse::new("fake-session-1"))
                    }
                    None => responder.respond(NewSessionResponse::new("fake-session-1").config_options(announced())),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let announced = announced.clone();
                let forms = forms.clone();
                async move |req: LoadSessionRequest, responder, cx| {
                    // Owned locals: the captured fields sit behind `&mut
                    // self` (this handler is called via a shared/mut
                    // reference, reused for every `session/load`), so
                    // anything moved later (into a nested `cx.spawn`, or
                    // into `answer_load`) must be its own clone first.
                    let script = script.clone();
                    let announced = announced.clone();
                    let forms = forms.load(Ordering::SeqCst);
                    // History first, then the answer: an ACP agent replays a
                    // loaded session as `session/update`s before it responds.
                    for update in &script.replay {
                        cx.send_notification(UntypedMessage::new(
                            "session/update",
                            serde_json::json!({ "sessionId": req.session_id, "update": update }),
                        )?)?;
                    }
                    if script.ask_on_load
                        && let Some(ask) = script.asks.first().copied()
                        && let Some(request) = ask_request(ask, &req.session_id, forms)?
                    {
                        // On the wire before the load's answer; awaited
                        // from a task of its own.
                        let sent = cx.send_request(request);
                        if script.ask_on_load_waits {
                            // The load is answered only once the question is,
                            // then exactly as the non-waiting path below:
                            // `load_error` / `config_in_update_only` still
                            // apply, just deferred until the answer is in.
                            let (cx2, session, script2, announced2) =
                                (cx.clone(), req.session_id.clone(), script.clone(), announced.clone());
                            return cx.spawn(async move {
                                let _ = sent.block_task().await;
                                answer_load(&script2, announced2, &session, &cx2, responder)
                            });
                        }
                        let (cx2, session) = (cx.clone(), req.session_id.clone());
                        cx.spawn(async move {
                            let echo = echo(ask, sent.block_task().await);
                            cx2.send_notification(chunk(&session, echo))
                        })?;
                    }
                    answer_load(&script, announced, &req.session_id, &cx, responder)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let catalogue = catalogue.clone();
                async move |req: SetSessionConfigOptionRequest, responder, cx| {
                    log_switch(&script, &req);
                    if script.hang_config {
                        // Keep the responder alive, unanswered, for good.
                        return cx.spawn(async move {
                            std::future::pending::<()>().await;
                            // Unreachable: `pending()` never resolves, so this task
                            // (and the responder it holds open) only ever ends when
                            // the process is killed.
                            drop(responder);
                            Ok(())
                        });
                    }
                    let is_model = catalogue
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|o| o.id == req.config_id && o.category == Some(SessionConfigOptionCategory::Model));
                    if let (Some(file), true) = (script.model_switch_answer_on_file.clone(), is_model) {
                        // Answered only once the test creates `file`, from a
                        // task of its own, backlog first (see the script).
                        let script = script.clone();
                        let catalogue = catalogue.clone();
                        let cx2 = cx.clone();
                        return cx.spawn(async move {
                            while !std::path::Path::new(&file).exists() {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            for i in 0..script.model_switch_chunks_first.unwrap_or(0) {
                                cx2.send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                        TextContent::new(format!("backlog{i}")),
                                    ))),
                                ))?;
                            }
                            let switched = {
                                let mut options = catalogue.lock().unwrap();
                                switch(&mut options, &req, None, None, &script.sticky_options).map(|()| options.clone())
                            };
                            match switched {
                                Ok(options) => responder.respond(SetSessionConfigOptionResponse::new(options)),
                                Err(message) => {
                                    responder.respond_with_error(agent_client_protocol::Error::new(-32602, message))
                                }
                            }
                        });
                    }
                    if is_model && let Some(n) = script.model_switch_chunks_first {
                        // Sent inline, with no sleep, so they land on the
                        // wire strictly before this switch's own answer
                        // (below): a known, deterministic backlog ahead of
                        // the answer, for testing `out_deadline` without
                        // racing another task's timing.
                        for i in 0..n {
                            cx.send_notification(SessionNotification::new(
                                req.session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                    TextContent::new(format!("backlog{i}")),
                                ))),
                            ))?;
                        }
                    }
                    if let (Some(delay), true) = (script.slow_model_switch_ms, is_model) {
                        // Answered late, from a task of its own, so other
                        // requests are handled meanwhile (the TS SDK the real
                        // adapters use does not await one request before
                        // reading the next). The mode clamp lands only after
                        // the answer: the client cannot see it coming.
                        let script = script.clone();
                        let catalogue = catalogue.clone();
                        return cx.spawn(async move {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            let switched = {
                                let mut options = catalogue.lock().unwrap();
                                switch(&mut options, &req, None, None, &script.sticky_options).map(|()| options.clone())
                            };
                            let answered = match switched {
                                Ok(options) => responder.respond(SetSessionConfigOptionResponse::new(options)),
                                Err(message) => {
                                    responder.respond_with_error(agent_client_protocol::Error::new(-32602, message))
                                }
                            };
                            if let Some(mode) = &script.model_switch_sets_mode {
                                set_select(&mut catalogue.lock().unwrap(), &SessionConfigOptionCategory::Mode, mode);
                            }
                            answered
                        });
                    }
                    if script.announce_before_switch {
                        // Sent before this switch is applied at all, so it
                        // lands on the wire before its response: exercises a
                        // notification older than the switch's own (newer)
                        // read-back (fix round 2, F2, case A).
                        let before = catalogue.lock().unwrap().clone();
                        cx.send_notification(SessionNotification::new(
                            req.session_id.clone(),
                            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(before)),
                        ))?;
                    }
                    let switched = {
                        let mut options = catalogue.lock().unwrap();
                        switch(
                            &mut options,
                            &req,
                            script.model_switch_sets_mode.as_deref(),
                            script.model_switch_drops_option.as_deref(),
                            &script.sticky_options,
                        )
                        .map(|()| options.clone())
                    };
                    match switched {
                        Ok(_) if script.empty_config_read_back => {
                            responder.respond(SetSessionConfigOptionResponse::new(Vec::new()))
                        }
                        Ok(options) => {
                            let answered = responder.respond(SetSessionConfigOptionResponse::new(options));
                            if script.announce_after_switch {
                                // Sent right after the response above, so it
                                // lands on the wire after it: exercises a
                                // switch's answer racing a live notification
                                // the agent sends right after it (fix round
                                // 1, F2).
                                let after = {
                                    let mut options = catalogue.lock().unwrap();
                                    set_select(&mut options, &SessionConfigOptionCategory::Mode, "bypass");
                                    options.clone()
                                };
                                cx.send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(after)),
                                ))?;
                            }
                            answered
                        }
                        Err(message) => {
                            responder.respond_with_error(agent_client_protocol::Error::new(-32602, message))
                        }
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let cancel = cancel.clone();
                let ignore = script.ignore_cancel;
                let cancel_received_file = script.cancel_received_file.clone();
                async move |_n: CancelNotification, _cx| {
                    if let Some(path) = &cancel_received_file {
                        // Marks arrival regardless of `ignore`: this is a
                        // signal for "the host forwarded it", not "the
                        // adapter obeyed it".
                        let mut log = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .expect("open cancel_received_file");
                        writeln!(log, "cancel").expect("write cancel_received_file");
                    }
                    if !ignore {
                        cancel.send_replace(true);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let forms = forms.clone();
                async move |req: PromptRequest, responder, cx| {
                    let script = script.clone();
                    let cx2 = cx.clone();
                    cancel.send_replace(false);
                    let mut cancelled = cancel.subscribe();
                    for update in &script.prompt_updates {
                        cx.send_notification(UntypedMessage::new(
                            "session/update",
                            serde_json::json!({ "sessionId": req.session_id, "update": update }),
                        )?)?;
                    }
                    if let Some(mode) = &script.prompt_sets_mode {
                        let options = {
                            let mut options = catalogue.lock().unwrap();
                            set_select(&mut options, &SessionConfigOptionCategory::Mode, mode);
                            options.clone()
                        };
                        cx.send_notification(SessionNotification::new(
                            req.session_id.clone(),
                            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                        ))?;
                    }
                    let forms = forms.load(Ordering::SeqCst);
                    let echoes: Vec<String> = req.prompt.iter().filter_map(image_echo).collect();
                    cx.spawn(async move {
                        for echo in echoes {
                            cx2.send_notification(chunk(&req.session_id, echo))?;
                        }
                        if script.crash_while_asking {
                            // Sent, never awaited: kept alive until the crash.
                            let mut sent: Vec<SentRequest<serde_json::Value>> = Vec::new();
                            for ask in &script.asks {
                                if let Some(request) = ask_request(*ask, &req.session_id, forms)? {
                                    sent.push(cx2.send_request(request));
                                }
                            }
                            crash().await;
                        }
                        if !script.asks.is_empty() {
                            for echo in ask_all(&cx2, &script, &req.session_id, forms, &cancelled).await? {
                                cx2.send_notification(chunk(&req.session_id, echo))?;
                            }
                            if *cancelled.borrow() {
                                return responder.respond(PromptResponse::new(StopReason::Cancelled));
                            }
                        }
                        if script.flood {
                            // Back to back until cancelled, yielding (never
                            // sleeping, unless paced) so the cancel can land.
                            for chunk in script.chunks.iter().cycle() {
                                if *cancelled.borrow() {
                                    return responder.respond(PromptResponse::new(StopReason::Cancelled));
                                }
                                cx2.send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                        TextContent::new(chunk.clone()),
                                    ))),
                                ))?;
                                match script.flood_interval_ms {
                                    Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                                    None => tokio::task::yield_now().await,
                                }
                            }
                        }
                        for (sent, chunk) in script.chunks.into_iter().enumerate() {
                            if script.exit_after_chunks == Some(sent) {
                                crash().await;
                            }
                            // A cancelled prompt stops streaming and answers
                            // `cancelled`, as ACP asks of an agent.
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_millis(script.chunk_delay_ms)) => {}
                                _ = cancelled.wait_for(|c| *c) => {
                                    return match script.cancel_error {
                                        Some(code) => responder
                                            .respond_with_error(agent_client_protocol::Error::new(code, "aborted")),
                                        None => responder.respond(PromptResponse::new(StopReason::Cancelled)),
                                    };
                                }
                            }
                            cx2.send_notification(SessionNotification::new(
                                req.session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                    TextContent::new(chunk),
                                ))),
                            ))?;
                        }
                        if script.exit_after_chunks.is_some() {
                            crash().await;
                        }
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}

/// Answer `session/load` as the script asks: the scripted error, options
/// announced only in a preceding update, or a normal read-back. Shared by
/// the immediate path and the `ask_on_load_waits` path, which only defers
/// reaching this until the load-time question is answered — the same rules
/// apply either way.
fn answer_load(
    script: &FakeScript,
    announced: impl FnOnce() -> Option<Vec<SessionConfigOption>>,
    session: &SessionId,
    cx: &ConnectionTo<Client>,
    responder: Responder<LoadSessionResponse>,
) -> agent_client_protocol::Result<()> {
    match script.load_error {
        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
        None if script.config_in_update_only => {
            if let Some(options) = announced() {
                cx.send_notification(SessionNotification::new(
                    session.clone(),
                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                ))?;
            }
            responder.respond(LoadSessionResponse::new())
        }
        None => responder.respond(LoadSessionResponse::new().config_options(announced())),
    }
}

/// What the fake says back for an image block: its type and the SHA-256
/// of the bytes it decoded (`FakeScript::no_images`).
fn image_echo(block: &ContentBlock) -> Option<String> {
    use base64::Engine;
    use sha2::Digest;
    let ContentBlock::Image(image) = block else {
        return None;
    };
    let digest = match base64::engine::general_purpose::STANDARD.decode(&image.data) {
        Ok(bytes) => format!("{:x}", sha2::Sha256::digest(bytes)),
        Err(_) => "undecodable".into(),
    };
    Some(format!("image:{}:{digest}\n", image.mime_type))
}

/// One text chunk of the agent's reply.
fn chunk(session: &SessionId, text: String) -> SessionNotification {
    SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text)))),
    )
}

/// The ACP request for one ask, sent untyped so the test sees the answer
/// exactly as the client wrote it. `None` for an elicitation to a client
/// that did not advertise form elicitation.
fn ask_request(
    ask: FakeAsk,
    session: &SessionId,
    forms: bool,
) -> agent_client_protocol::Result<Option<UntypedMessage>> {
    match ask {
        FakeAsk::Permission => UntypedMessage::new(
            "session/request_permission",
            serde_json::json!({
                "sessionId": session,
                "toolCall": {"toolCallId": "call-1", "title": "Write notes.txt", "kind": "edit"},
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
                ]
            }),
        )
        .map(Some),
        FakeAsk::FuturePermission => UntypedMessage::new(
            "session/request_permission",
            serde_json::json!({
                "sessionId": session,
                "toolCall": {"toolCallId": "call-1", "title": "Write notes.txt", "kind": "edit"},
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "allow_session", "name": "Allow for this session", "kind": "allow_for_session"}
                ]
            }),
        )
        .map(Some),
        FakeAsk::Elicitation if forms => UntypedMessage::new(
            "elicitation/create",
            serde_json::json!({
                "mode": "form",
                "sessionId": session,
                "message": "What should the file be called?",
                "requestedSchema": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            }),
        )
        .map(Some),
        FakeAsk::Elicitation => Ok(None),
        FakeAsk::Unknown => UntypedMessage::new("_fake/unknown", serde_json::json!({ "sessionId": session })).map(Some),
    }
}

/// An ask whose echo is known (not asked), or still out.
enum Asked {
    Echo(String),
    Out(FakeAsk, SentRequest<serde_json::Value>),
}

/// Ask the script's questions and return one echo per ask, in order. One
/// at a time, unless `asks_at_once`; a cancelled prompt asks nothing more.
async fn ask_all(
    cx: &ConnectionTo<Client>,
    script: &FakeScript,
    session: &SessionId,
    forms: bool,
    cancelled: &watch::Receiver<bool>,
) -> agent_client_protocol::Result<Vec<String>> {
    let mut asked = Vec::new();
    for ask in &script.asks {
        if *cancelled.borrow() {
            break;
        }
        let Some(request) = ask_request(*ask, session, forms)? else {
            asked.push(Asked::Echo("elicitation:unsupported".into()));
            continue;
        };
        let sent = cx.send_request(request);
        if script.withdraw_asks {
            sent.cancel()?;
        }
        asked.push(if script.asks_at_once {
            Asked::Out(*ask, sent)
        } else {
            Asked::Echo(echo(*ask, sent.block_task().await))
        });
    }
    let mut echoes = Vec::new();
    for asked in asked {
        echoes.push(match asked {
            Asked::Echo(echo) => echo,
            Asked::Out(ask, sent) => echo(ask, sent.block_task().await),
        });
    }
    Ok(echoes)
}

/// The answer to one ask, as the agent understood it.
fn echo(ask: FakeAsk, answer: agent_client_protocol::Result<serde_json::Value>) -> String {
    let name = match ask {
        FakeAsk::Permission | FakeAsk::FuturePermission => "permission",
        FakeAsk::Elicitation => "elicitation",
        FakeAsk::Unknown => "unknown",
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(err) => return format!("{name}:error:{}", i32::from(err.code)),
    };
    match ask {
        FakeAsk::Permission | FakeAsk::FuturePermission => match answer["outcome"]["outcome"].as_str() {
            Some("selected") => format!(
                "permission:selected:{}",
                answer["outcome"]["optionId"].as_str().unwrap_or("?")
            ),
            Some(other) => format!("permission:{other}"),
            None => format!("permission:unreadable:{answer}"),
        },
        FakeAsk::Elicitation => match (answer["action"].as_str(), answer.get("content")) {
            (Some(action), Some(content)) => format!("elicitation:{action}:{content}"),
            (Some(action), None) => format!("elicitation:{action}"),
            (None, _) => format!("elicitation:unreadable:{answer}"),
        },
        FakeAsk::Unknown => format!("unknown:answered:{answer}"),
    }
}

/// Record a switch in the script's `config_log`, if it has one.
fn log_switch(script: &FakeScript, req: &SetSessionConfigOptionRequest) {
    let Some(path) = &script.config_log else {
        return;
    };
    let value = match &req.value {
        SessionConfigOptionValue::ValueId { value } => value.to_string(),
        SessionConfigOptionValue::Boolean { value } => value.to_string(),
        other => format!("{other:?}"),
    };
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the config log");
    writeln!(log, "{}={value}", req.config_id).expect("write the config log");
}

/// Apply one switch as a real adapter would, or say why it cannot. A switch
/// of a `sticky` option is accepted and changes nothing.
fn switch(
    options: &mut Vec<SessionConfigOption>,
    req: &SetSessionConfigOptionRequest,
    model_switch_sets_mode: Option<&str>,
    model_switch_drops_option: Option<&str>,
    sticky: &[String],
) -> Result<(), String> {
    let option = options
        .iter_mut()
        .find(|o| o.id == req.config_id)
        .ok_or_else(|| format!("unknown config option {}", req.config_id))?;
    let is_model = option.category == Some(SessionConfigOptionCategory::Model);
    let sticky = sticky.iter().any(|id| **id == *req.config_id.0);
    match (&mut option.kind, &req.value) {
        (SessionConfigKind::Select(select), SessionConfigOptionValue::ValueId { value })
            if offers(&select.options, value) =>
        {
            if !sticky {
                select.current_value = value.clone();
            }
        }
        (SessionConfigKind::Boolean(toggle), SessionConfigOptionValue::Boolean { value }) => {
            if !sticky {
                toggle.current_value = *value;
            }
        }
        _ => return Err(format!("invalid value for {}", req.config_id)),
    }
    if is_model && !sticky {
        if let Some(mode) = model_switch_sets_mode {
            set_select(options, &SessionConfigOptionCategory::Mode, mode);
        }
        if let Some(drop_id) = model_switch_drops_option {
            options.retain(|o| &*o.id.0 != drop_id);
        }
    }
    Ok(())
}

fn offers(options: &SessionConfigSelectOptions, value: &SessionConfigValueId) -> bool {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options.iter().any(|o| &o.value == value),
        SessionConfigSelectOptions::Grouped(groups) => {
            groups.iter().flat_map(|g| &g.options).any(|o| &o.value == value)
        }
        _ => false,
    }
}

/// Turn every boolean option into an `on` / `off` select, for a client that
/// did not advertise boolean config options.
fn booleans_as_selects(options: &mut [SessionConfigOption]) {
    for option in options.iter_mut() {
        if let SessionConfigKind::Boolean(toggle) = &option.kind {
            let current = if toggle.current_value { "on" } else { "off" };
            let values = vec![
                SessionConfigSelectOption::new("on", "on"),
                SessionConfigSelectOption::new("off", "off"),
            ];
            option.kind = SessionConfigKind::Select(SessionConfigSelect::new(current, values));
        }
    }
}

/// Set the current value of the select option in `category`, unchecked.
fn set_select(options: &mut [SessionConfigOption], category: &SessionConfigOptionCategory, value: &str) {
    for option in options.iter_mut().filter(|o| o.category.as_ref() == Some(category)) {
        if let SessionConfigKind::Select(select) = &mut option.kind {
            select.current_value = SessionConfigValueId::new(value.to_string());
        }
    }
}

/// Exit mid-turn without answering the prompt. The short pause lets the
/// chunks already sent reach stdout first.
async fn crash() -> ! {
    tokio::time::sleep(Duration::from_millis(100)).await;
    eprintln!("fake-acp: crashing mid-turn");
    std::process::exit(CRASH_EXIT_CODE);
}
