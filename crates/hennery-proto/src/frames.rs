use crate::agents::{AgentList, MaybeRuntime};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// A session that a host still has an adapter for, reported in `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AttachedSession {
    pub session_id: String,
    #[ts(type = "number")]
    pub last_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_turn_id: Option<String>,
}

/// A feature a host implements, announced in `hello` (ACP core §3.3). The
/// collector never sends a frame that needs a capability to a host that
/// lacks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Project enumeration and browsing.
    Projects,
    /// Image content blocks in prompts.
    Images,
    /// Explicit park (`park_session`).
    Park,
    /// Resolving typed paths (`resolve_path`, kernel spec §5.4).
    ResolvePath,
    /// Removing a deleted session's transcript from the agent's own data
    /// (`forget_session`, plan 9d decision 4).
    ForgetSession,
    /// A session's MCP servers (plan 8c): the host passes a start's or
    /// resume's `mcp_servers` into `session/new` / `session/load` with the
    /// agent's isolation, as `hello.mcp_isolation` reports it, and refuses
    /// servers it cannot isolate unless the collector waived that
    /// (`mcp_isolation_unavailable`). A host without it gets no servers.
    McpServers,
    /// Checking its agents live (`probe_agents`, plan 4d-B1-i); such a host
    /// reports its agents in `hello` too.
    ProbeAgents,
}

/// The longest agent data root a host may report (plan 9d, O12's shape
/// check): Linux's `PATH_MAX`.
pub const AGENT_HOME_MAX_BYTES: usize = 4096;

/// Where an agent keeps its own data for a session, as its host resolved
/// it from the adapter's environment (plan 9d decision 1): `root` is
/// Claude's `CLAUDE_CONFIG_DIR` or `~/.claude`, Codex's `CODEX_HOME` or
/// `~/.codex`; `sqlite_root` is Codex's `CODEX_SQLITE_HOME` if set.
/// Canonical, its bytes as the filesystem gave them (O11). A host's report
/// is not verified by the collector beyond its shape (`is_well_formed`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentHome {
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub sqlite_root: Option<String>,
}

impl AgentHome {
    /// Absolute, bounded and with no NUL, each path (plan 9d, the shape
    /// check the collector makes before it stores a home).
    pub fn is_well_formed(&self) -> bool {
        let ok = |p: &str| p.starts_with('/') && p.len() <= AGENT_HOME_MAX_BYTES && !p.contains('\0');
        ok(&self.root) && self.sqlite_root.as_deref().is_none_or(ok)
    }
}

/// What a forget names on the host (plan 9d decision 4, B2): a kind of
/// entry, never a path. The masked path each stands for is `masked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ForgetKind {
    /// The whole forget, when it could not start (no home, an unknown id,
    /// an agent this host cannot forget for yet).
    Session,
    /// The transcript and its family in each project directory (B9); for
    /// Codex, its rollout files in `sessions/` and `archived_sessions/`
    /// (plan 9d-ii).
    Transcript,
    FileHistory,
    SessionEnv,
    Tasks,
    Debug,
    /// Codex's own database copies of the conversation (plan 9d-ii).
    CodexDatabaseCopies,
}

impl ForgetKind {
    /// The entries this kind stands for, relative to the agent's root, with
    /// the project directory masked (B2).
    pub fn masked(self) -> &'static str {
        match self {
            Self::Session => "<session>",
            Self::Transcript => "projects/*/<id>.jsonl (and its family), or sessions/**/rollout-*-<id>.jsonl",
            Self::FileHistory => "file-history/<id>/",
            Self::SessionEnv => "session-env/<id>/",
            Self::Tasks => "tasks/<id>/",
            Self::Debug => "debug/<id>.txt",
            Self::CodexDatabaseCopies => "<codex database>",
        }
    }
}

