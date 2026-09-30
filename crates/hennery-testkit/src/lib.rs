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
    /// Advertise `loadSession: false` in `initialize`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_load_session: bool,
    /// Receive `session/cancel` but keep streaming the prompt as if it
    /// never came (an adapter that does not honour cancellation).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_cancel: bool,
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
    /// Before answering a switch of the model option, send this many
    /// `agent_message_chunk` notifications with no sleep between them, then
    /// answer. Sent inline (not from a spawned task), so they land on the
    /// wire strictly before the switch's own answer: the host sees a known,
    /// deterministic backlog ahead of the answer, rather than one whose size
    /// depends on racing another task's timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_chunks_first: Option<usize>,
}

impl Default for FakeScript {
    fn default() -> Self {
        Self {
            chunks: vec!["Hello".into(), " world".into()],
            chunk_delay_ms: 0,
            exit_after_chunks: None,
            stderr_lines: Vec::new(),
            grandchild_pid_file: None,
            replay: Vec::new(),
            load_error: None,
            new_session_error: None,
            no_load_session: false,
            ignore_cancel: false,
            cancel_error: None,
            config_options: Vec::new(),
            model_switch_sets_mode: None,
            prompt_sets_mode: None,
            config_log: None,
            empty_config_read_back: false,
            hang_config: false,
            slow_model_switch_ms: None,
            sticky_options: Vec::new(),
            config_in_update_only: false,
            model_switch_drops_option: None,
            announce_after_switch: false,
            announce_before_switch: false,
            flood: false,
            model_switch_chunks_first: None,
        }
    }
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
