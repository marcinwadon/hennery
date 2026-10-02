//! Test support for hennery: the fake ACP adapter binary and shared helpers.

use serde::{Deserialize, Serialize};

/// Behaviour of `hennery-fake-acp`, passed as JSON in `HENNERY_FAKE_ACP_SCRIPT`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeScript {
    /// Text chunks streamed as `agent_message_chunk` updates for every prompt.
    pub chunks: Vec<String>,
    /// Delay before each chunk, in milliseconds.
    #[serde(default)]
    pub chunk_delay_ms: u64,
    /// Crash mid-turn: after sending this many chunks of a prompt, write a
    /// line to stderr and exit with status 3 without answering the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_after_chunks: Option<usize>,
    /// Lines written to stderr at startup (e.g. a fake token, to test that
    /// the host scrubs the stderr tail).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stderr_lines: Vec<String>,
    /// Lines written raw (not JSON-RPC) to stdout right before answering
    /// `session/new`: written while that answer does not exist yet, so the
    /// ACP crate has no JSON-RPC line of its own still being written to the
    /// same stdout (e.g. a fake token, to test that the host's log never
    /// shows what an adapter prints to its own stdout, plan 8c).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stdout_lines: Vec<String>,
    /// At startup, spawn a long-lived `sleep` child (a grandchild of the
    /// host) in the adapter's process group and write its pid to this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grandchild_pid_file: Option<String>,
    /// Raw ACP `update` objects streamed as `session/update` during
    /// `session/load`, in order, before the load is answered. Sent untyped,
    /// so unknown `sessionUpdate` kinds reach the host as they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replay: Vec<serde_json::Value>,
    /// Answer `session/load` with this JSON-RPC error code (after the replay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<i32>,
    /// Answer `session/new` with this JSON-RPC error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_session_error: Option<i32>,
    /// `new_session_error`'s message quotes the request's `mcpServers` (an
    /// adapter that echoes the config it refuses, plan 8c).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_session_error_echoes: bool,
    /// Answer every `session/prompt` with a JSON-RPC error carrying this
    /// message (an adapter whose error quotes its config, plan 8c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_error: Option<String>,
    /// Answer every `session/set_config_option` with a JSON-RPC error
    /// carrying this message, as `prompt_error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
    /// Advertise `loadSession: false` in `initialize`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_load_session: bool,
    /// Receive `session/cancel` but keep streaming the prompt as if it
    /// never came (an adapter that does not honour cancellation).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_cancel: bool,
    /// Append a line to this file the instant `session/cancel` is received
    /// on the wire — before deciding whether to honour or (`ignore_cancel`)
    /// ignore it. A marker of when the notification *arrived*, independent
    /// of how long streaming then takes to actually stop: lets a test prove
    /// the host forwarded a cancel promptly without waiting out however
    /// much backlog is left to drain afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_received_file: Option<String>,
    /// Answer a cancelled prompt with this JSON-RPC error code instead of
    /// the `cancelled` stop reason (an agent whose aborted work throws).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_error: Option<i32>,
    /// Config options (ACP `SessionConfigOption` JSON) announced by
    /// `session/new` and `session/load` and switched by
    /// `session/set_config_option`. A switch is validated like a real
    /// adapter does: an unknown id, or a value the option does not offer, is
    /// refused with `-32602`. A boolean option is announced as a boolean
    /// only to a client whose `initialize` advertises
    /// `session.configOptions.boolean`; any other client gets an `on` /
    /// `off` select instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_options: Vec<serde_json::Value>,
    /// A switch of the model option also sets the mode option to this value
    /// (a model that clamps the mode, ACP core §12 scenario 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_sets_mode: Option<String>,
    /// Every prompt first sets the mode option to this value and announces
    /// it with a `config_option_update` (an agent that leaves plan mode on
    /// its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sets_mode: Option<String>,
    /// Append one `id=value` line per `session/set_config_option` call to
    /// this file, refused calls included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_log: Option<String>,
    /// Append one JSON line per `session/new` and `session/load` to this
    /// file: `{"method", "params"}`, the request as the fake parsed it
    /// (plan 8c: its `mcpServers` and `_meta`). A file, never stderr, which
    /// would reach `adapter_exited`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_log: Option<String>,
    /// Apply switches but answer them with an empty `configOptions` list (a
    /// read-back the host cannot use).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub empty_config_read_back: bool,
    /// Never answer `session/set_config_option` (a hung switch).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hang_config: bool,
    /// Answer a switch of the model option only after this many
    /// milliseconds, while other requests are handled meanwhile, and apply
    /// `model_switch_sets_mode` only after that answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_model_switch_ms: Option<u64>,
    /// Switches of these option ids are accepted but change nothing (an
    /// adapter that reports a value other than the one it was given).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sticky_options: Vec<String>,
    /// Announce the config options in a `config_option_update` sent just
    /// before the `session/new` / `session/load` answer, which then has
    /// none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub config_in_update_only: bool,
    /// A switch of the model option also drops this option id from the
    /// catalogue (a model that no longer offers some other option) — for
    /// testing that a queued switch is validated against the catalogue
    /// *after* the switch ahead of it, not the one in effect when it
    /// arrived (fix round 1, F3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_drops_option: Option<String>,
    /// Send a `config_option_update` announcing mode `bypass`, right after
    /// answering a `session/set_config_option` request (the normal, fast
    /// path only) — an agent that independently changes its own config
    /// right after answering a switch. For testing the race between a
    /// switch's own (now stale) read-back and a notification sent right
    /// after it (fix round 1 and 2, F2, case B: N postdates R).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub announce_after_switch: bool,
    /// Send a `config_option_update` announcing the catalogue as it stood
    /// *before* this switch, immediately before answering a
    /// `session/set_config_option` request (the normal, fast path only). For
    /// testing that a notification older than a switch's own read-back
    /// (fix round 2, F2, case A: N0 predates R) never wins over it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub announce_before_switch: bool,
    /// Stream `chunks` over and over, back to back and without sleeping,
    /// until the prompt is cancelled (an adapter flooding the host).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flood: bool,
    /// With `flood`, sleep this long between chunks instead: still endless,
    /// but the host's connection task is idle between them, as it is for
    /// an agent that streams no faster than the host reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flood_interval_ms: Option<u64>,
    /// Before answering a switch of the model option, send this many
    /// `agent_message_chunk` notifications with no sleep between them, then
    /// answer. Sent inline (not from a spawned task), so they land on the
    /// wire strictly before the switch's own answer: the host sees a known,
    /// deterministic backlog ahead of the answer, rather than one whose size
    /// depends on racing another task's timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_chunks_first: Option<usize>,
    /// Answer a switch of the model option only once this file exists,
    /// from a task of its own (other requests are handled meanwhile), with
    /// `model_switch_chunks_first`'s backlog sent right before the answer
    /// rather than on receipt: a switch answered exactly when the test says
    /// so, instead of after a delay that races the host's own deadlines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_answer_on_file: Option<String>,
    /// Questions asked at the start of every prompt, before its chunks, in
    /// order, each awaited before the next. Each answer is echoed as an
    /// `agent_message_chunk` (see `FakeAsk`), so a test sees what reached
    /// the agent. A prompt cancelled meanwhile asks nothing more and ends
    /// `cancelled` once its open questions are answered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<FakeAsk>,
    /// Send every ask at once, then await the answers in the asks' order
    /// (several questions open together).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asks_at_once: bool,
    /// Send the asks, then crash (exit status 3) without awaiting their
    /// answers: an adapter lost with its questions open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crash_while_asking: bool,
    /// Send the first ask right before answering `session/load`, and echo
    /// its answer once it comes, outside any turn: a question that arrives
    /// while the host is still attaching the session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask_on_load: bool,
    /// With `ask_on_load`: answer `session/load` only once that ask is
    /// answered (an adapter that blocks its load on a question).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask_on_load_waits: bool,
    /// Withdraw every ask right after sending it (`$/cancel_request`), as an
    /// agent that no longer needs the answer does. The echo is whatever the
    /// client answers then, usually `<name>:error:-32800`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withdraw_asks: bool,
    /// Offer no images in `initialize` (`promptCapabilities.image: false`),
    /// like an agent that takes text only. By default the fake offers them,
    /// and echoes each image of a prompt, before its chunks, as a chunk
    /// `image:<mimeType>:<SHA-256 of the decoded bytes>\n`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_images: bool,
    /// Raw ACP `update` objects streamed as `session/update` at the start of
    /// every prompt, in order, before its asks and chunks. Sent untyped,
    /// like `replay`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_updates: Vec<serde_json::Value>,
    /// At the start of every prompt, print the `mcpServers` of the latest
    /// `session/new` or `session/load` as an agent message chunk, their
    /// headers' values included: an agent that prints its gateway token
    /// (plan 8e decision 11).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub echo_servers: bool,
}

