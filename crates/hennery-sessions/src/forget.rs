//! The agent's own transcript of a deleted session, on its host (plan 9d
//! decisions 4–7): sending `forget_session` for the records a delete left
//! (`store::HostForgets`), and what the answers come to.
//!
//! A record goes right after its delete commits (the route waits at most
//! `FORGET_WAIT` for all of them), after each reconciled handshake of its
//! host (`retry_host`), and after its session's `session_closed`
//! (`retry_session`, O10). One attempt per record is in flight at a time
//! (B7, `InFlight`). The host's answer is kinds and counts with fixed
//! reasons (B2): nothing in it is a path, and it is checked before it is
//! stored. What a host reports is not verified beyond that.

use crate::AppState;
use crate::hub::RequestError;
use crate::store::{ForgetRecord, HostForgets, final_result};
use hennery_proto::frames::{
    CollectorFrame, ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame,
};
use hennery_proto::rest::{
    HostRemovalItem, HostTranscripts, RemovalItem, RemovalPending, RemovalState, TranscriptRemoval,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a delete waits for its host's answers, all records together
/// (plan 9d decision 5), and how long one attempt waits after a handshake.
/// Above the host's own deadline (`hennery_host::forget::FORGET_DEADLINE`),
/// so a live host's answer comes first.
pub const FORGET_WAIT: Duration = Duration::from_secs(30);

/// How many `app_server_timed_out` answers in a row flag a record's next
/// forget `fallback` (plan 9d-ii, B5 as ruled).
pub const APP_SERVER_TIMEOUTS_BEFORE_FALLBACK: u32 = 3;

/// Whether `result` is one the hybrid counts (plan 9d-ii, B5 as ruled):
/// its `session` item is `app_server_timed_out`. Any other result resets
/// the count.
pub fn app_server_timed_out(result: &TranscriptRemoval) -> bool {
    result
        .remaining
        .iter()
        .any(|r| r.kind == ForgetKind::Session && r.reason == ForgetReason::AppServerTimedOut)
}

/// A record's count after `result` (the hybrid's): one more, or 0.
pub fn app_server_timeouts_after(count: u32, result: &TranscriptRemoval) -> u32 {
    if app_server_timed_out(result) {
        count.saturating_add(1)
    } else {
        0
    }
}

/// The most entries of each list an answer may hold (B2's size cap): a
/// forget names at most a handful of kinds.
pub const MAX_ANSWER_ITEMS: usize = 16;

/// What is never removed for a Claude session, whatever the outcome (plan
/// 9d's known limitation, O12, B9). Always in a Claude session's result.
pub const CLAUDE_NOTES: [&str; 3] = [
    "transcripts started after a context clear inside the agent are not removed",
    "the agent's history.jsonl, telemetry/1p_failed_events.* and todos/ may still name this session",
    "only the transcript's own names in each project directory are removed; anything else there is left",
];

/// What is never removed for a Codex session, whatever the outcome (plan
/// 9d-ii, O12). Always in a Codex session's result.
pub const CODEX_NOTES: [&str; 1] = ["the agent's history.jsonl and logs_2.sqlite may still name this session"];

/// What the fallback leaves (plan 9d decision 10, B5): in a Codex result
/// whose remaining items name `codex_database_copies`.
pub const CODEX_FALLBACK_NOTES: [&str; 2] = [
    "conversation copies may remain in Codex's own database",
    "rollouts of the session's subagent threads are out of the fallback's reach",
];

/// The notes a session of `agent` always gets (plan 9d decision 7).
pub fn notes(agent: &str) -> Vec<String> {
    match agent {
        "claude" => CLAUDE_NOTES.iter().map(|n| n.to_string()).collect(),
        "codex" => CODEX_NOTES.iter().map(|n| n.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// `notes`, and what a result's remaining items add: the fallback's for
/// `codex_database_copies` (plan 9d-ii).
pub fn notes_for(agent: &str, remaining: &[RemovalItem]) -> Vec<String> {
    let mut out = notes(agent);
    if remaining.iter().any(|r| r.kind == ForgetKind::CodexDatabaseCopies) {
        out.extend(CODEX_FALLBACK_NOTES.iter().map(|n| n.to_string()));
    }
    out
}

/// The records with an attempt in flight (B7): one at a time each. Each
/// says whether another attempt was asked for meanwhile (the review's
/// item 11): the holder then goes again, so a `session_closed` that lands
/// while an `attached` answer is on its way is not lost.
#[derive(Default)]
pub struct InFlight(Mutex<HashMap<String, bool>>);

impl InFlight {
    /// Take `id`, unless an attempt holds it, in which case that attempt is
    /// asked to go again. Released when the claim is dropped, however the
    /// attempt ends (its HTTP client gone included).
    fn claim(self: &Arc<Self>, id: &str) -> Option<Claim> {
        let mut held = self.0.lock().expect("in-flight lock");
        if let Some(again) = held.get_mut(id) {
            *again = true;
            return None;
        }
        held.insert(id.to_string(), false);
        Some(Claim {
            set: self.clone(),
            id: id.to_string(),
        })
    }
}

struct Claim {
    set: Arc<InFlight>,
    id: String,
}

impl Claim {
    /// Whether another attempt was asked for since the claim was taken (or
    /// last asked), clearing the request.
    fn asked_again(&self) -> bool {
        self.set
            .0
            .lock()
            .expect("in-flight lock")
            .get_mut(&self.id)
            .map(std::mem::take)
            .unwrap_or(false)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.set.0.lock().expect("in-flight lock").remove(&self.id);
    }
}

fn pending(why: RemovalPending) -> TranscriptRemoval {
    TranscriptRemoval {
        state: RemovalState::Pending,
        pending: Some(why),
        remaining: Vec::new(),
        notes: Vec::new(),
    }
}

/// An answer's lists within the caps (B2).
fn well_formed(removed: &[ForgetWhat], remaining: &[ForgetRemaining]) -> bool {
    removed.len() <= MAX_ANSWER_ITEMS && remaining.len() <= MAX_ANSWER_ITEMS
}

/// What a host's answer comes to, and whether the record is done with
/// (plan 9d decision 6): `complete`, or `partial` with nothing a retry can
/// change. A partial answer whose only retryable reason is `attached` is
/// pending for that reason (decision 7).
pub fn answered(outcome: ForgetOutcome, remaining: &[ForgetRemaining]) -> (TranscriptRemoval, bool) {
    if outcome == ForgetOutcome::Complete && remaining.is_empty() {
        let removed = TranscriptRemoval {
            state: RemovalState::Removed,
            pending: None,
            remaining: Vec::new(),
            notes: Vec::new(),
        };
        return (removed, true);
    }
    let items: Vec<RemovalItem> = remaining
        .iter()
        .map(|r| RemovalItem {
            kind: r.what.kind,
            count: r.what.count,
            reason: r.reason,
        })
        .collect();
    let retryable: Vec<&ForgetRemaining> = remaining.iter().filter(|r| r.retry).collect();
    // Waiting on the host only: its adapter (attached), or another forget
    // of the same agent session running there (in progress).
    let waiting = !retryable.is_empty()
        && retryable
            .iter()
            .all(|r| matches!(r.reason, ForgetReason::Attached | ForgetReason::InProgress));
    let why = if retryable.iter().any(|r| r.reason == ForgetReason::Attached) {
        RemovalPending::Attached
    } else {
        RemovalPending::InProgress
    };
    let result = TranscriptRemoval {
        state: if waiting {
            RemovalState::Pending
        } else {
            RemovalState::Partial
        },
        pending: waiting.then_some(why),
        remaining: items,
        notes: Vec::new(),
    };
    (result, retryable.is_empty())
}

/// One attempt at `record`, waiting at most `wait` for the host: its result,
/// stored unless another attempt holds the record (`in_progress`, nothing
/// stored; the holder goes again once it is done, within its own wait).
pub async fn attempt(state: &AppState, record: &ForgetRecord, wait: Duration) -> TranscriptRemoval {
    let Some(claim) = state.forgets.claim(&record.id) else {
        return pending(RemovalPending::InProgress);
    };
    let until = tokio::time::Instant::now() + wait;
    // The claim makes this the record's only attempt in flight, so the
    // hybrid's count is followed here as the store keeps it: a second
    // round sends what the first one's answer made it (plan 9d-ii).
    let mut record = record.clone();
    loop {
        let (result, done) = attempt_once(
            state,
            &record,
            until.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await;
        if done || !claim.asked_again() || tokio::time::Instant::now() >= until {
            return result;
        }
        record.app_server_timeouts = app_server_timeouts_after(record.app_server_timeouts, &result);
    }
}

/// One `forget_session` for `record`, under its claim: the result and
/// whether the record is done with.
async fn attempt_once(state: &AppState, record: &ForgetRecord, wait: Duration) -> (TranscriptRemoval, bool) {
    let Some(home) = record.agent_home.clone() else {
        let known = record
            .last_result
            .clone()
            .unwrap_or_else(|| final_result(ForgetReason::NoRecordedHome));
        return (known, true);
    };
    // Another kept session may have taken up the agent session since the
    // delete (a resume of a session that shares it): not forgotten then,
    // final (B8, the review's item 13).
    match state.store.forget_is_shared(record) {
        Ok(false) => {}
        Ok(true) => {
            let result = final_result(ForgetReason::Shared);
            if let Err(err) = state.store.forget_attempted(&record.id, &result, false, true) {
                tracing::error!(host_id = %record.host_id, error = %err, "recording a forget's result failed");
            }
            return (result, true);
        }
        Err(err) => {
            tracing::error!(host_id = %record.host_id, error = %err, "checking whether a forget is shared failed");
            return (pending(RemovalPending::NoReply), false);
        }
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ForgetSession {
        request_id: request_id.clone(),
        agent: record.agent.clone(),
        agent_session_id: record.agent_session_id.clone(),
        agent_home: home,
        // B5 as ruled (the hybrid): after this many app-server timeouts in
        // a row, the host is told to go straight to the fallback.
        fallback: record.app_server_timeouts >= APP_SERVER_TIMEOUTS_BEFORE_FALLBACK,
    };
    let (result, sent, done) = match state.hub.probe(&record.host_id, &request_id, frame, wait).await {
        Ok(HostFrame::SessionForgotten {
            outcome,
            removed,
            remaining,
            ..
        }) if well_formed(&removed, &remaining) => {
            let (result, done) = answered(outcome, &remaining);
            (result, true, done)
        }
        Ok(_) => {
            tracing::warn!(host_id = %record.host_id, "a forget's answer is out of bounds; it stays pending");
            (pending(RemovalPending::NoReply), true, false)
        }
        Err(RequestError::NotConnected) => (pending(RemovalPending::HostOffline), false, false),
        // A probe carries no servers: `McpUndeliverable` cannot happen.
        Err(RequestError::Unsupported | RequestError::McpUndeliverable) => {
            (pending(RemovalPending::HostNeedsUpdate), false, false)
        }
        Err(RequestError::Busy) => (pending(RemovalPending::NoReply), false, false),
        Err(RequestError::DeliveryUnknown) => (pending(RemovalPending::NoReply), true, false),
        // The id is not one the agent writes (decision 8): no retry
        // changes it (O10).
        Err(RequestError::Rejected { code, .. }) if code == "invalid" => {
            (final_result(ForgetReason::InvalidId), true, true)
        }
        Err(RequestError::Rejected { code, message }) => {
            tracing::warn!(host_id = %record.host_id, %code, %message, "a forget was refused; it stays pending");
            (pending(RemovalPending::NoReply), true, false)
        }
    };
    if let Err(err) = state.store.forget_attempted(&record.id, &result, sent, done) {
        tracing::error!(host_id = %record.host_id, error = %err, "recording a forget's result failed");
    }
    (result, done)
}

/// Right after a delete committed (plan 9d decision 5): each of its pending
/// records, while `FORGET_WAIT` lasts, all together; then the result for
/// the delete's answer (decision 7).
pub async fn after_delete(state: &AppState, forgets: &HostForgets) -> TranscriptRemoval {
    after_delete_until(state, forgets, tokio::time::Instant::now() + FORGET_WAIT).await
}

/// After a purge (plan 9d decision 7, O10): each purged session's records,
/// as `after_delete` sends them, within one `FORGET_WAIT` for all of them,
/// counted by each session's result. A session with no agent record counts
/// in none.
pub async fn after_purge(state: &AppState, purged: &[(String, HostForgets)]) -> HostTranscripts {
    let deadline = tokio::time::Instant::now() + FORGET_WAIT;
    let mut counts = HostTranscripts::default();
    for (session_id, forgets) in purged {
        match after_delete_until(state, forgets, deadline).await.state {
            RemovalState::Removed => counts.removed += 1,
            RemovalState::Partial => counts.partial += 1,
            RemovalState::Pending => {
                counts.pending += 1;
                counts.pending_sessions.push(session_id.clone());
            }
            RemovalState::None => {}
        }
    }
    counts
}

async fn after_delete_until(
    state: &AppState,
    forgets: &HostForgets,
    deadline: tokio::time::Instant,
) -> TranscriptRemoval {
    let mut results = Vec::new();
    for record in &forgets.records {
        let result = match &record.last_result {
            // Known without the host (no home, a revoked host).
            Some(known) => known.clone(),
            None => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    pending(RemovalPending::NoReply)
                } else {
                    attempt(state, record, left).await
                }
            }
        };
        results.push(result);
    }
    if forgets.shared > 0 {
        results.push(final_result(ForgetReason::Shared));
    }
    combined(forgets.had_agent_record, &forgets.agent, results)
}

/// The results of a session's records as one (decision 7): pending if any
/// is, else partial if any is, else removed; `none` with no agent record.
pub fn combined(had_agent_record: bool, agent: &str, results: Vec<TranscriptRemoval>) -> TranscriptRemoval {
    if !had_agent_record {
        return TranscriptRemoval {
            state: RemovalState::None,
            pending: None,
            remaining: Vec::new(),
            notes: notes(agent),
        };
    }
    let state = if results.iter().any(|r| r.state == RemovalState::Pending) {
        RemovalState::Pending
    } else if results.iter().any(|r| r.state == RemovalState::Partial) {
        RemovalState::Partial
    } else {
        RemovalState::Removed
    };
    let pending = results.iter().find_map(|r| r.pending);
    let remaining: Vec<RemovalItem> = results.into_iter().flat_map(|r| r.remaining).collect();
    TranscriptRemoval {
        state,
        pending,
        notes: notes_for(agent, &remaining),
        remaining,
    }
}

/// A record as `GET /api/settings/host-removals` lists it.
pub fn listed(record: ForgetRecord) -> HostRemovalItem {
    let agent = record.agent.clone();
    HostRemovalItem {
        id: record.id,
        host_id: record.host_id,
        session_id: record.session_id,
        state: record.state,
        attempts: record.attempts,
        last_result: record.last_result.map(|mut result| {
            result.notes = notes_for(&agent, &result.remaining);
            result
        }),
        agent: record.agent,
        created_at: record.created_at,
    }
}

/// After a reconciled handshake (plan 9d decision 5): the host's pending
/// records, one at a time, while it stays ready.
pub fn retry_host(state: &AppState, host_id: &str) {
    let (state, host_id) = (state.clone(), host_id.to_string());
    tokio::spawn(async move {
        let records = match state.store.forgets_to_send(&host_id) {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(%host_id, error = %err, "reading the host's pending removals failed");
                return;
            }
        };
        for record in records {
            if !state.hub.is_ready(&host_id) {
                return;
            }
            attempt(&state, &record, FORGET_WAIT).await;
        }
    });
}

/// After a deleted session's `session_closed` (plan 9d O10): its records,
/// which an `attached` answer left pending.
pub fn retry_session(state: &AppState, session_id: &str) {
    let (state, session_id) = (state.clone(), session_id.to_string());
    tokio::spawn(async move {
        let records = match state.store.forgets_of_session(&session_id) {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(%session_id, error = %err, "reading the session's pending removals failed");
                return;
            }
        };
        for record in records {
            attempt(&state, &record, FORGET_WAIT).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::ForgetKind;

    fn left(kind: ForgetKind, reason: ForgetReason, retry: bool) -> ForgetRemaining {
        ForgetRemaining {
            what: ForgetWhat { kind, count: 1 },
            reason,
            retry,
        }
    }

    #[test]
    fn a_record_is_done_when_complete_or_when_no_retry_can_change_what_is_left() {
        let (result, done) = answered(ForgetOutcome::Complete, &[]);
        assert_eq!((result.state, done), (RemovalState::Removed, true));
        let symlink = left(ForgetKind::FileHistory, ForgetReason::Symlink, false);
        let (result, done) = answered(ForgetOutcome::Partial, &[symlink]);
        assert_eq!((result.state, done), (RemovalState::Partial, true));
        let io = left(ForgetKind::Tasks, ForgetReason::IoError, true);
        let (result, done) = answered(ForgetOutcome::Partial, &[symlink, io]);
        assert_eq!((result.state, done), (RemovalState::Partial, false));
        let attached = left(ForgetKind::Session, ForgetReason::Attached, true);
        let (result, done) = answered(ForgetOutcome::Partial, &[attached]);
        assert_eq!(
            (result.state, result.pending, done),
            (RemovalState::Pending, Some(RemovalPending::Attached), false)
        );
        let busy = left(ForgetKind::Session, ForgetReason::InProgress, true);
        let (result, done) = answered(ForgetOutcome::Partial, &[busy]);
        assert_eq!(
            (result.state, result.pending, done),
            (RemovalState::Pending, Some(RemovalPending::InProgress), false)
        );
    }

    #[test]
    fn a_sessions_results_combine_to_the_worst_and_claude_always_has_its_notes() {
        let removed = answered(ForgetOutcome::Complete, &[]).0;
        let all = combined(true, "claude", vec![removed.clone(), removed.clone()]);
        assert_eq!(all.state, RemovalState::Removed);
        assert_eq!(all.notes.len(), CLAUDE_NOTES.len());
        assert!(all.notes[0].contains("context clear"));
        let mixed = combined(true, "codex", vec![removed, final_result(ForgetReason::Shared)]);
        assert_eq!(mixed.state, RemovalState::Partial);
        assert_eq!(mixed.notes, CODEX_NOTES);
        let none = combined(false, "claude", Vec::new());
        assert_eq!(none.state, RemovalState::None);
        assert_eq!(none.notes.len(), CLAUDE_NOTES.len());
        let waiting = combined(
            true,
            "claude",
            vec![pending(RemovalPending::HostOffline), final_result(ForgetReason::Shared)],
        );
        assert_eq!(
            (waiting.state, waiting.pending, waiting.remaining.len()),
            (RemovalState::Pending, Some(RemovalPending::HostOffline), 1)
        );
    }

    /// Plan 9d-ii, O12, decision 10: a Codex session always names Codex's
    /// other residue; one the fallback handled also says its database
    /// copies may remain and that subagent rollouts are out of its reach.
    /// Any other agent gets none.
    #[test]
    fn codex_notes_name_its_residue_and_what_the_fallback_leaves() {
        let removed = answered(ForgetOutcome::Complete, &[]).0;
        let deleted = combined(true, "codex", vec![removed.clone()]);
        assert_eq!(deleted.notes, CODEX_NOTES);
        assert!(deleted.notes[0].contains("history.jsonl") && deleted.notes[0].contains("logs_2.sqlite"));
        let copies = left(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false);
        let (fell_back, done) = answered(ForgetOutcome::Partial, &[copies]);
        assert_eq!((fell_back.state, done), (RemovalState::Partial, true));
        let fell_back = combined(true, "codex", vec![fell_back]);
        let mut want: Vec<&str> = CODEX_NOTES.to_vec();
        want.extend(CODEX_FALLBACK_NOTES);
        assert_eq!(fell_back.notes, want);
        assert!(
            fell_back
                .notes
                .iter()
                .any(|n| n == "conversation copies may remain in Codex's own database")
        );
        assert!(fell_back.notes.iter().any(|n| n.contains("subagent")));
        assert!(combined(true, "gemini", vec![removed]).notes.is_empty());
    }

    #[test]
    fn one_attempt_at_a_time_per_record() {
        let set = Arc::new(InFlight::default());
        let first = set.claim("f1").expect("free");
        assert!(!first.asked_again());
        assert!(set.claim("f1").is_none());
        // The refused claim asked the holder to go again, once.
        assert!(first.asked_again());
        assert!(!first.asked_again());
        assert!(set.claim("f2").is_some());
        drop(first);
        assert!(set.claim("f1").is_some());
    }
}
