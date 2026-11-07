//! Typed D2 dispatch contract. The endpoint passes its retained, duplicate-free
//! JSON tree here; this module never reparses the HTTP body or trusts authority
//! supplied in tool arguments.
use hennery_proto::delegation::*;
use hennery_proto::frames::{ConfigValue, ElicitationAction};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const MAX_OPERATION_KEY_BYTES: usize = 128;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CWD_BYTES: usize = 4096;
pub const MAX_CONFIG_ID_BYTES: usize = 512;
pub const MAX_AXES: usize = 64;
pub const MAX_LIST_LIMIT: u16 = 100;

#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    Capabilities(CapabilitiesArgs),
    Spawn(SpawnArgs),
    Send(SendArgs),
    Answer(AnswerArgs),
    Status(StatusArgs),
    List(ListArgs),
    Close(CloseArgs),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolArgsError {
    UnknownTool,
    InvalidArguments,
    InvalidAnswerContent,
}
impl std::fmt::Display for ToolArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownTool => "unknown tool",
            Self::InvalidArguments => "invalid tool arguments",
            Self::InvalidAnswerContent => "content must be a JSON object matching the child's requestedSchema in the question or session_status; use action accept with object content, or omit content for decline/cancel",
        })
    }
}
impl std::error::Error for ToolArgsError {}

/// Exact names and exact typed object fields only. No session, owner, hat or
/// parent identifiers from the caller can become authorization context.
pub fn decode_tool(name: &str, arguments: &Value) -> Result<ToolCall, ToolArgsError> {
    if !arguments.is_object() {
        return Err(ToolArgsError::InvalidArguments);
    }
    macro_rules! decode {
        ($ty:ty, $variant:ident) => {
            ToolCall::$variant(
                serde_json::from_value::<$ty>(arguments.clone()).map_err(|_| ToolArgsError::InvalidArguments)?,
            )
        };
    }
    let mut call = match name {
        "session_capabilities" => decode!(CapabilitiesArgs, Capabilities),
        "session_spawn" => decode!(SpawnArgs, Spawn),
        "session_send" => decode!(SendArgs, Send),
        "session_answer" => decode!(AnswerArgs, Answer),
        "session_status" => decode!(StatusArgs, Status),
        "session_list" => decode!(ListArgs, List),
        "session_close" => decode!(CloseArgs, Close),
        _ => return Err(ToolArgsError::UnknownTool),
    };
    if let ToolCall::Answer(args) = &mut call {
        // Claude ACP can encode an MCP object argument as a JSON string. Accept
        // that transport spelling only; canonicalize before digesting, retain
        // duplicate-key refusal, and apply the same stored form validator below.
        if let Some(Value::String(text)) = &args.content {
            if text.len() > MAX_TEXT_BYTES || args.action != ElicitationAction::Accept {
                return Err(ToolArgsError::InvalidAnswerContent);
            }
            args.content = Some(
                hennery_gateway::decode_json_once(text.as_bytes())
                    .filter(Value::is_object)
                    .ok_or(ToolArgsError::InvalidAnswerContent)?,
            );
        }
        if match args.action {
            ElicitationAction::Accept => !args.content.as_ref().is_some_and(Value::is_object),
            ElicitationAction::Decline | ElicitationAction::Cancel => args.content.is_some(),
        } {
            return Err(ToolArgsError::InvalidAnswerContent);
        }
    }
    if !call.valid() {
        return Err(ToolArgsError::InvalidArguments);
    }
    Ok(call)
}
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains('\0')
}
fn opaque(value: &str) -> bool {
    bounded(value, MAX_OPERATION_KEY_BYTES)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}