/// Why something named was not removed: a fixed code the host chooses
/// (plan 9d B2), never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ForgetReason {
    /// A live actor on the host has that agent session id (decision 13, B7).
    Attached,
    /// Another forget of the same agent session is running on the host
    /// (B7; the review's item 3).
    InProgress,
    /// This host cannot forget for that agent (yet).
    UnsupportedAgent,
    /// The session recorded no agent home (decision 11).
    NoRecordedHome,
    /// The host's registry has no such (agent, id, home) (B1).
    UnknownToHost,
    /// Another kept session refers to the same agent session (B8).
    Shared,
    /// The root failed a check: `/`, `$HOME` or an ancestor, the host's data
    /// directory, not canonical, not the host user's, writable by others.
    UnsafeRoot,
    /// The root is not there (any more).
    RootMissing,
    /// A symlink, reported and never followed or removed (decision 8).
    Symlink,
    /// A kind directory that is not a real directory.
    NotADirectory,
    /// A kind directory not the host user's, or writable by others (B3).
    UnsafeDirectory,
    /// The walk reached another file system (R2).
    MountPoint,
    /// The walk reached its depth bound (R2).
    TooDeep,
    /// The forget's deadline passed before the removal was done (B6). For
    /// Codex: after `thread/delete` was written, so it may have run.
    TimedOut,
    /// Codex's app-server did not answer in time before `thread/delete`
    /// was written (`--version`, its start, `initialize`): retried, and
    /// counted by the collector, which flags the record's next forget
    /// `fallback` after `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` in a row
    /// (plan 9d-ii, B5 as ruled).
    AppServerTimedOut,
    /// Still there after the removal (B4).
    StillPresent,
    /// The removal failed midway (B3).
    IoError,
    /// Codex's `thread/delete` refused: forked history in another thread
    /// still references the rollout (plan 9d-ii, decision 9). Final, and
    /// never followed by the fallback (B5).
    ForkedHistory,
    /// Codex's `thread/delete` refused: the thread was never persisted
    /// (plan 9d-ii, decision 9). Final.
    Ephemeral,
    /// The app-server named another `CODEX_HOME` than the session's
    /// recorded one in its `initialize` answer: nothing was asked of it, and
    /// no fallback ran (plan 9d-ii, the parent's rule).
    HomeMismatch,
    /// Only the fallback ran (plan 9d-ii, decision 10): Codex's own database
    /// may still hold copies of the conversation. Final.
    FallbackOnly,
    /// The collector's own: the host answered `error{invalid}` for the id
    /// (decision 8, O10).
    InvalidId,
    /// The collector's own: the host was revoked and never connects again
    /// (O10).
    HostRevoked,
}

/// One kind of entry and how many of them (plan 9d B2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ForgetWhat {
    pub kind: ForgetKind,
    pub count: u32,
}

/// Something a forget left (plan 9d decision 4). `retry: false` marks what
/// a retry cannot change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ForgetRemaining {
    pub what: ForgetWhat,
    pub reason: ForgetReason,
    pub retry: bool,
}

/// How a forget ended (plan 9d decision 4): `complete` when the check
/// afterwards found nothing named left (B4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ForgetOutcome {
    Complete,
    Partial,
}

/// `hello.capabilities`. Deserialized leniently: a capability this build
/// does not know (a newer host, a minor protocol bump) is skipped, never a
/// reason to refuse the whole `hello`. The generated schema still lists
/// `Capability` as a closed `oneOf` (there is no open-ended JSON Schema
/// equivalent), but that is a description of the known values, not a
/// constraint hennery itself enforces: an entry outside it is ignored, not
/// rejected, so a schema-validating client or proxy must not reject a
/// `hello` on an unknown capability either.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema, TS)]
pub struct Capabilities(pub Vec<Capability>);

impl Capabilities {
    pub fn has(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }
}

impl<'de> Deserialize<'de> for Capabilities {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: Vec<Value> = Deserialize::deserialize(deserializer)?;
        Ok(Self(
            raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        ))
    }
}

/// A name and a value: an HTTP header of an MCP server, or an environment
/// variable of a stdio one. The value can be a secret (a gateway session
/// token, a stdio server's key), so `Debug` never shows it (ACP core §8).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NameValue {
    pub name: String,
    pub value: String,
}

impl NameValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl std::fmt::Debug for NameValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NameValue")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// One MCP server for a session (gateway spec §3.2, §3.4): an entry of ACP
/// `session/new` / `session/load` `mcpServers`, tagged by `type` here (ACP
/// itself leaves stdio entries untagged; the host builds those). A new
/// server type or a new required field needs a new capability: a host that
/// cannot decode a start drops it unanswered, and the collector's timeout
/// then drops the connection.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServer {
    /// A streamable-HTTP server: the gateway's `/mcp/<slug>`, with the
    /// session's token in `Authorization`.
    Http {
        name: String,
        url: String,
        #[serde(default)]
        headers: Vec<NameValue>,
    },
    /// A local server the agent runs as its own child.
    Stdio {
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<NameValue>,
    },
}

impl McpServer {
    /// Every part that may be a secret, as `Debug` hides them: the
    /// headers' and the env's values, an HTTP server's URL past its origin
    /// and its userinfo (`url_secrets`), a stdio server's arguments.
    pub fn secret_values(&self) -> Vec<&str> {
        match self {
            Self::Http { url, headers, .. } => headers
                .iter()
                .map(|pair| pair.value.as_str())
                .chain(url_secrets(url))
                .collect(),
            Self::Stdio { args, env, .. } => env
                .iter()
                .map(|pair| pair.value.as_str())
                .chain(args.iter().map(String::as_str))
                .collect(),
        }
    }
}

