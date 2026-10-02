use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::frames::{ConfigValue, ElicitationAction, PendingKind, PendingReason, SessionConfig};

/// `POST /api/sessions` (ACP core §9): `{host_id, agent, cwd, model?, mode?, axes?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StartSessionRequest {
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    #[serde(flatten)]
    pub config: SessionConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StartSessionResponse {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PromptRequest {
    #[ts(type = "unknown[]")]
    pub content: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PromptResponse {
    pub turn_id: String,
}

/// One block of a prompt as the collector stores it (ACP core §7, plan 6a):
/// in `turns.content` and in the `user_turn` event's `content`. Text as it
/// was sent; an image as the attachment it was stored as, never its bytes.
/// `GET /api/attachments/{sha256}` serves the image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StoredBlock {
    Text {
        text: String,
    },
    Image {
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// The SHA-256 of the image's bytes, 64 lowercase hex digits.
        sha256: String,
        /// The image's size in bytes, decoded.
        #[ts(type = "number")]
        size: u64,
    },
}

/// `GET /api/settings/attachments` (plan 6a): the owner's attachment store,
/// for Settings (ACP core §15, maintainer decision 6a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AttachmentUsage {
    /// Stored images, each counted once however often it was sent.
    #[ts(type = "number")]
    pub count: u64,
    /// Their size in bytes, decoded.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// One stored timeline event, as served by REST and SSE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EventDto {
    #[ts(type = "number")]
    pub event_id: i64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | undefined", optional)]
    pub host_seq: Option<u64>,
    pub kind: String,
    #[ts(type = "unknown")]
    pub body: Value,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    /// Set when the error leaves a session reachable by id (e.g. a session
    /// start whose delivery is unknown: the session was created and may
    /// still start, but the caller has no other way to learn its id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub session_id: Option<String>,
}

/// Result of a park or close: the session's lifecycle once the request took
/// effect (`parked` or `closed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LifecycleResponse {
    pub session_id: String,
    pub lifecycle: String,
}

/// A session's open turn, for the detail view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct OpenTurn {
    pub turn_id: String,
    /// `sent` (not yet acknowledged by the adapter) or `started`.
    pub state: String,
}

/// The session list's caps, in bytes as JSON writes each field (plan 6b
/// decision 9, the review's A1). With every field at its cap, a list item
/// stays under 1 KiB with room for a `hat_id` (P-23). The title and the
/// branch are stored within theirs; an agent past its cap is refused at
/// the start; a model, mode or failure reason past its cap is left out of
/// the item (the stored value is kept: a resume re-applies it); a cwd past
/// its cap is shown by its end.
pub const TITLE_MAX_CHARS: usize = 120;
pub const TITLE_MAX_JSON_BYTES: usize = 160;
pub const BRANCH_MAX_CHARS: usize = 120;
pub const BRANCH_MAX_JSON_BYTES: usize = 120;
pub const MODEL_MAX_JSON_BYTES: usize = 48;
pub const MODE_MAX_JSON_BYTES: usize = 48;
pub const FAILURE_REASON_MAX_JSON_BYTES: usize = 32;
pub const AGENT_MAX_JSON_BYTES: usize = 32;
pub const CWD_MAX_JSON_BYTES: usize = 128;
/// A paired host's id is `host-` and 16 hex digits (21 bytes); an older
/// row's, from before the start checked it, is cut to this.
pub const HOST_ID_MAX_JSON_BYTES: usize = 32;

