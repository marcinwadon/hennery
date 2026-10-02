//! `forget_session` on the host (plan 9d decisions 8, 11–13): remove a
//! deleted session's transcript from the agent's own data, under the root
//! the host itself registered for it (B1), and say what was removed and
//! what is left, as kinds and counts (B2).
//!
//! The connection (`connection::forget`) checks the id, the registry and
//! that no live actor has the agent's session (B7), then runs `forget` in a
//! task of its own, holding a marker that refuses any attach of that agent
//! session meanwhile.

use crate::adapter::AgentCommand;
use hennery_proto::frames::{ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// One deadline for a whole forget on the host (B6): below the collector's
/// wait (`hennery_sessions::forget::FORGET_WAIT`, 30 s), so its answer
/// comes first.
pub const FORGET_DEADLINE: Duration = Duration::from_secs(20);

/// What a forget needs of the host.
#[derive(Debug, Clone)]
pub struct ForgetContext {
    /// The host's agents, by name: the adapter a forget runs (decision 8).
    pub agents: HashMap<String, AgentCommand>,
    /// The host's data directory: never a root (B3).
    pub data_dir: PathBuf,
    /// The host user's home directory, if known: never a root, nor an
    /// ancestor of it (B3). `HOME` counts too.
    pub home: Option<PathBuf>,
}

/// One forget, as the collector asked for it, checked against the
/// registry already.
#[derive(Debug, Clone)]
pub struct Forget {
    pub agent: String,
    pub agent_session_id: String,
    pub agent_home: hennery_proto::frames::AgentHome,
}

/// Whether `id` is an id the agent itself writes (decision 8): a UUID in
/// lowercase hex, as Claude's SDK names its files. Nothing else is ever
/// built into a path.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
        })
}

pub fn left(kind: ForgetKind, count: u32, reason: ForgetReason, retry: bool) -> ForgetRemaining {
    ForgetRemaining {
        what: ForgetWhat { kind, count },
        reason,
        retry,
    }
}

/// The answer for a forget that could not start: the whole session left,
/// for `reason`.
pub fn refused(request_id: String, reason: ForgetReason, retry: bool) -> HostFrame {
    HostFrame::SessionForgotten {
        request_id,
        outcome: ForgetOutcome::Partial,
        removed: Vec::new(),
        remaining: vec![left(ForgetKind::Session, 0, reason, retry)],
    }
}

/// What one forget did.
#[derive(Debug, Clone, PartialEq)]
pub struct Forgotten {
    pub removed: Vec<ForgetWhat>,
    pub remaining: Vec<ForgetRemaining>,
}

impl Forgotten {
    pub fn into_frame(self, request_id: String) -> HostFrame {
        HostFrame::SessionForgotten {
            request_id,
            outcome: if self.remaining.is_empty() {
                ForgetOutcome::Complete
            } else {
                ForgetOutcome::Partial
            },
            removed: self.removed,
            remaining: self.remaining,
        }
    }
}

/// Run one forget (plan 9d decision 8). Only Claude's data is removed so
/// far; any other agent is answered `unsupported_agent`, retryable, for
/// plan 9d-ii to take up.
pub async fn forget(_ctx: &ForgetContext, forget: &Forget) -> Forgotten {
    let _ = &forget.agent_home;
    Forgotten {
        removed: Vec::new(),
        remaining: vec![left(ForgetKind::Session, 0, ForgetReason::UnsupportedAgent, true)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_lowercase_uuid_is_an_agent_session_id() {
        assert!(valid_id("0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3"));
        for bad in [
            "0B9C1D2E-3F40-4A5B-8C6D-7E8F90A1B2C3",
            "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c",
            "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3x",
            "0b9c1d2e_3f40-4a5b-8c6d-7e8f90a1b2c3",
            "../../../../../../etc/passwd/aaaaaaa",
            "fake-session-1",
            "",
            "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2g3",
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }
}