/// `url` cut at its authority: `(scheme, userinfo, host[:port], rest)`, or
/// `None` when it does not read one way only. Fails closed: a scheme other
/// than `http` or `https` (an MCP server's only ones), a backslash, an `@` past the authority (`u:ab/cd@h`, a
/// userinfo with a `/` in it), or a host or port of other characters than
/// theirs, and the whole URL counts as a secret. Not a URL parser.
fn url_parts(url: &str) -> Option<(&str, Option<&str>, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) || url.contains('\\') {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(end);
    if after.contains('@') {
        return None;
    }
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let port = if let Some(v6) = host_port.strip_prefix('[') {
        // An IPv6 literal, `[…]`, with an optional port.
        let (inside, tail) = v6.split_once(']')?;
        if inside.is_empty() || !inside.chars().all(|c| c.is_ascii_hexdigit() || ":.".contains(c)) {
            return None;
        }
        match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':')?),
        }
    } else {
        let (host, port) = match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        };
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
            return None;
        }
        port
    };
    if port.is_some_and(|port| port.is_empty() || !port.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some((scheme, userinfo, host_port, after))
}

/// `url` as `scheme://host[:port]`: the one form of an upstream URL any
/// log, error body or `Debug` shows (the gateway lane's rule L11). Its
/// path, query, fragment and userinfo can carry a secret. A URL that does
/// not read one way only (`url_parts`) shows as `<redacted>`.
pub fn url_origin(url: &str) -> String {
    match url_parts(url) {
        Some((scheme, _, host_port, _)) => format!("{scheme}://{host_port}"),
        None => "<redacted>".into(),
    }
}

/// What `url_origin` leaves out: the userinfo, and everything after the
/// authority (path, query, fragment). Empty parts are left out; all of the
/// URL when it does not read one way only.
pub fn url_secrets(url: &str) -> Vec<&str> {
    let Some((_, userinfo, _, after)) = url_parts(url) else {
        return vec![url];
    };
    userinfo
        .into_iter()
        .chain(Some(after))
        .filter(|part| !part.is_empty())
        .collect()
}

/// Shows names, the URL's origin (`url_origin`) and the command; never a
/// header's or an env variable's value, the URL's path, nor a stdio
/// server's arguments (which may carry a key).
impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http { name, url, headers } => f
                .debug_struct("Http")
                .field("name", name)
                .field("url", &url_origin(url))
                .field("headers", headers)
                .finish(),
            Self::Stdio {
                name,
                command,
                args,
                env,
            } => f
                .debug_struct("Stdio")
                .field("name", name)
                .field("command", command)
                .field("args", &format_args!("<{} redacted>", args.len()))
                .field("env", env)
                .finish(),
        }
    }
}

/// The MCP part of a `start_session` / `resume_session` (ACP core §3.3,
/// §4.3; plan 8c). Flattened into the frame; empty fields are left out, so
/// a frame without servers is what an older host expects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpDelivery {
    /// Passed in `session/new` / `session/load`. Only to a host that
    /// announced `mcp_servers`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// The collector knowingly delivers to an agent the host cannot isolate
    /// (the mixed-host fallback's default hat, or a single-hat host,
    /// umbrella §8.5). Absent, the host refuses servers for such an agent
    /// (`mcp_isolation_unavailable`): isolation is never lost by omission.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolation_waived: bool,
}

/// How a host keeps an agent's sessions to the servers hennery passes (ACP
/// core §6). Lenient: a mechanism this build does not know reads as `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpIsolation {
    /// `--strict-mcp-config` through `_meta` on every `session/new` and
    /// `session/load` (the pinned Claude adapter, its own CLI).
    ClaudeStrict,
    /// None: the agent also loads the user's own MCP configuration.
    None,
}

/// `hello.mcp_isolation`: per agent id, how the host isolates its MCP
/// servers. An agent left out is not isolated. Deserialized leniently, like
/// `Capabilities`: an unknown mechanism counts as `none`, never as isolated
/// and never a reason to refuse the `hello`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema, TS)]
pub struct AgentIsolation(pub BTreeMap<String, McpIsolation>);

impl AgentIsolation {
    pub fn get(&self, agent: &str) -> McpIsolation {
        self.0.get(agent).copied().unwrap_or(McpIsolation::None)
    }

    /// The host keeps `agent`'s sessions to the servers it is given.
    pub fn isolates(&self, agent: &str) -> bool {
        self.get(agent) != McpIsolation::None
    }
}

impl<'de> Deserialize<'de> for AgentIsolation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Not a map at all reads as empty too: never a reason to refuse
        // the `hello`.
        let Value::Object(raw) = Value::deserialize(deserializer)? else {
            return Ok(Self::default());
        };
        Ok(Self(
            raw.into_iter()
                .map(|(agent, v)| (agent, serde_json::from_value(v).unwrap_or(McpIsolation::None)))
                .collect(),
        ))
    }
}