/// Bidi controls and zero-width characters: never shown as they are, since
/// they could make a row read as something else (plan 6b, the review's A2).
pub fn is_hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// How many bytes JSON takes to write `c` inside a string, as serde_json
/// escapes it: `"`, `\` and the short escapes take two, other control
/// characters six (`\u00XX`).
pub fn json_char_width(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// How many bytes JSON takes to write `s` inside a string.
pub fn json_width(s: &str) -> usize {
    s.chars().map(json_char_width).sum()
}

/// The longest start of `s` with at most `max_chars` characters and
/// `max_json_bytes` bytes as JSON writes them; never cut inside a character.
fn cut(s: &str, max_chars: usize, max_json_bytes: usize) -> String {
    let mut bytes = 0;
    s.chars()
        .take(max_chars)
        .take_while(|c| {
            bytes += json_char_width(*c);
            bytes <= max_json_bytes
        })
        .collect()
}

/// `s` if it fits in `max_json_bytes` as JSON writes it, else its longest
/// end that fits after `…`; never cut inside a character.
fn tail(s: &str, max_json_bytes: usize) -> String {
    if json_width(s) <= max_json_bytes {
        return s.to_string();
    }
    let mut bytes = '…'.len_utf8();
    let kept: Vec<char> = s
        .chars()
        .rev()
        .take_while(|c| {
            bytes += json_char_width(*c);
            bytes <= max_json_bytes
        })
        .collect();
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// One session as the session list shows it (ACP core §8, §9; frontend
/// §5), read from `sessions` alone: never the catalogue, commands or plan
/// (P-23). The detail serves it as stored; the list, `bounded`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionItem {
    pub session_id: String,
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    /// The title the agent reported, on one line and capped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub title: Option<String>,
    pub lifecycle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub failure_reason: Option<String>,
    /// Parked only because its host has been offline past the threshold
    /// (ACP core §5.3); the host may still be running it.
    pub presumed_parked: bool,
    /// The branch checked out in `cwd`, as the host last reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub git_branch: Option<String>,
    /// Whether `cwd`'s work tree had changes, as the host last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub git_dirty: Option<bool>,
    /// The current model and mode, as the host last reported them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub mode: Option<String>,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// When its last listed event was written (RFC 3339, UTC, three
    /// fractional digits): the list's sort key, newest first.
    pub last_event_at: String,
}

impl SessionItem {
    /// The item as the session list serves it, every field within its cap
    /// (see `TITLE_MAX_CHARS`): with them, it stays under 1 KiB.
    pub fn bounded(mut self) -> Self {
        // Shown as they are in every row: one with a control or hidden
        // character is left out too (the second review's P1).
        let within = |value: Option<String>, max: usize| {
            value.filter(|v| json_width(v) <= max && !v.chars().any(|c| c.is_control() || is_hidden_format(c)))
        };
        self.title = self.title.map(|t| cut(&t, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES));
        self.git_branch = self
            .git_branch
            .map(|b| cut(&b, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES));
        self.model = within(self.model, MODEL_MAX_JSON_BYTES);
        self.mode = within(self.mode, MODE_MAX_JSON_BYTES);
        self.failure_reason = within(self.failure_reason, FAILURE_REASON_MAX_JSON_BYTES);
        self.cwd = tail(&self.cwd, CWD_MAX_JSON_BYTES);
        // An older row's, from before the start checked them.
        self.agent = cut(&self.agent, usize::MAX, AGENT_MAX_JSON_BYTES);
        self.host_id = cut(&self.host_id, usize::MAX, HOST_ID_MAX_JSON_BYTES);
        self
    }
}

/// `GET /api/sessions` (ACP core §9): one page of the session list, newest
/// `last_event_at` first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionPage {
    pub sessions: Vec<SessionItem>,
    /// Where the next page starts, for `cursor` (opaque); absent on the last
    /// page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub next_cursor: Option<String>,
}

/// `GET /api/sessions/{id}` (ACP core §9): the list item, as stored, the
/// open turn and the pending requests still open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub session: SessionItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "OpenTurn | undefined", optional)]
    pub open_turn: Option<OpenTurn>,
    /// Open pending requests, oldest first: what the operator can answer.
    #[serde(default)]
    pub pending: Vec<PendingItem>,
}

/// Where a pending request stands (ACP core §4.6): `open`, then
/// `delivered` or `cancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    Open,
    Delivered,
    Cancelled,
}

/// One pending request, as the collector holds it: an entry of
/// `SessionDetail.pending`, and the data of SSE `pending_changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingItem {
    pub pending_id: String,
    pub session_id: String,
    pub kind: PendingKind,
    pub state: PendingState,
    /// Why it was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "PendingReason | undefined", optional)]
    pub reason: Option<PendingReason>,
    /// The turn it was asked in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub turn_id: Option<String>,
    /// A permission's option ids: the only valid answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string[] | undefined", optional)]
    pub option_ids: Option<Vec<String>>,
    /// The adapter's ACP request, verbatim.
    #[ts(type = "unknown")]
    pub payload: Value,
    /// An answer has been accepted for it (at most one is).
    pub answered: bool,
    /// The host's verdict on that answer, once one arrived. `true` sticks
    /// (umbrella §6.8): a card shows "answered" only then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub delivered: Option<bool>,
}

/// `POST /api/sessions/{id}/pending/{pending_id}/answer` (ACP core §9): an
/// option for a permission request, or an action for an elicitation, with
/// the form's content when it accepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AnswerRequest {
    Permission {
        option_id: String,
    },
    Elicitation {
        action: ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown", optional)]
        content: Option<Value>,
    },
}

/// 202 to an answer: it is queued for the session's host, and delivered now
/// or on the host's next connection (ACP core §4.6). The verdict follows on
/// the session stream as `pending_changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AnswerResponse {
    pub pending_id: String,
    /// Carried by the host's `answer_result` for this answer.
    pub request_id: String,
}

/// `POST /api/sessions/{id}/cancel`: how the open turn ended. Usually
/// `cancelled`, but `completed` or `failed` if the turn ended before the
/// cancel reached the agent, and `interrupted` if the session was parked or
/// closed while the cancel was outstanding, or its adapter exited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CancelResponse {
    pub turn_id: String,
    pub outcome: crate::frames::TurnOutcome,
}

