//! Version boundary for the session actor. Actor-facing catalogue types stay
//! stable while v2 wire notifications remain verbatim for persisted projections.
use agent_client_protocol::{
    Agent, ConnectionTo, Error, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, PreparedRequest, UntypedMessage,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
    },
};
use tokio::sync::oneshot;

struct Completion {
    reply: oneshot::Sender<agent_client_protocol::Result<Value>>,
    started: bool,
}

#[derive(Default)]
struct Lifecycle {
    pending: BTreeMap<String, Completion>,
}

/// A locally verified reason to restart the adapter with v1, before creating
/// or resuming any session. The actor owns process teardown and replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryV1 {
    VersionUnsupported,
    SteeringUnavailable,
}

impl RetryV1 {
    pub fn reason(self) -> &'static str {
        match self {
            Self::VersionUnsupported => "adapter explicitly supports ACP v1 but not v2",
            Self::SteeringUnavailable => {
                "this managed adapter does not advertise its existing steering feature over ACP v2; choose ACP v1 or v2/v1 to preserve steering"
            }
        }
    }
    pub fn error_code(self) -> &'static str {
        match self {
            Self::VersionUnsupported => "acp_v2_unsupported",
            Self::SteeringUnavailable => "acp_v2_feature_gap",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Driver {
    conn: ConnectionTo<Agent>,
    version: Arc<AtomicU16>,
    prefer_v2: bool,
    allow_v1: bool,
    require_steering: bool,
    retry_v1: Arc<Mutex<Option<RetryV1>>>,
    fallback: Arc<Mutex<Option<String>>>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    root_session: Arc<Mutex<Option<String>>>,
}

impl Driver {
    pub fn new(conn: ConnectionTo<Agent>, prefer_v2: bool, allow_v1: bool) -> Self {
        Self {
            conn,
            version: Arc::new(AtomicU16::new(1)),
            prefer_v2,
            allow_v1,
            require_steering: false,
            retry_v1: Arc::default(),
            fallback: Arc::new(Mutex::new(None)),
            lifecycle: Arc::default(),
            root_session: Arc::default(),
        }
    }

    /// Managed profiles retain their existing steering feature across protocol
    /// selection. An accepted v2 handshake alone is not proof of feature parity.
    pub fn with_steering_required(mut self, required: bool) -> Self {
        self.require_steering = required;
        self
    }

    pub fn retry_v1(&self) -> Option<RetryV1> {
        *self.retry_v1.lock().expect("retry cause lock")
    }

    pub fn closed(&self) {
        let pending = std::mem::take(&mut self.lifecycle.lock().expect("lifecycle lock").pending);
        for completion in pending.into_values() {
            let _ = completion.reply.send(Err(
                Error::internal_error().data("ACP transport closed before foreground completion")
            ));
        }
    }

    pub fn root_session(&self) -> Option<String> {
        self.root_session.lock().expect("root session lock").clone()
    }

    pub fn version(&self) -> u16 {
        self.version.load(Ordering::SeqCst)
    }
    pub fn fallback(&self) -> Option<String> {
        self.fallback.lock().expect("fallback lock").clone()
    }

    /// Called after the notification was queued to the actor: its final update
    /// must be emitted before the actor reports completion of a submitted turn.
    pub fn observe(&self, payload: &Value) {
        if self.version() != 2 || payload["update"]["sessionUpdate"] != "state_update" {
            return;
        }
        let Some(session) = payload["sessionId"].as_str() else {
            return;
        };
        let update = &payload["update"];
        let mut lifecycle = self.lifecycle.lock().expect("lifecycle lock");
        if matches!(update["state"].as_str(), Some("running" | "requires_action")) {
            if let Some(completion) = lifecycle.pending.get_mut(session) {
                completion.started = true;
            }
            return;
        }
        if update["state"] != "idle" {
            return;
        }
        let reason = update.get("stopReason").and_then(Value::as_str);
        if !lifecycle.pending.get(session).is_some_and(|p| p.started) {
            return;
        }
        let completion = lifecycle.pending.remove(session);
        drop(lifecycle);
        if let Some(completion) = completion {
            let reason = reason.unwrap_or("end_turn");
            let result = if reason == "error" {
                Err(Error::internal_error().data(update.get("error").cloned().unwrap_or(Value::Null)))
            } else {
                Ok(json!({"stopReason":reason,"usage":update.get("usage"),"_meta":update.get("_meta")}))
            };
            let _ = completion.reply.send(result);
        }
    }

    pub fn prepare_request(&self, request: UntypedMessage) -> PreparedRequest<Value> {
        self.conn.prepare_request(request)
    }

    pub fn send_notification<N: JsonRpcNotification>(&self, notification: N) -> agent_client_protocol::Result<()> {
        self.conn.send_notification(notification.to_untyped_message()?)
    }

    pub fn send_request<R: JsonRpcRequest>(&self, request: R) -> Request<R::Response> {
        Request {
            driver: self.clone(),
            request: request.to_untyped_message(),
            _response: PhantomData,
        }
    }
}

pub(crate) struct Request<T> {
    driver: Driver,
    request: agent_client_protocol::Result<UntypedMessage>,
    _response: PhantomData<T>,
}

impl<T: JsonRpcResponse> Request<T> {
    pub fn block_task(self) -> std::pin::Pin<Box<dyn Future<Output = agent_client_protocol::Result<T>> + Send>> {
        let driver = self.driver;
        let request = match self.request {
            Ok(request) => request,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let original_method = request.method.clone();
        if original_method == "initialize" {
            return Box::pin(async move {
                let init = driver.initialize(request.params).await?;
                T::from_json(&original_method, init)
            });
        }
        let version = driver.version();
        let request = match wire_request(request, version) {
            Ok(request) => request,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        let session = request
            .params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if original_method == "session/load" {
            *driver.root_session.lock().expect("root session lock") = session.clone();
        }
        let done = if version == 2 && original_method == "session/prompt" {
            let Some(session) = session.clone() else {
                return Box::pin(std::future::ready(Err(Error::invalid_params())));
            };
            let (tx, rx) = oneshot::channel();
            let mut lifecycle = driver.lifecycle.lock().expect("lifecycle lock");
            if lifecycle.pending.contains_key(&session) {
                return Box::pin(std::future::ready(Err(Error::invalid_request())));
            }
            lifecycle.pending.insert(
                session,
                Completion {
                    reply: tx,
                    started: false,
                },
            );
            Some(rx)
        } else {
            None
        };
        // Publish before returning, just like the SDK. In particular, Cancel
        // read immediately after TurnStarted must follow the prompt on wire.
        let response = driver.conn.send_request(request).block_task();
        Box::pin(async move {
            let response = match response.await {
                Ok(response) => response,
                Err(err) => {
                    if done.is_some() {
                        driver
                            .lifecycle
                            .lock()
                            .expect("lifecycle lock")
                            .pending
                            .remove(session.as_deref().unwrap());
                    }
                    return Err(err);
                }
            };
            if let Some(done) = done {
                if response.get("messageId").and_then(Value::as_str).is_none() {
                    driver
                        .lifecycle
                        .lock()
                        .expect("lifecycle lock")
                        .pending
                        .remove(session.as_deref().unwrap());
                    return Err(Error::invalid_params());
                }
                let ended = done.await.map_err(|_| Error::internal_error())??;
                return T::from_json(&original_method, ended);
            }
            if original_method == "session/new" {
                *driver.root_session.lock().expect("root session lock") =
                    response.get("sessionId").and_then(Value::as_str).map(str::to_owned);
            }
            T::from_json(&original_method, actor_response(response, version))
        })
    }

    pub fn on_receiving_result<F>(
        self,
        task: impl FnOnce(agent_client_protocol::Result<T>) -> F + Send + 'static,
    ) -> agent_client_protocol::Result<()>
    where
        F: Future<Output = agent_client_protocol::Result<()>> + Send + 'static,
    {
        let original = self.request?;
        let method = original.method.clone();
        let version = self.driver.version();
        let request = wire_request(original, version)?;
        self.driver
            .conn
            .prepare_request(request)
            .on_receiving_result(move |result| {
                task(result.and_then(|value| T::from_json(&method, actor_response(value, version))))
            })
    }
}

impl Driver {
    async fn initialize(&self, params: Value) -> agent_client_protocol::Result<Value> {
        let params = if self.prefer_v2 {
            json!({"protocolVersion":2,"info":{"name":"hennery","version":env!("CARGO_PKG_VERSION")},
                "capabilities":{"elicitation":{"form":{}}}})
        } else {
            params
        };
        let response = self
            .conn
            .send_request(UntypedMessage::new("initialize", params.clone())?)
            .block_task()
            .await;
        let response = match response {
            Ok(value) => value,
            Err(error)
                if self.prefer_v2
                    && self.allow_v1
                    && i32::from(error.code) == -32022
                    && error
                        .data
                        .as_ref()
                        .and_then(|d| d.get("supported"))
                        .and_then(Value::as_array)
                        .is_some_and(|versions| {
                            versions.iter().any(|v| v.as_u64() == Some(1))
                                && !versions.iter().any(|v| v.as_u64() == Some(2))
                        })
                    && error
                        .data
                        .as_ref()
                        .and_then(|d| d.get("requested"))
                        .and_then(Value::as_u64)
                        == Some(2) =>
            {
                *self.retry_v1.lock().expect("retry cause lock") = Some(RetryV1::VersionUnsupported);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let selected = response
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .ok_or_else(Error::invalid_params)?;
        if !matches!(selected, 1 | 2)
            || (!self.prefer_v2 && selected != 1)
            || (self.prefer_v2 && !self.allow_v1 && selected != 2)
        {
            return Err(Error::invalid_request().data("adapter selected an unsupported ACP version"));
        }
        self.version.store(selected as u16, Ordering::SeqCst);
        if self.prefer_v2 && selected == 1 && self.fallback().is_none() {
            *self.fallback.lock().expect("fallback lock") = Some("adapter selected ACP v1".into());
        }
        if selected == 1 {
            return Ok(response);
        }
        // Validate required v2 handshake fields before adapting the catalogue.
        let _: agent_client_protocol::v2::schema::InitializeResponse = serde_json::from_value(response.clone())?;
        if self.require_steering && response.pointer("/_meta/steering/supported").and_then(Value::as_bool) != Some(true)
        {
            let reason = RetryV1::SteeringUnavailable;
            *self.retry_v1.lock().expect("retry cause lock") = Some(reason);
            return Err(Error::new(-32603, reason.reason()));
        }
        let caps = &response["capabilities"];
        Ok(
            json!({"protocolVersion":1,"agentInfo":response["info"],"_meta":response.get("_meta"),
            "agentCapabilities":{"loadSession":true,"promptCapabilities":{
                "image":caps.pointer("/session/prompt/image").is_some(),
                "embeddedContext":caps.pointer("/session/prompt/embeddedContext").is_some()}}}),
        )
    }
}

fn wire_request(mut request: UntypedMessage, version: u16) -> agent_client_protocol::Result<UntypedMessage> {
    if version != 2 {
        return Ok(request);
    }
    match request.method.as_str() {
        "session/load" => request.method = "session/resume".into(),
        "session/set_config_option" => {
            if request.params.get("type").is_none() {
                request.params["type"] = if request.params["value"].is_boolean() {
                    "boolean"
                } else {
                    "id"
                }
                .into();
            }
            let _: agent_client_protocol::v2::schema::SetSessionConfigOptionRequest =
                serde_json::from_value(request.params.clone())?;
        }
        _ => {}
    }
    Ok(request)
}

/// Only adapt known catalogue fields, not tool arguments or arbitrary metadata.
pub(crate) fn actor_response(mut value: Value, version: u16) -> Value {
    if version == 2
        && let Some(options) = value.get_mut("configOptions").and_then(Value::as_array_mut)
    {
        for option in options {
            if let Some(option) = option.as_object_mut() {
                if let Some(id) = option.remove("configId") {
                    option.insert("id".into(), id);
                }
                if let Some(groups) = option.get_mut("options").and_then(Value::as_array_mut) {
                    for group in groups {
                        if let Some(group) = group.as_object_mut()
                            && let Some(id) = group.remove("groupId")
                        {
                            group.insert("group".into(), id);
                        }
                    }
                }
            }
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    async fn fixture(
        prefer: bool,
        fallback: bool,
    ) -> (
        Driver,
        DuplexStream,
        BufReader<DuplexStream>,
        tokio::task::JoinHandle<()>,
    ) {
        let (client_write, server_read) = tokio::io::duplex(4096);
        let (server_write, client_read) = tokio::io::duplex(4096);
        let transport = agent_client_protocol::ByteStreams::new(client_write.compat_write(), client_read.compat());
        let (ready, received) = oneshot::channel();
        let slot = Arc::new(std::sync::OnceLock::<Driver>::new());
        let notifications = slot.clone();
        let task = tokio::spawn(async move {
            let _ = agent_client_protocol::Builder::new(agent_client_protocol::Client)
                .on_receive_notification(
                    async move |msg: UntypedMessage, _cx| {
                        if msg.method == "session/update"
                            && let Some(driver) = notifications.get()
                        {
                            driver.observe(&msg.params);
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |conn| {
                    let driver = Driver::new(conn, prefer, fallback);
                    let _ = slot.set(driver.clone());
                    let _ = ready.send(driver);
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await;
        });
        (received.await.unwrap(), server_write, BufReader::new(server_read), task)
    }

    async fn read(reader: &mut BufReader<DuplexStream>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    async fn write(writer: &mut DuplexStream, value: Value) {
        writer.write_all(format!("{value}\n").as_bytes()).await.unwrap();
    }
    async fn initialize(
        driver: &Driver,
        writer: &mut DuplexStream,
        reader: &mut BufReader<DuplexStream>,
        selected: u16,
    ) -> agent_client_protocol::Result<agent_client_protocol::v1::schema::InitializeResponse> {
        let driver = driver.clone();
        let initialized = tokio::spawn(async move {
            driver
                .send_request(agent_client_protocol::v1::schema::InitializeRequest::new(
                    agent_client_protocol::schema::ProtocolVersion::V1,
                ))
                .block_task()
                .await
        });
        let request = read(reader).await;
        let result = if selected == 2 {
            json!({"protocolVersion":2,"info":{"name":"test","version":"0"},"capabilities":{"session":{}}})
        } else {
            json!({"protocolVersion":selected,"agentCapabilities":{}})
        };
        write(writer, json!({"jsonrpc":"2.0","id":request["id"],"result":result})).await;
        initialized.await.unwrap()
    }

    #[tokio::test]
    async fn v2_prompt_ack_is_not_completion_and_idle_without_reason_ends_running_work() {
        let (driver, mut writer, mut reader, task) = fixture(true, false).await;
        initialize(&driver, &mut writer, &mut reader, 2).await.unwrap();
        assert_eq!(driver.version(), 2);
        let command = driver.clone();
        let mut prompt = tokio::spawn(async move {
            command
                .send_request(agent_client_protocol::v1::schema::PromptRequest::new("s", vec![]))
                .block_task()
                .await
        });
        let request = read(&mut reader).await;
        write(
            &mut writer,
            json!({"jsonrpc":"2.0","id":request["id"],"result":{"messageId":"m"}}),
        )
        .await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut prompt)
                .await
                .is_err()
        );
        write(&mut writer,json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn","background":[]}}})).await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut prompt)
                .await
                .is_err()
        );
        for state in ["running", "idle"] {
            write(&mut writer,json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"state_update","state":state,"background":[]}}})).await;
        }
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), prompt)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            response.stop_reason,
            agent_client_protocol::v1::schema::StopReason::EndTurn
        );
        task.abort();
    }

    #[tokio::test]
    async fn automatic_accepts_v1_selection_but_strict_v2_rejects_it() {
        for allow in [false, true] {
            let (driver, mut writer, mut reader, task) = fixture(true, allow).await;
            let result = initialize(&driver, &mut writer, &mut reader, 1).await;
            assert_eq!(result.is_ok(), allow);
            assert_eq!(driver.fallback().is_some(), allow);
            task.abort();
        }
    }

    #[tokio::test]
    async fn managed_v2_requires_steering_before_any_session_is_created() {
        let (driver, mut writer, mut reader, task) = fixture(true, true).await;
        let driver = driver.with_steering_required(true);
        assert!(initialize(&driver, &mut writer, &mut reader, 2).await.is_err());
        assert_eq!(driver.retry_v1(), Some(RetryV1::SteeringUnavailable));
        assert!(driver.root_session().is_none());
        task.abort();
    }

    #[tokio::test]
    async fn generic_invalid_params_is_not_a_reason_to_retry_v1() {
        let (driver, mut writer, mut reader, task) = fixture(true, true).await;
        let command = driver.clone();
        let initialized = tokio::spawn(async move {
            command
                .send_request(agent_client_protocol::v1::schema::InitializeRequest::new(
                    agent_client_protocol::schema::ProtocolVersion::V1,
                ))
                .block_task()
                .await
        });
        let request = read(&mut reader).await;
        write(
            &mut writer,
            json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"message":"bad provider configuration"}}),
        )
        .await;
        assert!(initialized.await.unwrap().is_err());
        assert!(driver.fallback().is_none());
        let mut line = String::new();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), reader.read_line(&mut line))
                .await
                .is_err()
        );
        task.abort();
    }

    #[tokio::test]
    async fn explicit_version_error_requests_process_restart_without_reinitializing_connection() {
        let (driver, mut writer, mut reader, task) = fixture(true, true).await;
        let command = driver.clone();
        let initialized = tokio::spawn(async move {
            command
                .send_request(agent_client_protocol::v1::schema::InitializeRequest::new(
                    agent_client_protocol::schema::ProtocolVersion::V1,
                ))
                .block_task()
                .await
        });
        let request = read(&mut reader).await;
        write(
            &mut writer,
            json!({"jsonrpc":"2.0","id":request["id"],"error":{
                "code":-32022,"message":"Unsupported protocol version","data":{"requested":2,"supported":[1]}
            }}),
        )
        .await;
        assert!(initialized.await.unwrap().is_err());
        assert_eq!(driver.retry_v1(), Some(RetryV1::VersionUnsupported));
        assert!(driver.root_session().is_none());
        let mut line = String::new();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), reader.read_line(&mut line))
                .await
                .is_err()
        );
        task.abort();
    }

    #[tokio::test]
    async fn prompt_is_published_before_cancel_and_disconnect_fails_accepted_work() {
        let (driver, mut writer, mut reader, task) = fixture(true, false).await;
        initialize(&driver, &mut writer, &mut reader, 2).await.unwrap();
        let response = driver
            .send_request(agent_client_protocol::v1::schema::PromptRequest::new("s", vec![]))
            .block_task();
        driver
            .send_notification(agent_client_protocol::v1::schema::CancelNotification::new("s"))
            .unwrap();
        let request = read(&mut reader).await;
        assert_eq!(request["method"], "session/prompt");
        assert_eq!(read(&mut reader).await["method"], "session/cancel");
        write(
            &mut writer,
            json!({"jsonrpc":"2.0","id":request["id"],"result":{"messageId":"m"}}),
        )
        .await;
        driver.closed();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), response)
                .await
                .unwrap()
                .is_err()
        );
        task.abort();
    }

    #[test]
    fn v2_resume_and_config_are_not_v1_envelopes() {
        let request = wire_request(
            UntypedMessage::new("session/load", json!({"sessionId":"s","cwd":"/tmp","mcpServers":[]})).unwrap(),
            2,
        )
        .unwrap();
        assert_eq!(request.method, "session/resume");
        let request = wire_request(
            UntypedMessage::new(
                "session/set_config_option",
                json!({"sessionId":"s","configId":"model","value":"x"}),
            )
            .unwrap(),
            2,
        )
        .unwrap();
        assert_eq!(request.params["type"], "id");
    }
    #[test]
    fn catalogue_adaptation_is_scoped() {
        let value = actor_response(
            json!({"configOptions":[{"configId":"model","type":"select","options":[{"groupId":"g"}]}],"_meta":{"configId":"keep"}}),
            2,
        );
        assert_eq!(value["configOptions"][0]["id"], "model");
        assert_eq!(value["configOptions"][0]["options"][0]["group"], "g");
        assert_eq!(value["_meta"]["configId"], "keep");
    }
}
