//! Collector session storage (ACP core §8). SQLite; writes are serialised by
//! the connection mutex (kernel §1's writer thread replaces it later).
//!
//! Every row carries `owner_id` and every query names it (kernel spec §1):
//! the database's owner, bound when the store opens (`Store::init`).
//! Another owner's sessions, turns, events and questions are not there for
//! this store: it reads none of them, changes none, and writes nothing
//! for them.

use crate::content::{Checked, Image};
use anyhow::{Context, Result};
use hennery_gateway::revocation::Cut;
use hennery_gateway::session::{NoSessionMcp, SessionMcp, SessionRef};
use hennery_proto::frames::{
    AttachedSession, CollectorFrame, ConfigValue, ElicitationAction, Indexed, McpIsolation, McpServer, ParkReason,
    PendingKind, PendingReason, PendingResolution, SessionBody, SessionConfig, TurnOutcome,
};
use hennery_proto::rest::{
    AnswerRequest, AttachmentUsage, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, EventDto, McpSessionDelivery,
    McpSessionDeliveryMode, PendingItem, PendingState, SessionCatalog, SessionItem, SessionPage, TITLE_MAX_CHARS,
    TITLE_MAX_JSON_BYTES, json_char_width, mcp_session_delivery,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

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
    // Permission and elicitation (ACP core §4.6, §8): the pending set, which
    // is canonical here, and the durable answer queue, keyed by pending_id.
    // `delivered` stays NULL until a verdict: `answer_result`, or the
    // question's cancellation (which gives `false`, decision 10). A host's
    // refusal of the answer's request is logged and is no verdict (amended
    // decision 10): `delivered` stays NULL, and the answer goes again after
    // the next handshake while the question is open.
    "
    CREATE TABLE pending (
        pending_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        kind TEXT NOT NULL,
        turn_id TEXT,
        option_ids TEXT,
        payload TEXT NOT NULL,
        state TEXT NOT NULL,
        reason TEXT,
        opened_at TEXT NOT NULL,
        resolved_at TEXT);
    CREATE INDEX pending_by_session ON pending(session_id, state);
    CREATE TABLE answer_queue (
        pending_id TEXT PRIMARY KEY REFERENCES pending(pending_id),
        session_id TEXT NOT NULL REFERENCES sessions(id),
        request_id TEXT NOT NULL UNIQUE,
        answer TEXT NOT NULL,
        submitted_at TEXT NOT NULL,
        delivered INTEGER);
",
    // `owner_id` everywhere (plan 3b-iii decision 4), filled with the
    // database's owner, which the kernel's migrations made (`Store::init`
    // runs them first). No foreign key: SQLite adds a REFERENCES column only
    // with a NULL default, and rebuilding six tables that reference each
    // other needs foreign keys off, which a migration's transaction cannot
    // turn off. The default names no owner, so a row written without one is
    // invisible.
    "
    ALTER TABLE sessions ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE turns ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE events ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE session_catalog ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE pending ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE answer_queue ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
    UPDATE sessions SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
    UPDATE turns SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
    UPDATE events SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
    UPDATE session_catalog SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
    UPDATE pending SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
    UPDATE answer_queue SET owner_id = (SELECT id FROM owners ORDER BY created_at, id LIMIT 1);
",
    // Image attachments (ACP core §7, §8; plan 6a): one row per owner and
    // image, whose bytes are the file `attachments/<sha256>` beside the
    // database, and one row per image block of a `user_turn` event, at the
    // block's index in its content. Keyed by owner and hash (decision 5),
    // so another owner's copy of an image is a row of its own.
    "
    CREATE TABLE attachments (
        owner_id TEXT NOT NULL REFERENCES owners(id),
        sha256 TEXT NOT NULL,
        mime TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (owner_id, sha256));
    CREATE TABLE event_attachments (
        event_id INTEGER NOT NULL REFERENCES events(event_id),
        sha256 TEXT NOT NULL,
        position INTEGER NOT NULL,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        PRIMARY KEY (event_id, position),
        FOREIGN KEY (owner_id, sha256) REFERENCES attachments(owner_id, sha256));
    CREATE INDEX event_attachments_by_image ON event_attachments(owner_id, sha256);
",
    // The session list and its extracts (plan 6b, ACP core §8): the title
    // and the git state the host reports, the session's recency (the list's
    // sort key: the time and id of its last listed event), and the latest
    // slash commands, off the list (P-23). Recency is rewritten to one
    // fixed width, so text order is time order (decision 5); a value that
    // is not a time is left as it is. The git columns are filled from
    // `git_state` (6b-ii).
    "
    ALTER TABLE sessions ADD COLUMN title TEXT;
    ALTER TABLE sessions ADD COLUMN git_branch TEXT;
    ALTER TABLE sessions ADD COLUMN git_dirty INTEGER;
    ALTER TABLE sessions ADD COLUMN git_worktree INTEGER;
    ALTER TABLE sessions ADD COLUMN base_commit TEXT;
    ALTER TABLE sessions ADD COLUMN last_event_id INTEGER;
    ALTER TABLE session_catalog ADD COLUMN commands TEXT;
    UPDATE sessions SET last_event_at = COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ', last_event_at), last_event_at);
    UPDATE sessions SET last_event_id = (
        SELECT MAX(e.event_id) FROM events e
        WHERE e.session_id = sessions.id AND e.applied = 1 AND e.owner_id = sessions.owner_id);
    CREATE INDEX sessions_by_recency ON sessions(owner_id, last_event_at DESC, id DESC);
",
    // Hats (umbrella §8.2; plan 5c decision 1): the hat a session belongs
    // to, decided once at its start, and the rule that decided it (none:
    // its host's default hat), for audit. A session from before hats gets
    // its host's default hat, or the owner's default for new hosts if its
    // host is gone; the kernel's migrations ran first (`Store::init`). The
    // list filtered by hat walks an index of its own (plan 6b's "After this
    // plan").
    "
    ALTER TABLE sessions ADD COLUMN hat_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE sessions ADD COLUMN hat_rule_id TEXT;
    UPDATE sessions SET hat_id = COALESCE(
        (SELECT h.default_hat_id FROM hosts h WHERE h.id = sessions.host_id AND h.owner_id = sessions.owner_id),
        (SELECT s.value FROM settings s WHERE s.owner_id = sessions.owner_id AND s.key = 'default_hat_id'),
        '');
    CREATE INDEX sessions_by_hat ON sessions(owner_id, hat_id, last_event_at DESC, id DESC);
",
    // Web Push (plan 10b-iii; its review's A3): the event that opened each
    // question, so a withdrawal is ordered against the owner's last prompt
    // by id, without a scan of the session's events; and the index that
    // finds a session's latest event of a kind. Questions from before have
    // none, and never count as withdrawn since a prompt.
    "
    ALTER TABLE pending ADD COLUMN opened_event_id INTEGER;
    CREATE INDEX events_by_kind ON events(session_id, kind, event_id);
",
    // A session's events in order, from an id on: a page of a stream's
    // replay or of the timeline reads only its own rows, with no sort
    // (smoke test #1, F2; without it each page read every later row of the
    // session and sorted them, under the store's lock). `IF NOT EXISTS`:
    // the tests that roll a database back to an older version run it again.
    "
    CREATE INDEX IF NOT EXISTS events_by_session ON events(session_id, event_id);
",
    // Session delete (ACP core §4.10; plan 9a decisions 1, 2 and 6, A5,
    // A9). A turn's own links to the images it shows, as `event_attachments`
    // are an event's, so an image a turn shows that never started (no
    // `user_turn`) is still referenced; they go with the turn. Backfilled
    // from each turn's content as `link_attachments` links an event: an
    // image block with a hash, at its index, where the owner's row exists.
    // A block that is no object is never read as JSON (`CASE` decides
    // before `json_extract` runs; `AND` does not promise an order). Whether
    // any owner still names a file (`shared_files`) walks its hash's index.
    //
    // A deleted session (`lifecycle = 'deleted'`) is a tombstone and never
    // comes back: nothing is added for it, and its row is never changed
    // again. The store's own writers check first, so a racing writer gets
    // a typed answer; these triggers are the schema's word. A later
    // migration that UPDATEs `sessions` must exclude tombstones
    // (`lifecycle <> 'deleted'`), or this trigger aborts it.
    "
    CREATE TABLE turn_attachments (
        turn_id TEXT NOT NULL REFERENCES turns(turn_id) ON DELETE CASCADE,
        sha256 TEXT NOT NULL,
        position INTEGER NOT NULL,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        PRIMARY KEY (turn_id, position),
        FOREIGN KEY (owner_id, sha256) REFERENCES attachments(owner_id, sha256));
    CREATE INDEX turn_attachments_by_image ON turn_attachments(owner_id, sha256);
    CREATE INDEX attachments_by_hash ON attachments(sha256);
    INSERT INTO turn_attachments(turn_id, sha256, position, owner_id)
        SELECT turn_id, sha256, position, owner_id FROM (
            SELECT t.turn_id, t.owner_id, b.key AS position,
                CASE WHEN b.type = 'object' THEN
                    CASE WHEN json_extract(b.value, '$.type') = 'image' AND json_type(b.value, '$.sha256') = 'text'
                        THEN json_extract(b.value, '$.sha256') END
                END AS sha256
            FROM turns t, json_each(t.content) b
            WHERE json_type(t.content) = 'array') linked
        WHERE sha256 IS NOT NULL
            AND EXISTS (SELECT 1 FROM attachments a WHERE a.owner_id = linked.owner_id AND a.sha256 = linked.sha256);
    CREATE TRIGGER events_of_a_tombstone BEFORE INSERT ON events
        WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
        BEGIN SELECT RAISE(ABORT, 'a deleted session gets no events'); END;
    CREATE TRIGGER turns_of_a_tombstone BEFORE INSERT ON turns
        WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
        BEGIN SELECT RAISE(ABORT, 'a deleted session gets no turns'); END;
    CREATE TRIGGER pending_of_a_tombstone BEFORE INSERT ON pending
        WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
        BEGIN SELECT RAISE(ABORT, 'a deleted session gets no questions'); END;
    CREATE TRIGGER answers_of_a_tombstone BEFORE INSERT ON answer_queue
        WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
        BEGIN SELECT RAISE(ABORT, 'a deleted session gets no answers'); END;
    CREATE TRIGGER catalog_of_a_tombstone BEFORE INSERT ON session_catalog
        WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
        BEGIN SELECT RAISE(ABORT, 'a deleted session gets no catalogue'); END;
    CREATE TRIGGER a_tombstone_stays BEFORE UPDATE ON sessions
        WHEN OLD.lifecycle = 'deleted'
        BEGIN SELECT RAISE(ABORT, 'a deleted session is never changed'); END;
",
    // Plan 8e decision E10: what the session's latest start or resume was
    // given of the gateway, for its detail: the mode, how many servers,
    // when. Never a server, a header or a token. `NULL` until a start or
    // resume since plan 8e.
    "
    ALTER TABLE sessions ADD COLUMN mcp_delivery_mode TEXT;
    ALTER TABLE sessions ADD COLUMN mcp_delivery_servers INTEGER;
    ALTER TABLE sessions ADD COLUMN mcp_delivery_at TEXT;
",
];

/// `Store::events`: `?1` the session, `?2` after, `?3` the limit, `?4` the
/// owner.
const EVENTS_AFTER: &str = "SELECT event_id, host_seq, kind, body, ts FROM events
     WHERE session_id = ?1 AND event_id > ?2 AND applied = 1 AND owner_id = ?4 ORDER BY event_id LIMIT ?3";

/// What `Store::ingest_fact` did with one fact.
#[derive(Debug, Clone, PartialEq)]
pub struct Ingested {
    /// The events it created, in order: none for a duplicate or a fact not
    /// applied.
    pub events: Vec<EventDto>,
    /// The push edge it crossed, if any.
    pub edge: Option<Edge>,
}

/// A push edge, with the session as the fact left it: read in the fact's
/// transaction, so a re-assignment or rename after it cannot change which
/// hat's policy applies (10b-i's review, A3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub kind: PushEdge,
    pub session: EdgeSession,
}

/// What a notice needs of its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeSession {
    pub id: String,
    pub hat_id: String,
    /// On one line and capped, as stored.
    pub title: Option<String>,
    pub cwd: String,
}

/// The edge's session from a row read later: for tests, and for any caller
/// that holds a row rather than an edge.
impl From<&SessionRow> for EdgeSession {
    fn from(row: &SessionRow) -> Self {
        Self {
            id: row.id.clone(),
            hat_id: row.hat_id.clone(),
            title: row.title.clone(),
            cwd: row.cwd.clone(),
        }
    }
}

/// A change a host fact made that may notify the owner (ACP core §10).
/// Only an applied fact, ingested from its host, crosses one: recovery and
/// reconciliation write no facts, and never push. Which edges notify is
/// decided in one place (`notify::notice_for`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushEdge {
    /// The session's activity went from `running` to `blocked`: the first
    /// open question of the turn. `title` is the question's, from the host
    /// (`PendingExtract::title`), on one line.
    Blocked { pending_id: String, title: Option<String> },
    /// A question the agent asked outside a turn: it leaves the activity
    /// alone (ACP core §4.2), so it is not `Blocked`.
    QuestionOutsideTurn { pending_id: String, title: Option<String> },
    /// The open turn ended with this outcome (a real `turn_ended`; a
    /// synthesised one is never a fact).
    TurnEnded(TurnOutcome),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub host_id: String,
    pub agent: String,
    /// Canonical on its host since plan 5c (umbrella §8.2).
    pub cwd: String,
    /// The hat it belongs to (umbrella §8.2), decided at its start.
    pub hat_id: String,
    /// The adapter's own session id, once a start produced one: a resume
    /// needs it (ACP core §4.3).
    pub agent_session_id: Option<String>,
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
    /// When the session's last listed event was written (`stamp`), or its
    /// creation; the session list's sort key.
    pub last_event_at: String,
    /// That event's id; `None` until the session has one.
    pub last_event_id: Option<i64>,
    /// The title the agent last reported, on one line and capped (plan 6b
    /// decision 1); `None` until it reports one, or once it clears it.
    pub title: Option<String>,
    /// Whether `cwd` was in a linked work tree, as the host last reported.
    pub git_worktree: Option<bool>,
    /// The commit a new session started from, recorded once (ACP core §7).
    pub base_commit: Option<String>,
}

/// What `request_resume` reads: lifecycle, agent session id, open turn,
/// config and hat.
type ResumeRow = (String, Option<String>, Option<String>, ConfigColumns, String);

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
        /// Its MCP servers, with a fresh token (plan 8e).
        mcp: McpGiven,
    },
    /// Refused: the session is `starting` or `active` (this lifecycle).
    Busy(String),
    /// The agent never created a session for it: there is nothing to load.
    NoRecord,
    /// Its path now resolves to another hat than the one it belongs to
    /// (ACP core §4.3): the stored hat. Nothing changed.
    HatMismatch(String),
    /// The hat it belongs to is frozen for its purge (plan 9c decision
    /// 10c): nothing changed.
    HatPurging,
    NotFound,
}

/// The outcome of `Store::reassign_hat` (ACP core §4.9).
#[derive(Debug, PartialEq)]
pub enum Reassign {
    /// Its `hat_reassigned` event.
    Done(EventDto),
    /// It is in that hat already: nothing written.
    Unchanged,
    /// It may have a running adapter (this lifecycle, or `presumed_parked`
    /// while its host is away): refused (plan 5d decision 1).
    Attached(String),
    /// No such hat of the owner's.
    UnknownHat,
    /// The hat it is in, or the one it would move to, is frozen for its
    /// purge (plan 9c A3): nothing changed.
    HatPurging,
    NotFound,
}

/// A kept session of a hat, as a purge judges it (plan 9c decision 10b):
/// whether it may have an adapter the collector can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatSession {
    pub id: String,
    pub lifecycle: String,
    pub presumed_parked: bool,
    pub host_id: String,
}

/// The state a delete's route judged to have no adapter it can reach
/// (plan 9a decision 5): a session parked, failed, presumed parked, or
/// `starting` or `active` on a host that is away. The store closes it
/// collector-side first only while it is still exactly this (A4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unattached {
    pub lifecycle: String,
    pub presumed_parked: bool,
}

/// The outcome of `Store::delete_session` (ACP core §4.10).
#[derive(Debug, PartialEq)]
pub enum Deletion {
    /// Deleted: its `session_deleted` event. `unconfirmed`: it was closed
    /// here, collector-side, while its host may still run it (presumed
    /// parked, or starting or active on a host away); that host closes its
    /// adapter when it is back (plan 9a decision 4, A13). A parked or
    /// failed session runs nowhere: its close is confirmed.
    Done { event: EventDto, unconfirmed: bool },
    /// Not closed, and not what the route judged unattached: this
    /// lifecycle. Nothing changed.
    Refused(String),
    /// No such session, or a tombstone already.
    NotFound,
}

/// The outcome of `Store::submit_answer` (ACP core §4.6).
#[derive(Debug, PartialEq)]
pub enum AnswerSubmission {
    Queued(Box<QueuedAnswer>),
    /// No such pending request in that session.
    NotFound,
    /// Answered or cancelled already.
    NotOpen,
    /// An answer is queued for it already.
    AlreadyAnswered,
    /// The answer does not fit the question (why).
    Invalid(String),
}