/// `POST /api/sessions/{id}/config`: switch one config option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ConfigRequest {
    pub config_id: String,
    pub value: ConfigValue,
}

/// A session's catalogue: its config options and their current values,
/// and its slash commands. `GET /api/sessions/{id}/catalog`, the answer to
/// `POST …/config`, and the data of the SSE `catalog_changed` message (ACP
/// core §9), always as it stands when sent. Plan and usage join it with
/// the plans that produce them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionCatalog {
    pub session_id: String,
    /// The adapter's ACP `SessionConfigOption` objects, as last reported.
    #[ts(type = "unknown[]")]
    pub config_options: Vec<Value>,
    /// The adapter's slash commands (ACP `AvailableCommand` objects), as
    /// last reported; empty until it reports any (ACP core §7). Always
    /// sent, so the schema and the TypeScript type agree that it is there.
    #[ts(type = "unknown[]")]
    pub commands: Vec<Value>,
    #[serde(flatten)]
    pub current: SessionConfig,
}

/// 201 to `POST /api/hosts/pairing-codes` (kernel spec §4.1): a single-use
/// code for `hennery host join`, shown once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PairingCodeResponse {
    /// `XXXX-XXXX`, Crockford base32.
    pub code: String,
    /// RFC 3339.
    pub expires_at: String,
}

/// `POST /api/hosts/enroll` (kernel spec §4.1): a host pairs itself with a
/// code and the public half of the key it generated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnrollRequest {
    pub code: String,
    /// Ed25519 public key, 64 hex characters.
    pub public_key: String,
    pub name: String,
    pub host_version: String,
    pub platform: String,
}

/// 201 to an enrollment: the id the host names itself by in `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnrollResponse {
    pub host_id: String,
}

/// One entry of `GET /api/hosts` (kernel spec §8): a paired host, revoked
/// ones included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HostItem {
    pub host_id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    /// From its latest accepted `hello`.
    pub capabilities: crate::frames::Capabilities,
    /// The hat of its sessions that no path rule claims (kernel spec §5.1).
    pub default_hat_id: String,
    /// From its latest reconciled connection, as configured on the host
    /// (ACP core §7): where its projects are, and where browsing may start.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Connected and reconciled: requests reach it now.
    pub connected: bool,
    /// RFC 3339.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub revoked_at: Option<String>,
}

/// A directory a session started or resumed in on the host (kernel spec
/// §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RecentProject {
    pub path: String,
    /// RFC 3339.
    pub last_used_at: String,
}

/// `GET /api/hosts/{id}/projects?path=` (ACP core §7, §9): the recent
/// projects of one hat, and the git repositories under the host's
/// workspace roots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HostProjects {
    /// The hat `recents` belong to: the one `path` resolves to on the host,
    /// or the host's default hat without one (kernel spec §5.3).
    pub recents_hat_id: String,
    /// Newest first: only those whose path resolves to that hat now.
    pub recents: Vec<RecentProject>,
    pub items: Vec<crate::frames::Project>,
    /// The host cut its enumeration short: there may be more.
    pub partial: bool,
    /// The host user's home directory, for expanding `~` (frontend §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub home: Option<String>,
}

/// `GET /api/hosts/{id}/browse?path=` (ACP core §7, §9): the
/// subdirectories of a directory on the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct DirectoryListing {
    /// The directory, canonical (symlinks resolved).
    pub path: String,
    /// Its parent, if browsing it is allowed too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub parent: Option<String>,
    pub entries: Vec<crate::frames::DirEntry>,
    /// Not every subdirectory is listed.
    pub truncated: bool,
}

/// `POST /api/setup` (kernel spec §3.1): the one-time owner setup, with the
/// token from the setup link. `Debug` leaves the password out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetupRequest {
    pub token: String,
    pub password: String,
    /// `https://…`, or `http://` to a loopback address; an origin only.
    pub public_url: String,
    /// The default hat's name (kernel spec §3.1), 1 to 64 printable
    /// characters. Absent: it stays "Personal".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub default_hat_name: Option<String>,
}

impl std::fmt::Debug for SetupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupRequest")
            .field("public_url", &self.public_url)
            .field("default_hat_name", &self.default_hat_name)
            .finish_non_exhaustive()
    }
}

/// 201 to a setup: the owner is created and signed in (the session cookie
/// is set), and `public_url` is stored as this origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetupResponse {
    pub public_url: String,
}

/// `POST /api/auth/login` (kernel spec §3.2): the owner's password. 204 and
/// the session cookie on success. `Debug` leaves the password out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LoginRequest {
    pub password: String,
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest").finish_non_exhaustive()
    }
}