/// How a turn ended. Exactly one `turn_ended` per accepted turn (ACP core §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

/// Why a session was parked (ACP core §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    Idle,
    AdapterExited,
    Operator,
}

/// What a pending request asks the operator (ACP core §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    /// `session/request_permission`: pick one of the offered options.
    Permission,
    /// `elicitation/create`: fill in a form, or decline.
    Elicitation,
}

/// How a pending request ended (ACP core §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingResolution {
    /// The operator's answer reached the waiting adapter.
    Delivered,
    /// The adapter was told the question is off (see `PendingReason`).
    Cancelled,
}

/// Why a pending request was cancelled (ACP core §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    TurnCancelled,
    SessionClosed,
    SessionParked,
    AdapterLost,
    HostRestarted,
    /// The adapter withdrew its own question (`$/cancel_request`).
    AgentWithdrew,
    /// The operator revoked the session's host (kernel spec §4.3): it will
    /// never connect again to deliver an answer.
    HostRevoked,
}

/// The operator's answer to an elicitation (ACP `elicitation/create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

/// The `pending` extract (ACP core §3.2): what the collector needs to hold
/// a pending request and validate its answer, without reading the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingExtract {
    pub id: String,
    pub kind: PendingKind,
    /// The permission's option ids. Absent for an elicitation, and for a
    /// permission request whose options hennery could not parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_ids: Option<Vec<String>>,
    /// What the question is about, as the agent put it: a permission's tool
    /// call title, an elicitation's message (plan 10b). A push shows it
    /// only under a hat with `details` (kernel spec §6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// The value of one config option (ACP `session/set_config_option`): a
/// select's value id, or a boolean toggle's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum ConfigValue {
    Bool(bool),
    Id(String),
}

/// Model, mode and the other config axes of a session (ACP core §3.3,
/// §4.3): what a start asks for, and what a resume re-applies. `model` and
/// `mode` are the values of the adapter's model and mode options; `axes`
/// holds every other option, by config id. Flattened into the frames and
/// the start request, so the wire stays `model?, mode?, axes{}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, ConfigValue>,
}

impl SessionConfig {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.mode.is_none() && self.axes.is_empty()
    }
}

/// Fields the collector may read from a session event. Closed set (ACP core §3.2).
///
/// The catalogue extracts (`config_options`, `current_model`,
/// `current_mode`, `current_axes`) travel together: when `config_options`
/// is present and not empty, the four are one snapshot of the adapter's
/// config. An absent or empty `config_options` means "no read-back", never
/// "the adapter has no config" (a response that failed to parse looks
/// empty).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Indexed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// On a `session_info_update` that names a title: the title the agent
    /// reported, as it sent it; empty when it cleared it (ACP `null`). The
    /// collector normalises and caps it for the session list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// On an `available_commands_update`: the adapter's slash commands, the
    /// full list (ACP `AvailableCommand` objects, those hennery can parse).
    /// An empty list means the adapter has none, unlike an empty
    /// `config_options`. Never part of the catalogue snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown[] | undefined", optional)]
    pub commands: Option<Vec<Value>>,
    /// On an update the adapter sent before the session was announced:
    /// replayed by `session/load`, or sent while the start ran (ACP core
    /// §4.5). What it says may be older than what the collector holds, so
    /// its title only fills an empty one (plan 6b decision 2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub early: bool,
    /// The full config catalogue: the adapter's ACP `SessionConfigOption`
    /// objects, for the UI. The collector stores it and never reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown[] | undefined", optional)]
    pub config_options: Option<Vec<Value>>,
    /// The current value of the model option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_model: Option<String>,
    /// The current value of the mode option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<String>,
    /// The current value of every other option, by config id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_axes: Option<BTreeMap<String, ConfigValue>>,
    /// On `pending_opened`: the request's id, kind, option ids and title.
    // Boxed: it is on one fact in many, and inline it made every host frame
    // larger (plan 10b-i decision 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Box<PendingExtract>>,
}

impl Indexed {
    /// The config this event reports as current, if it carries a catalogue
    /// snapshot (see the type's doc).
    pub fn current_config(&self) -> Option<SessionConfig> {
        let options = self.config_options.as_ref()?;
        if options.is_empty() {
            return None;
        }
        Some(SessionConfig {
            model: self.current_model.clone(),
            mode: self.current_mode.clone(),
            axes: self.current_axes.clone().unwrap_or_default(),
        })
    }
}

