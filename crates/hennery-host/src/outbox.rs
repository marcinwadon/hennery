//! On-disk outbox for sequenced session frames (ACP core §5.5).
//!
//! Every session frame is written here before it is sent, and deleted only
//! when the collector acks it. Sequence numbers are persisted, so a restarted
//! host continues a session's numbering instead of reusing seqs.

use anyhow::Result;
use hennery_proto::frames::{HostFrame, SessionBody};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub struct Outbox {
    conn: Connection,
}

impl Outbox {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS outbox (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 seq INTEGER NOT NULL,
                 frame TEXT NOT NULL,
                 UNIQUE(session_id, seq));
             CREATE TABLE IF NOT EXISTS seqs (
                 session_id TEXT PRIMARY KEY,
                 last_seq INTEGER NOT NULL);",
        )?;
        Ok(Self { conn })
    }

    /// Assign the next seq for `session_id`, persist the frame, return it.
    pub fn enqueue(&mut self, session_id: &str, body: SessionBody) -> Result<HostFrame> {
        let tx = self.conn.transaction()?;
        let last: i64 = tx
            .query_row("SELECT last_seq FROM seqs WHERE session_id = ?1", [session_id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(0);
        let seq = last as u64 + 1;
        let frame = HostFrame::Session {
            session_id: session_id.to_string(),
            seq,
            body,
        };
        tx.execute(
            "INSERT INTO seqs(session_id, last_seq) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET last_seq = excluded.last_seq",
            params![session_id, seq as i64],
        )?;
        tx.execute(
            "INSERT INTO outbox(session_id, seq, frame) VALUES (?1, ?2, ?3)",
            params![session_id, seq as i64, serde_json::to_string(&frame)?],
        )?;
        tx.commit()?;
        Ok(frame)
    }

    /// Every unacked frame, in insertion order (which is per-session seq order).
    pub fn pending(&self) -> Result<Vec<HostFrame>> {
        let mut stmt = self.conn.prepare("SELECT frame FROM outbox ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Delete every frame of `session_id` with seq <= `ack_seq`.
    pub fn ack(&mut self, session_id: &str, ack_seq: u64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM outbox WHERE session_id = ?1 AND seq <= ?2",
            params![session_id, ack_seq as i64],
        )?)
    }

    /// Raise the counter for `session_id` to at least `seq`. Used when the
    /// collector has committed more than this outbox remembers (outbox file
    /// lost or restored from an old backup), so new frames never reuse seqs.
    pub fn fast_forward(&mut self, session_id: &str, seq: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO seqs(session_id, last_seq) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET last_seq = MAX(last_seq, excluded.last_seq)",
            params![session_id, seq as i64],
        )?;
        Ok(())
    }

    /// Highest seq ever assigned for `session_id` (0 if none).
    pub fn last_seq(&self, session_id: &str) -> Result<u64> {
        let last: Option<i64> = self
            .conn
            .query_row("SELECT last_seq FROM seqs WHERE session_id = ?1", [session_id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(last.unwrap_or(0) as u64)
    }
}
