//! A scripted ACP agent for tests. Speaks ACP over stdio via the
//! `agent-client-protocol` crate, so the host is tested against the same
//! wire format real adapters use.

use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk, InitializeRequest,
    InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelect, SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
    TextContent,
};
use agent_client_protocol::{Agent, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeScript, SCRIPT_ENV};
use std::io::Write;
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
    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            {
                let catalogue = catalogue.clone();
                async move |req: InitializeRequest, responder, _cx| {
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
                        InitializeResponse::new(req.protocol_version)
                            .agent_capabilities(AgentCapabilities::new().load_session(load_session)),
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
                async move |req: LoadSessionRequest, responder, cx| {
                    // History first, then the answer: an ACP agent replays a
                    // loaded session as `session/update`s before it responds.
                    for update in &script.replay {
                        cx.send_notification(UntypedMessage::new(
                            "session/update",
                            serde_json::json!({ "sessionId": req.session_id, "update": update }),
                        )?)?;
                    }
                    match script.load_error {
                        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                        None if script.config_in_update_only => {
                            if let Some(options) = announced() {
                                cx.send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                                ))?;
                            }
                            responder.respond(LoadSessionResponse::new())
                        }
                        None => responder.respond(LoadSessionResponse::new().config_options(announced())),
                    }
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
                            drop(responder);
                            Ok(())
                        });
                    }
                    let is_model = catalogue
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|o| o.id == req.config_id && o.category == Some(SessionConfigOptionCategory::Model));
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
                                switch(&mut options, &req, None, &script.sticky_options).map(|()| options.clone())
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
                    let switched = {
                        let mut options = catalogue.lock().unwrap();
                        switch(
                            &mut options,
                            &req,
                            script.model_switch_sets_mode.as_deref(),
                            &script.sticky_options,
                        )
                        .map(|()| options.clone())
                    };
                    match switched {
                        Ok(_) if script.empty_config_read_back => {
                            responder.respond(SetSessionConfigOptionResponse::new(Vec::new()))
                        }
                        Ok(options) => responder.respond(SetSessionConfigOptionResponse::new(options)),
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
                async move |_n: CancelNotification, _cx| {
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
                async move |req: PromptRequest, responder, cx| {
                    let script = script.clone();
                    let cx2 = cx.clone();
                    cancel.send_replace(false);
                    let mut cancelled = cancel.subscribe();
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
                    cx.spawn(async move {
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
    options: &mut [SessionConfigOption],
    req: &SetSessionConfigOptionRequest,
    model_switch_sets_mode: Option<&str>,
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
    if is_model
        && !sticky
        && let Some(mode) = model_switch_sets_mode
    {
        set_select(options, &SessionConfigOptionCategory::Mode, mode);
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