/// An answer the collector has queued durably.
#[derive(Debug, PartialEq)]
pub struct QueuedAnswer {
    /// Its `answer_submitted` event.
    pub event: EventDto,
    /// Where `frame` goes: now if that host is connected and reconciled,
    /// else after its next handshake.
    pub host_id: String,
    pub request_id: String,
    pub frame: CollectorFrame,
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
    /// The database's owner (`hennery_kernel::db::kernel_owner`), whom
    /// every query names.
    owner: String,
    /// Where the attachment files go: `attachments/` beside the database
    /// (kernel spec §1). An in-memory store has none.
    attachments: Option<PathBuf>,
    /// The checkpoint a delete owes (plan 9a A8); an in-memory store has
    /// none.
    checkpoints: Option<Arc<Checkpoints>>,
    /// The gateway, as sessions reach it (umbrella §9, ACP core §1): handed
    /// every start's, resume's and revoke's transaction (lane L1). None
    /// (`NoSessionMcp` stands in) until the collector sets it
    /// (`set_session_mcp`).
    mcp: RwLock<Option<Arc<dyn SessionMcp>>>,
}

/// What the start or resume route read of the host for the delivery
/// decision (umbrella §8.5, lane L2), outside the store: from the hub, the
/// host's live connection; from the kernel, whether its rules name another
/// hat. The rest (the host's default hat, its live sessions' hats) is read
/// in the transition's own transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpContext {
    /// The connection announced `mcp_servers`.
    pub capable: bool,
    /// How it isolates the session's agent.
    pub isolation: McpIsolation,
    /// `Hosts::rules_name_other_hats`.
    pub rules_name_other_hats: bool,
}

impl McpContext {
    /// A host that takes no servers: what a store without a gateway, or a
    /// host that is not connected, decides.
    pub const NONE: Self = Self {
        capable: false,
        isolation: McpIsolation::None,
        rules_name_other_hats: false,
    };
}

/// What a start or resume was given (plan 8e): its servers, for its frame
/// and nowhere else, and the mode, for `isolation_waived`.
#[derive(Clone, PartialEq)]
pub struct McpGiven {
    pub mode: McpSessionDeliveryMode,
    pub servers: Vec<McpServer>,
}

impl McpGiven {
    /// The frame's MCP part: `isolation_waived` only for the default hat of
    /// a host that cannot isolate the agent (plan 8c's hand-off).
    pub fn frame(self) -> hennery_proto::frames::McpDelivery {
        hennery_proto::frames::McpDelivery {
            isolation_waived: self.mode == McpSessionDeliveryMode::Unisolated,
            mcp_servers: self.servers,
        }
    }
}

// By hand: the servers carry the session's token and stdio values.
impl std::fmt::Debug for McpGiven {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpGiven")
            .field("mode", &self.mode)
            .field("servers", &self.servers)
            .finish()
    }
}

/// What one sweep removed (plan 9b): the owner's rows nothing of theirs
/// showed, the image files no row of any owner named, and the leftover
/// temporary files.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub rows: u64,
    pub files: u64,
    pub temps: u64,
}

/// A stored image (plan 6a), for `GET /api/attachments/{sha256}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// `at` as RFC 3339 UTC with exactly three fractional digits
/// (`2026-10-07T12:34:56.789Z`, truncated to the millisecond). Every stamp
/// has the same width, so comparing two as text compares the times: the
/// session list sorts on them (plan 6b decision 5). `time`'s own RFC 3339
/// output trims trailing zeros, so `…:05.1Z` would sort after `…:05.12Z`.
fn stamp(at: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond()
    )
}

fn now() -> String {
    stamp(time::OffsetDateTime::now_utc())
}

/// Write a collector-originated event (`host_seq` NULL, ACP core §8), for
/// a session of `owner`'s only: for any other, or a tombstone (plan 9a
/// A1), it fails and writes nothing.
fn collector_event(
    tx: &Transaction<'_>,
    owner: &str,
    session_id: &str,
    kind: &str,
    body: Value,
    ts: &str,
) -> Result<EventDto> {
    let written = tx.execute(
        "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
         SELECT ?1, NULL, ?2, ?3, ?4, ?5
         WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?5)",
        params![session_id, kind, body.to_string(), ts, owner],
    )?;
    anyhow::ensure!(written == 1, "no session {session_id}");
    let event_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE sessions SET last_event_at = ?2, last_event_id = ?3 WHERE id = ?1 AND owner_id = ?4",
        params![session_id, ts, event_id, owner],
    )?;
    Ok(EventDto {
        event_id,
        session_id: session_id.to_string(),
        host_seq: None,
        kind: kind.to_string(),
        body,
        ts: ts.to_string(),
    })
}

/// The image blocks of a stored content that name their image, each with
/// its index in the content. A block stored before plan 6a names no
/// attachment (decision 9).
fn image_hashes<'a>(blocks: impl IntoIterator<Item = &'a Value>) -> Vec<(usize, &'a str)> {
    blocks
        .into_iter()
        .enumerate()
        .filter(|(_, block)| block.get("type").and_then(Value::as_str) == Some("image"))
        .filter_map(|(position, block)| Some((position, block.get("sha256")?.as_str()?)))
        .collect()
}

/// Link a `user_turn` event to the images it shows (ACP core §8): one row
/// per image block, at its index in the content.
fn link_attachments(tx: &Transaction<'_>, owner: &str, event_id: i64, content: &Value) -> Result<()> {
    for (position, sha256) in image_hashes(content.as_array().into_iter().flatten()) {
        tx.execute(
            "INSERT INTO event_attachments(event_id, sha256, position, owner_id)
             SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM attachments WHERE owner_id = ?4 AND sha256 = ?2)",
            params![event_id, sha256, position as i64, owner],
        )?;
    }
    Ok(())
}

/// Link a turn to the images its content shows (plan 9a decision 6), as
/// `link_attachments` links its `user_turn`: an image a turn shows that
/// never started is still referenced. The links go with the turn.
fn link_turn_attachments(tx: &Transaction<'_>, owner: &str, turn_id: &str, content: &[Value]) -> Result<()> {
    for (position, sha256) in image_hashes(content) {
        tx.execute(
            "INSERT INTO turn_attachments(turn_id, sha256, position, owner_id)
             SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM attachments WHERE owner_id = ?4 AND sha256 = ?2)",
            params![turn_id, sha256, position as i64, owner],
        )?;
    }
    Ok(())
}

/// Delete the owner's rows of those of `hashes` that nothing of theirs
/// shows any more, no turn and no event (plan 9a decision 6); the hashes
/// whose row went. Their files go after the commit (`Store::remove_files`).
fn drop_unreferenced(tx: &Transaction<'_>, owner: &str, hashes: &BTreeSet<String>) -> Result<Vec<String>> {
    let mut dropped = Vec::new();
    for sha256 in hashes {
        let gone = tx.execute(
            "DELETE FROM attachments WHERE owner_id = ?1 AND sha256 = ?2
                 AND NOT EXISTS (SELECT 1 FROM turn_attachments WHERE owner_id = ?1 AND sha256 = ?2)
                 AND NOT EXISTS (SELECT 1 FROM event_attachments WHERE owner_id = ?1 AND sha256 = ?2)",
            [owner, sha256],
        )?;
        if gone == 1 {
            dropped.push(sha256.clone());
        }
    }
    Ok(dropped)
}

/// How many files a sweep looks at under one hold of the store's lock
/// (A14), so a prompt waits for a few files, not the whole directory.
const SWEEP_BATCH: usize = 32;

/// Close an open turn that the host will never end, as `interrupted`.
fn synthesize_turn_end(
    tx: &Transaction<'_>,
    owner: &str,
    session_id: &str,
    turn_id: &str,
    ts: &str,
) -> Result<EventDto> {
    tx.execute(
        "UPDATE turns SET state = 'ended', outcome = 'interrupted' WHERE turn_id = ?1 AND owner_id = ?2",
        [turn_id, owner],
    )?;
    tx.execute(
        "UPDATE sessions SET open_turn_id = NULL, activity = 'idle'
         WHERE id = ?1 AND open_turn_id = ?2 AND owner_id = ?3",
        params![session_id, turn_id, owner],
    )?;
    collector_event(
        tx,
        owner,
        session_id,
        "turn_ended_synthesized",
        json!({ "turn_id": turn_id, "outcome": "interrupted" }),
        ts,
    )
}

/// Release an open turn whose prompt never reached the adapter.
fn turn_not_delivered(
    tx: &Transaction<'_>,
    owner: &str,
    session_id: &str,
    turn_id: &str,
    ts: &str,
) -> Result<EventDto> {
    tx.execute(
        "UPDATE turns SET state = 'not_delivered' WHERE turn_id = ?1 AND owner_id = ?2",
        [turn_id, owner],
    )?;
    tx.execute(
        "UPDATE sessions SET open_turn_id = NULL, activity = 'idle'
         WHERE id = ?1 AND open_turn_id = ?2 AND owner_id = ?3",
        params![session_id, turn_id, owner],
    )?;
    collector_event(
        tx,
        owner,
        session_id,
        "turn_not_delivered",
        json!({ "turn_id": turn_id }),
        ts,
    )
}

/// Resolve an open turn the host will never end: `interrupted` if the
/// adapter had it (`started`), otherwise `turn_not_delivered`.
fn resolve_open_turn(tx: &Transaction<'_>, owner: &str, session_id: &str, turn_id: &str, ts: &str) -> Result<EventDto> {
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
            [turn_id, owner],
            |r| r.get(0),
        )
        .optional()?;
    match state.as_deref() {
        Some("started") => synthesize_turn_end(tx, owner, session_id, turn_id, ts),
        _ => turn_not_delivered(tx, owner, session_id, turn_id, ts),
    }
}

/// The host detached an active session (`session_parked`/`session_closed`).
/// A turn still open is one the host never acknowledged: its `not_attached`
/// answer is not outboxed and can be lost. Release it, or the next resume
/// inherits a permanent 409 (plan A, "After this plan").
fn release_turn_on_detach(tx: &Transaction<'_>, owner: &str, session_id: &str, ts: &str) -> Result<Option<EventDto>> {
    let row: Option<(String, Option<String>, bool)> = tx
        .query_row(
            "SELECT lifecycle, open_turn_id, presumed_parked FROM sessions WHERE id = ?1 AND owner_id = ?2",
            [session_id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        Some((lifecycle, Some(turn), presumed)) if lifecycle == "active" || presumed => {
            Ok(Some(resolve_open_turn(tx, owner, session_id, &turn, ts)?))
        }
        _ => Ok(None),
    }
}

/// Whether a host fact with no transition of its own (an update, a
/// diagnostic) still belongs on the timeline: not once the operator has
/// closed the session, and an update of a turn only while that turn is
/// open. An update for a turn the collector already ended (a synthesized
/// end) would otherwise be listed after that end.
fn fact_applies(tx: &Transaction<'_>, owner: &str, session_id: &str, turn_id: Option<&str>) -> Result<bool> {
    let lifecycle: String = tx.query_row(
        "SELECT lifecycle FROM sessions WHERE id = ?1 AND owner_id = ?2",
        [session_id, owner],
        |r| r.get(0),
    )?;
    if lifecycle == "closed" {
        return Ok(false);
    }
    let Some(turn_id) = turn_id else {
        return Ok(true);
    };
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
            [turn_id, owner],
            |r| r.get(0),
        )
        .optional()?;
    Ok(state.as_deref() == Some("started"))
}

/// `Store::close_now`'s body, inside the caller's transaction, with the
/// session's token revoked there too (lane L4): the cut is the caller's to
/// make once it commits. A tombstone is left alone (plan 9a A1).
fn close_in(tx: &Transaction<'_>, owner: &str, mcp: &dyn SessionMcp, session_id: &str) -> Result<(Vec<EventDto>, Cut)> {
    let events = close_session_in(tx, owner, session_id)?;
    // Whether or not this closed it: a closed session's token is revoked
    // already, so this is a no-op then, never a fresh token's revoke (a
    // resume moves the row to `starting` first, in its own transaction).
    let cut = mcp.revoke_in(tx, session_id)?;
    Ok((events, cut))
}

/// Closes the session, if it is not closed already: the events that wrote.
fn close_session_in(tx: &Transaction<'_>, owner: &str, session_id: &str) -> Result<Vec<EventDto>> {
    let row: Option<(String, bool, Option<String>)> = tx
        .query_row(
            "SELECT lifecycle, close_requested, open_turn_id FROM sessions
             WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
            [session_id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let mut events = Vec::new();
    if let Some((lifecycle, close_requested, open_turn)) = row
        && lifecycle != "closed"
    {
        let ts = now();
        if let Some(turn) = open_turn.as_deref() {
            events.push(resolve_open_turn(tx, owner, session_id, turn, &ts)?);
        }
        events.extend(cancel_open_pending(
            tx,
            owner,
            session_id,
            PendingReason::SessionClosed,
            &ts,
        )?);
        if !close_requested {
            events.push(collector_event(
                tx,
                owner,
                session_id,
                "operator_closed",
                json!({}),
                &ts,
            )?);
        }
        tx.execute(
            "UPDATE sessions SET lifecycle = 'closed', activity = NULL, open_turn_id = NULL, close_requested = 0,
                 presumed_parked = 0
             WHERE id = ?1 AND owner_id = ?2",
            [session_id, owner],
        )?;
    }
    Ok(events)
}

/// Store the catalogue snapshot a fact's extracts carry (ACP core §3.2,
/// §8): the options for `GET …/catalog`, and the current values in the
/// session's `model`, `mode` and `config_axes`, which a resume re-applies.
/// Extracts without a snapshot (none, or an empty read-back) change
/// nothing: the stored values are never replaced by a guess (P-13).
fn store_catalogue(tx: &Transaction<'_>, owner: &str, session_id: &str, indexed: &Indexed, ts: &str) -> Result<()> {
    let Some(current) = indexed.current_config() else {
        return Ok(());
    };
    let options = indexed.config_options.clone().unwrap_or_default();
    tx.execute(
        "UPDATE sessions SET model = ?2, mode = ?3, config_axes = ?4 WHERE id = ?1 AND owner_id = ?5",
        params![
            session_id,
            current.model,
            current.mode,
            serde_json::to_string(&current.axes)?,
            owner
        ],
    )?;
    tx.execute(
        "INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(session_id) DO UPDATE SET config_options = excluded.config_options, updated_at = excluded.updated_at
             WHERE session_catalog.owner_id = excluded.owner_id",
        params![session_id, serde_json::to_string(&options)?, ts, owner],
    )?;
    Ok(())
}

/// `raw` as one line for the session list (plan 6b decision 1): bidi and
/// zero-width characters dropped, every other control character a space,
/// runs of whitespace one space and the ends trimmed; then cut, never
/// inside a character, to `max_chars` characters and `max_json_bytes` bytes
/// as JSON writes it. With control characters gone, only `"` and `\` are
/// escaped, as two bytes each; a cap on the raw bytes would not hold, since
/// JSON writes a control character as six. `None` when nothing is left.
pub(crate) fn one_line(raw: &str, max_chars: usize, max_json_bytes: usize) -> Option<String> {
    let spaced: String = raw
        .chars()
        .filter(|c| !hennery_proto::rest::is_hidden_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = String::new();
    let (mut chars, mut bytes) = (0, 0);
    for c in spaced.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        let width = json_char_width(c);
        if chars == max_chars || bytes + width > max_json_bytes {
            break;
        }
        out.push(c);
        chars += 1;
        bytes += width;
    }
    let out = out.trim_end();
    (!out.is_empty()).then(|| out.to_string())
}

/// Store the title and the commands a fact's extracts carry (ACP core §3.2,
/// §7). The title goes on one line and is capped (decision 1); a title the
/// adapter sent before the session was announced (`early`, replayed by a
/// load) may be older than the stored one, so it only fills an empty title
/// (decision 2). The commands replace the stored list, and never touch the
/// config catalogue (decision 3).
fn store_state(tx: &Transaction<'_>, owner: &str, session_id: &str, indexed: &Indexed, ts: &str) -> Result<()> {
    if let Some(title) = indexed.title.as_deref() {
        let title = one_line(title, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES);
        if !indexed.early {
            tx.execute(
                "UPDATE sessions SET title = ?2 WHERE id = ?1 AND owner_id = ?3",
                params![session_id, title, owner],
            )?;
        } else if title.is_some() {
            tx.execute(
                "UPDATE sessions SET title = ?2 WHERE id = ?1 AND title IS NULL AND owner_id = ?3",
                params![session_id, title, owner],
            )?;
        }
    }
    if let Some(commands) = &indexed.commands {
        tx.execute(
            "INSERT INTO session_catalog(session_id, config_options, commands, updated_at, owner_id)
             VALUES (?1, '[]', ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET commands = excluded.commands, updated_at = excluded.updated_at
                 WHERE session_catalog.owner_id = excluded.owner_id",
            params![session_id, serde_json::to_string(commands)?, ts, owner],
        )?;
    }
    Ok(())
}

/// The columns of a list item, in `read_item`'s order.
const SESSION_ITEM_COLUMNS: &str = "id, host_id, agent, cwd, title, lifecycle, activity, failure_reason,
     presumed_parked, git_branch, git_dirty, model, mode, created_at, last_event_at, hat_id";

/// A row of `SESSION_ITEM_COLUMNS` as a list item, as stored.
fn read_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionItem> {
    Ok(SessionItem {
        session_id: r.get(0)?,
        host_id: r.get(1)?,
        agent: r.get(2)?,
        cwd: r.get(3)?,
        title: r.get(4)?,
        lifecycle: r.get(5)?,
        activity: r.get(6)?,
        failure_reason: r.get(7)?,
        presumed_parked: r.get(8)?,
        git_branch: r.get(9)?,
        git_dirty: r.get(10)?,
        model: r.get(11)?,
        mode: r.get(12)?,
        created_at: r.get(13)?,
        last_event_at: r.get(14)?,
        hat_id: r.get(15)?,
    })
}

/// Every lifecycle a session can be in (ACP core §4.2).
pub const LIFECYCLES: [&str; 5] = ["starting", "active", "parked", "closed", "failed"];

/// The session list's page size when none is asked for, and the largest
/// it serves (plan 6b decision 8).
pub const LIST_DEFAULT_LIMIT: u32 = 50;
pub const LIST_MAX_LIMIT: u32 = 200;

/// A position in the session list: the last item of a page, by its sort
/// key (decision 8). It goes out opaque, as hex, and is a position only, so
/// a cursor from another query is harmless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub last_event_at: String,
    pub session_id: String,
}

impl Cursor {
    pub fn encode(&self) -> String {
        hex::encode(format!("{}\n{}", self.last_event_at, self.session_id))
    }

    /// `None` for anything `encode` did not make: not hex, not UTF-8, no
    /// separator, a part empty or longer than 64 bytes, or more than 256
    /// characters in all (the review's O4).
    pub fn decode(cursor: &str) -> Option<Self> {
        if cursor.len() > 256 {
            return None;
        }
        let text = String::from_utf8(hex::decode(cursor).ok()?).ok()?;
        let (at, id) = text.split_once('\n')?;
        let fits = |part: &str| (1..=64).contains(&part.len());
        (fits(at) && fits(id)).then(|| Self {
            last_event_at: at.to_string(),
            session_id: id.to_string(),
        })
    }
}

/// What a page of the session list holds (ACP core §9; plan 6b decision 8).
#[derive(Debug, Clone, Copy)]
pub struct ListQuery<'a> {
    /// Start after this position (the previous page's `next_cursor`).
    pub after: Option<&'a Cursor>,
    /// At most this many sessions, clamped to 1..=`LIST_MAX_LIMIT`.
    pub limit: u32,
    /// Only sessions in one of these lifecycles (names from `LIFECYCLES`);
    /// all when `None`. Ignored while `search` is set (frontend §5).
    pub lifecycles: Option<&'a [&'a str]>,
    /// Only sessions whose title, cwd, branch or id holds this text.
    pub search: Option<&'a str>,
    /// Only sessions of this hat (plan 5c), with or without `search`
    /// (frontend §5: a search bypasses every filter but the hat's).
    pub hat: Option<&'a str>,
}

