//! Collector session storage (ACP core §8). SQLite; writes are serialised by
//! the connection mutex (kernel §1's writer thread replaces it later).

use anyhow::Result;
use hennery_proto::frames::SessionBody;
use hennery_proto::rest::EventDto;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

const MIGRATIONS: &[&str] = &["
    CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        host_id TEXT NOT NULL,
        agent TEXT NOT NULL,
        cwd TEXT NOT NULL,
        agent_session_id TEXT,
        lifecycle TEXT NOT NULL,
        activity TEXT,
        failure_reason TEXT,
        open_turn_id TEXT,
        created_at TEXT NOT NULL,
        last_event_at TEXT NOT NULL);
    CREATE TABLE turns (
        turn_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        content TEXT NOT NULL,
        outcome TEXT,
        created_at TEXT NOT NULL);
    CREATE TABLE events (
        event_id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        host_seq INTEGER,
        kind TEXT NOT NULL,
        body TEXT NOT NULL,
        ts TEXT NOT NULL,
        UNIQUE(session_id, host_seq));
"];

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    pub lifecycle: String,
    pub activity: Option<String>,
    pub open_turn_id: Option<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of the current time")
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(hennery_kernel::db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(hennery_kernel::db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        hennery_kernel::db::migrate(&mut conn, MIGRATIONS)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store lock")
    }

    pub fn create_session(&self, id: &str, host_id: &str, agent: &str, cwd: &str) -> Result<()> {
        let ts = now();
        self.conn().execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at)
             VALUES (?1, ?2, ?3, ?4, 'starting', ?5, ?5)",
            params![id, host_id, agent, cwd, ts],
        )?;
        Ok(())
    }

    pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2 WHERE id = ?1",
            params![id, reason],
        )?;
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, host_id, agent, cwd, lifecycle, activity, open_turn_id FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        host_id: r.get(1)?,
                        agent: r.get(2)?,
                        cwd: r.get(3)?,
                        lifecycle: r.get(4)?,
                        activity: r.get(5)?,
                        open_turn_id: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    /// Open a turn if the session is active and has none open. Returns false
    /// when a turn is already in flight (ACP core §4.4: one turn at a time).
    pub fn open_turn(&self, session_id: &str, turn_id: &str, content: &[Value]) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE sessions SET open_turn_id = ?2
             WHERE id = ?1 AND lifecycle = 'active' AND open_turn_id IS NULL",
            params![session_id, turn_id],
        )?;
        if changed == 1 {
            tx.execute(
                "INSERT INTO turns(turn_id, session_id, content, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![turn_id, session_id, serde_json::to_string(content)?, now()],
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Undo `open_turn` after the host rejected the prompt.
    pub fn abandon_turn(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET open_turn_id = NULL WHERE id = ?1 AND open_turn_id = ?2",
            params![session_id, turn_id],
        )?;
        tx.execute("DELETE FROM turns WHERE turn_id = ?1", [turn_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Highest committed host seq for a session (0 if none).
    pub fn committed_seq(&self, session_id: &str) -> Result<u64> {
        let v: Option<i64> = self.conn().query_row(
            "SELECT MAX(host_seq) FROM events WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        Ok(v.unwrap_or(0) as u64)
    }

    /// Ingest one sequenced host frame. Idempotent on (session_id, seq).
    /// Returns the events it created (empty for a duplicate), in order.
    pub fn ingest(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let kind = body_kind(body);
        let inserted = tx.execute(
            "INSERT INTO events(session_id, host_seq, kind, body, ts) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, host_seq) DO NOTHING",
            params![session_id, seq as i64, kind, serde_json::to_string(body)?, ts],
        )?;
        if inserted == 0 {
            tx.commit()?;
            return Ok(Vec::new());
        }
        let mut created = vec![EventDto {
            event_id: tx.last_insert_rowid(),
            session_id: session_id.to_string(),
            host_seq: Some(seq),
            kind: kind.to_string(),
            body: serde_json::to_value(body)?,
            ts: ts.clone(),
        }];
        match body {
            SessionBody::SessionStarted { agent_session_id, .. } => {
                tx.execute(
                    "UPDATE sessions SET lifecycle = 'active', activity = 'idle', agent_session_id = ?2
                     WHERE id = ?1 AND lifecycle IN ('starting', 'failed')",
                    params![session_id, agent_session_id],
                )?;
            }
            SessionBody::StartFailed { code, .. } => {
                tx.execute(
                    "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2 WHERE id = ?1 AND lifecycle = 'starting'",
                    params![session_id, code],
                )?;
            }
            SessionBody::TurnStarted { turn_id, .. } => {
                tx.execute("UPDATE sessions SET activity = 'running' WHERE id = ?1", [session_id])?;
                // The user's turn is recorded only once the adapter has it
                // (ACP core §4.4), in seq order before the turn's updates.
                let content: Option<String> = tx
                    .query_row("SELECT content FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
                    .optional()?;
                if let Some(content) = content {
                    let body =
                        serde_json::json!({ "turn_id": turn_id, "content": serde_json::from_str::<Value>(&content)? });
                    tx.execute(
                        "INSERT INTO events(session_id, host_seq, kind, body, ts) VALUES (?1, NULL, 'user_turn', ?2, ?3)",
                        params![session_id, body.to_string(), ts],
                    )?;
                    created.push(EventDto {
                        event_id: tx.last_insert_rowid(),
                        session_id: session_id.to_string(),
                        host_seq: None,
                        kind: "user_turn".into(),
                        body,
                        ts: ts.clone(),
                    });
                }
            }
            SessionBody::TurnEnded { turn_id, outcome, .. } => {
                // Applied only to the open turn; a late duplicate for an
                // already-ended turn is stored but not applied, and (ACP core
                // §4.4) must never be pushed to a caller — only the store
                // knows whether the transition actually applied.
                let applied = tx.execute(
                    "UPDATE sessions SET open_turn_id = NULL, activity = 'idle' WHERE id = ?1 AND open_turn_id = ?2",
                    params![session_id, turn_id],
                )?;
                tx.execute(
                    "UPDATE turns SET outcome = ?2 WHERE turn_id = ?1 AND outcome IS NULL",
                    params![turn_id, serde_json::to_value(outcome)?.as_str().unwrap_or_default()],
                )?;
                if applied == 0 {
                    created.clear();
                }
            }
            // Stored on the timeline; their state transitions land with
            // reconciliation.
            SessionBody::AcpUpdate { .. }
            | SessionBody::SessionParked { .. }
            | SessionBody::SessionClosed
            | SessionBody::AdapterExited { .. } => {}
        }
        tx.execute(
            "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1",
            params![session_id, ts],
        )?;
        tx.commit()?;
        Ok(created)
    }

    /// Events of one session with `event_id > after`, oldest first.
    pub fn events(&self, session_id: &str, after: i64, limit: u32) -> Result<Vec<EventDto>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT event_id, host_seq, kind, body, ts FROM events
             WHERE session_id = ?1 AND event_id > ?2 ORDER BY event_id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![session_id, after, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, host_seq, kind, body, ts) = row?;
            out.push(EventDto {
                event_id,
                session_id: session_id.to_string(),
                host_seq: host_seq.map(|s| s as u64),
                kind,
                body: serde_json::from_str(&body)?,
                ts,
            });
        }
        Ok(out)
    }
}

fn body_kind(body: &SessionBody) -> &'static str {
    match body {
        SessionBody::SessionStarted { .. } => "session_started",
        SessionBody::StartFailed { .. } => "start_failed",
        SessionBody::TurnStarted { .. } => "turn_started",
        SessionBody::AcpUpdate { .. } => "acp_update",
        SessionBody::TurnEnded { .. } => "turn_ended",
        SessionBody::SessionParked { .. } => "session_parked",
        SessionBody::SessionClosed => "session_closed",
        SessionBody::AdapterExited { .. } => "adapter_exited",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamps_are_rfc3339_utc() {
        let ts = super::now();
        assert!(ts.ends_with('Z') && ts.as_bytes()[10] == b'T', "{ts}");
    }
}