/// The body of a sequenced, outboxed session frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionBody {
    /// The adapter session exists. Resolves the collector's start waiter.
    /// `indexed` carries the catalogue after the start's config switches
    /// (ACP core §4.3, P-13).
    SessionStarted {
        request_id: String,
        agent_session_id: String,
        #[serde(default)]
        indexed: Indexed,
        /// Where the agent keeps its data for this session (plan 9d
        /// decision 1), registered on the host before this was sent (B1).
        /// Absent from an older host, for an agent hennery cannot forget
        /// for, or when the host could not resolve or register it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "AgentHome | undefined", optional)]
        agent_home: Option<AgentHome>,
    },
    /// The host accepted a start but could not create the adapter session
    /// (spawn, `initialize` or `session/new` failed). Rejects the start waiter.
    StartFailed {
        request_id: String,
        code: String,
        message: String,
    },
    /// The prompt reached the adapter. Resolves the collector's prompt waiter.
    TurnStarted { request_id: String, turn_id: String },
    /// An ACP message from the adapter, verbatim in `payload`.
    AcpUpdate {
        #[serde(default)]
        indexed: Indexed,
        #[ts(type = "unknown")]
        payload: Value,
    },
    TurnEnded {
        turn_id: String,
        outcome: TurnOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// The adapter process is gone and the session is detached (idle reap,
    /// adapter exit, or an operator park). Completes `park_session`.
    SessionParked { reason: ParkReason },
    /// The operator closed an attached session. Completes `close_session`.
    SessionClosed,
    /// The adapter exited without being asked to (ACP core §2.3). Followed by
    /// `session_parked{adapter_exited}`. The stderr tail is scrubbed.
    AdapterExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        stderr_tail: String,
    },
    /// hennery's own diagnostic that is not state (ACP core §3.2): a failed
    /// re-apply, or update kinds dropped during `session/load` (§4.5).
    /// `note` is a machine code; `text` is scrubbed.
    HostNote { note: String, text: String },
    /// A `set_config` took effect: the catalogue the adapter answered with,
    /// the authoritative read-back (ACP core §3.2). Resolves the collector's
    /// config waiter.
    ConfigApplied {
        request_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
    /// The adapter asked the operator something (ACP core §4.6): its ACP
    /// request verbatim in `payload`, with `indexed.pending`. It waits, with
    /// no timeout, until it is answered or cancelled. Its kind is
    /// `indexed.pending.kind` (the body's own `kind` is its tag).
    PendingOpened {
        pending_id: String,
        #[serde(default)]
        indexed: Indexed,
        #[ts(type = "unknown")]
        payload: Value,
    },
    /// A pending request is over: answered (`delivered`), or cancelled for
    /// `reason`.
    PendingResolved {
        pending_id: String,
        resolution: PendingResolution,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<PendingReason>,
    },
    /// The host's verdict on one answer (umbrella §6.8): `delivered` if the
    /// adapter was still waiting for it. A delivered answer is followed by
    /// `pending_resolved{delivered}`.
    AnswerResult {
        pending_id: String,
        request_id: String,
        delivered: bool,
    },
    /// The git state of the session's cwd (ACP core §3.2, §7): after the
    /// start and after each turn, when `cwd` is in a work tree and `git`
    /// answered within 3 s (plan 6b-ii decision 11). Never in place of, or
    /// ahead of, the `turn_ended` it follows.
    GitState {
        /// The branch checked out; absent when detached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "string | undefined", optional)]
        branch: Option<String>,
        /// Any staged, unstaged or untracked change.
        dirty: bool,
        /// `cwd` is in a linked work tree (`git worktree add`), not the
        /// repository's main one.
        worktree: bool,
        /// The commit checked out; absent on a branch with no commit yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "string | undefined", optional)]
        head: Option<String>,
        /// On the first state after a new session's start: the commit it
        /// started from. The collector records it once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "string | undefined", optional)]
        base_commit: Option<String>,
    },
}

impl SessionBody {
    /// A `session_started` with nothing but its ids, for tests and for
    /// callers that have no catalogue to announce.
    pub fn session_started(request_id: impl Into<String>, agent_session_id: impl Into<String>) -> Self {
        Self::SessionStarted {
            request_id: request_id.into(),
            agent_session_id: agent_session_id.into(),
            indexed: Indexed::default(),
            agent_home: None,
        }
    }
}

/// One git repository `list_projects` found under a workspace root (ACP
/// core §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Project {
    /// Absolute and canonical (symlinks resolved), as the host sees it.
    pub path: String,
}

/// One subdirectory in a `browse_directory` answer (ACP core §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DirEntry {
    /// The entry's file name, not a path.
    pub name: String,
    /// It holds `.git`: a repository, or a worktree of one.
    pub git: bool,
}