impl Default for ListQuery<'_> {
    fn default() -> Self {
        Self {
            after: None,
            limit: LIST_DEFAULT_LIMIT,
            lifecycles: None,
            search: None,
            hat: None,
        }
    }
}

/// `search` as a `LIKE` pattern that matches it anywhere, its `%`, `_` and
/// `\` taken literally (`ESCAPE '\'`). SQLite's `LIKE` ignores case for
/// ASCII letters only.
fn like_pattern(search: &str) -> String {
    let mut pattern = String::from("%");
    for c in search.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// What every page of the session list filters on (decision 8): the
/// owner's sessions after the cursor `(?2, ?3)`, in one of the lifecycles
/// `?4`…`?8` (a NULL slot matches nothing), matching the pattern `?9`
/// unless it is NULL.
const LIST_FILTERS: &str = "owner_id = ?1 AND (last_event_at, id) < (?2, ?3) AND lifecycle IN (?4, ?5, ?6, ?7, ?8)
             AND (?9 IS NULL OR title LIKE ?9 ESCAPE '\\' OR cwd LIKE ?9 ESCAPE '\\'
                  OR git_branch LIKE ?9 ESCAPE '\\' OR id LIKE ?9 ESCAPE '\\')";

/// The session list's statement (decision 8): `LIST_FILTERS`, the newest
/// `last_event_at` first and the id breaking ties, at most `?10`; with
/// `by_hat`, only the sessions of the hat `?11` (plan 5c). It walks
/// `sessions_by_recency`, or `sessions_by_hat` for one hat, and sorts
/// nothing (the review's A11). The hat has a statement of its own: a plan
/// is made before `?11` is known, so `?11 IS NULL OR hat_id = ?11` would
/// never take the hat's index.
fn list_statement(by_hat: bool) -> String {
    if by_hat {
        format!(
            "SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE {LIST_FILTERS} AND hat_id = ?11
             ORDER BY last_event_at DESC, id DESC LIMIT ?10"
        )
    } else {
        format!(
            "SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE {LIST_FILTERS} AND ?11 IS NULL
             ORDER BY last_event_at DESC, id DESC LIMIT ?10"
        )
    }
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

/// A wire enum's snake_case name, as stored.
fn tag(value: impl serde::Serialize) -> Result<String> {
    Ok(serde_json::to_value(value)?.as_str().unwrap_or_default().to_string())
}

/// A stored snake_case name back as its wire enum.
fn untag<T: serde::de::DeserializeOwned>(name: String) -> Result<T> {
    Ok(serde_json::from_value(Value::String(name))?)
}

/// Move an open pending request of `session_id` to `state` (with `reason`
/// if cancelled). An answer queued for a cancelled one can never be
/// delivered any more, so it gets its verdict. A session blocked on
/// nothing else runs again. `false` if the request was not open.
fn resolve_pending(
    tx: &Transaction<'_>,
    owner: &str,
    session_id: &str,
    pending_id: &str,
    state: PendingState,
    reason: Option<PendingReason>,
    ts: &str,
) -> Result<bool> {
    let changed = tx.execute(
        "UPDATE pending SET state = ?3, reason = ?4, resolved_at = ?5
         WHERE pending_id = ?1 AND session_id = ?2 AND state = 'open' AND owner_id = ?6",
        params![
            pending_id,
            session_id,
            tag(state)?,
            reason.map(tag).transpose()?,
            ts,
            owner
        ],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    if state == PendingState::Cancelled {
        tx.execute(
            "UPDATE answer_queue SET delivered = 0 WHERE pending_id = ?1 AND delivered IS NULL AND owner_id = ?2",
            [pending_id, owner],
        )?;
    }
    tx.execute(
        "UPDATE sessions SET activity = 'running'
         WHERE id = ?1 AND activity = 'blocked' AND owner_id = ?2
             AND NOT EXISTS (SELECT 1 FROM pending WHERE session_id = ?1 AND state = 'open' AND owner_id = ?2)",
        [session_id, owner],
    )?;
    Ok(true)
}

/// Cancel, collector-side, every pending request of `session_id` that is
/// still open, with one `pending_cancelled` event each (ACP core §4.6,
/// §5.2): the host will never resolve them (it restarted, or the session
/// is gone), and a question must not stay answerable.
fn cancel_open_pending(
    tx: &Transaction<'_>,
    owner: &str,
    session_id: &str,
    reason: PendingReason,
    ts: &str,
) -> Result<Vec<EventDto>> {
    let ids: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT pending_id FROM pending WHERE session_id = ?1 AND state = 'open' AND owner_id = ?2 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([session_id, owner], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut events = Vec::new();
    for id in ids {
        resolve_pending(tx, owner, session_id, &id, PendingState::Cancelled, Some(reason), ts)?;
        let body = json!({ "pending_id": id, "reason": reason });
        events.push(collector_event(tx, owner, session_id, "pending_cancelled", body, ts)?);
    }
    Ok(events)
}

/// Whether `answer` fits a pending request of `kind` (ACP core §4.6): one
/// of the stored options for a permission request, an action for an
/// elicitation, with content (an object) only to accept.
fn check_answer(kind: PendingKind, option_ids: Option<&[String]>, answer: &AnswerRequest) -> Result<(), String> {
    match (kind, answer) {
        (PendingKind::Permission, AnswerRequest::Permission { option_id }) => match option_ids {
            Some(ids) if ids.contains(option_id) => Ok(()),
            Some(ids) if !ids.is_empty() => Err(format!("the request offers no option {option_id}")),
            // `None`, or `Some(&[])` (every option lacked a string optionId,
            // decision 4): either way there is nothing to validate against,
            // so this is the same "stop, park or close" case, not "no option
            // X".
            _ => Err("the request's options could not be read; stop, park or close the session".into()),
        },
        (PendingKind::Elicitation, AnswerRequest::Elicitation { action, content }) => match (action, content) {
            (_, None) => Ok(()),
            (ElicitationAction::Accept, Some(content)) if content.is_object() => Ok(()),
            (ElicitationAction::Accept, Some(_)) => Err("the form's content must be an object".into()),
            (_, Some(_)) => Err("only an accepted form has content".into()),
        },
        (PendingKind::Permission, _) => Err("a permission request is answered with an option_id".into()),
        (PendingKind::Elicitation, _) => Err("an elicitation is answered with an action".into()),
    }
}

/// The frame that carries a queued answer to its host.
fn answer_frame(request_id: String, session_id: &str, pending_id: &str, answer: AnswerRequest) -> CollectorFrame {
    let (session_id, pending_id) = (session_id.to_string(), pending_id.to_string());
    match answer {
        AnswerRequest::Permission { option_id } => CollectorFrame::AnswerPermission {
            request_id,
            session_id,
            pending_id,
            option_id,
        },
        AnswerRequest::Elicitation { action, content } => CollectorFrame::AnswerElicitation {
            request_id,
            session_id,
            pending_id,
            action,
            content,
        },
    }
}

/// One `pending` row joined with its answer, as read.
struct PendingRow {
    pending_id: String,
    session_id: String,
    kind: String,
    state: String,
    reason: Option<String>,
    turn_id: Option<String>,
    option_ids: Option<String>,
    payload: String,
    answered: bool,
    delivered: Option<bool>,
}

const PENDING_COLUMNS: &str = "p.pending_id, p.session_id, p.kind, p.state, p.reason, p.turn_id, p.option_ids,
     p.payload, q.pending_id IS NOT NULL, q.delivered
     FROM pending p LEFT JOIN answer_queue q ON q.pending_id = p.pending_id AND q.owner_id = p.owner_id";

impl PendingRow {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            pending_id: r.get(0)?,
            session_id: r.get(1)?,
            kind: r.get(2)?,
            state: r.get(3)?,
            reason: r.get(4)?,
            turn_id: r.get(5)?,
            option_ids: r.get(6)?,
            payload: r.get(7)?,
            answered: r.get(8)?,
            delivered: r.get(9)?,
        })
    }

    fn item(self) -> Result<PendingItem> {
        Ok(PendingItem {
            pending_id: self.pending_id,
            session_id: self.session_id,
            kind: untag(self.kind)?,
            state: untag(self.state)?,
            reason: self.reason.map(untag).transpose()?,
            turn_id: self.turn_id,
            option_ids: self.option_ids.map(|ids| serde_json::from_str(&ids)).transpose()?,
            payload: serde_json::from_str(&self.payload)?,
            answered: self.answered,
            delivered: self.delivered,
        })
    }
}

/// Keep a stored host fact that did not apply as the idempotency key only:
/// it is hidden from `Store::events` (and so from SSE replay).
fn mark_unapplied(tx: &Transaction<'_>, owner: &str, event_id: i64) -> Result<()> {
    tx.execute(
        "UPDATE events SET applied = 0 WHERE event_id = ?1 AND owner_id = ?2",
        params![event_id, owner],
    )?;
    Ok(())
}

/// Whether a `conflict` event with this exact `received` body is already
/// recorded for `(session_id, seq)` (fix round 1, ruling D): a re-sent
/// conflicting frame must not pile up a second `conflict` event.
fn conflict_already_recorded(
    tx: &Transaction<'_>,
    owner: &str,
    session_id: &str,
    seq: u64,
    received: &Value,
) -> Result<bool> {
    let mut stmt = tx.prepare(
        "SELECT body FROM events
         WHERE session_id = ?1 AND kind = 'conflict' AND json_extract(body, '$.seq') = ?2 AND owner_id = ?3",
    )?;
    let mut rows = stmt.query(params![session_id, seq as i64, owner])?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(0)?;
        let value: Value = serde_json::from_str(&body)?;
        if value["received"] == *received {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `owner` froze `hat_id` for its purge (plan 9c decision 10c): the
/// kernel's `purged_hats`, read on the caller's transaction, so a freeze
/// committed before it is seen and one committed after it fails its write.
fn hat_frozen(tx: &Transaction<'_>, owner: &str, hat_id: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM purged_hats WHERE hat_id = ?1 AND owner_id = ?2)",
        [hat_id, owner],
        |r| r.get(0),
    )?)
}

/// How a checkpoint after a delete is retried while a reader holds the
/// WAL (plan 9a A8): every `retry`, `fast_retries` times, then (logged once)
/// every `slow_retry`, until it completes. The defaults are a second, five
/// minutes of them, then a minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointPolicy {
    pub retry: Duration,
    pub fast_retries: u32,
    pub slow_retry: Duration,
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            retry: Duration::from_secs(1),
            fast_retries: 300,
            slow_retry: Duration::from_secs(60),
        }
    }
}

/// The checkpoint a delete owes: the WAL folded back into the database and
/// truncated, so the pages the delete wrote over leave it too (plan 9a A8).
/// It runs on a connection of its own, so the store's lock is not held
/// while it waits for readers. Hennery's own reads are single statements
/// under the store's or the kernel's lock, so only a reader outside it (a
/// `sqlite3` shell, a backup tool) can hold the WAL for long. While one
/// does, the debt is durable: `<db>-checkpoint-owed` is written, retried by
/// one thread until a checkpoint completes, and at the next start. A
/// connection is opened per attempt: deletes are rare.
struct Checkpoints {
    path: PathBuf,
    policy: Mutex<CheckpointPolicy>,
    /// A retry thread is running; a new debt joins it.
    retrying: AtomicBool,
}

/// How long one attempt waits for readers before it counts as busy.
const CHECKPOINT_WAIT: Duration = Duration::from_secs(1);

impl Checkpoints {
    fn owed_marker(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push("-checkpoint-owed");
        PathBuf::from(name)
    }