/// One question the fake asks its client during a prompt, and the chunk it
/// echoes the answer as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FakeAsk {
    /// `session/request_permission` for tool call `call-1`, with options
    /// `allow` (`allow_once`) and `reject` (`reject_once`). Echoed as
    /// `permission:selected:<option>` or `permission:cancelled`.
    Permission,
    /// `elicitation/create` in form mode, asking for a `name` string: only
    /// of a client whose `initialize` advertised `elicitation.form`, as the
    /// real adapters do (P-19). Echoed as `elicitation:accept:<content>`,
    /// `elicitation:decline`, `elicitation:cancel`, or, when not asked,
    /// `elicitation:unsupported`.
    Elicitation,
    /// `_fake/unknown`, a method no client serves. Echoed as
    /// `unknown:error:<JSON-RPC code>`.
    Unknown,
    /// Like `Permission`, with a second option `allow_session` of a kind
    /// this build's schema does not know (`allow_for_session`): an adapter
    /// newer than hennery. Echoed like `Permission`.
    FuturePermission,
}

impl Default for FakeScript {
    fn default() -> Self {
        Self {
            chunks: vec!["Hello".into(), " world".into()],
            chunk_delay_ms: 0,
            exit_after_chunks: None,
            stderr_lines: Vec::new(),
            stdout_lines: Vec::new(),
            grandchild_pid_file: None,
            replay: Vec::new(),
            load_error: None,
            new_session_error: None,
            new_session_error_echoes: false,
            prompt_error: None,
            config_error: None,
            no_load_session: false,
            ignore_cancel: false,
            cancel_received_file: None,
            cancel_error: None,
            config_options: Vec::new(),
            model_switch_sets_mode: None,
            prompt_sets_mode: None,
            config_log: None,
            session_log: None,
            empty_config_read_back: false,
            hang_config: false,
            slow_model_switch_ms: None,
            sticky_options: Vec::new(),
            config_in_update_only: false,
            model_switch_drops_option: None,
            announce_after_switch: false,
            announce_before_switch: false,
            flood: false,
            flood_interval_ms: None,
            model_switch_chunks_first: None,
            model_switch_answer_on_file: None,
            asks: Vec::new(),
            asks_at_once: false,
            crash_while_asking: false,
            ask_on_load: false,
            ask_on_load_waits: false,
            withdraw_asks: false,
            no_images: false,
            prompt_updates: Vec::new(),
            echo_servers: false,
        }
    }
}

