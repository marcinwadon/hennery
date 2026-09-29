//! Collector session storage (ACP core §8). SQLite; writes are serialised by
//! the connection mutex (kernel §1's writer thread replaces it later).

use anyhow::Result;
use hennery_proto::frames::{AttachedSession, ConfigValue, Indexed, SessionBody, SessionConfig, TurnOutcome};
use hennery_proto::rest::{EventDto, SessionCatalog};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;

const MIGRATIONS: &[&str] = &[
    "
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
",
    // Teardown and reconciliation: a durable close intent (so a close whose
    // delivery is unknown is re-sent after the next handshake) and the turn
    // states reconciliation needs (sent → started → ended | not_delivered).
    "
    ALTER TABLE sessions ADD COLUMN close_requested INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE turns ADD COLUMN state TEXT NOT NULL DEFAULT 'sent';
    UPDATE turns SET state = 'ended' WHERE outcome IS NOT NULL;
    UPDATE turns SET state = 'started' WHERE outcome IS NULL AND EXISTS (
        SELECT 1 FROM events e
        WHERE e.kind = 'turn_started' AND json_extract(e.body, '$.turn_id') = turns.turn_id);
",
    // A host fact that is stored (its row is the (session_id, host_seq)
    // idempotency key) but did not apply — e.g. a real `turn_ended` after the
    // collector already synthesized the turn's end — is kept with
    // `applied = 0` and never listed (final review F1).
    "
    ALTER TABLE events ADD COLUMN applied INTEGER NOT NULL DEFAULT 1;
",
    // Resume: a session parked only because its host has been offline past
    // the threshold (ACP core §5.3). A presumption, not a fact: the host
    // may still run it, so it keeps its open turn.
    "
    ALTER TABLE sessions ADD COLUMN presumed_parked INTEGER NOT NULL DEFAULT 0;
",
    // Model, axes and mode (ACP core §8): the current values from the last
    // catalogue a host reported, which a resume re-applies, and the
    // catalogue itself, off the session list (P-23).
    "
    ALTER TABLE sessions ADD COLUMN model TEXT;
    ALTER TABLE sessions ADD COLUMN mode TEXT;
    ALTER TABLE sessions ADD COLUMN config_axes TEXT;
    CREATE TABLE session_catalog (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        config_options TEXT NOT NULL,
        updated_at TEXT NOT NULL);
",
];

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    pub lifecycle: String,
    pub activity: Option<String>,
    pub open_turn_id: Option<String>,
    pub failure_reason: Option<String>,
    /// The operator closed the session while it was attached and the host
    /// has not confirmed yet (ACP core §4.8).
    pub close_requested: bool,
    /// `parked` only because the host has been offline past the threshold
    /// (ACP core §5.3).
    pub presumed_parked: bool,
    /// The model, mode and other axes the host last reported as current;
    /// a resume re-applies them (ACP core §4.3).
    pub config: SessionConfig,
}

/// The outcome of `Store::request_resume`.
#[derive(Debug, PartialEq)]
pub enum ResumeRequest {
    /// The session is now `starting`: send `resume_session` with these.
    Starting {
        /// Collector events written (a released turn, `operator_resumed`).
        events: Vec<EventDto>,
        agent_session_id: String,
        committed_seq: u64,
        /// The stored config, re-applied by the host after the load.
        config: SessionConfig,
    },
    /// Refused: the session is `starting` or `active` (this lifecycle).
    Busy(String),
    /// The agent never created a session for it: there is nothing to load.
    NoRecord,
    NotFound,
}

/// What the collector did after a host's `resend_complete` (ACP core §5.1).
#[derive(Debug, Default, PartialEq)]
pub struct Reconciliation {
    /// Collector-originated events, in the order written.
    pub events: Vec<EventDto>,
    /// Attached sessions the operator has closed: send them `close_session`.
    pub close: Vec<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of the current time")
}