    /// Checkpoint now; if a reader holds it up, record the debt and retry
    /// it apart until it completes.
    fn run(self: &Arc<Self>) {
        if self.once() {
            self.settle();
            return;
        }
        if let Err(err) = self.owe() {
            tracing::error!("recording a checkpoint owed failed: {err:#}");
        }
        if self.retrying.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            let policy = *this.policy.lock().expect("checkpoint policy");
            let mut attempts: u64 = 0;
            loop {
                let pause = if attempts < u64::from(policy.fast_retries) {
                    policy.retry
                } else {
                    if attempts == u64::from(policy.fast_retries) {
                        tracing::warn!(
                            "a reader has held the database's WAL since a delete: the deleted pages stay in the \
                             WAL until it is released; the checkpoint is retried every {:?}",
                            policy.slow_retry
                        );
                    }
                    policy.slow_retry
                };
                std::thread::sleep(pause);
                attempts += 1;
                if this.once() {
                    this.retrying.store(false, Ordering::SeqCst);
                    this.settle();
                    tracing::info!(attempts, "the checkpoint a delete owed completed");
                    return;
                }
            }
        });
    }

    /// One `wal_checkpoint(TRUNCATE)`, waiting `CHECKPOINT_WAIT` for
    /// readers: whether it completed. A failure to open or run it is
    /// logged, and counts as not completed.
    fn once(&self) -> bool {
        let checkpointed = hennery_kernel::db::open(&self.path).and_then(|conn| {
            conn.busy_timeout(CHECKPOINT_WAIT)?;
            Ok(conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))?)
        });
        match checkpointed {
            Ok(0) => true,
            Ok(_) => false,
            Err(err) => {
                tracing::warn!("a checkpoint after a delete failed: {err:#}");
                false
            }
        }
    }

    /// Record the debt durably: the marker, synced, and its directory.
    fn owe(&self) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let marker = self.owed_marker();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&marker)
            .with_context(|| format!("open {}", marker.display()))?;
        file.sync_all()?;
        if let Some(dir) = marker.parent() {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    }

    /// The debt is paid: remove the marker, if there is one.
    fn settle(&self) {
        match std::fs::remove_file(self.owed_marker()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!("removing the checkpoint-owed marker failed: {err}"),
        }
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let attachments = path.parent().map(|dir| dir.join(crate::attachments::DIR));
        Self::init(hennery_kernel::db::open(path)?, attachments, Some(path.to_path_buf()))
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(hennery_kernel::db::open_in_memory()?, None, None)
    }

    /// The kernel's tables first: they hold the owner, which this store's
    /// rows name and its last migration fills in. So the store and the
    /// kernel agree on the owner whichever opens the database first.
    fn init(mut conn: Connection, attachments: Option<PathBuf>, path: Option<PathBuf>) -> Result<Self> {
        let owner = hennery_kernel::db::kernel_owner(&mut conn)?;
        hennery_kernel::db::migrate(&mut conn, MIGRATIONS)?;
        let checkpoints = path.map(|path| {
            Arc::new(Checkpoints {
                path,
                policy: Mutex::new(CheckpointPolicy::default()),
                retrying: AtomicBool::new(false),
            })
        });
        // A checkpoint owed from before a restart is paid first (A8).
        if let Some(checkpoints) = &checkpoints
            && checkpoints.owed_marker().exists()
        {
            checkpoints.run();
        }
        Ok(Self {
            conn: Mutex::new(conn),
            owner,
            attachments,
            checkpoints,
            mcp: RwLock::new(None),
        })
    }

    /// The gateway, on the same `hennery.db` (the collector sets it before
    /// it serves; plan 8e).
    pub fn set_session_mcp(&self, mcp: Arc<dyn SessionMcp>) {
        *self.mcp.write().expect("session mcp lock") = Some(mcp);
    }

    /// Whether the gateway is set (the collector's wiring test).
    pub fn has_session_mcp(&self) -> bool {
        self.mcp.read().expect("session mcp lock").is_some()
    }

    /// The gateway's part of a hat's purge (lane L6): through the gateway
    /// this store was given, so a store without one has none to purge.
    pub fn purge_gateway_hat(&self, hat_id: &str) -> Result<()> {
        self.mcp().purge_hat(hat_id)
    }

    /// The gateway, or the stand-in that gives nothing.
    pub(crate) fn mcp(&self) -> Arc<dyn SessionMcp> {
        self.mcp
            .read()
            .expect("session mcp lock")
            .clone()
            .unwrap_or_else(|| Arc::new(NoSessionMcp))
    }

    /// How a checkpoint a reader holds up is retried (plan 9a A8), for the
    /// next one that is.
    pub fn set_checkpoint_policy(&self, policy: CheckpointPolicy) {
        if let Some(checkpoints) = &self.checkpoints {
            *checkpoints.policy.lock().expect("checkpoint policy") = policy;
        }
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store lock")
    }

    /// The owner whose sessions these are.
    pub fn owner_id(&self) -> &str {
        &self.owner
    }

    /// `create_session_with_mcp` for a host that takes no MCP servers: what
    /// the store's own tests start. `false` where that answers `None`.
    pub fn create_session(
        &self,
        id: &str,
        host_id: &str,
        agent: &str,
        cwd: &str,
        hat_id: &str,
        rule_id: Option<&str>,
    ) -> Result<bool> {
        Ok(self
            .create_session_with_mcp(id, host_id, agent, cwd, hat_id, rule_id, McpContext::NONE)?
            .is_some())
    }

    /// A new session, `starting`, in `hat_id` as `rule_id` decided (none:
    /// its host's default hat). `cwd` is canonical on its host. `None`, and
    /// nothing stored, if the hat is frozen for its purge: the start
    /// resolved its hat before the freeze (plan 9c decision 10c); the check
    /// and the insert are one statement. In the same transaction (lane L1):
    /// the delivery decision and the session's MCP servers, its token
    /// minted, so a failed mint stores no session.
    #[allow(clippy::too_many_arguments)]
    pub fn create_session_with_mcp(
        &self,
        id: &str,
        host_id: &str,
        agent: &str,
        cwd: &str,
        hat_id: &str,
        rule_id: Option<&str>,
        mcp: McpContext,
    ) -> Result<Option<McpGiven>> {
        let ts = now();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let created = tx.execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, hat_id, hat_rule_id, lifecycle, created_at, last_event_at,
                                  owner_id)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, 'starting', ?7, ?7, ?8
             WHERE NOT EXISTS (SELECT 1 FROM purged_hats WHERE hat_id = ?5 AND owner_id = ?8)",
            params![id, host_id, agent, cwd, hat_id, rule_id, ts, self.owner],
        )?;
        if created == 0 {
            return Ok(None);
        }
        // A fresh id had no token before this one: nothing to cut.
        let (given, _superseded) = self.deliver_in(&tx, id, host_id, hat_id, mcp, &ts)?;
        tx.commit()?;
        Ok(Some(given))
    }

    /// The delivery decision (umbrella §8.5, lane L2) and the servers it
    /// gives, inside a start's or resume's transaction, recorded on the
    /// session (decision E10). The session is `starting` already, so it
    /// counts itself in the mixedness: a session outside the default hat
    /// always finds its host mixed.
    fn deliver_in(
        &self,
        tx: &Transaction<'_>,
        session_id: &str,
        host_id: &str,
        hat_id: &str,
        mcp: McpContext,
        ts: &str,
    ) -> Result<(McpGiven, Cut)> {
        // A host not in the registry (the store's own tests) has no default
        // hat: then no session is in it.
        let default_hat: String = tx
            .query_row(
                "SELECT default_hat_id FROM hosts WHERE id = ?1 AND owner_id = ?2",
                [host_id, &self.owner],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        // One query on `sessions.hat_id` (lane L2): a live session of
        // another hat than the default, this one included.
        let other_hat_live: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sessions
                 WHERE host_id = ?1 AND owner_id = ?2 AND hat_id <> ?3
                     AND (lifecycle IN ('starting', 'active') OR presumed_parked = 1))",
            params![host_id, self.owner, default_hat],
            |r| r.get(0),
        )?;
        let mixed = mcp.rules_name_other_hats || other_hat_live;
        let mode = mcp_session_delivery(mcp.capable, mcp.isolation, mixed, hat_id == default_hat);
        let delivered = self.mcp().servers_in(
            tx,
            SessionRef {
                session_id,
                host_id,
                hat_id,
            },
            mode,
        )?;
        tx.execute(
            "UPDATE sessions SET mcp_delivery_mode = ?2, mcp_delivery_servers = ?3, mcp_delivery_at = ?4
             WHERE id = ?1 AND owner_id = ?5",
            params![
                session_id,
                mode.as_str(),
                delivered.servers.len() as i64,
                ts,
                self.owner
            ],
        )?;
        Ok((
            McpGiven {
                mode,
                servers: delivered.servers,
            },
            delivered.cut,
        ))
    }

    /// What the session's latest start or resume was given (decision
    /// E10); `None` before one since plan 8e.
    pub fn mcp_delivery(&self, id: &str) -> Result<Option<McpSessionDelivery>> {
        let row: Option<(Option<String>, Option<i64>, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT mcp_delivery_mode, mcp_delivery_servers, mcp_delivery_at FROM sessions
                 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                [id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((Some(mode), Some(servers), Some(at))) => Some(McpSessionDelivery {
                mode: McpSessionDeliveryMode::parse(&mode).context("a stored delivery mode")?,
                servers: u32::try_from(servers).context("a stored server count")?,
                at,
            }),
            _ => None,
        })
    }

    /// Fail a session's start; a tombstone is left alone (plan 9a A1). Its
    /// token goes with it (lane L4).
    pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let failed = tx.execute(
            "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2
             WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?3",
            params![id, reason, self.owner],
        )?;
        let cut = if failed == 1 {
            self.mcp().revoke_in(&tx, id)?
        } else {
            Cut::default()
        };
        tx.commit()?;
        self.mcp().cut(cut);
        Ok(())
    }

    /// Like `mark_failed`, for a resume whose request failed: only a
    /// session still `starting` is failed, and only its token revoked.
    /// Whatever moved it on while the request was out (its
    /// `session_started`, a close, a newer resume's outcome) is left as it
    /// is.
    pub fn mark_failed_if_starting(&self, id: &str, reason: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let failed = tx.execute(
            "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2
             WHERE id = ?1 AND lifecycle = 'starting' AND owner_id = ?3",
            params![id, reason, self.owner],
        )?;
        let cut = if failed == 1 {
            self.mcp().revoke_in(&tx, id)?
        } else {
            Cut::default()
        };
        tx.commit()?;
        self.mcp().cut(cut);
        Ok(())
    }

    /// One session; never a tombstone (plan 9a decision 3), so every route
    /// that reads it answers 404 for a deleted session.
    pub fn find_session(&self, id: &str) -> Result<Option<SessionRow>> {
        let row = self
            .conn()
            .query_row(
                "SELECT id, host_id, agent, cwd, lifecycle, activity, open_turn_id, failure_reason, close_requested,
                        presumed_parked, model, mode, config_axes, last_event_at, last_event_id, title,
                        git_worktree, base_commit, hat_id, agent_session_id
                 FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                [id, &self.owner],
                |r| {
                    let config: ConfigColumns = (r.get(10)?, r.get(11)?, r.get(12)?);
                    let row = SessionRow {
                        id: r.get(0)?,
                        host_id: r.get(1)?,
                        agent: r.get(2)?,
                        cwd: r.get(3)?,
                        hat_id: r.get(18)?,
                        agent_session_id: r.get(19)?,
                        lifecycle: r.get(4)?,
                        activity: r.get(5)?,
                        open_turn_id: r.get(6)?,
                        failure_reason: r.get(7)?,
                        close_requested: r.get(8)?,
                        presumed_parked: r.get(9)?,
                        config: SessionConfig::default(),
                        last_event_at: r.get(13)?,
                        last_event_id: r.get(14)?,
                        title: r.get(15)?,
                        git_worktree: r.get(16)?,
                        base_commit: r.get(17)?,
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

    /// Which of `ids` are tombstones of `host_id`'s (plan 9a decision 4):
    /// what a host's connection, once ready, still has to close of the
    /// sessions its `hello` listed, if a delete committed after its
    /// reconciliation.
    pub(crate) fn tombstones_of(&self, host_id: &str, ids: &[&str]) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM sessions WHERE id = ?1 AND host_id = ?2 AND lifecycle = 'deleted' AND owner_id = ?3",
        )?;
        let mut deleted = Vec::new();
        for id in ids {
            if stmt.exists(params![id, host_id, self.owner])? {
                deleted.push(id.to_string());
            }
        }
        Ok(deleted)
    }

    /// The host a session runs on, tombstones included: what a host's frame
    /// is checked against (ACP core §3.3). A frame of this host's for its
    /// deleted session goes on to `ingest`, which stores nothing, and is
    /// acked, so the host prunes its outbox (plan 9a decision 4).
    pub fn session_host(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT host_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// One session as a list item, as stored (the detail's; the list serves
    /// it `bounded`); never a tombstone (plan 9a decision 3).
    pub fn find_session_item(&self, id: &str) -> Result<Option<SessionItem>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2"
                ),
                [id, &self.owner],
                read_item,
            )
            .optional()?)
    }

    /// One page of the session list (ACP core §9; decision 8), each item
    /// `bounded`, with where the next page starts if there is one.
    pub fn list(&self, query: &ListQuery<'_>) -> Result<SessionPage> {
        let limit = query.limit.clamp(1, LIST_MAX_LIMIT);
        // A page with no cursor starts above every stamp.
        let (at, id) = match query.after {
            Some(cursor) => (cursor.last_event_at.as_str(), cursor.session_id.as_str()),
            None => ("\u{10FFFF}", ""),
        };
        let pattern = query.search.map(like_pattern);
        let slots: [Option<&str>; 5] = match query.lifecycles.filter(|_| pattern.is_none()) {
            Some(named) => std::array::from_fn(|i| named.get(i).copied()),
            None => LIFECYCLES.map(Some),
        };
        let conn = self.conn();
        let mut stmt = conn.prepare(&list_statement(query.hat.is_some()))?;
        let rows = stmt.query_map(
            params![
                self.owner,
                at,
                id,
                slots[0],
                slots[1],
                slots[2],
                slots[3],
                slots[4],
                pattern,
                limit + 1,
                query.hat
            ],
            read_item,
        )?;
        let mut sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = if sessions.len() > limit as usize {
            sessions.truncate(limit as usize);
            sessions.last().map(|last| {
                Cursor {
                    last_event_at: last.last_event_at.clone(),
                    session_id: last.session_id.clone(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok(SessionPage {
            sessions: sessions.into_iter().map(SessionItem::bounded).collect(),
            next_cursor,
        })
    }

    /// Every kept session of `hat_id` (plan 9c decisions 10b and 10d), of
    /// every lifecycle and host, by id: those re-assigned into it included
    /// (the 5d hand-on), tombstones not.
    pub fn hat_sessions(&self, hat_id: &str) -> Result<Vec<HatSession>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, lifecycle, presumed_parked, host_id FROM sessions
             WHERE hat_id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2 ORDER BY id",
        )?;
        let rows = stmt.query_map([hat_id, &self.owner], |r| {
            Ok(HatSession {
                id: r.get(0)?,
                lifecycle: r.get(1)?,
                presumed_parked: r.get(2)?,
                host_id: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The kept sessions of no hat (`hat_id = ''`: from before hats, when
    /// no default hat could be found), the newest first, at most `limit`,
    /// each `bounded`; and how many there are (plan 9c decision 11, the 5c
    /// hand-on). A purge never deletes them: they are listed for the
    /// operator.
    pub fn unassigned(&self, limit: u32) -> Result<(Vec<SessionItem>, u64)> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_ITEM_COLUMNS} FROM sessions
             WHERE hat_id = '' AND lifecycle <> 'deleted' AND owner_id = ?1
             ORDER BY last_event_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![self.owner, limit], read_item)?;
        let items = rows
            .map(|row| row.map(SessionItem::bounded))
            .collect::<rusqlite::Result<_>>()?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM sessions WHERE hat_id = '' AND lifecycle <> 'deleted' AND owner_id = ?1",
            [&self.owner],
            |r| r.get(0),
        )?;
        Ok((items, count as u64))
    }

    /// The checkpoint a purge owes for its deletes (plan 9a A8; plan 9c
    /// decision 10d), recorded before the first of them
    /// (`delete_session_owing_checkpoint`), so a crash before `checkpoint`
    /// normally leaves it owed, paid at the next start (a retry of an
    /// earlier delete's debt that completes meanwhile settles it early). A failure to record it is
    /// logged: the purge's own `checkpoint` still runs. An in-memory store
    /// owes none.
    pub fn owe_checkpoint(&self) {
        if let Some(checkpoints) = &self.checkpoints
            && let Err(err) = checkpoints.owe()
        {
            tracing::error!("recording a checkpoint owed failed: {err:#}");
        }
    }

    /// Fold the WAL back into the database and truncate it (plan 9a A8), as
    /// a delete does: once, after a purge, for its deletes and its kernel
    /// steps' pages. Each of its waits for readers is `CHECKPOINT_WAIT`
    /// (one held up takes a few seconds in all); one that holds it up
    /// leaves it owed and retried apart, as a delete's (`Checkpoints::run`).
    /// An in-memory store has none.
    pub fn checkpoint(&self) {
        if let Some(checkpoints) = &self.checkpoints {
            checkpoints.run();
        }
    }

    /// The session's catalogue (ACP core §9): its config options and
    /// current values, and its slash commands; `None` for an unknown
    /// session or a tombstone, an empty catalogue for one whose host has
    /// reported none.
    pub fn catalog(&self, session_id: &str) -> Result<Option<SessionCatalog>> {
        let row: Option<(ConfigColumns, Option<String>, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT s.model, s.mode, s.config_axes, c.config_options, c.commands
                 FROM sessions s LEFT JOIN session_catalog c ON c.session_id = s.id AND c.owner_id = s.owner_id
                 WHERE s.id = ?1 AND s.lifecycle <> 'deleted' AND s.owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((config, options, commands)) = row else {
            return Ok(None);
        };
        let list = |json: Option<String>| -> Result<Vec<Value>> {
            Ok(match json {
                Some(json) => serde_json::from_str(&json)?,
                None => Vec::new(),
            })
        };
        Ok(Some(SessionCatalog {
            session_id: session_id.to_string(),
            config_options: list(options)?,
            commands: list(commands)?,
            current: stored_config(config)?,
        }))
    }

    /// A session's open pending requests, oldest first (ACP core §9: the
    /// session detail).
    pub fn open_pending(&self, session_id: &str) -> Result<Vec<PendingItem>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PENDING_COLUMNS} WHERE p.session_id = ?1 AND p.state = 'open' AND p.owner_id = ?2 ORDER BY p.rowid"
        ))?;
        let rows = stmt.query_map([session_id, &self.owner], PendingRow::read)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.item()?);
        }
        Ok(out)
    }

    /// One pending request, whatever its state (SSE `pending_changed`).
    pub fn pending_item(&self, pending_id: &str) -> Result<Option<PendingItem>> {
        let row = self
            .conn()
            .query_row(
                &format!("SELECT {PENDING_COLUMNS} WHERE p.pending_id = ?1 AND p.owner_id = ?2"),
                [pending_id, &self.owner],
                PendingRow::read,
            )
            .optional()?;
        row.map(PendingRow::item).transpose()
    }

    /// Accept an operator's answer to an open pending request of
    /// `session_id` (ACP core §4.6): validated against the stored kind and
    /// option ids, written as `answer_submitted` and queued durably. At most
    /// one answer per request: the check and the insert are one
    /// transaction, and the queue's key is the pending id.
    pub fn submit_answer(
        &self,
        session_id: &str,
        pending_id: &str,
        answer: &AnswerRequest,
    ) -> Result<AnswerSubmission> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, String, Option<String>, String, bool)> = tx
            .query_row(
                "SELECT p.kind, p.state, p.option_ids, s.host_id,
                        EXISTS(SELECT 1 FROM answer_queue q WHERE q.pending_id = p.pending_id AND q.owner_id = p.owner_id)
                 FROM pending p JOIN sessions s ON s.id = p.session_id AND s.owner_id = p.owner_id
                 WHERE p.pending_id = ?1 AND p.session_id = ?2 AND p.owner_id = ?3",
                params![pending_id, session_id, self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((kind, state, option_ids, host_id, queued)) = row else {
            return Ok(AnswerSubmission::NotFound);
        };
        if state != "open" {
            return Ok(AnswerSubmission::NotOpen);
        }
        if queued {
            return Ok(AnswerSubmission::AlreadyAnswered);
        }
        let option_ids: Option<Vec<String>> = option_ids.map(|ids| serde_json::from_str(&ids)).transpose()?;
        if let Err(why) = check_answer(untag(kind)?, option_ids.as_deref(), answer) {
            return Ok(AnswerSubmission::Invalid(why));
        }
        let request_id = uuid::Uuid::now_v7().to_string();
        let ts = now();
        tx.execute(
            "INSERT INTO answer_queue(pending_id, session_id, request_id, answer, submitted_at, owner_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                pending_id,
                session_id,
                request_id,
                serde_json::to_string(answer)?,
                ts,
                self.owner
            ],
        )?;
        let body = json!({ "pending_id": pending_id, "request_id": request_id, "answer": answer });
        let event = collector_event(&tx, &self.owner, session_id, "answer_submitted", body, &ts)?;
        tx.commit()?;
        Ok(AnswerSubmission::Queued(Box::new(QueuedAnswer {
            event,
            host_id,
            frame: answer_frame(request_id.clone(), session_id, pending_id, answer.clone()),
            request_id,
        })))
    }

    /// The answers still owed to `host_id` (ACP core §4.6, §5.1): queued,
    /// with no verdict, for a question still open; oldest first. Sent after
    /// every handshake's reconciliation, so one lost with a connection goes
    /// again; the host dedupes by pending id.
    pub fn answers_to_send(&self, host_id: &str) -> Result<Vec<CollectorFrame>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT q.request_id, q.session_id, q.pending_id, q.answer
             FROM answer_queue q
                 JOIN pending p ON p.pending_id = q.pending_id AND p.owner_id = q.owner_id
                 JOIN sessions s ON s.id = q.session_id AND s.owner_id = q.owner_id
             WHERE s.host_id = ?1 AND q.delivered IS NULL AND p.state = 'open' AND q.owner_id = ?2
             ORDER BY q.rowid",
        )?;
        let rows = stmt.query_map([host_id, &self.owner], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (request_id, session_id, pending_id, answer) = row?;
            out.push(answer_frame(
                request_id,
                &session_id,
                &pending_id,
                serde_json::from_str(&answer)?,
            ));
        }
        Ok(out)
    }

    /// A turn's state: `sent`, `started`, `ended` or `not_delivered`.
    pub fn turn_state(&self, turn_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT state FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
                [turn_id, &self.owner],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// How a turn ended, once it has (`None` while it is open, or if it was
    /// never delivered).
    pub fn ended_turn_outcome(&self, turn_id: &str) -> Result<Option<TurnOutcome>> {
        let outcome: Option<String> = self
            .conn()
            .query_row(
                "SELECT outcome FROM turns WHERE turn_id = ?1 AND state = 'ended' AND owner_id = ?2",
                [turn_id, &self.owner],
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
        self.open_turn_with(session_id, turn_id, content, &[])
    }

    /// Open a turn for a checked prompt (plan 6a): the turn holds the
    /// stored blocks and its links to the images (plan 9a decision 6), and
    /// each image gets the owner's row, if it has none yet. Its file must
    /// be saved first (`save_images`).
    pub fn open_prompt(&self, session_id: &str, turn_id: &str, prompt: &Checked) -> Result<bool> {
        self.open_turn_with(session_id, turn_id, &prompt.stored_json(), &prompt.images)
    }

    fn open_turn_with(&self, session_id: &str, turn_id: &str, content: &[Value], images: &[Image]) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE sessions SET open_turn_id = ?2
             WHERE id = ?1 AND lifecycle = 'active' AND open_turn_id IS NULL AND owner_id = ?3",
            params![session_id, turn_id, self.owner],
        )?;
        if changed == 1 {
            let ts = now();
            tx.execute(
                "INSERT INTO turns(turn_id, session_id, content, created_at, owner_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![turn_id, session_id, serde_json::to_string(content)?, ts, self.owner],
            )?;
            for image in images {
                // The row and its turn's link (`link_turn_attachments`) go in
                // this one transaction: the sweep gives rows no grace, and
                // deletes one no turn or event links.
                tx.execute(
                    "INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(owner_id, sha256) DO NOTHING",
                    params![self.owner, image.sha256, image.mime, image.bytes.len() as i64, ts],
                )?;
                // A delete since `save_images` may have removed the file
                // (plan 9a decision 6). Under the store's lock, as a delete
                // removes files, it is written again if it is gone.
                if let Some(dir) = self.attachments.as_deref() {
                    crate::attachments::write(dir, &image.sha256, &image.bytes)
                        .with_context(|| format!("store attachment {}", image.sha256))?;
                }
            }
            link_turn_attachments(&tx, &self.owner, turn_id, content)?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Save each image's file (plan 6a), before `open_prompt` records it.
    /// An image already stored is kept as it is, its mtime refreshed: until
    /// `open_prompt` records it, no row may name it, and only the sweep's
    /// grace keeps it (plan 9b decision 9).
    pub fn save_images(&self, images: &[Image]) -> Result<()> {
        if images.is_empty() {
            return Ok(());
        }
        let dir = self
            .attachments
            .as_deref()
            .context("an in-memory store keeps no attachment files")?;
        for image in images {
            let refreshed = crate::attachments::refresh(dir, &image.sha256)
                .with_context(|| format!("refresh attachment {}", image.sha256))?;
            if !refreshed {
                crate::attachments::write(dir, &image.sha256, &image.bytes)
                    .with_context(|| format!("store attachment {}", image.sha256))?;
            }
        }
        Ok(())
    }

    /// One of the owner's images, by its hash: `None` for a name that is
    /// not a hash, an image of no row of the owner's, or one whose file is
    /// gone.
    pub fn attachment(&self, sha256: &str) -> Result<Option<Attachment>> {
        if !crate::attachments::is_sha256(sha256) {
            return Ok(None);
        }
        let mime: Option<String> = self
            .conn()
            .query_row(
                "SELECT mime FROM attachments WHERE owner_id = ?1 AND sha256 = ?2",
                [&self.owner, sha256],
                |r| r.get(0),
            )
            .optional()?;
        let (Some(mime), Some(dir)) = (mime, self.attachments.as_deref()) else {
            return Ok(None);
        };
        match crate::attachments::read(dir, sha256)? {
            Some(bytes) => Ok(Some(Attachment { mime, bytes })),
            None => {
                tracing::warn!(%sha256, "an attachment's file is missing");
                Ok(None)
            }
        }
    }

    /// The owner's images, each counted once, and their size (plan 6a).
    pub fn attachment_usage(&self) -> Result<AttachmentUsage> {
        let (count, bytes): (i64, i64) = self.conn().query_row(
            "SELECT count(*), coalesce(sum(size), 0) FROM attachments WHERE owner_id = ?1",
            [&self.owner],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(AttachmentUsage {
            count: count as u64,
            bytes: bytes as u64,
        })
    }

    /// Undo `open_turn` after the host rejected the prompt. The turn's
    /// images go with it unless something else of the owner's shows them
    /// (plan 9a decision 6): their rows in this transaction, their files
    /// after it.
    pub fn abandon_turn(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET open_turn_id = NULL WHERE id = ?1 AND open_turn_id = ?2 AND owner_id = ?3",
            params![session_id, turn_id, self.owner],
        )?;
        let hashes: BTreeSet<String> = {
            let mut stmt = tx.prepare("SELECT sha256 FROM turn_attachments WHERE turn_id = ?1 AND owner_id = ?2")?;
            let rows = stmt.query_map([turn_id, &self.owner], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        // Its links go with it (`ON DELETE CASCADE`).
        tx.execute(
            "DELETE FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
            [turn_id, &self.owner],
        )?;
        let dropped = drop_unreferenced(&tx, &self.owner, &hashes)?;
        tx.commit()?;
        self.remove_files(&conn, &dropped);
        Ok(())
    }

    /// After the commit that dropped their rows, and still under the store's
    /// lock, so no prompt records one of them meanwhile (plan 9a decision
    /// 6): remove each image's file unless a row of any owner still names
    /// it, since files are shared by hash. A file gone already is fine. One
    /// that cannot be removed is logged and left: its rows are gone, and
    /// plan 9b's sweep removes a file no row names (decision 7).
    fn remove_files(&self, conn: &Connection, dropped: &[String]) {
        let Some(dir) = self.attachments.as_deref() else {
            return;
        };
        for sha256 in dropped {
            let removed = crate::shared_files::hash_named_by_any_owner(conn, sha256).and_then(|named| {
                if !named {
                    crate::attachments::remove(dir, sha256)?;
                }
                Ok(())
            });
            if let Err(err) = removed {
                tracing::warn!(%sha256, "an unreferenced attachment's file was left: {err:#}");
            }
        }
    }

    /// Sweep the attachments (plan 9b decision 9, A14), at `now`:
    /// - the owner's rows that no turn and no event of theirs shows go;
    /// - then `attachments/` is listed without the store's lock, and only
    ///   two kinds of name are taken from it: an image's (`is_sha256`) and
    ///   a write's temporary file (`is_temp`). Under the lock, a few at a
    ///   time, each is looked at again and removed if it is a regular file
    ///   (never through a link), its mtime is older than `sweep::GRACE`,
    ///   and, for an image, no row of any owner names it.
    ///
    /// The checks are made under the lock so they hold at the unlink:
    /// `open_prompt` records an image under it (and re-writes a file gone
    /// meanwhile, decision 6), and `save_images`, which does not take it,
    /// refreshes a file's mtime first. A file that cannot be looked at or
    /// removed is logged and left for the next sweep. Once `cancel` fires
    /// (the collector's shutdown), no further batch is begun.
    pub fn sweep_attachments(
        &self,
        now: std::time::SystemTime,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<SweepReport> {
        let rows = self.conn().execute(
            "DELETE FROM attachments WHERE owner_id = ?1
                 AND NOT EXISTS (SELECT 1 FROM turn_attachments
                     WHERE turn_attachments.owner_id = ?1 AND turn_attachments.sha256 = attachments.sha256)
                 AND NOT EXISTS (SELECT 1 FROM event_attachments
                     WHERE event_attachments.owner_id = ?1 AND event_attachments.sha256 = attachments.sha256)",
            [&self.owner],
        )?;
        let mut report = SweepReport {
            rows: rows as u64,
            ..SweepReport::default()
        };
        let Some(dir) = self.attachments.as_deref() else {
            return Ok(report);
        };
        let listed = match std::fs::read_dir(dir) {
            Ok(listed) => listed,
            // Nothing sent yet.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(err) => return Err(err).context("list the attachment files"),
        };
        let mut names = Vec::new();
        for entry in listed {
            let Ok(name) = entry.context("list the attachment files")?.file_name().into_string() else {
                continue;
            };
            if crate::attachments::is_sha256(&name) || crate::attachments::is_temp(&name) {
                names.push(name);
            }
        }
        for batch in names.chunks(SWEEP_BATCH) {
            if cancel.is_cancelled() {
                break;
            }
            let conn = self.conn();
            for name in batch {
                match crate::sweep::sweep_file(&conn, dir, name, now) {
                    Ok(Some(crate::sweep::Swept::Image)) => report.files += 1,
                    Ok(Some(crate::sweep::Swept::Temp)) => report.temps += 1,
                    Ok(None) => {}
                    Err(err) => tracing::warn!(%name, "an attachment file was not swept: {err:#}"),
                }
            }
        }
        Ok(report)
    }

    /// Delete a session (ACP core §4.10; plan 9a decisions 1, 5 and 6), in
    /// one transaction:
    /// - a tombstone, or no session, is `NotFound`;
    /// - one not `closed` is refused with its lifecycle, unless it is still
    ///   exactly what the route judged `unattached` (A4): then it is closed
    ///   here first, as `close_now` closes, and the delete is `unconfirmed`
    ///   if its host may still run it;
    /// - its events, turns (their image links with them), questions,
    ///   answers and catalogue are deleted, and the owner's images nothing
    ///   else of theirs shows; its project recent too, unless another kept
    ///   session of that host and hat has that cwd (R1–R4);
    /// - `session_deleted` is written, and the row is scrubbed to a
    ///   tombstone: `deleted`, keeping only its id, owner, host, hat,
    ///   creation and recency.
    ///
    /// Then, still under the store's lock, the images' files that no owner
    /// names any more go; once it is released, the WAL is checkpointed so
    /// the deleted pages leave it too (A8). Both best-effort. A crash before the files go
    /// leaves files no row names, for plan 9b's sweep (decision 7).
    pub fn delete_session(&self, session_id: &str, unattached: Option<&Unattached>) -> Result<Deletion> {
        let deletion = self.delete_rows(session_id, unattached)?;
        // Only a delete that deleted something owes one.
        if matches!(deletion, Deletion::Done { .. })
            && let Some(checkpoints) = &self.checkpoints
        {
            checkpoints.run();
        }
        Ok(deletion)
    }

    /// `delete_session` for a purge (plan 9c decision 10d), without its
    /// checkpoint: a reader that holds the WAL holds each checkpoint up by
    /// seconds, so one per session would stall a purge of hundreds for
    /// minutes. The purge records the debt first
    /// (`owe_checkpoint`) and pays it once after its last delete
    /// (`checkpoint`).
    pub fn delete_session_owing_checkpoint(
        &self,
        session_id: &str,
        unattached: Option<&Unattached>,
    ) -> Result<Deletion> {
        self.delete_rows(session_id, unattached)
    }

    /// The delete's rows, in one transaction, and its files after the
    /// commit, under the store's lock (`delete_session`).
    fn delete_rows(&self, session_id: &str, unattached: Option<&Unattached>) -> Result<Deletion> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // Its host, hat and cwd are read before the scrub clears them (R1).
        let row: Option<(String, bool, String, String, String)> = tx
            .query_row(
                "SELECT lifecycle, presumed_parked, host_id, hat_id, cwd FROM sessions
                 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((lifecycle, presumed, host_id, hat_id, cwd)) = row else {
            return Ok(Deletion::NotFound);
        };
        let mcp = self.mcp();
        let mut cut = Cut::default();
        let mut unconfirmed = false;
        if lifecycle != "closed" {
            match unattached {
                Some(judged) if judged.lifecycle == lifecycle && judged.presumed_parked == presumed => {
                    cut = close_in(&tx, &self.owner, mcp.as_ref(), session_id)?.1;
                    // Only its host can still run it: presumed parked, or
                    // starting or active on a host the route cannot reach.
                    unconfirmed = presumed || matches!(lifecycle.as_str(), "starting" | "active");
                }
                _ => return Ok(Deletion::Refused(lifecycle)),
            }
        }
        let hashes: BTreeSet<String> = {
            let mut stmt = tx.prepare(
                "SELECT sha256 FROM turn_attachments
                 WHERE owner_id = ?2 AND turn_id IN (SELECT turn_id FROM turns WHERE session_id = ?1 AND owner_id = ?2)
                 UNION
                 SELECT sha256 FROM event_attachments
                 WHERE owner_id = ?2 AND event_id IN (SELECT event_id FROM events WHERE session_id = ?1 AND owner_id = ?2)",
            )?;
            let rows = stmt.query_map([session_id, &self.owner], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        // Children before their parents: an event's image links before the
        // event, an answer before its question. A turn's links go with it
        // (`ON DELETE CASCADE`).
        tx.execute(
            "DELETE FROM event_attachments WHERE owner_id = ?2
                 AND event_id IN (SELECT event_id FROM events WHERE session_id = ?1 AND owner_id = ?2)",
            [session_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM answer_queue WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM pending WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM session_catalog WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM turns WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM events WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        // The kernel's table, on this transaction (R3). The path compares
        // byte for byte (`=`, the column's BINARY collation; R4), and only a
        // session kept counts (R2): this one is not yet a tombstone, so it
        // is left out by its id. A tombstone's cwd is '' already; its filter
        // guards a later change of the scrub.
        tx.execute(
            "DELETE FROM project_recents WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3 AND path = ?4
                 AND NOT EXISTS (SELECT 1 FROM sessions
                     WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3 AND cwd = ?4 AND id <> ?5
                         AND lifecycle <> 'deleted')",
            params![self.owner, host_id, hat_id, cwd, session_id],
        )?;
        let dropped = drop_unreferenced(&tx, &self.owner, &hashes)?;
        let event = collector_event(&tx, &self.owner, session_id, "session_deleted", json!({}), &now())?;
        let scrubbed = tx.execute(
            "UPDATE sessions SET lifecycle = 'deleted', cwd = '', agent = '', title = NULL, git_branch = NULL,
                 git_dirty = NULL, git_worktree = NULL, base_commit = NULL, model = NULL, mode = NULL,
                 config_axes = NULL, agent_session_id = NULL, failure_reason = NULL, hat_rule_id = NULL,
                 open_turn_id = NULL, activity = NULL, presumed_parked = 0, close_requested = 0,
                 mcp_delivery_mode = NULL, mcp_delivery_servers = NULL, mcp_delivery_at = NULL
             WHERE id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        // Read in this transaction; an event without its tombstone would be
        // worse than an error.
        anyhow::ensure!(scrubbed == 1, "tombstoning {session_id} changed {scrubbed} rows");
        // Its token, in this transaction (lane L4; the purge lane's marker):
        // a parked or failed session's is revoked already, a no-op then.
        let cut = cut.and(mcp.revoke_in(&tx, session_id)?);
        tx.commit()?;
        mcp.cut(cut);
        self.remove_files(&conn, &dropped);
        Ok(Deletion::Done { event, unconfirmed })
    }

    /// Record an operator park before `park_session` is sent.
    pub fn record_park_request(&self, session_id: &str) -> Result<EventDto> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let event = collector_event(&tx, &self.owner, session_id, "operator_parked", json!({}), &now())?;
        tx.commit()?;
        Ok(event)
    }

    /// Record an operator close of an attached session before
    /// `close_session` is sent. The intent is durable: if the host never
    /// confirms, the next handshake sends `close_session` again. For a
    /// tombstone it fails as for an unknown session, and writes nothing.
    pub fn record_close_request(&self, session_id: &str) -> Result<EventDto> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET close_requested = 1 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        let event = collector_event(&tx, &self.owner, session_id, "operator_closed", json!({}), &now())?;
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
        let mcp = self.mcp();
        let (events, cut) = close_in(&tx, &self.owner, mcp.as_ref(), session_id)?;
        tx.commit()?;
        mcp.cut(cut);
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
                // A tombstone's `close_requested` is 0 already; the filter
                // guards a later edit of the predicate.
                "SELECT close_requested = 1 AND (lifecycle = 'active' OR presumed_parked = 1)
                 FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                [session_id, &self.owner],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let mcp = self.mcp();
        let (events, cut) = if still_requested {
            close_in(&tx, &self.owner, mcp.as_ref(), session_id)?
        } else {
            (Vec::new(), Cut::default())
        };
        tx.commit()?;
        mcp.cut(cut);
        Ok(events)
    }

    /// `request_resume_with_mcp` for a host that takes no MCP servers: what
    /// the store's own tests resume.
    pub fn request_resume(&self, session_id: &str, hat_id: &str) -> Result<ResumeRequest> {
        self.request_resume_with_mcp(session_id, hat_id, McpContext::NONE)
    }

    /// Move a `parked`, `closed` or `failed` session to `starting` for a
    /// resume (ACP core §4.2), if its path still resolves to its own hat:
    /// `hat_id` is what it resolves to now (ACP core §4.3). Atomic: of two
    /// concurrent resumes, the second sees `starting` and is refused (§12
    /// scenario 11), and a re-assignment that lands in between is seen. A
    /// turn still open (a database written before plan B) is released
    /// first. In the same transaction (lane L1): the delivery decision and
    /// the session's MCP servers, a fresh token minted that supersedes the
    /// one before (ACP core §4.3), so a failed mint leaves the session as
    /// it was.
    pub fn request_resume_with_mcp(&self, session_id: &str, hat_id: &str, mcp: McpContext) -> Result<ResumeRequest> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<ResumeRow> = tx
            .query_row(
                "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes, hat_id FROM sessions
                 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                [session_id, &self.owner],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        (r.get(3)?, r.get(4)?, r.get(5)?),
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((lifecycle, agent_session_id, open_turn, config, stored_hat)) = row else {
            return Ok(ResumeRequest::NotFound);
        };
        if !matches!(lifecycle.as_str(), "parked" | "closed" | "failed") {
            return Ok(ResumeRequest::Busy(lifecycle));
        }
        let Some(agent_session_id) = agent_session_id else {
            return Ok(ResumeRequest::NoRecord);
        };
        // Before the mismatch: the freeze took the hat's rules, so its
        // path resolves to another hat by now.
        if hat_frozen(&tx, &self.owner, &stored_hat)? {
            return Ok(ResumeRequest::HatPurging);
        }
        if stored_hat != hat_id {
            return Ok(ResumeRequest::HatMismatch(stored_hat));
        }
        let ts = now();
        let mut events = Vec::new();
        if let Some(turn) = open_turn.as_deref() {
            events.push(resolve_open_turn(&tx, &self.owner, session_id, turn, &ts)?);
        }
        events.push(collector_event(
            &tx,
            &self.owner,
            session_id,
            "operator_resumed",
            json!({}),
            &ts,
        )?);
        tx.execute(
            "UPDATE sessions SET lifecycle = 'starting', activity = NULL, failure_reason = NULL,
                 close_requested = 0, open_turn_id = NULL, presumed_parked = 0
             WHERE id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
        )?;
        let committed: Option<i64> = tx.query_row(
            "SELECT MAX(host_seq) FROM events WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
            |r| r.get(0),
        )?;
        let host_id: String = tx.query_row(
            "SELECT host_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
            |r| r.get(0),
        )?;
        let config = stored_config(config)?;
        let (given, cut) = self.deliver_in(&tx, session_id, &host_id, &stored_hat, mcp, &ts)?;
        tx.commit()?;
        self.mcp().cut(cut);
        Ok(ResumeRequest::Starting {
            events,
            agent_session_id,
            committed_seq: committed.unwrap_or(0) as u64,
            config,
            mcp: given,
        })
    }

    /// Move a session with no running adapter to another hat (ACP core
    /// §4.9): `parked` (and not only presumed so), `closed` or `failed`. The
    /// hat must be the owner's. Writes `hat_reassigned{from, to}`. The rule
    /// that decided the old hat no longer did, so it is cleared. The next
    /// resume must agree with the new hat (`hat_mismatch` otherwise).
    pub fn reassign_hat(&self, session_id: &str, hat_id: &str) -> Result<Reassign> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, bool, String)> = tx
            .query_row(
                "SELECT lifecycle, presumed_parked, hat_id FROM sessions
                 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((lifecycle, presumed, from)) = row else {
            return Ok(Reassign::NotFound);
        };
        if presumed {
            return Ok(Reassign::Attached("presumed_parked".into()));
        }
        if !matches!(lifecycle.as_str(), "parked" | "closed" | "failed") {
            return Ok(Reassign::Attached(lifecycle));
        }
        let known = tx
            .query_row(
                "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2",
                [hat_id, &self.owner],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !known {
            return Ok(Reassign::UnknownHat);
        }
        if hat_frozen(&tx, &self.owner, &from)? || hat_frozen(&tx, &self.owner, hat_id)? {
            return Ok(Reassign::HatPurging);
        }
        if from == hat_id {
            return Ok(Reassign::Unchanged);
        }
        let changed = tx.execute(
            "UPDATE sessions SET hat_id = ?2, hat_rule_id = NULL
             WHERE id = ?1 AND lifecycle IN ('parked', 'closed', 'failed') AND presumed_parked = 0 AND owner_id = ?3",
            params![session_id, hat_id, self.owner],
        )?;
        // The checks above read the same row in this transaction; an event
        // without the change it records would be worse than an error.
        anyhow::ensure!(changed == 1, "re-assigning {session_id} changed {changed} rows");
        let event = collector_event(
            &tx,
            &self.owner,
            session_id,
            "hat_reassigned",
            json!({ "from": from, "to": hat_id }),
            &now(),
        )?;
        // A token's hat is fixed at its mint (plan 8d's O8): one still live
        // would reach the old hat's connections. No adapter runs, so none
        // should be; revoked here whatever a missed revoke left (lane L4).
        let mcp = self.mcp();
        let cut = mcp.revoke_in(&tx, session_id)?;
        tx.commit()?;
        mcp.cut(cut);
        Ok(Reassign::Done(event))
    }

    /// The host has been offline past the threshold: presume its `active`
    /// sessions parked (ACP core §5.3). Their open turns stay open, since
    /// the host may still be running them.
    pub fn presume_parked(&self, host_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM sessions
                 WHERE host_id = ?1 AND lifecycle = 'active' AND lifecycle <> 'deleted' AND owner_id = ?2 ORDER BY id",
            )?;
            let rows = stmt.query_map([host_id, &self.owner], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut events = Vec::new();
        for id in &ids {
            events.push(collector_event(
                &tx,
                &self.owner,
                id,
                "presumed_parked",
                json!({ "reason": "host_offline" }),
                &ts,
            )?);
            tx.execute(
                "UPDATE sessions SET lifecycle = 'parked', presumed_parked = 1 WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
            )?;
        }
        tx.commit()?;
        Ok(events)
    }

    /// The host was revoked (kernel spec §4.3). It never connects again, so
    /// nothing will ever reconcile its sessions:
    /// - a start or resume still in flight fails `host_revoked`;
    /// - every session it ran (`active`, or presumed parked while it was
    ///   away) is presumed parked for good (`presumed_parked{host_revoked}`),
    ///   its open turn ends (`interrupted` if the adapter had it) and its
    ///   open questions are cancelled `host_revoked`, which also gives any
    ///   queued answer its `delivered: false`;
    /// - one the operator had asked to close is closed instead.
    ///
    /// Idempotent, but only a session that is *currently* converged (still
    /// `parked`, presumed for this revoke, with no open question) is left as
    /// it is. A revoke whose wait for the connection timed out can still be
    /// reconciled by that connection's late `resend_complete` before it is
    /// gone — `reconcile_host` treats a presumed park as the active session
    /// it may still be and reattaches it — or that connection can still
    /// deliver a turnless question on top of it (fix round 1, F1): either
    /// leaves the session looking "already parked for this revoke" by its
    /// last `presumed_parked` event alone, so a repeated revoke must check
    /// its current state, not just that event, and (re)park it if it does
    /// not actually match.
    pub fn revoke_host(&self, host_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let starting: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM sessions
                     WHERE host_id = ?1 AND lifecycle = 'starting' AND lifecycle <> 'deleted' AND owner_id = ?2",
            )?;
            let rows = stmt.query_map([host_id, &self.owner], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut events = Vec::new();
        for id in &starting {
            // Consistent with how reconciliation reports its own analogous
            // `starting` failure (`start_not_delivered`): the timeline gets
            // an event, not just a silent column change.
            events.push(collector_event(
                &tx,
                &self.owner,
                id,
                "start_not_delivered",
                json!({}),
                &ts,
            )?);
            tx.execute(
                "UPDATE sessions SET lifecycle = 'failed', failure_reason = 'host_revoked' WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
            )?;
        }
        let rows: Vec<(String, Option<String>, String, bool)> = {
            let mut stmt = tx.prepare(
                "SELECT id, open_turn_id, lifecycle, presumed_parked FROM sessions
                 WHERE host_id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1) AND lifecycle <> 'deleted'
                     AND owner_id = ?2
                 ORDER BY id",
            )?;
            let rows = stmt.query_map([host_id, &self.owner], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, open_turn, lifecycle, presumed_parked) in rows {
            let presumed_for: Option<Option<String>> = tx
                .query_row(
                    "SELECT json_extract(body, '$.reason') FROM events
                     WHERE session_id = ?1 AND kind = 'presumed_parked' AND owner_id = ?2
                     ORDER BY event_id DESC LIMIT 1",
                    [&id, &self.owner],
                    |r| r.get(0),
                )
                .optional()?;
            let has_open_pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND state = 'open' AND owner_id = ?2)",
                [&id, &self.owner],
                |r| r.get(0),
            )?;
            let already_converged = lifecycle == "parked"
                && presumed_parked
                && presumed_for.flatten().as_deref() == Some("host_revoked")
                && !has_open_pending;
            if already_converged {
                continue;
            }
            events.push(collector_event(
                &tx,
                &self.owner,
                &id,
                "presumed_parked",
                json!({ "reason": "host_revoked" }),
                &ts,
            )?);
            if let Some(turn) = open_turn {
                events.push(resolve_open_turn(&tx, &self.owner, &id, &turn, &ts)?);
            }
            events.extend(cancel_open_pending(
                &tx,
                &self.owner,
                &id,
                PendingReason::HostRevoked,
                &ts,
            )?);
            tx.execute(
                "UPDATE sessions SET
                     lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                     presumed_parked = CASE WHEN close_requested = 1 THEN 0 ELSE 1 END,
                     activity = NULL, open_turn_id = NULL, close_requested = 0
                 WHERE id = ?1 AND owner_id = ?2",
                [&id, &self.owner],
            )?;
        }
        // Every token of the host, in this transaction (ACP core §4.8, lane
        // L4): the proxy refuses a revoked host's tokens by its join anyway,
        // but a token row must not outlive its host's revoke as live.
        let mcp = self.mcp();
        let cut = mcp.revoke_host_in(&tx, host_id)?;
        tx.commit()?;
        mcp.cut(cut);
        Ok(events)
    }

    /// Hosts the collector believes are running at least one session.
    pub fn hosts_with_active_sessions(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT host_id FROM sessions
             WHERE lifecycle = 'active' AND lifecycle <> 'deleted' AND owner_id = ?1 ORDER BY host_id",
        )?;
        let rows = stmt.query_map([&self.owner], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Highest committed host seq for a session (0 if none).
    pub fn committed_seq(&self, session_id: &str) -> Result<u64> {
        let v: Option<i64> = self.conn().query_row(
            "SELECT MAX(host_seq) FROM events WHERE session_id = ?1 AND owner_id = ?2",
            [session_id, &self.owner],
            |r| r.get(0),
        )?;
        Ok(v.unwrap_or(0) as u64)
    }

    /// Ingest one sequenced host frame (`ingest_fact`): the events it
    /// created, in order.
    pub fn ingest(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Vec<EventDto>> {
        Ok(self.ingest_fact(session_id, seq, body)?.events)
    }

    /// Ingest one sequenced host frame. Idempotent on (session_id, seq): a
    /// duplicate with the same body is discarded; one with a different body
    /// is kept as a `conflict` event (ACP core §3.6). Returns the events it
    /// created, in order, and the push edge it crossed, if any (ACP core
    /// §10; plan 10b): read in the same transaction, from what the fact
    /// changed, so a duplicate, a fact stored but not applied, and a fact
    /// for an already ended turn cross none. A frame for a session that is
    /// not the owner's fails, and nothing is written. One for a tombstone
    /// creates nothing and stores nothing, and is not an error (plan 9a
    /// decision 4, A1): the host's frame is acked, so it prunes its outbox.
    pub fn ingest_fact(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Ingested> {
        // Before anything reads it (plan 8e decision 11): a session token
        // the agent printed is stored, compared, extracted and published
        // only redacted.
        let redacted = crate::redact::body(body)?;
        let body = redacted.as_ref().unwrap_or(body);
        let mcp = self.mcp();
        let mut cut = Cut::default();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let lifecycle: Option<String> = tx
            .query_row(
                "SELECT lifecycle FROM sessions WHERE id = ?1 AND owner_id = ?2",
                [session_id, &self.owner],
                |r| r.get(0),
            )
            .optional()?;
        let Some(lifecycle) = lifecycle else {
            anyhow::bail!("no session {session_id}");
        };
        if lifecycle == "deleted" {
            return Ok(Ingested {
                events: Vec::new(),
                edge: None,
            });
        }
        let ts = now();
        let kind = body_kind(body);
        let received = serde_json::to_value(body)?;
        let inserted = tx.execute(
            "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id, host_seq) DO NOTHING",
            params![session_id, seq as i64, kind, received.to_string(), ts, self.owner],
        )?;
        if inserted == 0 {
            let stored: String = tx.query_row(
                "SELECT body FROM events WHERE session_id = ?1 AND host_seq = ?2 AND owner_id = ?3",
                params![session_id, seq as i64, self.owner],
                |r| r.get(0),
            )?;
            // Structural comparison: key order is not stable across builds
            // (serde_json's `preserve_order` is feature-unified).
            let created = if serde_json::from_str::<Value>(&stored)? == received {
                Vec::new()
            } else if conflict_already_recorded(&tx, &self.owner, session_id, seq, &received)? {
                // A re-sent conflicting frame: already on record, ruling D.
                Vec::new()
            } else {
                vec![collector_event(
                    &tx,
                    &self.owner,
                    session_id,
                    "conflict",
                    json!({ "seq": seq, "received": received }),
                    &ts,
                )?]
            };
            tx.commit()?;
            return Ok(Ingested {
                events: created,
                edge: None,
            });
        }
        let fact_id = tx.last_insert_rowid();
        let mut edge = None;
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
                     WHERE id = ?1 AND lifecycle IN ('starting', 'failed') AND owner_id = ?3",
                    params![session_id, agent_session_id, self.owner],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    // The catalogue after the start's switches (P-13).
                    store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
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
                     WHERE id = ?1 AND owner_id = ?3 AND (lifecycle = 'starting'
                         OR (lifecycle = 'failed' AND failure_reason = 'start_not_delivered'))",
                    params![session_id, code, self.owner],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    // The start that failed holds its token no more (lane
                    // L4). Only when it applied: a late one must not end a
                    // resume's fresh token.
                    cut = mcp.revoke_in(&tx, session_id)?;
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
                    .query_row(
                        "SELECT state FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
                        [turn_id, &self.owner],
                        |r| r.get(0),
                    )
                    .optional()?;
                let (lifecycle, slot, presumed): (String, Option<String>, bool) = tx.query_row(
                    "SELECT lifecycle, open_turn_id, presumed_parked FROM sessions WHERE id = ?1 AND owner_id = ?2",
                    [session_id, &self.owner],
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
                                .query_row(
                                    "SELECT state FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
                                    [other, &self.owner],
                                    |r| r.get(0),
                                )
                                .optional()?;
                            if other_state.as_deref() == Some("sent") {
                                created.push(turn_not_delivered(&tx, &self.owner, session_id, other, &ts)?);
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
                        "UPDATE sessions SET activity = 'running', open_turn_id = ?2 WHERE id = ?1 AND owner_id = ?3",
                        params![session_id, turn_id, self.owner],
                    )?;
                    tx.execute(
                        "UPDATE turns SET state = 'started' WHERE turn_id = ?1 AND owner_id = ?2",
                        [turn_id, &self.owner],
                    )?;
                    // The user's turn is recorded only once the adapter has
                    // it (ACP core §4.4), in seq order before the turn's
                    // updates.
                    let content: Option<String> = tx
                        .query_row(
                            "SELECT content FROM turns WHERE turn_id = ?1 AND owner_id = ?2",
                            [turn_id, &self.owner],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(content) = content {
                        let body = json!({ "turn_id": turn_id, "content": serde_json::from_str::<Value>(&content)? });
                        let user_turn = collector_event(&tx, &self.owner, session_id, "user_turn", body, &ts)?;
                        link_attachments(&tx, &self.owner, user_turn.event_id, &user_turn.body["content"])?;
                        created.push(user_turn);
                    }
                } else {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
            SessionBody::TurnEnded { turn_id, outcome, .. } => {
                // Applied only to the open turn; a late duplicate for an
                // already-ended turn is stored but not applied, and (ACP core
                // §4.4) must never be pushed to a caller — only the store
                // knows whether the transition actually applied.
                let applied = tx.execute(
                    "UPDATE sessions SET open_turn_id = NULL, activity = 'idle'
                     WHERE id = ?1 AND open_turn_id = ?2 AND owner_id = ?3",
                    params![session_id, turn_id, self.owner],
                )?;
                // Only a `started` turn can be ended by a real `turn_ended`
                // (fix round 1, ruling B): a stray end must not jump a
                // `sent`/`not_delivered` turn straight to `ended`.
                tx.execute(
                    "UPDATE turns SET state = 'ended', outcome = ?2 WHERE turn_id = ?1 AND state = 'started' AND owner_id = ?3",
                    params![
                        turn_id,
                        serde_json::to_value(outcome)?.as_str().unwrap_or_default(),
                        self.owner
                    ],
                )?;
                if applied == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    edge = Some(PushEdge::TurnEnded(*outcome));
                }
            }
            SessionBody::SessionParked { reason } => {
                created.extend(release_turn_on_detach(&tx, &self.owner, session_id, &ts)?);
                // A park that overtakes an operator close ends the session
                // as the operator asked: closed.
                let changed = tx.execute(
                    "UPDATE sessions SET
                         lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                         activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND owner_id = ?2 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id, &self.owner],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    // The host cancels its questions before it detaches;
                    // one it left open goes with the session.
                    let reason = match reason {
                        ParkReason::AdapterExited => PendingReason::AdapterLost,
                        ParkReason::Idle | ParkReason::Operator => PendingReason::SessionParked,
                    };
                    created.extend(cancel_open_pending(&tx, &self.owner, session_id, reason, &ts)?);
                    // Detached: its token is done (lane L4).
                    cut = mcp.revoke_in(&tx, session_id)?;
                }
            }
            SessionBody::SessionClosed => {
                created.extend(release_turn_on_detach(&tx, &self.owner, session_id, &ts)?);
                // Also the host's confirmation of a close the collector
                // already made (an offline close): nothing left to change.
                let changed = tx.execute(
                    "UPDATE sessions SET lifecycle = 'closed', activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND owner_id = ?2 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id, &self.owner],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    created.extend(cancel_open_pending(
                        &tx,
                        &self.owner,
                        session_id,
                        PendingReason::SessionClosed,
                        &ts,
                    )?);
                    // Closed: its token is done (lane L4).
                    cut = mcp.revoke_in(&tx, session_id)?;
                }
            }
            SessionBody::AcpUpdate { indexed, .. } => {
                if !fact_applies(&tx, &self.owner, session_id, indexed.turn_id.as_deref())? {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    // A live `config_option_update` (the agent changed its
                    // own config); the host never sends a replayed one
                    // with extracts.
                    store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
                    store_state(&tx, &self.owner, session_id, indexed, &ts)?;
                }
            }
            SessionBody::ConfigApplied { indexed, .. } => {
                // The read-back of a switch on an attached session. A late
                // one for a session that has detached since changes nothing.
                let (lifecycle, presumed): (String, bool) = tx.query_row(
                    "SELECT lifecycle, presumed_parked FROM sessions WHERE id = ?1 AND owner_id = ?2",
                    [session_id, &self.owner],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if lifecycle == "active" || presumed {
                    store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
                } else {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
            SessionBody::PendingOpened {
                pending_id,
                indexed,
                payload,
            } => {
                // A question of the attached session, asked in a turn that
                // is still open if it names one. Everything the collector
                // keeps comes from the extract (ACP core §3.2).
                let (lifecycle, presumed): (String, bool) = tx.query_row(
                    "SELECT lifecycle, presumed_parked FROM sessions WHERE id = ?1 AND owner_id = ?2",
                    [session_id, &self.owner],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let extract = indexed.pending.as_ref().filter(|p| p.id == *pending_id);
                let applies = (lifecycle == "active" || presumed)
                    && fact_applies(&tx, &self.owner, session_id, indexed.turn_id.as_deref())?;
                let inserted = match extract.filter(|_| applies) {
                    Some(extract) => tx.execute(
                        "INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at,
                                             owner_id, opened_event_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8, ?9)
                         ON CONFLICT(pending_id) DO NOTHING",
                        params![
                            pending_id,
                            session_id,
                            tag(extract.kind)?,
                            indexed.turn_id,
                            extract.option_ids.as_ref().map(serde_json::to_string).transpose()?,
                            payload.to_string(),
                            ts,
                            self.owner,
                            fact_id
                        ],
                    )?,
                    None => 0,
                };
                if inserted == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    let blocked = tx.execute(
                        "UPDATE sessions SET activity = 'blocked' WHERE id = ?1 AND activity = 'running' AND owner_id = ?2",
                        [session_id, &self.owner],
                    )?;
                    // Edge-triggered: only the question that blocks the
                    // turn; a second one finds it blocked already. The
                    // activity decides, not the turn id: a question with
                    // none that blocks a running turn is `Blocked`. One that
                    // leaves the activity alone (ACP core §4.2), asked
                    // outside a turn, is an edge of its own.
                    let title = extract
                        .and_then(|e| e.title.as_deref())
                        .and_then(|t| one_line(t, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES));
                    // A question asked again after the agent withdrew one
                    // in the same turn notifies nothing: an agent could
                    // otherwise ask and withdraw in a loop, each one an
                    // urgent push, with no one but it pacing them
                    // (10b-i's review, A2). Answered and asked again is
                    // the operator's pace, and still notifies.
                    //
                    // A question with no turn id, in a turn or outside one,
                    // is bounded by the owner's last prompt instead (10b-i's
                    // re-confirmation, N1; plan 10b-iii's review, A1): a
                    // withdrawal counts if its question was opened after
                    // the session's latest `user_turn`, which only a turn
                    // the owner prompted writes. Ordered by event id, which
                    // only grows, not by the clock (the review's N3): the
                    // question's own (`opened_event_id`) against the
                    // prompt's, found by `events_by_kind` (A3).
                    let rewithdrawn = blocked > 0
                        && tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND pending_id <> ?3
                                 AND reason = 'agent_withdrew' AND turn_id = ?2 AND owner_id = ?4)
                             OR (?2 IS NULL AND EXISTS(SELECT 1 FROM pending WHERE session_id = ?1
                                 AND pending_id <> ?3 AND owner_id = ?4 AND turn_id IS NULL
                                 AND reason = 'agent_withdrew'
                                 AND opened_event_id > coalesce((SELECT max(event_id) FROM events
                                     WHERE session_id = ?1 AND kind = 'user_turn' AND owner_id = ?4), 0)))",
                            params![session_id, indexed.turn_id, pending_id, self.owner],
                            |r| r.get::<_, bool>(0),
                        )?;
                    // Outside a turn the same two rules hold (operator
                    // decision 2026-10-02): only the first open question
                    // notifies, and one asked again after the agent withdrew
                    // one is quiet until the owner's next turn, bounded as
                    // above.
                    let outside_quiet = blocked == 0
                        && indexed.turn_id.is_none()
                        && tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND pending_id <> ?2
                                 AND state = 'open' AND owner_id = ?3)
                             OR EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND pending_id <> ?2
                                 AND owner_id = ?3 AND turn_id IS NULL AND reason = 'agent_withdrew'
                                 AND opened_event_id > coalesce((SELECT max(event_id) FROM events
                                     WHERE session_id = ?1 AND kind = 'user_turn' AND owner_id = ?3), 0))",
                            params![session_id, pending_id, self.owner],
                            |r| r.get::<_, bool>(0),
                        )?;
                    edge = if rewithdrawn || outside_quiet {
                        None
                    } else if blocked > 0 {
                        Some(PushEdge::Blocked {
                            pending_id: pending_id.clone(),
                            title,
                        })
                    } else if indexed.turn_id.is_none() {
                        Some(PushEdge::QuestionOutsideTurn {
                            pending_id: pending_id.clone(),
                            title,
                        })
                    } else {
                        None
                    };
                }
            }
            SessionBody::PendingResolved {
                pending_id,
                resolution,
                reason,
            } => {
                let state = match resolution {
                    PendingResolution::Delivered => PendingState::Delivered,
                    PendingResolution::Cancelled => PendingState::Cancelled,
                };
                if !resolve_pending(&tx, &self.owner, session_id, pending_id, state, *reason, &ts)? {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
            SessionBody::AnswerResult {
                pending_id, delivered, ..
            } => {
                // Folded monotonically: `delivered` sticks, a later `false`
                // never overwrites it (umbrella §6.8). The guard makes that
                // a real no-op check, not just a match on the row: SQLite's
                // changed-row count is rows matched, not rows whose value
                // moved, so a WHERE on the id alone would call a same-value
                // resend "applied" (decision 14 says a verdict that changes
                // nothing is stored but not applied).
                let changed = tx.execute(
                    "UPDATE answer_queue SET delivered = CASE WHEN delivered = 1 THEN 1 ELSE ?3 END
                     WHERE pending_id = ?1 AND session_id = ?2 AND owner_id = ?4
                         AND (delivered IS NULL OR (delivered = 0 AND ?3 = 1))",
                    params![pending_id, session_id, delivered, self.owner],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
            SessionBody::GitState {
                branch,
                dirty,
                worktree,
                base_commit,
                ..
            } => {
                // The branch on one line and capped, like the title; the
                // base commit once, and only a commit id (plan 6b-ii
                // decision 11). A state that changes nothing is kept as the
                // idempotency key only, like a verdict that changes nothing
                // (the review's O1): not listed, and the session does not
                // move up the list.
                let branch = branch
                    .as_deref()
                    .and_then(|b| one_line(b, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES));
                let base = base_commit
                    .as_deref()
                    .filter(|c| (4..=64).contains(&c.len()) && c.chars().all(|c| c.is_ascii_hexdigit()));
                let changed = if fact_applies(&tx, &self.owner, session_id, None)? {
                    tx.execute(
                        "UPDATE sessions SET git_branch = ?2, git_dirty = ?3, git_worktree = ?4,
                             base_commit = COALESCE(base_commit, ?5)
                         WHERE id = ?1 AND owner_id = ?6
                             AND (git_branch IS NOT ?2 OR git_dirty IS NOT ?3 OR git_worktree IS NOT ?4
                                  OR (base_commit IS NULL AND ?5 IS NOT NULL))",
                        params![session_id, branch, dirty, worktree, base, self.owner],
                    )?
                } else {
                    0
                };
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
            // Diagnostics only, with no transition of their own: an
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing.
            SessionBody::AdapterExited { .. } => {
                if !fact_applies(&tx, &self.owner, session_id, None)? {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                } else {
                    // The adapter of an attached session is gone, and its
                    // token with it (lane L4), ahead of the `session_parked`
                    // that follows. Not a `starting` one: its start's own
                    // `start_failed` revokes, and a resume's fresh token is
                    // not this adapter's.
                    let attached: bool = tx.query_row(
                        "SELECT lifecycle = 'active' OR presumed_parked = 1 FROM sessions
                         WHERE id = ?1 AND owner_id = ?2",
                        [session_id, &self.owner],
                        |r| r.get(0),
                    )?;
                    if attached {
                        cut = mcp.revoke_in(&tx, session_id)?;
                    }
                }
            }
            SessionBody::HostNote { .. } => {
                if !fact_applies(&tx, &self.owner, session_id, None)? {
                    created.clear();
                    mark_unapplied(&tx, &self.owner, fact_id)?;
                }
            }
        }
        // Only a listed event moves the session's recency (plan 6b decision
        // 6): a fact kept just as the idempotency key is hidden from the
        // timeline, so it must not move the session up the list either.
        // The fact's collector events come after it, so the last of them
        // is the session's last event.
        if let Some(last) = created.iter().map(|e| e.event_id).max() {
            tx.execute(
                "UPDATE sessions SET last_event_at = ?2, last_event_id = ?3 WHERE id = ?1 AND owner_id = ?4",
                params![session_id, ts, last, self.owner],
            )?;
        }
        // The session as this fact left it, for the notice (A3).
        let edge = match edge {
            Some(kind) => Some(Edge {
                kind,
                session: tx.query_row(
                    "SELECT id, hat_id, title, cwd FROM sessions WHERE id = ?1 AND owner_id = ?2",
                    [session_id, &self.owner],
                    |r| {
                        Ok(EdgeSession {
                            id: r.get(0)?,
                            hat_id: r.get(1)?,
                            title: r.get(2)?,
                            cwd: r.get(3)?,
                        })
                    },
                )?,
            }),
            None => None,
        };
        tx.commit()?;
        mcp.cut(cut);
        Ok(Ingested { events: created, edge })
    }

    /// Whether `pending_id` is still open in `session_id`: a question asked
    /// outside a turn, resent before reconnecting, is notified only if it
    /// is (operator decision 2026-10-02).
    pub fn still_open(&self, session_id: &str, pending_id: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM pending WHERE pending_id = ?1 AND session_id = ?2 AND state = 'open'
                 AND owner_id = ?3)",
            [pending_id, session_id, &self.owner],
            |r| r.get(0),
        )?)
    }

    /// Whether `pending_id` is still open in `session_id`, and the session
    /// still blocked: a `Blocked` edge the host resent before reconnecting
    /// is notified only if it still holds once the resend is complete
    /// (10b-i's review, A1).
    pub fn still_blocked_on(&self, session_id: &str, pending_id: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM pending p JOIN sessions s ON s.id = p.session_id AND s.owner_id = p.owner_id
                 WHERE p.pending_id = ?1 AND p.session_id = ?2 AND p.state = 'open' AND s.activity = 'blocked'
                     AND p.owner_id = ?3)",
            [pending_id, session_id, &self.owner],
            |r| r.get(0),
        )?)
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
    /// - attached sessions the operator closed or deleted → returned in
    ///   `close`.
    pub fn reconcile_host(&self, host_id: &str, attached: &[AttachedSession]) -> Result<Reconciliation> {
        let listed: HashMap<&str, &AttachedSession> = attached.iter().map(|a| (a.session_id.as_str(), a)).collect();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let rows: Vec<(String, String, Option<String>, bool, bool)> = {
            let mut stmt = tx.prepare(
                "SELECT id, lifecycle, open_turn_id, close_requested, presumed_parked FROM sessions
                 WHERE host_id = ?1 AND (lifecycle IN ('starting', 'active', 'closed', 'deleted') OR presumed_parked = 1)
                     AND owner_id = ?2
                 ORDER BY id",
            )?;
            let rows = stmt.query_map([host_id, &self.owner], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Reconciliation::default();
        let mcp = self.mcp();
        let mut cut = Cut::default();
        for (id, lifecycle, open_turn, close_requested, presumed) in rows {
            let host = listed.get(id.as_str());
            // A presumed park was a guess made while the host was away: now
            // that it is back, treat the session as the active one it may
            // still be (ACP core §5.1 step 4, §5.3).
            let lifecycle = if presumed { "active" } else { lifecycle.as_str() };
            match (lifecycle, host) {
                ("starting", None) => {
                    out.events.push(collector_event(
                        &tx,
                        &self.owner,
                        &id,
                        "start_not_delivered",
                        json!({}),
                        &ts,
                    )?);
                    tx.execute(
                        "UPDATE sessions SET lifecycle = 'failed', failure_reason = 'start_not_delivered'
                         WHERE id = ?1 AND owner_id = ?2",
                        [&id, &self.owner],
                    )?;
                    // Never started: its token goes with it (lane L4).
                    cut = cut.and(mcp.revoke_in(&tx, &id)?);
                }
                ("active", _) => {
                    if host.is_none() {
                        out.events.push(collector_event(
                            &tx,
                            &self.owner,
                            &id,
                            "host_restarted",
                            json!({}),
                            &ts,
                        )?);
                    } else if presumed {
                        out.events
                            .push(collector_event(&tx, &self.owner, &id, "reattached", json!({}), &ts)?);
                        tx.execute(
                            "UPDATE sessions SET lifecycle = 'active', presumed_parked = 0 WHERE id = ?1 AND owner_id = ?2",
                            [&id, &self.owner],
                        )?;
                    }
                    let host_turn = host.and_then(|a| a.open_turn_id.as_deref());
                    if let Some(turn) = open_turn.as_deref()
                        && host_turn != Some(turn)
                    {
                        out.events.push(resolve_open_turn(&tx, &self.owner, &id, turn, &ts)?);
                    }
                    if host.is_none() {
                        // The restarted host holds none of its questions.
                        out.events.extend(cancel_open_pending(
                            &tx,
                            &self.owner,
                            &id,
                            PendingReason::HostRestarted,
                            &ts,
                        )?);
                        tx.execute(
                            "UPDATE sessions SET
                                 lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                                 activity = NULL, open_turn_id = NULL, close_requested = 0, presumed_parked = 0
                             WHERE id = ?1 AND owner_id = ?2",
                            [&id, &self.owner],
                        )?;
                        // The restarted host runs no adapter of it (lane L4).
                        cut = cut.and(mcp.revoke_in(&tx, &id)?);
                    } else if close_requested {
                        out.close.push(id);
                    }
                }
                // Nothing is written for a tombstone (plan 9a decision 4): its
                // host is told to close the adapter, and its answer, a
                // `session_closed` or `not_attached`, changes nothing.
                ("closed" | "deleted", Some(_)) => out.close.push(id),
                _ => {}
            }
        }
        tx.commit()?;
        mcp.cut(cut);
        Ok(out)
    }

    /// Events of one session with `event_id > after`, oldest first: a range
    /// of `events_by_session`. Host
    /// facts stored but not applied are left out (final review F1).
    pub fn events(&self, session_id: &str, after: i64, limit: u32) -> Result<Vec<EventDto>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(EVENTS_AFTER)?;
        let rows = stmt.query_map(params![session_id, after, limit, self.owner], |r| {
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
        SessionBody::PendingOpened { .. } => "pending_opened",
        SessionBody::PendingResolved { .. } => "pending_resolved",
        SessionBody::AnswerResult { .. } => "answer_result",
        SessionBody::GitState { .. } => "git_state",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
    use hennery_kernel::operator::{Operator, SetupOutcome};

    /// A page of events reads a range of `events_by_session` in order: no
    /// scan of the session's other rows and no sort (smoke test #1, F2).
    #[test]
    fn a_page_of_events_is_a_range_of_its_index() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {EVENTS_AFTER}"))
            .unwrap()
            .query_map(params!["s1", 0, 500, "owner"], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(plan.len(), 1, "{plan:?}");
        assert!(
            plan[0].contains("USING INDEX events_by_session (session_id=? AND event_id>?)"),
            "{plan:?}"
        );
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let ts = super::now();
        assert!(ts.ends_with('Z') && ts.as_bytes()[10] == b'T', "{ts}");
    }

    /// Plan 6b decision 1: one line, then the caps, by characters and by
    /// the bytes JSON takes; never a broken character.
    #[test]
    fn one_line_collapses_whitespace_and_cuts_to_the_caps() {
        assert_eq!(one_line("  a\n\tb \u{7}\u{2028} c ", 10, 10).as_deref(), Some("a b c"));
        assert_eq!(one_line(" \n\t\u{0} ", 10, 10), None);
        assert_eq!(one_line("", 10, 10), None);
        assert_eq!(one_line("abcdef", 4, 100).as_deref(), Some("abcd"));
        assert_eq!(one_line("abcdef", 100, 4).as_deref(), Some("abcd"));
        // `"` and `\` take two bytes in JSON.
        assert_eq!(one_line("a\"b\\c", 100, 3).as_deref(), Some("a\""));
        assert_eq!(one_line("a\"b\\c", 100, 5).as_deref(), Some("a\"b"));
        // Two bytes each, then three: the cut never splits a character.
        assert_eq!(one_line("ééé€€", 100, 7).as_deref(), Some("ééé"));
        // A cut that ends on a space drops it.
        assert_eq!(one_line("ab cd", 3, 100).as_deref(), Some("ab"));
        // Bidi and zero-width characters, which could make a row read as
        // something else, are dropped.
        assert_eq!(
            one_line("a\u{202E}b\u{200B}c\u{FEFF}d\u{061C}e\u{2066}f\u{200F}", 100, 100).as_deref(),
            Some("abcdef")
        );
    }

    /// The review's A11: the list's statement walks the recency index, or
    /// for one hat the hat's (plan 5c), with no sort of its own, with or
    /// without a search.
    #[test]
    fn the_list_walks_the_recency_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        Store::open(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        for (by_hat, index) in [(false, "sessions_by_recency"), (true, "sessions_by_hat")] {
            for pattern in [Some("%x%"), None] {
                let plan = list_plan(&conn, by_hat, pattern);
                assert!(plan.contains(&format!("USING INDEX {index}")), "{plan}");
                assert!(!plan.contains("TEMP B-TREE"), "{plan}");
            }
        }
    }

    fn list_plan(conn: &Connection, by_hat: bool, pattern: Option<&str>) -> String {
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", list_statement(by_hat)))
            .unwrap()
            .query_map(
                params![
                    "o",
                    "~",
                    "",
                    "active",
                    "parked",
                    None::<String>,
                    None::<String>,
                    None::<String>,
                    pattern,
                    50,
                    by_hat.then_some("hat-1")
                ],
                |r| r.get(3),
            )
            .unwrap()
            .map(Result::unwrap)
            .collect();
        plan.join("\n")
    }

    /// Decision 8, the review's O4: a cursor goes out opaque and comes back
    /// the same; anything else is refused.
    #[test]
    fn a_cursor_round_trips_and_a_malformed_one_is_refused() {
        let cursor = Cursor {
            last_event_at: "2026-10-07T12:00:00.000Z".into(),
            session_id: "0199a4c2-7e1f-7c3a-9b2d-4f6e8a0c1d2e".into(),
        };
        let encoded = cursor.encode();
        assert!(encoded.chars().all(|c| c.is_ascii_hexdigit()), "{encoded}");
        assert_eq!(Cursor::decode(&encoded), Some(cursor));
        for bad in [
            "",
            "zz",
            "abc",
            &hex::encode("no separator"),
            &hex::encode("\nid"),
            &hex::encode("at\n"),
            &hex::encode([0xff, b'\n', b'a']),
            &hex::encode(format!("at\n{}", "x".repeat(65))),
            // Well formed, each part within 64 bytes, but past 256 characters.
            &hex::encode(format!("{}\n{}", "a".repeat(64), "b".repeat(64))),
        ] {
            assert_eq!(Cursor::decode(bad), None, "{bad}");
        }
    }

    /// `%`, `_` and `\` in a search are literal (decision 8).
    #[test]
    fn a_search_escapes_like_wildcards() {
        assert_eq!(like_pattern("100%_a\\b"), "%100\\%\\_a\\\\b%");
        assert_eq!(like_pattern("plain"), "%plain%");
    }

    /// Plan 6b decision 5: every stamp has the same width, so comparing
    /// them as text compares the times, within a second too.
    #[test]
    fn stamps_have_one_width_so_text_order_is_time_order() {
        let at = |nanos: i64| {
            super::stamp(
                time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap() + time::Duration::nanoseconds(nanos),
            )
        };
        let stamps = [
            at(0),
            at(100_000_000),
            at(120_000_000),
            at(999_999_999),
            at(1_000_000_000),
        ];
        assert_eq!(stamps[0], "2027-01-15T08:00:00.000Z");
        assert_eq!(stamps[1], "2027-01-15T08:00:00.100Z");
        assert_eq!(stamps[3], "2027-01-15T08:00:00.999Z");
        assert!(stamps.iter().all(|s| s.len() == 24), "{stamps:?}");
        assert!(stamps.windows(2).all(|w| w[0] < w[1]), "{stamps:?}");
    }

    /// Plan 6b's migration on a database from before it: `last_event_at`
    /// rewritten to the fixed width (an unreadable value left alone),
    /// `last_event_id` from the session's last listed event, and the list's
    /// index in place.
    #[test]
    fn the_session_list_migration_normalises_recency_on_an_older_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        {
            let mut conn = hennery_kernel::db::open(&db).unwrap();
            let owner = hennery_kernel::db::kernel_owner(&mut conn).unwrap();
            hennery_kernel::db::migrate(&mut conn, &MIGRATIONS[..8]).unwrap();
            conn.execute_batch(&format!(
                "
                INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id) VALUES
                    ('s1', 'h1', 'fake', '/tmp', 'active', 't', '2026-10-01T10:00:05.12Z', '{owner}'),
                    ('s2', 'h1', 'fake', '/tmp', 'active', 't', '2026-10-01T10:00:05Z', '{owner}'),
                    ('s3', 'h1', 'fake', '/tmp', 'active', 't', 't', '{owner}');
                INSERT INTO events(session_id, host_seq, kind, body, ts, applied, owner_id) VALUES
                    ('s1', 1, 'session_started', '{{}}', 't', 1, '{owner}'),
                    ('s1', 2, 'acp_update', '{{}}', 't', 1, '{owner}'),
                    ('s1', 3, 'turn_ended', '{{}}', 't', 0, '{owner}');
                "
            ))
            .unwrap();
        }
        Store::open(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        let mut stmt = conn
            .prepare("SELECT id, last_event_at, last_event_id FROM sessions ORDER BY id")
            .unwrap();
        let rows: Vec<(String, String, Option<i64>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            [
                ("s1".into(), "2026-10-01T10:00:05.120Z".into(), Some(2)),
                ("s2".into(), "2026-10-01T10:00:05.000Z".into(), None),
                ("s3".into(), "t".into(), None),
            ]
        );
        let index: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'sessions_by_recency'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
    }

    /// Plan 9a decision 6: the turn links are backfilled from each turn's
    /// content, as `link_attachments` links an event's: an image block with
    /// a hash, at its index, only where the owner's row for it exists.
    /// Anything else in the content (a block that is no object, an image
    /// with no hash, a hash with no row) links nothing and fails nothing.
    #[test]
    fn the_turn_links_are_backfilled_from_the_turns_content() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let (a, b, gone) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        {
            let mut conn = hennery_kernel::db::open(&db).unwrap();
            let owner = hennery_kernel::db::kernel_owner(&mut conn).unwrap();
            // Every migration before plan 9a's, wherever later lanes put it.
            let before = MIGRATIONS
                .iter()
                .position(|m| m.contains("CREATE TABLE turn_attachments"))
                .unwrap();
            hennery_kernel::db::migrate(&mut conn, &MIGRATIONS[..before]).unwrap();
            let content = json!([
                { "type": "text", "text": "x" },
                { "type": "image", "mimeType": "image/png", "sha256": a },
                "a string",
                { "type": "image", "mimeType": "image/png" },
                { "type": "image", "mimeType": "image/png", "sha256": gone },
                { "type": "image", "mimeType": "image/png", "sha256": 7 },
                { "type": "image", "mimeType": "image/png", "sha256": b },
                { "type": "image", "mimeType": "image/png", "sha256": a },
            ]);
            conn.execute_batch(&format!(
                "
                INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id)
                    VALUES ('s1', 'h1', 'fake', '/tmp', 'active', 't', 't', '{owner}');
                INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES
                    ('{owner}', '{a}', 'image/png', 1, 't'), ('{owner}', '{b}', 'image/png', 1, 't');
                INSERT INTO turns(turn_id, session_id, content, created_at, owner_id) VALUES
                    ('t1', 's1', '{content}', 't', '{owner}'),
                    ('t2', 's1', '[]', 't', '{owner}'),
                    ('t3', 's1', '{{\"not\": \"a list\"}}', 't', '{owner}');
                "
            ))
            .unwrap();
        }
        let store = Store::open(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        let mut stmt = conn
            .prepare("SELECT turn_id, sha256, position, owner_id FROM turn_attachments ORDER BY turn_id, position")
            .unwrap();
        let links: Vec<(String, String, i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let owner = store.owner_id().to_string();
        assert_eq!(
            links,
            [
                ("t1".into(), a.clone(), 1, owner.clone()),
                ("t1".into(), b, 6, owner.clone()),
                ("t1".into(), a, 7, owner),
            ]
        );
    }

    /// The kernel's first two migrations, as 3b-ii shipped them.
    const KERNEL_3B_II: &[&str] = &[
        "
        CREATE TABLE hosts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            public_key TEXT NOT NULL UNIQUE,
            platform TEXT NOT NULL,
            host_version TEXT NOT NULL,
            capabilities TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER,
            revoked_at INTEGER);
        CREATE TABLE pairing_codes (
            code_hash TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER);
        ",
        "
        CREATE TABLE owners (
            id TEXT PRIMARY KEY,
            contact TEXT,
            created_at INTEGER NOT NULL);
        CREATE TABLE password_credentials (
            owner_id TEXT PRIMARY KEY REFERENCES owners(id),
            phc TEXT NOT NULL,
            updated_at INTEGER NOT NULL);
        CREATE TABLE auth_sessions (
            id_hash TEXT PRIMARY KEY,
            owner_id TEXT NOT NULL REFERENCES owners(id),
            user_agent TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            last_step_up_at INTEGER,
            expires_at INTEGER NOT NULL);
        CREATE TABLE settings (
            owner_id TEXT NOT NULL REFERENCES owners(id),
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (owner_id, key));
        ",
    ];

    const PASSWORD: &str = "correct horse battery";
    /// RFC 8032's first two test keys: valid Ed25519 public keys.
    const OLD_KEY: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    const NEW_KEY: &str = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";

    /// A PHC string of `PASSWORD`, as 3b-ii's setup stored it.
    fn phc() -> String {
        let op = Operator::open_in_memory().unwrap();
        let token = op.issue_setup_token(0).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", 0).unwrap() else {
            panic!("setup failed");
        };
        phc
    }

    /// `hennery.db` as 3b-ii made it: the kernel's first two migrations
    /// and this store's first six, verbatim, with a row in each of the
    /// eight tables from before `owner_id` (a host and a live pairing code;
    /// a session with a turn, a fact, its catalogue, an open question and
    /// an answer queued for it), and the owner set up if `set_up`.
    fn a_3b_ii_database(path: &Path, set_up: bool) {
        let mut conn = Connection::open(path).unwrap();
        hennery_kernel::db::migrate_component(&mut conn, "kernel", KERNEL_3B_II).unwrap();
        hennery_kernel::db::migrate(&mut conn, &MIGRATIONS[..6]).unwrap();
        let code = hennery_kernel::secret::sha256_hex(b"AAAAAAAA");
        conn.execute_batch(&format!(
            "
            INSERT INTO hosts(id, name, public_key, platform, host_version, created_at)
                VALUES ('host-old', 'laptop', '{OLD_KEY}', 'macos-aarch64', '0.0.0', 1700000000);
            INSERT INTO pairing_codes VALUES ('{code}', 1700000000, 9000000000000000000, NULL);
            INSERT INTO sessions(id, host_id, agent, cwd, agent_session_id, lifecycle, activity, open_turn_id,
                                 created_at, last_event_at)
                VALUES ('session-old', 'host-old', 'fake', '/tmp', 'agent-old', 'active', 'blocked', 'turn-old',
                        't', 't');
            INSERT INTO turns(turn_id, session_id, content, created_at, state)
                VALUES ('turn-old', 'session-old', '[]', 't', 'started');
            INSERT INTO events(session_id, host_seq, kind, body, ts)
                VALUES ('session-old', 1, 'session_started', '{{}}', 't');
            INSERT INTO session_catalog(session_id, config_options, updated_at) VALUES ('session-old', '[]', 't');
            INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at)
                VALUES ('pending-old', 'session-old', 'permission', 'turn-old', '[\"allow\"]', '{{}}', 'open', 't');
            INSERT INTO answer_queue(pending_id, session_id, request_id, answer, submitted_at)
                VALUES ('pending-old', 'session-old', 'request-old',
                        '{{\"kind\":\"permission\",\"option_id\":\"allow\"}}', 't');
            "
        ))
        .unwrap();
        if set_up {
            conn.execute_batch("INSERT INTO owners(id, created_at) VALUES ('owner-00000000000000a1', 1700000000);")
                .unwrap();
            conn.execute(
                "INSERT INTO password_credentials VALUES ('owner-00000000000000a1', ?1, 1700000000)",
                [phc()],
            )
            .unwrap();
            conn.execute_batch(
                "INSERT INTO settings VALUES ('owner-00000000000000a1', 'public_url', 'https://hennery.example');",
            )
            .unwrap();
        }
    }

    /// 3b-iii review, O2: a 3b-ii database, set up or not, opened as
    /// `run_collector` opens it (the store, then the registry, then the
    /// operator): kernel 2 to 4 and store 6 to 7 in one start, store first.
    /// One owner, the same for all three; every row of the eight older
    /// tables carries it and is found; the password from before still
    /// works, or setup does.
    #[test]
    fn a_3b_ii_database_opened_in_the_collectors_order_has_one_owner_for_every_row() {
        for set_up in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("hennery.db");
            a_3b_ii_database(&db, set_up);
            let store = Store::open(&db).unwrap();
            let hosts = Hosts::open(&db).unwrap();
            let operator = Operator::open(&db).unwrap();
            let owner = store.owner_id().to_string();
            assert_eq!(
                (hosts.owner_id(), operator.owner_id()),
                (owner.as_str(), owner.as_str())
            );
            assert_eq!(set_up, owner == "owner-00000000000000a1", "{owner}");

            let conn = Connection::open(&db).unwrap();
            let owners: i64 = conn.query_row("SELECT count(*) FROM owners", [], |r| r.get(0)).unwrap();
            assert_eq!(owners, 1);
            for table in [
                "hosts",
                "pairing_codes",
                "sessions",
                "turns",
                "events",
                "session_catalog",
                "pending",
                "answer_queue",
            ] {
                let mut stmt = conn.prepare(&format!("SELECT owner_id FROM {table}")).unwrap();
                let found: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
                assert_eq!(found, vec![owner.clone()], "{table}");
            }

            let listed: Vec<String> = hosts.list().unwrap().into_iter().map(|h| h.id).collect();
            assert_eq!(listed, ["host-old"]);
            assert!(store.find_session("session-old").unwrap().is_some());
            assert!(store.catalog("session-old").unwrap().is_some());
            assert_eq!(store.turn_state("turn-old").unwrap().as_deref(), Some("started"));
            assert_eq!(store.events("session-old", 0, 10).unwrap().len(), 1);
            assert_eq!(store.open_pending("session-old").unwrap().len(), 1);
            assert_eq!(store.answers_to_send("host-old").unwrap().len(), 1);
            let enrollment = Enrollment {
                public_key: NEW_KEY.into(),
                name: "desktop".into(),
                host_version: "0.0.0".into(),
                platform: "macos-aarch64".into(),
            };
            assert!(matches!(
                hosts.enroll("AAAA-AAAA", &enrollment, 1_800_000_000).unwrap(),
                EnrollOutcome::Enrolled { .. }
            ));

            if set_up {
                assert!(operator.is_set_up().unwrap());
                assert!(operator.verify_password(PASSWORD).unwrap().is_some());
                assert_eq!(operator.public_url().unwrap().origin(), "https://hennery.example");
            } else {
                assert!(!operator.is_set_up().unwrap());
                let token = operator.issue_setup_token(1_800_000_000).unwrap().unwrap();
                let SetupOutcome::Done { owner_id, .. } = operator
                    .set_up(&token, PASSWORD, "https://hennery.example", 1_800_000_000)
                    .unwrap()
                else {
                    panic!("setup failed");
                };
                assert_eq!(owner_id, owner);
                assert_eq!(hosts.list().unwrap().len(), 2);
            }
        }
    }
}