fn optional(value: &Option<String>, check: impl Fn(&str) -> bool) -> bool {
    value.as_deref().is_none_or(check)
}
impl ToolCall {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Capabilities(_) => "session_capabilities",
            Self::Spawn(_) => "session_spawn",
            Self::Send(_) => "session_send",
            Self::Answer(_) => "session_answer",
            Self::Status(_) => "session_status",
            Self::List(_) => "session_list",
            Self::Close(_) => "session_close",
        }
    }
    pub fn operation_key(&self) -> Option<&str> {
        match self {
            Self::Spawn(a) => Some(&a.operation_key),
            Self::Send(a) => Some(&a.operation_key),
            Self::Answer(a) => Some(&a.operation_key),
            Self::Close(a) => Some(&a.operation_key),
            Self::Status(a) => a.operation_key.as_deref(),
            _ => None,
        }
    }
    fn valid(&self) -> bool {
        if self.operation_key().is_some_and(|key| !opaque(key)) {
            return false;
        }
        match self {
            Self::Capabilities(a) => {
                optional(&a.agent, |s| bounded(s, 128)) && optional(&a.cwd, |s| bounded(s, MAX_CWD_BYTES))
            }
            Self::Spawn(a) => {
                bounded(&a.cwd, MAX_CWD_BYTES)
                    && bounded(&a.brief, MAX_TEXT_BYTES)
                    && optional(&a.name, |s| bounded(s, 256))
                    && optional(&a.agent, |s| bounded(s, 128))
                    && optional(&a.config.model, |s| bounded(s, MAX_CONFIG_ID_BYTES))
                    && optional(&a.config.mode, |s| bounded(s, MAX_CONFIG_ID_BYTES))
                    && a.config.axes.len() <= MAX_AXES
                    && a.config.axes.iter().all(|(key, value)| {
                        bounded(key, MAX_CONFIG_ID_BYTES)
                            && match value {
                                ConfigValue::Bool(_) => true,
                                ConfigValue::Id(id) => bounded(id, MAX_CONFIG_ID_BYTES),
                            }
                    })
            }
            Self::Send(a) => opaque(&a.child_id) && bounded(&a.text, MAX_TEXT_BYTES),
            Self::Answer(a) => {
                opaque(&a.child_id)
                    && opaque(&a.pending_id)
                    && a.content.as_ref().is_none_or(|v| v.to_string().len() <= MAX_TEXT_BYTES)
                    && match a.action {
                        ElicitationAction::Accept => a.content.as_ref().is_some_and(Value::is_object),
                        ElicitationAction::Decline | ElicitationAction::Cancel => a.content.is_none(),
                    }
            }
            Self::Status(a) => opaque(&a.child_id),
            Self::List(a) => {
                optional(&a.cursor, opaque) && a.limit.is_none_or(|limit| (1..=MAX_LIST_LIMIT).contains(&limit))
            }
            Self::Close(a) => opaque(&a.child_id),
        }
    }
    /// Canonical typed request, including the tool name. Optional absent/null
    /// fields and sorted nested keys have identical semantics and digests.
    pub fn canonical_request(&self) -> Value {
        fn value<T: Serialize>(args: &T) -> Value {
            serde_json::to_value(args).expect("typed tool args serialize")
        }
        let arguments = match self {
            Self::Capabilities(a) => value(a),
            Self::Spawn(a) => value(a),
            Self::Send(a) => value(a),
            Self::Answer(a) => value(a),
            Self::Status(a) => value(a),
            Self::List(a) => value(a),
            Self::Close(a) => value(a),
        };
        hennery_proto::codegen::sort_keys(json!({"name":self.name(),"arguments":arguments}))
    }
    /// Persist alongside the operation receipt before performing side effects.
    /// The caller scope lives in storage's unique key, never in these arguments.
    pub fn request_digest(&self) -> String {
        hex::encode(Sha256::digest(self.canonical_request().to_string().as_bytes()))
    }
}

/// MCP discovery uses Rust-generated argument schemas. Returned schemas are
/// self-contained; no manually maintained JSON or TypeScript argument contract.
pub fn discovery() -> Value {
    const RETRY: &str = " Mutations use a durable operation_key scoped to this caller session across tool names, including token rotation. Retry the same canonical typed request with the same key to retrieve its receipt; changed requests return operation_conflict. Delivery is not completion.";
    let tools: Vec<Value> = argument_schemas().into_iter().map(|(name, schema)| {
        let summary = match name {
            "session_capabilities" => "Discover the caller host's runtimes and configuration catalogs for a same-hat cwd. With Delegation on, every listed model, mode and option is allowed. Unknown or stale catalogs are reported honestly.",
            "session_spawn" => "Spawn one direct child in a selected runtime. With Delegation on, you may pick any model, mode and option in the child runtime's catalog (session_capabilities), including modes that don't ask for permission (Claude bypassPermissions, Codex agent-full-access). If mode is omitted, inherit the parent's mode for the same runtime, or map installed Claude bypassPermissions to Codex agent-full-access (and vice versa) across runtimes when offered; otherwise use the runtime default. No provider or model substitute is used. The brief waits for confirmed configuration. Permission requests raised by a child go to the operator; the parent cannot answer them. Children cannot spawn grandchildren.",
            "session_send" => "Queue bounded follow-up text for a direct child. A genuinely parked child may resume after fresh configuration evidence; a closed child never reopens.",
            "session_answer" => "Answer ordinary elicitation only using its pending_id. For action accept, content must be a JSON object matching the child's requestedSchema, embedded in the question delivery and session_status. Example: requestedSchema={\"type\":\"object\",\"properties\":{\"question_0\":{\"type\":\"string\",\"enum\":[\"blue\",\"green\"]}},\"required\":[\"question_0\"]} requires content={\"question_0\":\"blue\"}. Use the actual schema's property names and values, not this example's. Omit content for decline/cancel. Invalid content returns invalid_arguments as a tool error; correct the content and retry. Permission requests raised by a child go to the operator; the parent cannot answer them (operator_required). Escalate missing facts and product decisions to the operator.",
            "session_status" => "Read one owned direct child's lifecycle, configuration, questions, reports and operation receipts. Permission requests raised by a child go to the operator; the parent cannot answer them and receives no answerable schema for them.",
            "session_list" => "Page only the caller's direct children with shared work observations, bounded last-response excerpts, waiting and queue counts, and advisory plans. Missing background telemetry does not establish completion. Follow next_cursor; no owner-wide or recursive enumeration.",
            "session_close" => "Close one direct child and cancel its unstarted follow-ups. Does not delete files, cascade or delete sessions.",
            _ => unreachable!("generated tool names are fixed"),
        };
        let mutation = matches!(name, "session_spawn" | "session_send" | "session_answer" | "session_close");
        let description = if mutation { format!("{summary}{RETRY}") } else { summary.to_owned() };
        json!({"name":name,"description":description,"inputSchema":schema})
    }).collect();
    json!({"tools":tools})
}