/// Host -> collector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFrame {
    Hello {
        protocol_version: String,
        host_version: String,
        host_id: String,
        /// Proof of possession of the host's key (ACP core §3.5): its
        /// Ed25519 signature, hex, over `hello_proof_message(nonce, host_id,
        /// protocol_version)`, where the nonce is the one the collector sent
        /// in the upgrade response's `hennery-hello-nonce` header.
        proof: String,
        /// What this host implements (ACP core §3.3). `Capability` lists
        /// the values this version knows; an unknown one is ignored, not
        /// rejected (see `Capabilities`). Absent means none.
        #[serde(default)]
        capabilities: Capabilities,
        /// Per agent, how this host isolates its MCP servers (plan 8c).
        /// Absent means none is isolated (an older host).
        #[serde(default)]
        mcp_isolation: AgentIsolation,
        /// The workspace roots from the host's config (ACP core §7), as
        /// configured. Absent means none (an older host).
        #[serde(default)]
        workspace_roots: Vec<String>,
        attached_sessions: Vec<AttachedSession>,
        /// Its agents, as configured (plan 4d-B1-i): `available` says each
        /// can be launched, `auth` is `unknown`, `images` absent. Read
        /// leniently: an entry this build cannot read is skipped. Absent
        /// from an older host.
        #[serde(default)]
        #[ts(as = "Vec<crate::agents::AgentInfo>")]
        agents: AgentList,
        /// Where its agents come from. Absent from an older host, or one
        /// this build cannot read.
        #[serde(default, skip_serializing_if = "MaybeRuntime::is_none")]
        #[ts(type = "RuntimeInfo | undefined", optional)]
        runtime: MaybeRuntime,
    },
    /// Every state-bearing fact is a sequenced frame: it goes through the host
    /// outbox and is acked (ACP core §3.3).
    Session {
        session_id: String,
        #[ts(type = "number")]
        seq: u64,
        body: SessionBody,
    },
    /// A rejected request. Not outboxed: a rejection means nothing happened,
    /// so losing it only costs the collector a timeout.
    Error {
        request_id: String,
        code: String,
        message: String,
    },
    /// The answer to `resolve_path` (kernel spec §5.4). Not outboxed: a
    /// probe changes nothing, so a lost answer costs only a retry.
    /// `canonical` is absolute, symlinks resolved, with no `.`, `..` or
    /// trailing slash; for a path that does not exist, its deepest existing
    /// ancestor is resolved and the rest normalised by its text.
    ResolvedPath {
        request_id: String,
        canonical: String,
        exists: bool,
        is_dir: bool,
    },
    /// Sent once per connection after the unacked outbox has been resent.
    /// The collector reconciles `hello.attached_sessions` only after this
    /// frame, so a resent `turn_ended` is never duplicated by a synthesised
    /// one (ACP core §5.2).
    ResendComplete,
    /// The answer to `list_projects` (ACP core §3.3, §7): the git
    /// repositories under the workspace roots. A probe reply: not outboxed,
    /// it answers only the connection it was asked on.
    Projects {
        request_id: String,
        items: Vec<Project>,
        /// A bound cut the enumeration short: there may be more.
        partial: bool,
        /// The host user's home directory, canonical, so the picker can
        /// expand `~` (frontend §7). Absent if the host has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<String>,
    },
    /// The answer to `forget_session` (plan 9d decision 4): what was
    /// removed and what is left, as kinds and counts (B2). Like a probe's
    /// reply it is not outboxed and answers only the connection it was
    /// asked on: a forget is idempotent, so a lost answer costs a retry.
    SessionForgotten {
        request_id: String,
        outcome: ForgetOutcome,
        removed: Vec<ForgetWhat>,
        remaining: Vec<ForgetRemaining>,
    },
    /// The answer to `probe_agents` (plan 4d-B1-i): each agent started as
    /// the host starts it and asked `initialize`, and its CLI asked whether
    /// it is logged in. A probe reply, like `projects`; read leniently, like
    /// `hello`'s agents.
    Agents {
        request_id: String,
        #[serde(default)]
        #[ts(as = "Vec<crate::agents::AgentInfo>")]
        agents: AgentList,
        #[serde(default, skip_serializing_if = "MaybeRuntime::is_none")]
        #[ts(type = "RuntimeInfo | undefined", optional)]
        runtime: MaybeRuntime,
    },
    /// The answer to `browse_directory` (ACP core §3.3, §7): the
    /// subdirectories of `path`. A probe reply, like `projects`.
    Directory {
        request_id: String,
        /// The browsed directory, canonical.
        path: String,
        /// Its parent, canonical, if browsing it is allowed too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        entries: Vec<DirEntry>,
        /// Not every subdirectory is listed.
        truncated: bool,
    },
}

/// A collector frame that is not a probe (`CollectorFrame::probe_capability`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotAProbe;