/// Write a collector-originated event (`host_seq` NULL, ACP core §8).
fn collector_event(tx: &Transaction<'_>, session_id: &str, kind: &str, body: Value, ts: &str) -> Result<EventDto> {
    tx.execute(
        "INSERT INTO events(session_id, host_seq, kind, body, ts) VALUES (?1, NULL, ?2, ?3, ?4)",
        params![session_id, kind, body.to_string(), ts],
    )?;
    tx.execute(
        "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1",
        params![session_id, ts],
    )?;
    Ok(EventDto {
        event_id: tx.last_insert_rowid(),
        session_id: session_id.to_string(),
        host_seq: None,
        kind: kind.to_string(),
        body,
        ts: ts.to_string(),
    })
}

/// Close an open turn that the host will never end, as `interrupted`.
fn synthesize_turn_end(tx: &Transaction<'_>, session_id: &str, turn_id: &str, ts: &str) -> Result<EventDto> {
    tx.execute(
        "UPDATE turns SET state = 'ended', outcome = 'interrupted' WHERE turn_id = ?1",
        [turn_id],
    )?;
    tx.execute(
        "UPDATE sessions SET open_turn_id = NULL, activity = 'idle' WHERE id = ?1 AND open_turn_id = ?2",
        params![session_id, turn_id],
    )?;
    collector_event(
        tx,
        session_id,
        "turn_ended_synthesized",
        json!({ "turn_id": turn_id, "outcome": "interrupted" }),
        ts,
    )
}

/// Release an open turn whose prompt never reached the adapter.
fn turn_not_delivered(tx: &Transaction<'_>, session_id: &str, turn_id: &str, ts: &str) -> Result<EventDto> {
    tx.execute("UPDATE turns SET state = 'not_delivered' WHERE turn_id = ?1", [turn_id])?;
    tx.execute(
        "UPDATE sessions SET open_turn_id = NULL, activity = 'idle' WHERE id = ?1 AND open_turn_id = ?2",
        params![session_id, turn_id],
    )?;
    collector_event(tx, session_id, "turn_not_delivered", json!({ "turn_id": turn_id }), ts)
}

/// Resolve an open turn the host will never end: `interrupted` if the
/// adapter had it (`started`), otherwise `turn_not_delivered`.
fn resolve_open_turn(tx: &Transaction<'_>, session_id: &str, turn_id: &str, ts: &str) -> Result<EventDto> {
    let state: Option<String> = tx
        .query_row("SELECT state FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
        .optional()?;
    match state.as_deref() {
        Some("started") => synthesize_turn_end(tx, session_id, turn_id, ts),
        _ => turn_not_delivered(tx, session_id, turn_id, ts),
    }
}

/// The host detached an active session (`session_parked`/`session_closed`).
/// A turn still open is one the host never acknowledged: its `not_attached`
/// answer is not outboxed and can be lost. Release it, or the next resume
/// inherits a permanent 409 (plan A, "After this plan").
fn release_turn_on_detach(tx: &Transaction<'_>, session_id: &str, ts: &str) -> Result<Option<EventDto>> {
    let row: Option<(String, Option<String>, bool)> = tx
        .query_row(
            "SELECT lifecycle, open_turn_id, presumed_parked FROM sessions WHERE id = ?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        Some((lifecycle, Some(turn), presumed)) if lifecycle == "active" || presumed => {
            Ok(Some(resolve_open_turn(tx, session_id, &turn, ts)?))
        }
        _ => Ok(None),
    }
}