/// `POST /api/auth/step-up/password` (kernel spec §3.4): the owner's
/// password again, within a session. 204 on success. `Debug` leaves the
/// password out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StepUpRequest {
    pub password: String,
}

impl std::fmt::Debug for StepUpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepUpRequest").finish_non_exhaustive()
    }
}

/// One entry of `GET /api/auth/sessions` (kernel spec §3.2): a signed-in
/// device, most recently used first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AuthSessionItem {
    /// What `DELETE /api/auth/sessions/{id}` takes: the SHA-256 of the
    /// session's token, never the token.
    pub id: String,
    pub user_agent: String,
    /// RFC 3339.
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    /// The session this request came with.
    pub current: bool,
}

/// `POST /api/auth/passkeys/register/start` (kernel spec §3.2, plan 3c):
/// the new passkey's label, 1 to 64 characters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PasskeyRegisterRequest {
    pub label: String,
}

/// 200 to the start of every passkey ceremony: registration, login and
/// step-up. `options` is WebAuthn's JSON, for
/// `navigator.credentials.create()` (registration) or `.get()` (login,
/// step-up): its `publicKey` goes through
/// `PublicKeyCredential.parseCreationOptionsFromJSON` or
/// `parseRequestOptionsFromJSON`. The finish names `ceremony_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PasskeyCeremony {
    pub ceremony_id: String,
    #[ts(type = "unknown")]
    pub options: Value,
}

/// The finish of every passkey ceremony: the credential the browser
/// returned, as `PublicKeyCredential.toJSON()` gives it. `Debug` leaves
/// the credential out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PasskeyFinishRequest {
    pub ceremony_id: String,
    #[ts(type = "unknown")]
    pub credential: Value,
}

impl std::fmt::Debug for PasskeyFinishRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasskeyFinishRequest")
            .field("ceremony_id", &self.ceremony_id)
            .finish_non_exhaustive()
    }
}

/// One of the owner's passkeys: an entry of `GET /api/auth/passkeys`, and
/// 201 to a registration's finish.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PasskeyItem {
    /// What `DELETE /api/auth/passkeys/{id}` takes.
    pub id: String,
    pub label: String,
    /// RFC 3339.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub last_used_at: Option<String>,
}

/// `PATCH /api/hosts/{id}` (kernel spec §4.3, §8): rename a host, or change
/// its default hat. Absent fields stay as they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UpdateHostRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub default_hat_id: Option<String>,
}

/// One hat (kernel spec §5.1): an entry of `GET /api/hats`, and the answer
/// to its creation and its changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HatItem {
    pub id: String,
    pub name: String,
    /// `#rrggbb`, lowercase.
    pub colour: String,
    /// RFC 3339.
    pub created_at: String,
    /// The hat newly paired hosts get as their default.
    pub default_for_new_hosts: bool,
}

/// `POST /api/hats`: a name, 1 to 64 printable characters, unique in any
/// case, and a `#rrggbb` colour (slate when absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CreateHatRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub colour: Option<String>,
}

/// `PATCH /api/hats/{id}`: absent fields stay as they are.
/// `default_for_new_hosts` can only be `true`: a hat stops being that
/// default when another takes its place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UpdateHatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub default_for_new_hosts: Option<bool>,
}

/// One path rule of a host (kernel spec §5.1, §5.2): sessions whose
/// canonical cwd is `prefix` or under it, by whole segments, belong to
/// `hat_id`, unless a longer rule covers them too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PathRuleItem {
    pub id: String,
    /// Canonical: absolute, no `.` or `..`, no trailing slash.
    pub prefix: String,
    pub hat_id: String,
    /// The host resolved the prefix when the rule was saved; an unverified
    /// rule is the path as typed, normalised by its text alone.
    pub verified: bool,
}

/// One rule of `PUT /api/hosts/{id}/path-rules`, as the operator typed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PathRuleInput {
    pub prefix: String,
    pub hat_id: String,
}

/// `PUT /api/hosts/{id}/path-rules` (kernel spec §8): the host's whole set
/// of rules, replacing the one before. The answer is the stored set,
/// longest prefix first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PathRulesRequest {
    pub rules: Vec<PathRuleInput>,
}

/// `POST /api/hats/resolve` (kernel spec §8): which hat `path` resolves to
/// on `host_id`, as a session started there would get. The host resolves
/// the path; `~` and `~/…` are its user's home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HatResolveRequest {
    pub host_id: String,
    pub path: String,
}

/// 200 to `POST /api/hats/resolve`: the canonical path, whether it exists
/// and is a directory there, and its hat, with the rule that decided it
/// (absent: the host's default hat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HatResolution {
    pub canonical: String,
    pub exists: bool,
    pub is_dir: bool,
    pub hat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub rule_id: Option<String>,
}