impl CollectorFrame {
    /// If this frame is a probe (ACP core §3.3): the capability its host
    /// must have announced, if any. Exhaustive on purpose, like
    /// `HostFrame::probe_request_id`: a new collector frame must say whether
    /// it is a probe and what it needs, or this does not compile (umbrella
    /// §5.4).
    pub fn probe_capability(&self) -> Result<Option<Capability>, NotAProbe> {
        match self {
            Self::ListProjects { .. } | Self::BrowseDirectory { .. } => Ok(Some(Capability::Projects)),
            Self::ResolvePath { .. } => Ok(Some(Capability::ResolvePath)),
            // Not a probe of state, but carried as one (`session_forgotten`).
            Self::ForgetSession { .. } => Ok(Some(Capability::ForgetSession)),
            Self::ProbeAgents { .. } => Ok(Some(Capability::ProbeAgents)),
            Self::HelloAck { .. }
            | Self::HelloError { .. }
            | Self::StartSession { .. }
            | Self::ResumeSession { .. }
            | Self::Prompt { .. }
            | Self::CancelTurn { .. }
            | Self::SetConfig { .. }
            | Self::AnswerPermission { .. }
            | Self::AnswerElicitation { .. }
            | Self::Ack { .. }
            | Self::ParkSession { .. }
            | Self::CloseSession { .. }
            | Self::ForgetHat { .. } => Err(NotAProbe),
        }
    }

    /// A start's or resume's MCP part; `None` for every other frame. The
    /// hub's guard reads it (plan 8c), so it is exhaustive on purpose: a
    /// new frame must say whether it carries servers, or this does not
    /// compile.
    pub fn mcp_delivery(&self) -> Option<&McpDelivery> {
        match self {
            Self::StartSession { mcp, .. } | Self::ResumeSession { mcp, .. } => Some(mcp),
            Self::HelloAck { .. }
            | Self::HelloError { .. }
            | Self::Prompt { .. }
            | Self::CancelTurn { .. }
            | Self::SetConfig { .. }
            | Self::AnswerPermission { .. }
            | Self::AnswerElicitation { .. }
            | Self::Ack { .. }
            | Self::ResolvePath { .. }
            | Self::ParkSession { .. }
            | Self::CloseSession { .. }
            | Self::ListProjects { .. }
            | Self::BrowseDirectory { .. }
            | Self::ForgetHat { .. }
            | Self::ForgetSession { .. }
            | Self::ProbeAgents { .. } => None,
        }
    }

    /// The agent a start or resume names.
    pub fn agent(&self) -> Option<&str> {
        match self {
            Self::StartSession { agent, .. } | Self::ResumeSession { agent, .. } => Some(agent),
            _ => None,
        }
    }
}

impl HostFrame {
    /// The probe this frame answers, if it is a probe reply (ACP core §3.3).
    /// Exhaustive on purpose: a new host frame must say whether it is one,
    /// or this does not compile (umbrella §5.4).
    pub fn probe_request_id(&self) -> Option<&str> {
        match self {
            Self::Projects { request_id, .. }
            | Self::Directory { request_id, .. }
            | Self::ResolvedPath { request_id, .. }
            | Self::SessionForgotten { request_id, .. }
            | Self::Agents { request_id, .. } => Some(request_id),
            Self::Hello { .. } | Self::Session { .. } | Self::Error { .. } | Self::ResendComplete => None,
        }
    }
}