/// Whether a host fact with no transition of its own (an update, a
/// diagnostic) still belongs on the timeline: not once the operator has
/// closed the session, and an update of a turn only while that turn is
/// open. An update for a turn the collector already ended (a synthesized
/// end) would otherwise be listed after that end.
fn fact_applies(tx: &Transaction<'_>, session_id: &str, turn_id: Option<&str>) -> Result<bool> {
    let lifecycle: String = tx.query_row("SELECT lifecycle FROM sessions WHERE id = ?1", [session_id], |r| {
        r.get(0)
    })?;
    if lifecycle == "closed" {
        return Ok(false);
    }
    let Some(turn_id) = turn_id else {
        return Ok(true);
    };
    let state: Option<String> = tx
        .query_row("SELECT state FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
        .optional()?;
    Ok(state.as_deref() == Some("started"))
}

/// `Store::close_now`'s body, inside the caller's transaction.
fn close_in(tx: &Transaction<'_>, session_id: &str) -> Result<Vec<EventDto>> {
    let row: Option<(String, bool, Option<String>)> = tx
        .query_row(
            "SELECT lifecycle, close_requested, open_turn_id FROM sessions WHERE id = ?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let mut events = Vec::new();
    if let Some((lifecycle, close_requested, open_turn)) = row
        && lifecycle != "closed"
    {
        let ts = now();
        if let Some(turn) = open_turn.as_deref() {
            events.push(resolve_open_turn(tx, session_id, turn, &ts)?);
        }
        if !close_requested {
            events.push(collector_event(tx, session_id, "operator_closed", json!({}), &ts)?);
        }
        tx.execute(
            "UPDATE sessions SET lifecycle = 'closed', activity = NULL, open_turn_id = NULL, close_requested = 0,
                 presumed_parked = 0
             WHERE id = ?1",
            [session_id],
        )?;
    }
    Ok(events)
}

/// Store the catalogue snapshot a fact's extracts carry (ACP core §3.2,
/// §8): the options for `GET …/catalog`, and the current values in the
/// session's `model`, `mode` and `config_axes`, which a resume re-applies.
/// Extracts without a snapshot (none, or an empty read-back) change
/// nothing: the stored values are never replaced by a guess (P-13).
fn store_catalogue(tx: &Transaction<'_>, session_id: &str, indexed: &Indexed, ts: &str) -> Result<()> {
    let Some(current) = indexed.current_config() else {
        return Ok(());
    };
    let options = indexed.config_options.clone().unwrap_or_default();
    tx.execute(
        "UPDATE sessions SET model = ?2, mode = ?3, config_axes = ?4 WHERE id = ?1",
        params![
            session_id,
            current.model,
            current.mode,
            serde_json::to_string(&current.axes)?
        ],
    )?;
    tx.execute(
        "INSERT INTO session_catalog(session_id, config_options, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id) DO UPDATE SET config_options = excluded.config_options, updated_at = excluded.updated_at",
        params![session_id, serde_json::to_string(&options)?, ts],
    )?;
    Ok(())
}

/// A session's `model`, `mode` and `config_axes` columns.
type ConfigColumns = (Option<String>, Option<String>, Option<String>);

/// A session's stored config, from its `model`, `mode` and `config_axes`.
fn stored_config((model, mode, axes): ConfigColumns) -> Result<SessionConfig> {
    let axes: BTreeMap<String, ConfigValue> = match axes {
        Some(axes) => serde_json::from_str(&axes)?,
        None => BTreeMap::new(),
    };
    Ok(SessionConfig { model, mode, axes })
}

/// Keep a stored host fact that did not apply as the idempotency key only:
/// it is hidden from `Store::events` (and so from SSE replay).
fn mark_unapplied(tx: &Transaction<'_>, event_id: i64) -> Result<()> {
    tx.execute("UPDATE events SET applied = 0 WHERE event_id = ?1", [event_id])?;
    Ok(())
}

/// Whether a `conflict` event with this exact `received` body is already
/// recorded for `(session_id, seq)` (fix round 1, ruling D): a re-sent
/// conflicting frame must not pile up a second `conflict` event.
fn conflict_already_recorded(tx: &Transaction<'_>, session_id: &str, seq: u64, received: &Value) -> Result<bool> {
    let mut stmt = tx.prepare(
        "SELECT body FROM events WHERE session_id = ?1 AND kind = 'conflict' AND json_extract(body, '$.seq') = ?2",
    )?;
    let mut rows = stmt.query(params![session_id, seq as i64])?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(0)?;
        let value: Value = serde_json::from_str(&body)?;
        if value["received"] == *received {
            return Ok(true);
        }
    }
    Ok(false)
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

    /// Like `mark_failed`, for a resume whose request failed: only a
    /// session still `starting` is failed. Whatever moved it on while the
    /// request was out (its `session_started`, a close, a newer resume's
    /// outcome) is left as it is.
    pub fn mark_failed_if_starting(&self, id: &str, reason: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2 WHERE id = ?1 AND lifecycle = 'starting'",
            params![id, reason],
        )?;
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        let row = self
            .conn()
            .query_row(
                "SELECT id, host_id, agent, cwd, lifecycle, activity, open_turn_id, failure_reason, close_requested,
                        presumed_parked, model, mode, config_axes
                 FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    let config: ConfigColumns = (r.get(10)?, r.get(11)?, r.get(12)?);
                    let row = SessionRow {
                        id: r.get(0)?,
                        host_id: r.get(1)?,
                        agent: r.get(2)?,
                        cwd: r.get(3)?,
                        lifecycle: r.get(4)?,
                        activity: r.get(5)?,
                        open_turn_id: r.get(6)?,
                        failure_reason: r.get(7)?,
                        close_requested: r.get(8)?,
                        presumed_parked: r.get(9)?,
                        config: SessionConfig::default(),
                    };
                    Ok((row, config))
                },
            )
            .optional()?;
        let Some((mut row, config)) = row else {
            return Ok(None);
        };
        row.config = stored_config(config)?;
        Ok(Some(row))
    }

    /// The session's config catalogue and current values (ACP core §9);
    /// `None` for an unknown session, an empty catalogue for one whose
    /// host has reported none.
    pub fn catalog(&self, session_id: &str) -> Result<Option<SessionCatalog>> {
        let row: Option<(ConfigColumns, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT s.model, s.mode, s.config_axes, c.config_options
                 FROM sessions s LEFT JOIN session_catalog c ON c.session_id = s.id WHERE s.id = ?1",
                [session_id],
                |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?)),
            )
            .optional()?;
        let Some((config, options)) = row else {
            return Ok(None);
        };
        Ok(Some(SessionCatalog {
            session_id: session_id.to_string(),
            config_options: match options {
                Some(options) => serde_json::from_str(&options)?,
                None => Vec::new(),
            },
            current: stored_config(config)?,
        }))
    }

    /// A turn's state: `sent`, `started`, `ended` or `not_delivered`.
    pub fn turn_state(&self, turn_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT state FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
            .optional()?)
    }

    /// How a turn ended, once it has (`None` while it is open, or if it was
    /// never delivered).
    pub fn ended_turn_outcome(&self, turn_id: &str) -> Result<Option<TurnOutcome>> {
        let outcome: Option<String> = self
            .conn()
            .query_row(
                "SELECT outcome FROM turns WHERE turn_id = ?1 AND state = 'ended'",
                [turn_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(match outcome {
            Some(outcome) => Some(serde_json::from_value(Value::String(outcome))?),
            None => None,
        })
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

    /// Record an operator park before `park_session` is sent.
    pub fn record_park_request(&self, session_id: &str) -> Result<EventDto> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let event = collector_event(&tx, session_id, "operator_parked", json!({}), &now())?;
        tx.commit()?;
        Ok(event)
    }

    /// Record an operator close of an attached session before
    /// `close_session` is sent. The intent is durable: if the host never
    /// confirms, the next handshake sends `close_session` again.
    pub fn record_close_request(&self, session_id: &str) -> Result<EventDto> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("UPDATE sessions SET close_requested = 1 WHERE id = ?1", [session_id])?;
        let event = collector_event(&tx, session_id, "operator_closed", json!({}), &now())?;
        tx.commit()?;
        Ok(event)
    }

    /// Close a session with no adapter the collector can reach (parked,
    /// failed, or its host offline) immediately (ACP core §4.8). Resolves
    /// any open turn first — `started` → `turn_ended_synthesized`,
    /// otherwise → `turn_not_delivered` (fix round 1, ruling C: a turn must
    /// not be left open under a closed session, or a later real
    /// `turn_ended` staying unapplied would mean it never gets an end at
    /// all) — then writes `operator_closed` unless a close request already
    /// recorded it. Idempotent: a closed session is left alone.
    pub fn close_now(&self, session_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let events = close_in(&tx, session_id)?;
        tx.commit()?;
        Ok(events)
    }

    /// A `close_session` that reconciliation sent was answered
    /// `not_attached`: close collector-side like `close_now`, but only while
    /// the session is still what reconciliation asked to close — attached
    /// (`active` or presumed parked) with the close still requested. A
    /// resume that began since (`starting`, close request cleared) is left
    /// alone: the rejection is about the old adapter, not the fresh start
    /// (final review F1). Checked and closed in one transaction.
    pub fn close_after_rejected_reconcile_close(&self, session_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let still_requested: bool = tx
            .query_row(
                "SELECT close_requested = 1 AND (lifecycle = 'active' OR presumed_parked = 1)
                 FROM sessions WHERE id = ?1",
                [session_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let events = if still_requested {
            close_in(&tx, session_id)?
        } else {
            Vec::new()
        };
        tx.commit()?;
        Ok(events)
    }

    /// Move a `parked`, `closed` or `failed` session to `starting` for a
    /// resume (ACP core §4.2). Atomic: of two concurrent resumes, the second
    /// sees `starting` and is refused (§12 scenario 11). A turn still open
    /// (a database written before plan B) is released first.
    pub fn request_resume(&self, session_id: &str) -> Result<ResumeRequest> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, Option<String>, Option<String>, ConfigColumns)> = tx
            .query_row(
                "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes FROM sessions WHERE id = ?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, (r.get(3)?, r.get(4)?, r.get(5)?))),
            )
            .optional()?;
        let Some((lifecycle, agent_session_id, open_turn, config)) = row else {
            return Ok(ResumeRequest::NotFound);
        };
        if !matches!(lifecycle.as_str(), "parked" | "closed" | "failed") {
            return Ok(ResumeRequest::Busy(lifecycle));
        }
        let Some(agent_session_id) = agent_session_id else {
            return Ok(ResumeRequest::NoRecord);
        };
        let ts = now();
        let mut events = Vec::new();
        if let Some(turn) = open_turn.as_deref() {
            events.push(resolve_open_turn(&tx, session_id, turn, &ts)?);
        }
        events.push(collector_event(&tx, session_id, "operator_resumed", json!({}), &ts)?);
        tx.execute(
            "UPDATE sessions SET lifecycle = 'starting', activity = NULL, failure_reason = NULL,
                 close_requested = 0, open_turn_id = NULL, presumed_parked = 0
             WHERE id = ?1",
            [session_id],
        )?;
        let committed: Option<i64> = tx.query_row(
            "SELECT MAX(host_seq) FROM events WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(ResumeRequest::Starting {
            events,
            agent_session_id,
            committed_seq: committed.unwrap_or(0) as u64,
            config: stored_config(config)?,
        })
    }

    /// The host has been offline past the threshold: presume its `active`
    /// sessions parked (ACP core §5.3). Their open turns stay open, since
    /// the host may still be running them.
    pub fn presume_parked(&self, host_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT id FROM sessions WHERE host_id = ?1 AND lifecycle = 'active' ORDER BY id")?;
            let rows = stmt.query_map([host_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut events = Vec::new();
        for id in &ids {
            events.push(collector_event(
                &tx,
                id,
                "presumed_parked",
                json!({ "reason": "host_offline" }),
                &ts,
            )?);
            tx.execute(
                "UPDATE sessions SET lifecycle = 'parked', presumed_parked = 1 WHERE id = ?1",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(events)
    }

    /// Hosts the collector believes are running at least one session.
    pub fn hosts_with_active_sessions(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT DISTINCT host_id FROM sessions WHERE lifecycle = 'active' ORDER BY host_id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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

    /// Ingest one sequenced host frame. Idempotent on (session_id, seq): a
    /// duplicate with the same body is discarded; one with a different body
    /// is kept as a `conflict` event (ACP core §3.6). Returns the events it
    /// created, in order.
    pub fn ingest(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let kind = body_kind(body);
        let received = serde_json::to_value(body)?;
        let inserted = tx.execute(
            "INSERT INTO events(session_id, host_seq, kind, body, ts) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, host_seq) DO NOTHING",
            params![session_id, seq as i64, kind, received.to_string(), ts],
        )?;
        if inserted == 0 {
            let stored: String = tx.query_row(
                "SELECT body FROM events WHERE session_id = ?1 AND host_seq = ?2",
                params![session_id, seq as i64],
                |r| r.get(0),
            )?;
            // Structural comparison: key order is not stable across builds
            // (serde_json's `preserve_order` is feature-unified).
            let created = if serde_json::from_str::<Value>(&stored)? == received {
                Vec::new()
            } else if conflict_already_recorded(&tx, session_id, seq, &received)? {
                // A re-sent conflicting frame: already on record, ruling D.
                Vec::new()
            } else {
                vec![collector_event(
                    &tx,
                    session_id,
                    "conflict",
                    json!({ "seq": seq, "received": received }),
                    &ts,
                )?]
            };
            tx.commit()?;
            return Ok(created);
        }
        let fact_id = tx.last_insert_rowid();
        let mut created = vec![EventDto {
            event_id: fact_id,
            session_id: session_id.to_string(),
            host_seq: Some(seq),
            kind: kind.to_string(),
            body: received,
            ts: ts.clone(),
        }];
        match body {
            SessionBody::SessionStarted {
                agent_session_id,
                indexed,
                ..
            } => {
                // A re-emitted `session_started` for a session already
                // active (a retried start or resume) changes nothing. It
                // also clears a stale `failure_reason`: a start reconciled
                // as `start_not_delivered` that in fact ran must not stay
                // `active` with that guess still attached.
                let changed = tx.execute(
                    "UPDATE sessions SET lifecycle = 'active', activity = 'idle', agent_session_id = ?2,
                         failure_reason = NULL
                     WHERE id = ?1 AND lifecycle IN ('starting', 'failed')",
                    params![session_id, agent_session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    // The catalogue after the start's switches (P-13).
                    store_catalogue(&tx, session_id, indexed, &ts)?;
                }
            }
            SessionBody::StartFailed { code, .. } => {
                // Also applies from `failed` when the recorded reason is the
                // collector's own guess (`start_not_delivered`, reconciled
                // after no answer ever came, ACP core §5.1 step 4): the
                // host's real failure code replaces that guess rather than
                // being swallowed as a fact that "changes nothing" (ACP core
                // §4.2 stores the `start_failed` reason; resume plan
                // decision 3 sets `failed` with that code).
                let changed = tx.execute(
                    "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2
                     WHERE id = ?1 AND (lifecycle = 'starting'
                         OR (lifecycle = 'failed' AND failure_reason = 'start_not_delivered'))",
                    params![session_id, code],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::TurnStarted { turn_id, .. } => {
                // The fact wins over reconciliation, but never over a fact
                // already resolved (fix round 1, ruling A, extending
                // decision 2): a late `turn_started` applies only when the
                // session is still active and `turn_id`'s own state is
                // still `sent` or `not_delivered` — it never reopens a turn
                // that has already `ended`.
                let turn_state: Option<String> = tx
                    .query_row("SELECT state FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
                    .optional()?;
                let (lifecycle, slot, presumed): (String, Option<String>, bool) = tx.query_row(
                    "SELECT lifecycle, open_turn_id, presumed_parked FROM sessions WHERE id = ?1",
                    [session_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                // A presumed-parked session is still attached as far as its
                // host's facts go (ACP core §5.3).
                let applies = (lifecycle == "active" || presumed)
                    && matches!(turn_state.as_deref(), Some("sent") | Some("not_delivered"));
                let mut takes_slot = false;
                if applies {
                    takes_slot = match slot.as_deref() {
                        None => true,
                        Some(t) if t == turn_id => true,
                        Some(other) => {
                            // The host serialises turns: if the one holding
                            // the slot never started, `turn_id`'s late fact
                            // takes the slot back and that turn is released
                            // as `turn_not_delivered` — before `turn_id`'s
                            // own `user_turn` (timeline order).
                            let other_state: Option<String> = tx
                                .query_row("SELECT state FROM turns WHERE turn_id = ?1", [other], |r| r.get(0))
                                .optional()?;
                            if other_state.as_deref() == Some("sent") {
                                created.push(turn_not_delivered(&tx, session_id, other, &ts)?);
                                true
                            } else {
                                // The slot holds a turn already `started`:
                                // store the fact, apply nothing.
                                false
                            }
                        }
                    };
                }
                if takes_slot {
                    tx.execute(
                        "UPDATE sessions SET activity = 'running', open_turn_id = ?2 WHERE id = ?1",
                        params![session_id, turn_id],
                    )?;
                    tx.execute("UPDATE turns SET state = 'started' WHERE turn_id = ?1", [turn_id])?;
                    // The user's turn is recorded only once the adapter has
                    // it (ACP core §4.4), in seq order before the turn's
                    // updates.
                    let content: Option<String> = tx
                        .query_row("SELECT content FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
                        .optional()?;
                    if let Some(content) = content {
                        let body = json!({ "turn_id": turn_id, "content": serde_json::from_str::<Value>(&content)? });
                        created.push(collector_event(&tx, session_id, "user_turn", body, &ts)?);
                    }
                } else {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
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
                // Only a `started` turn can be ended by a real `turn_ended`
                // (fix round 1, ruling B): a stray end must not jump a
                // `sent`/`not_delivered` turn straight to `ended`.
                tx.execute(
                    "UPDATE turns SET state = 'ended', outcome = ?2 WHERE turn_id = ?1 AND state = 'started'",
                    params![turn_id, serde_json::to_value(outcome)?.as_str().unwrap_or_default()],
                )?;
                if applied == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::SessionParked { .. } => {
                created.extend(release_turn_on_detach(&tx, session_id, &ts)?);
                // A park that overtakes an operator close ends the session
                // as the operator asked: closed.
                let changed = tx.execute(
                    "UPDATE sessions SET
                         lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                         activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::SessionClosed => {
                created.extend(release_turn_on_detach(&tx, session_id, &ts)?);
                // Also the host's confirmation of a close the collector
                // already made (an offline close): nothing left to change.
                let changed = tx.execute(
                    "UPDATE sessions SET lifecycle = 'closed', activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::AcpUpdate { indexed, .. } => {
                if !fact_applies(&tx, session_id, indexed.turn_id.as_deref())? {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    // A live `config_option_update` (the agent changed its
                    // own config); the host never sends a replayed one
                    // with extracts.
                    store_catalogue(&tx, session_id, indexed, &ts)?;
                }
            }
            SessionBody::ConfigApplied { indexed, .. } => {
                // The read-back of a switch on an attached session. A late
                // one for a session that has detached since changes nothing.
                let (lifecycle, presumed): (String, bool) = tx.query_row(
                    "SELECT lifecycle, presumed_parked FROM sessions WHERE id = ?1",
                    [session_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if lifecycle == "active" || presumed {
                    store_catalogue(&tx, session_id, indexed, &ts)?;
                } else {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            // Diagnostics only, with no transition of their own: an
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } => {
                if !fact_applies(&tx, session_id, None)? {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
        }
        tx.execute(
            "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1",
            params![session_id, ts],
        )?;
        tx.commit()?;
        Ok(created)
    }

    /// Reconcile a host's sessions after its `resend_complete` (ACP core
    /// §5.1 step 4, §5.2). Everything the host had in its outbox has been
    /// ingested by now, so anything still unresolved never happened:
    ///
    /// - `starting`, not attached → `failed{start_not_delivered}`;
    /// - `active`, not attached → the host restarted: `host_restarted`,
    ///   parked (or closed, if the operator asked), its open turn ended;
    /// - an open turn the host does not report: `turn_not_delivered` if it
    ///   never started, `turn_ended_synthesized{interrupted}` if it did;
    /// - attached sessions the operator closed → returned in `close`.
    pub fn reconcile_host(&self, host_id: &str, attached: &[AttachedSession]) -> Result<Reconciliation> {
        let listed: HashMap<&str, &AttachedSession> = attached.iter().map(|a| (a.session_id.as_str(), a)).collect();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let rows: Vec<(String, String, Option<String>, bool, bool)> = {
            let mut stmt = tx.prepare(
                "SELECT id, lifecycle, open_turn_id, close_requested, presumed_parked FROM sessions
                 WHERE host_id = ?1 AND (lifecycle IN ('starting', 'active', 'closed') OR presumed_parked = 1)
                 ORDER BY id",
            )?;
            let rows = stmt.query_map([host_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Reconciliation::default();
        for (id, lifecycle, open_turn, close_requested, presumed) in rows {
            let host = listed.get(id.as_str());
            // A presumed park was a guess made while the host was away: now
            // that it is back, treat the session as the active one it may
            // still be (ACP core §5.1 step 4, §5.3).
            let lifecycle = if presumed { "active" } else { lifecycle.as_str() };
            match (lifecycle, host) {
                ("starting", None) => {
                    out.events
                        .push(collector_event(&tx, &id, "start_not_delivered", json!({}), &ts)?);
                    tx.execute(
                        "UPDATE sessions SET lifecycle = 'failed', failure_reason = 'start_not_delivered' WHERE id = ?1",
                        [&id],
                    )?;
                }
                ("active", _) => {
                    if host.is_none() {
                        out.events
                            .push(collector_event(&tx, &id, "host_restarted", json!({}), &ts)?);
                    } else if presumed {
                        out.events
                            .push(collector_event(&tx, &id, "reattached", json!({}), &ts)?);
                        tx.execute(
                            "UPDATE sessions SET lifecycle = 'active', presumed_parked = 0 WHERE id = ?1",
                            [&id],
                        )?;
                    }
                    let host_turn = host.and_then(|a| a.open_turn_id.as_deref());
                    if let Some(turn) = open_turn.as_deref()
                        && host_turn != Some(turn)
                    {
                        out.events.push(resolve_open_turn(&tx, &id, turn, &ts)?);
                    }
                    if host.is_none() {
                        tx.execute(
                            "UPDATE sessions SET
                                 lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                                 activity = NULL, open_turn_id = NULL, close_requested = 0, presumed_parked = 0
                             WHERE id = ?1",
                            [&id],
                        )?;
                    } else if close_requested {
                        out.close.push(id);
                    }
                }
                ("closed", Some(_)) => out.close.push(id),
                _ => {}
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Events of one session with `event_id > after`, oldest first. Host
    /// facts stored but not applied are left out (final review F1).
    pub fn events(&self, session_id: &str, after: i64, limit: u32) -> Result<Vec<EventDto>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT event_id, host_seq, kind, body, ts FROM events
             WHERE session_id = ?1 AND event_id > ?2 AND applied = 1 ORDER BY event_id LIMIT ?3",
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
        SessionBody::HostNote { .. } => "host_note",
        SessionBody::ConfigApplied { .. } => "config_applied",
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
