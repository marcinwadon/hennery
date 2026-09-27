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
}

impl Default for FakeScript {
    fn default() -> Self {
        Self {
            chunks: vec!["Hello".into(), " world".into()],
            chunk_delay_ms: 0,
        }
    }
}

/// Environment variable carrying the script.
pub const SCRIPT_ENV: &str = "HENNERY_FAKE_ACP_SCRIPT";