/// Collector -> host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectorFrame {
    HelloAck {
        protocol_version: String,
        collector_version: String,
        /// Highest committed seq per session listed in `hello`; the host
        /// fast-forwards its counters if they are lower (lost outbox).
        #[ts(type = "Record<string, number>")]
        committed: BTreeMap<String, u64>,
    },
    HelloError {
        code: String,
        message: String,
    },
    /// Start a new session: spawn the adapter, `session/new`, then apply the
    /// requested config (ACP core §3.3, §4.3). Completed by
    /// `session_started` | `start_failed`.
    StartSession {
        request_id: String,
        session_id: String,
        /// The collector's highest committed seq for this session; the host
        /// continues from the larger of this and its own counter (§5.1).
        #[ts(type = "number")]
        committed_seq: u64,
        agent: String,
        cwd: String,
        /// Applied after `session/new`: model, then the other axes, then
        /// mode (ACP core §4.3).
        #[serde(flatten)]
        config: SessionConfig,
        /// The session's hat (`sessions.hat_id`); empty for a session from
        /// before hats. Carried, not yet used by the host (plan 8c).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hat_id: String,
        #[serde(flatten)]
        mcp: McpDelivery,
    },
    /// Attach a parked, closed or failed session again: spawn the adapter and
    /// `session/load` it with replay suppression (ACP core §4.3, §4.5).
    /// Completed by `session_started` | `start_failed`, like a start.
    ResumeSession {
        request_id: String,
        session_id: String,
        /// Fast-forward the host's counter before the first frame (§5.1).
        #[ts(type = "number")]
        committed_seq: u64,
        agent: String,
        cwd: String,
        /// The adapter's own session id, from the stored `session_started`:
        /// the host keeps no copy across restarts.
        agent_session_id: String,
        /// The stored config, re-applied after `session/load` (ACP core
        /// §4.3). A switch that fails is a `host_note`, not a failed resume.
        #[serde(flatten)]
        config: SessionConfig,
        /// As on `start_session`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hat_id: String,
        /// Sent again on every resume: an agent keeps no servers across
        /// `session/load` (the spike).
        #[serde(flatten)]
        mcp: McpDelivery,
    },
    Prompt {
        request_id: String,
        session_id: String,
        turn_id: String,
        /// ACP ContentBlocks, built by the frontend.
        #[ts(type = "unknown[]")]
        content: Vec<Value>,
    },
    /// Stop the turn in flight: `session/cancel` to the adapter. Completed
    /// by that turn's `turn_ended` (ACP core §3.3, §4.4), whatever its
    /// outcome: a turn that finished first is not cancelled.
    CancelTurn {
        request_id: String,
        session_id: String,
        turn_id: String,
    },
    /// Switch one config option of an attached session (`session/set_config_option`).
    /// Completed by `config_applied` | `error` (ACP core §3.3).
    SetConfig {
        request_id: String,
        session_id: String,
        config_id: String,
        value: ConfigValue,
    },
    /// The operator's choice for a permission request. Completed by
    /// `answer_result` (ACP core §4.6); it has no collector waiter.
    AnswerPermission {
        request_id: String,
        session_id: String,
        pending_id: String,
        option_id: String,
    },
    /// The operator's answer to an elicitation; `content` only with
    /// `accept`. Completed by `answer_result`, like a permission's.
    AnswerElicitation {
        request_id: String,
        session_id: String,
        pending_id: String,
        action: ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        content: Option<Value>,
    },
    Ack {
        session_id: String,
        #[ts(type = "number")]
        ack_seq: u64,
    },
    /// Resolve a typed path on the host, where the filesystem is (kernel
    /// spec §5.2, §5.4): absolute, or `~` / `~/…` for the host user's home.
    /// Completed by `resolved_path` | `error{invalid}`.
    ResolvePath {
        request_id: String,
        path: String,
    },
    /// Completed by `session_parked{operator}` (ACP core §4.8).
    ParkSession {
        request_id: String,
        session_id: String,
    },
    /// Completed by `session_closed` (ACP core §4.8).
    CloseSession {
        request_id: String,
        session_id: String,
    },
    /// Enumerate the git repositories under the host's workspace roots
    /// (ACP core §7). Answered by `projects`; only to a host with the
    /// `projects` capability.
    ListProjects {
        request_id: String,
    },
    /// List the subdirectories of `path`, an absolute path inside the host's
    /// browse fence (ACP core §7). Answered by `directory` | `error`; only to
    /// a host with the `projects` capability.
    BrowseDirectory {
        request_id: String,
        path: String,
    },
    /// A hat the operator purged (kernel spec §5.5; plan 9c decisions 10
    /// and 12, A7): sent after every reconciled handshake, one per purged
    /// hat, for good. The host forgets what it keeps for the hat (its
    /// composed agent home, plan 8): only for an id that is `hat-<hex>`,
    /// and only once no adapter of that hat runs. No answer; a host that
    /// does not know the frame logs and ignores it, so no capability is
    /// needed.
    ForgetHat {
        hat_id: String,
    },
    /// Remove a deleted session's transcript from the agent's own data on
    /// the host (plan 9d decision 4): only to a host with the
    /// `forget_session` capability. The host acts only on an exact match
    /// of its own registry (B1). Answered by `session_forgotten` |
    /// `error{invalid}`.
    ForgetSession {
        request_id: String,
        agent: String,
        agent_session_id: String,
        agent_home: AgentHome,
        /// Plan 9d-ii, B5 as ruled (the hybrid): the record's last
        /// `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` answers were all
        /// `app_server_timed_out`, so a Codex host spawns no Codex and runs
        /// the fallback at once, after the same checks. Only ever a
        /// downgrade of this session's own removal; other agents ignore it.
        /// Absent when false.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fallback: bool,
    },
    /// Check the host's own agents live (plan 4d-B1-i): only to a host with
    /// the `probe_agents` capability. It names nothing: the host runs a
    /// fixed set of read-only checks on the agents it is configured with.
    /// Answered by `agents` | `error{busy}`.
    ProbeAgents {
        request_id: String,
    },
}