/// The `public_url` every test collector is set up with, and so the
/// `Origin` its state-changing requests carry.
pub const PUBLIC_URL: &str = "https://hennery.example";

/// The owner's password in test collectors.
pub const OWNER_PASSWORD: &str = "correct horse battery";

/// The owner's PHC string, from a check of `OWNER_PASSWORD` as a login
/// makes: what `Operator::open_session` opens a session on.
pub fn owner_phc(operator: &hennery_kernel::operator::Operator) -> String {
    operator
        .verify_password(OWNER_PASSWORD)
        .unwrap()
        .expect("the owner's password")
}

/// A client signed in as the owner of `operator`'s collector (kernel spec
/// §3.2), which is set up first if it is not: every request carries the
/// session cookie and the `public_url`'s `Origin`. The session is opened
/// through the operator directly, as a login would.
pub fn operator_client(operator: &hennery_kernel::operator::Operator) -> reqwest::Client {
    let now = hennery_kernel::secret::unix_now();
    if !operator.is_set_up().unwrap() {
        let token = operator.issue_setup_token(now).unwrap().unwrap();
        operator.set_up(&token, OWNER_PASSWORD, PUBLIC_URL, now).unwrap();
    }
    let token = operator
        .open_session("hennery-testkit", &owner_phc(operator), now)
        .unwrap()
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::COOKIE,
        format!("{}={token}", hennery_kernel::operator::SESSION_COOKIE)
            .parse()
            .unwrap(),
    );
    headers.insert(reqwest::header::ORIGIN, PUBLIC_URL.parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

/// Environment variable carrying the script.
pub const SCRIPT_ENV: &str = "HENNERY_FAKE_ACP_SCRIPT";

/// Exit status of the fake adapter when `exit_after_chunks` fires.
pub const CRASH_EXIT_CODE: i32 = 3;

/// Whether a process with this pid is still alive (signal 0 probe). A zombie
/// counts as alive until its parent reaps it.
pub fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill(2) with signal 0 only checks for existence/permission.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Make `fd` this process's descriptor `at`, open across `exec`: for a
/// `pre_exec` hook that hands a child a descriptor at a fixed number. When
/// `fd` already is `at`, `dup2` would be a no-op that keeps close-on-exec,
/// so the flag is cleared instead. Async-signal-safe (`dup2`, `fcntl`).
pub fn place_fd(fd: i32, at: i32) -> std::io::Result<()> {
    // SAFETY: dup2(2) and fcntl(2) on descriptor numbers; neither allocates.
    let rc = unsafe {
        if fd == at {
            libc::fcntl(at, libc::F_SETFD, 0)
        } else {
            libc::dup2(fd, at)
        }
    };
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A config catalogue like a real adapter's, for `FakeScript::config_options`:
/// `model` (category `model`: `small` | `large`, current `small`), `effort`
/// (category `thought_level`: `low` | `high`, current `low`), `fast` (a
/// boolean, off) and `mode` (category `mode`: `default` | `plan` |
/// `bypass`, current `default`).
pub fn sample_config_options() -> Vec<serde_json::Value> {
    let select = |id: &str, category: &str, current: &str, values: &[&str]| {
        let options: Vec<serde_json::Value> = values
            .iter()
            .map(|v| serde_json::json!({ "value": v, "name": v }))
            .collect();
        serde_json::json!({
            "id": id, "name": id, "category": category, "type": "select",
            "currentValue": current, "options": options
        })
    };
    vec![
        select("model", "model", "small", &["small", "large"]),
        select("effort", "thought_level", "low", &["low", "high"]),
        serde_json::json!({ "id": "fast", "name": "fast", "type": "boolean", "currentValue": false }),
        select("mode", "mode", "default", &["default", "plan", "bypass"]),
    ]
}
