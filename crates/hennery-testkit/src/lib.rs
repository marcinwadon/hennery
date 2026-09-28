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
