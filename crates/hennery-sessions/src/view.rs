//! The view API (client view spec §4; plan 4a-ii): a session's items, a
//! page of whole turns at a time and as a stream of changes, and the
//! session list's summaries, as a page and a stream. The items are
//! `hennery_view`'s fold of the stored events: nothing new is stored.
//!
//! Every read goes through the store, which names the owner in every
//! statement; a session route answers 404 for a session that is not the
//! owner's before it reads any event (A1). The streams take the hub as a
//! doorbell only: each reads the store from its own cursor, so a receiver
//! that lags loses nothing, it reads now (the review's A-2).
//!
//! What one connection holds at most (the security review's B-1, B-7):
//! - an item stream: the fold of the session's current group (by its
//!   budget, `GROUP_MAX_BYTES` of items plus `GROUP_MAX_QUESTIONS`
//!   questions of `QUESTION_MAX_BYTES`), the items it changed since the
//!   last send, up to `GROUP_MAX_QUESTIONS` questions of earlier groups,
//!   and the queue (`QUEUE_BYTES`, or one message past it), plus the
//!   message being written. A resume adds what it sends, as JSON text, up
//!   to `RESUME_MAX_BYTES` (past it, `resync_required`), plus one group's
//!   items. A `catalog_changed` holds the session's catalogue as
//!   `GET /api/sessions/{id}/catalog` serves it, read as it is sent.
//! - the list stream: the summaries it last sent, one bounded list item
//!   per session of the owner's, and the queue.
//!
//! The list stream reads `LIST_COALESCE` after a ring, the rings meanwhile
//! absorbed (the plan review's 3): each fact moves a session's
//! `last_event_at`, so without the hold every chunk of a reply would be
//! an upsert for every subscriber. At most 4 upserts a second per active
//! session per subscriber, from ordinary events. A `session_deleted`
//! ring, or a lagged one, is read at once, with no hold: a purge of n
//! sessions can make n rounds at once per subscriber. A round that finds
//! no new event reads one statement; one that finds some also recounts
//! the sessions waiting (the owner's sessions, tombstones included).
//! - a page: `PAGE_MAX_BYTES` of items as JSON text, plus one group, plus
//!   the group being folded; then the page's text, built as they are
//!   dropped.

use crate::AppState;
use crate::api::{ListFilter, ListParams, changes_catalogue, error, internal, refuse_empty_hat};
use crate::store::{Store, stamp_ago};
use anyhow::Result;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures::StreamExt;
use hennery_kernel::operator::Authenticated;
use hennery_proto::rest::{EventDto, PendingState, SessionItem};
use hennery_view::api::{
    SessionRemoved, SessionSummary, SummaryPage, TurnContent, WaitingChanged, anchor, parse_anchor,
};
use hennery_view::cap::{GROUP_MAX_QUESTIONS, ID_MAX_BYTES};
use hennery_view::{Body, Fold, Item, Question, group_key, question_id};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;

/// A page's groups when none is asked for, and the most one serves (A2).
pub const PAGE_DEFAULT_GROUPS: u32 = 20;
pub const PAGE_MAX_GROUPS: u32 = 100;
/// Events read and folded at a time: raw events are never all held.
const BATCH: u32 = 500;
/// Past either, a resume sends `resync_required` and the client fetches a
/// page instead (A5; the review's A-10). They bound what a resume sends,
/// not the work of reading it.
pub const RESUME_MAX_EVENTS: u32 = 20_000;
pub const RESUME_MAX_GROUPS: u32 = 20;
/// How long message and thought chunks are held, merged, before their
/// item is sent: a long reply would otherwise resend its whole text per
/// chunk (A5).
pub const COALESCE: Duration = Duration::from_millis(100);
/// How long the list stream holds after a ring before it reads (the plan
/// review's 3): one upsert per session per hold.
pub const LIST_COALESCE: Duration = Duration::from_millis(250);
/// How far back the list stream resumes (ACP core §9).
pub const LIST_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// The bytes of SSE messages one stream may have waiting for a slow
/// client (the security review's B-7). A message heavier than this alone
/// waits for an empty queue, then goes: an item is bounded by its caps,
/// and none is ever split or dropped.
pub const QUEUE_BYTES: usize = 16 << 20;
/// And how many, so a stream of small messages is bounded too.
const QUEUE_DEPTH: usize = 1024;
/// The keep-alive comment's period (ACP core §9).
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// The most a page's items weigh, as JSON text (4a-i's whole-branch
/// review): past it the page drops its oldest groups, keeping the newest
/// one whatever it weighs, and says `older`. A group's fold is bounded
/// (`GROUP_MAX_BYTES` and its questions), so a page holds this, one group
/// more, and the group being folded.
pub const PAGE_MAX_BYTES: usize = 32 << 20;
/// The most a resume sends, as JSON text (the security review's B-1):
/// past it, `resync_required`, and the client fetches a page instead.
pub const RESUME_MAX_BYTES: usize = PAGE_MAX_BYTES;
/// The event that ends a session (plan 9a): nothing of it is written after,
/// and it makes no item; the item stream ends on it.
const SESSION_DELETED: &str = "session_deleted";

/// Every route here is an operator's (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/view/sessions", get(summaries))
        .route("/api/view/sessions/{id}", get(page))
        .route("/api/view/sessions/{id}/turns/{turn_id}", get(undelivered_turn))
        .route("/api/stream/view/sessions/{id}", get(item_stream))
        .route("/api/stream/sessions", get(list_stream));
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}

/// This collector process's epoch (`hennery_view::api::epoch`): its boot
/// token, 16 random hex digits, is made on first use. A restart, a
/// restored backup or a new fold each change it, and every client
/// resyncs once (the review's A-1).
pub fn epoch() -> &'static str {
    static EPOCH: OnceLock<String> = OnceLock::new();
    EPOCH.get_or_init(|| {
        let boot = hex::encode(hennery_kernel::secret::random_bytes::<8>());
        hennery_view::api::epoch(&boot)
    })
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "no such session")
}

/// `GET /api/view/sessions` (A7): the session list's page, each item a
/// summary, filtered and paged as `GET /api/sessions` is.
async fn summaries(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    let filter = match ListFilter::parse(&params) {
        Ok(filter) => filter,
        Err(refused) => return refused.into_response(),
    };
    let page = || -> Result<SummaryPage> {
        // Before the list: the stream resumes after it, so a change made
        // meanwhile is sent again rather than missed.
        let revision = state.store.max_event_id()?;
        let page = state.store.list(&filter.query())?;
        let waiting = state.store.sessions_waiting()?;
        Ok(SummaryPage {
            sessions: page.sessions.into_iter().map(|s| summary(s, &waiting)).collect(),
            next_cursor: page.next_cursor,
            revision,
            epoch: epoch().to_string(),
            // The hat's alone of the filters: the count is the list's
            // whole, not the page's.
            waiting: state.store.waiting_count(filter.hat())?,
        })
    };
    match page() {
        Ok(page) => Json(page).into_response(),
        Err(err) => internal(err),
    }
}

/// A list item as a summary: `waiting` holds the sessions with an open
/// question.
fn summary(session: SessionItem, waiting: &HashSet<String>) -> SessionSummary {
    let question_waits = waiting.contains(&session.session_id);
    SessionSummary {
        session,
        question_waits,
    }
}

/// `GET /api/view/sessions/{id}`'s query.
#[derive(Deserialize)]
struct PageParams {
    before_turn: Option<String>,
    limit: Option<String>,
}

/// `GET /api/view/sessions/{id}` (A2): whole groups of the session's items,
/// `limit` of them (20 by default, clamped to 1..=100), the last ones or
/// those before `before_turn`'s.
async fn page(State(state): State<AppState>, Path(id): Path<String>, Query(params): Query<PageParams>) -> Response {
    // Before anything else: a session that is not the owner's answers 404
    // having read no event (A1).
    match state.store.find_session_item(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    }
    let limit = match params.limit.as_deref().map(str::parse::<u32>) {
        None => PAGE_DEFAULT_GROUPS,
        Some(Ok(limit)) => limit.clamp(1, PAGE_MAX_GROUPS),
        Some(Err(_)) => return error(StatusCode::BAD_REQUEST, "invalid", "limit must be a whole number"),
    };
    match read_page(&state.store, &id, params.before_turn.as_deref(), limit, PAGE_MAX_BYTES) {
        Ok(Some(page)) => ([(header::CONTENT_TYPE, "application/json")], page).into_response(),
        Ok(None) => error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "before_turn is not a turn of this session",
        ),
        Err(err) => internal(err),
    }
}

/// A page of `limit` groups, ending before `before_turn`'s or at the
/// revision, as `ItemPage`'s JSON; `None` when `before_turn` names no turn
/// of the session. Past `budget` bytes of items, its oldest groups are left
/// out (`older`), the newest kept.
fn read_page(store: &Store, id: &str, before_turn: Option<&str>, limit: u32, budget: usize) -> Result<Option<String>> {
    // First: the page reads no event past it (A3).
    let revision = store.revision(id)?;
    let end = match before_turn {
        None => revision + 1,
        Some(turn) => match turn_start(store, id, turn)? {
            Some(start) => start,
            None => return Ok(None),
        },
    };
    let until = (end - 1).min(revision);
    let starts = store.group_starts(id, end, limit + 1)?;
    let limit = limit as usize;
    // The preamble counts as a group: with fewer starts than `limit`, the
    // page holds it too.
    let (start, mut older) = if starts.len() > limit {
        (starts[limit - 1], true)
    } else if starts.len() == limit {
        let start = starts[limit - 1];
        (start, store.has_events_before(id, start)?)
    } else {
        (0, false)
    };
    // Group by group: each group's items are taken when the next begins
    // (locality: nothing later changes them), overlaid and written out, and
    // the fold holds one group at a time (the review's A-6; 4a-i's
    // whole-branch review).
    let mut groups = PageGroups {
        groups: VecDeque::new(),
        bytes: 0,
        budget,
        dropped: false,
    };
    let mut fold = Fold::new();
    let mut after = start - 1;
    loop {
        let batch = store.events_between(id, after, until, BATCH)?;
        let Some(last) = batch.last() else { break };
        after = last.event_id;
        for event in &batch {
            if event.kind == SESSION_DELETED {
                continue;
            }
            if event.kind == "user_turn" {
                groups.push(written(store, id, fold.take())?);
            }
            fold.apply(event, &unanswerable);
        }
    }
    groups.push(written(store, id, fold.take())?);
    older |= groups.dropped;
    // Each item's text is dropped as the page's is written: the two are
    // never both held whole (the security review's 6c).
    let mut page = String::from("{\"items\":[");
    let mut first = true;
    while let Some(group) = groups.groups.pop_front() {
        for item in group {
            if !first {
                page.push(',');
            }
            first = false;
            page.push_str(&item);
        }
    }
    page.push_str(&format!(
        "],\"revision\":{revision},\"epoch\":{},\"older\":{older}}}",
        serde_json::to_string(epoch())?
    ));
    Ok(Some(page))
}

/// A page's groups, oldest first, each as its items' JSON.
struct PageGroups {
    groups: VecDeque<Vec<String>>,
    bytes: usize,
    budget: usize,
    /// Groups were left out for the budget.
    dropped: bool,
}

impl PageGroups {
    /// Add the next group; then leave out the oldest while the page is
    /// past its budget, never the newest.
    fn push(&mut self, items: Vec<String>) {
        if items.is_empty() {
            return;
        }
        self.bytes += items.iter().map(String::len).sum::<usize>();
        self.groups.push_back(items);
        while self.bytes > self.budget && self.groups.len() > 1 {
            let oldest = self.groups.pop_front().unwrap_or_default();
            self.bytes -= oldest.iter().map(String::len).sum::<usize>();
            self.dropped = true;
        }
    }
}

/// Items as a page sends them: overlaid, as JSON text.
fn written(store: &Store, id: &str, items: Vec<Item>) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(items.len());
    for mut item in items {
        overlay(store, id, &mut item)?;
        out.push(serde_json::to_string(&item)?);
    }
    Ok(out)
}

/// `GET /api/view/sessions/{id}/turns/{turn_id}` (plan 4a-i's hand-off;
/// frontend §6.1 "send again"): the prompt of a turn of the session that
/// was not delivered, as a `turn_not_delivered` marker's `about_turn`
/// names it. 404 for any other turn: one that was delivered is its
/// `user_turn` item.
async fn undelivered_turn(State(state): State<AppState>, Path((id, turn_id)): Path<(String, String)>) -> Response {
    // Before anything else, as every route of a session's (A1).
    match state.store.find_session_item(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    }
    let content = match state.store.undelivered_turn(&id, &turn_id) {
        Ok(Some(content)) => content,
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no turn of this session went undelivered",
            );
        }
        Err(err) => return internal(err),
    };
    // Whole, as stored: a prompt sent again is the one written (the
    // security review's B-4). The prompt route bounded it when it came.
    let Ok(content) = serde_json::from_str(&content) else {
        return internal(anyhow::anyhow!("turn {turn_id} holds content that is not a prompt's"));
    };
    Json(TurnContent { turn_id, content }).into_response()
}

/// Where the group `turn` names starts: its turn's `user_turn`, or for a
/// group keyed by its event (`event-<id>`, `group_key`), that event.
fn turn_start(store: &Store, id: &str, turn: &str) -> Result<Option<i64>> {
    if let Some(start) = store.turn_start(id, turn)? {
        return Ok(Some(start));
    }
    let Some(event_id) = turn.strip_prefix("event-").and_then(|n| n.parse::<i64>().ok()) else {
        return Ok(None);
    };
    let (start, turn_id) = store.group_start_at(id, event_id)?;
    Ok((start > 0 && start == event_id && group_key(start, turn_id.as_deref()) == turn).then_some(start))
}

/// The fold's `answerable` input: none. A question's `answerable` is the
/// overlay's alone (`overlay`), read from its record as the item is sent,
/// after every event it shows has been read (4a-i's hand-off). Every
/// change to what decides it (the question's state, an answer queued) is
/// written with a question event in the same transaction, so the version
/// the overlay raises it to is that event's (the answerable pair: one
/// verified line instead of two equal ones).
fn unanswerable(_: &str) -> bool {
    false
}

/// A question item as the store's record has it (A4; the review's A-8):
/// its state, never going back, and `answerable` the server's, decided
/// here only. A change raises its version to the question's last event,
/// never lowers it. Without a record (none exists for a question the fold
/// holds, short of a delete racing the read) it stays as folded: not
/// answerable.
fn overlay(store: &Store, id: &str, item: &mut Item) -> Result<()> {
    let Body::Question(question) = &mut item.body else {
        return Ok(());
    };
    let Some(record) = store.session_pending(id, &question.pending_id)? else {
        return Ok(());
    };
    let before = question.clone();
    question.absorb(record.state, record.reason, record.answered, record.delivered);
    question.answerable = record.state == PendingState::Open && !record.answered;
    if *question != before
        && let Some(last) = store.pending_last_event(id, &question.pending_id)?
    {
        item.version = item.version.max(last);
    }
    Ok(())
}

/// A question asked before the events a fold has, as the store holds it
/// (A4; the review's A-8): in the group its `pending_opened` is in, with
/// that event's time, at the version of its last event. `None` without a
/// record or that event, and for an id no fold would give an item.
fn stored_question(store: &Store, id: &str, pending_id: &str) -> Result<Option<Item>> {
    if !shown(pending_id) {
        return Ok(None);
    }
    let Some(record) = store.session_pending(id, pending_id)? else {
        return Ok(None);
    };
    let (Some((opened, ts)), Some(version)) = (
        store.pending_opened_at(id, pending_id)?,
        store.pending_last_event(id, pending_id)?,
    ) else {
        return Ok(None);
    };
    let (start, turn) = store.group_start_at(id, opened)?;
    Ok(Some(Item {
        id: question_id(pending_id),
        version,
        turn_id: (start > 0).then(|| group_key(start, turn.as_deref())),
        ts,
        body: Body::Question(Question::from_pending(&record)),
    }))
}

/// Whether a pending id is one the fold makes an item of: not empty, and
/// within `ID_MAX_BYTES` (4a-i's caps). One it refuses is never shown, nor
/// counted (the plan review's 5).
fn shown(pending_id: &str) -> bool {
    !pending_id.is_empty() && pending_id.len() <= ID_MAX_BYTES
}

/// `GET /api/stream/view/sessions/{id}`'s query.
#[derive(Deserialize)]
struct StreamParams {
    after: Option<String>,
}

/// `GET /api/stream/sessions`' query: `hat` scopes `waiting_changed`'s
/// count only; every session's upserts are sent.
#[derive(Deserialize)]
struct ListStreamParams {
    after: Option<String>,
    hat: Option<String>,
}

/// Where a stream resumes: `Last-Event-ID`, else `?after=` (the review's
/// A-1). A header decides when it is there, readable or not.
fn stream_anchor(headers: &HeaderMap, params: StreamParams) -> Option<String> {
    match headers.get("last-event-id") {
        Some(value) => Some(value.to_str().unwrap_or_default().to_string()),
        None => params.after,
    }
}

/// The revision `anchor` names, if it is of this process's epoch.
fn revision_of(anchor: Option<&str>) -> Option<i64> {
    let (anchor_epoch, revision) = parse_anchor(anchor?)?;
    (anchor_epoch == epoch()).then_some(revision)
}

/// An SSE stream of what `rx` gets, until the collector shuts down or the
/// operator's session that opened it ends (3b decision 7). Its task sees
/// `rx` dropped then, and ends.
fn sse(state: &AppState, operator_session: Authenticated, rx: mpsc::Receiver<Queued>) -> Response {
    let stream = delivered(rx)
        .take_until(state.shutdown.clone().cancelled_owned())
        .take_until(hennery_kernel::auth::session_ended(
            state.operator.clone(),
            operator_session,
        ));
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response()
}

/// The queue's messages as the response takes them: each gives its room
/// back as it leaves.
fn delivered(rx: mpsc::Receiver<Queued>) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    ReceiverStream::new(rx).map(|(message, _room)| Ok::<_, Infallible>(message))
}

/// An SSE message, with what it weighs.
struct Out {
    event: Event,
    bytes: usize,
}

impl Out {
    /// The message `kind` with `data`.
    fn new(kind: &str, data: String) -> Self {
        let bytes = kind.len() + data.len();
        Self {
            event: Event::default().event(kind).data(data),
            bytes,
        }
    }
}

fn resync() -> Out {
    Out::new("resync_required", "{}".into())
}

/// A message in the queue, and the room it takes there.
type Queued = (Event, OwnedSemaphorePermit);

/// The sending end of a stream's queue, bounded by `QUEUE_DEPTH` messages
/// and by bytes (the security review's B-7): a message waits for its room,
/// so a slow client holds its producer, not more memory.
struct Outbox {
    tx: mpsc::Sender<Queued>,
    room: Arc<Semaphore>,
    cap: usize,
}

impl Outbox {
    /// A queue of `cap` bytes, and its receiving end.
    fn new(cap: usize) -> (Self, mpsc::Receiver<Queued>) {
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        let outbox = Self {
            tx,
            room: Arc::new(Semaphore::new(cap)),
            cap,
        };
        (outbox, rx)
    }

    /// Queue `out` once it has room: its bytes, or the whole queue for one
    /// heavier, which then goes alone. Whether the stream still has a
    /// reader.
    async fn send(&self, out: Out) -> bool {
        let need = out.bytes.clamp(1, self.cap) as u32;
        let room = tokio::select! {
            room = self.room.clone().acquire_many_owned(need) => room,
            () = self.tx.closed() => return false,
        };
        let Ok(room) = room else { return false };
        self.tx.send((out.event, room)).await.is_ok()
    }

    /// Ready once the reader is gone.
    async fn closed(&self) {
        self.tx.closed().await;
    }
}

/// Send `messages`, only the last carrying `id` (the review's A-3: a
/// client that keeps the id has had every upsert up to it). Whether the
/// stream still has a reader.
async fn send_burst(tx: &Outbox, messages: Vec<Out>, id: String) -> bool {
    let last = messages.len().saturating_sub(1);
    for (at, mut message) in messages.into_iter().enumerate() {
        if at == last {
            message.bytes += id.len();
            message.event = message.event.id(id.clone());
        }
        if !tx.send(message).await {
            return false;
        }
    }
    true
}

/// The last message of an item stream whose session was deleted (plan
/// 9a; the security review's B-3), with no id: there is nothing to resume.
fn removed(session_id: &str) -> Result<Out> {
    let gone = SessionRemoved {
        session_id: session_id.to_string(),
    };
    Ok(Out::new("session_removed", serde_json::to_string(&gone)?))
}

/// `GET /api/stream/view/sessions/{id}` (A5): SSE `item`, one whole item
/// each, from the anchor on; `resync_required` when it cannot resume
/// there. Every burst's last message carries `<epoch>:<cursor>`.
async fn item_stream(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    Query(params): Query<StreamParams>,
    headers: HeaderMap,
) -> Response {
    // Before anything else: a session that is not the owner's answers 404
    // having read no event (A1).
    match state.store.find_session_item(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    }
    let anchor = stream_anchor(&headers, params);
    // Before any event is read: one stored after the reads rings it.
    let doorbell = state.hub.subscribe();
    let (tx, rx) = Outbox::new(QUEUE_BYTES);
    let mut items = ItemStream::new(state.store.clone(), id);
    tokio::spawn(async move {
        if let Err(err) = items.run(anchor.as_deref(), doorbell, &tx).await {
            tracing::error!(error = %err, session_id = %items.id, "item stream ended");
        }
    });
    sse(&state, operator_session, rx)
}

/// One item stream's state (A6): the fold of the session's current group,
/// and what it has read but not yet sent.
struct ItemStream {
    store: Arc<Store>,
    id: String,
    fold: Fold,
    /// The last event read: every upsert of the events up to it is sent
    /// or held.
    cursor: i64,
    /// Where the fold's current group starts.
    group_start: i64,
    /// Questions asked in an earlier group whose events came since the
    /// last send: their items are the store's.
    outside: Vec<String>,
    /// An item changed since the last send by something other than a
    /// message or thought chunk: send now.
    urgent: bool,
    /// A message or thought chunk changed an item since the last send.
    chunks: bool,
    /// When the stream began to hold those chunks' items.
    held: Option<Instant>,
    /// The session was deleted: what was read is sent, then
    /// `session_removed`, and the stream ends.
    deleted: bool,
    /// The catalogue changed since the last send (`changes_catalogue`):
    /// `catalog_changed` ends the next burst (frontend §4.1).
    catalog: bool,
    /// More questions of earlier groups changed since the last send than
    /// `GROUP_MAX_QUESTIONS`: the stream resyncs rather than leave any out
    /// (the security review's B-2).
    overflow: bool,
}

impl ItemStream {
    fn new(store: Arc<Store>, id: String) -> Self {
        Self {
            store,
            id,
            fold: Fold::new(),
            cursor: 0,
            group_start: 0,
            outside: Vec::new(),
            urgent: false,
            chunks: false,
            held: None,
            deleted: false,
            catalog: false,
            overflow: false,
        }
    }

    async fn run(
        &mut self,
        anchor: Option<&str>,
        mut doorbell: broadcast::Receiver<EventDto>,
        tx: &Outbox,
    ) -> Result<()> {
        let Some(burst) = self.resume(anchor, RESUME_MAX_BYTES)? else {
            // Sent once, then the stream ends; a reader gone is no error.
            tx.send(resync()).await;
            return Ok(());
        };
        if !burst.is_empty() && !send_burst(tx, burst, anchor_now(self.cursor)).await {
            return Ok(());
        }
        if self.deleted {
            tx.send(removed(&self.id)?).await;
            return Ok(());
        }
        loop {
            let flush_at = self.held.map(|held| held + COALESCE);
            tokio::select! {
                () = tx.closed() => return Ok(()),
                rung = doorbell.recv() => match rung {
                    Ok(event) if event.session_id != self.id => continue,
                    // Lagged: whatever it skipped is in the store.
                    Ok(_) | Err(RecvError::Lagged(_)) => {
                        if !self.catch_up(tx).await? {
                            return Ok(());
                        }
                        if self.overflow {
                            tx.send(resync()).await;
                            return Ok(());
                        }
                        // Deleted (plan 9a): what is held goes now, then
                        // `session_removed`, and the stream ends.
                        if self.deleted {
                            if self.send(tx).await? {
                                tx.send(removed(&self.id)?).await;
                            }
                            return Ok(());
                        }
                    }
                    Err(RecvError::Closed) => return Ok(()),
                },
                () = sleep_until(flush_at), if flush_at.is_some() => {
                    if !self.send(tx).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Fold from the start of the group holding the event after `anchor`'s
    /// revision to the session's revision: the items changed past the
    /// anchor, each once, as they stand, and the questions of earlier
    /// groups whose events came since, from the store. `None`: no anchor,
    /// another epoch, a revision the session never reached, or more than a
    /// resume sends.
    fn resume(&mut self, anchor: Option<&str>, budget: usize) -> Result<Option<Vec<Out>>> {
        let revision = self.store.revision(&self.id)?;
        let Some(after) = revision_of(anchor).filter(|after| *after <= revision) else {
            return Ok(None);
        };
        let (events, groups) = self
            .store
            .count_after(&self.id, after, RESUME_MAX_EVENTS, RESUME_MAX_GROUPS)?;
        if events > RESUME_MAX_EVENTS || groups > RESUME_MAX_GROUPS {
            return Ok(None);
        }
        let first = self
            .store
            .first_event_after(&self.id, after)?
            .filter(|first| *first <= revision)
            .unwrap_or(revision);
        (self.group_start, _) = self.store.group_start_at(&self.id, first)?;
        let window = self.group_start;
        self.cursor = window - 1;
        // Group by group (the security review's B-1): a group's changes are
        // taken when the next begins (locality: nothing later changes
        // them), written out and counted against `budget`.
        let mut burst = Burst {
            messages: Vec::new(),
            bytes: 0,
            budget,
        };
        loop {
            let batch = self.store.events_between(&self.id, self.cursor, revision, BATCH)?;
            let Some(last) = batch.last() else { break };
            self.cursor = last.event_id;
            for event in &batch {
                if event.kind == SESSION_DELETED {
                    self.deleted = true;
                    continue;
                }
                self.catalog |= event.event_id > after && changes_catalogue(event);
                if event.kind == "user_turn" {
                    let ended = self.fold.take().into_iter().filter(|item| item.version > after);
                    if !burst.add(self.messages(ended.collect())?) {
                        return Ok(None);
                    }
                    self.group_start = event.event_id;
                }
                self.fold.apply(event, &unanswerable);
            }
        }
        let last = self.fold.take().into_iter().filter(|item| item.version > after);
        if !burst.add(self.messages(last.collect())?) {
            return Ok(None);
        }
        self.cursor = self.cursor.max(revision);
        let mut stored = 0;
        for pending_id in self.store.pending_touched_after(&self.id, after)? {
            let opened = self.store.pending_opened_at(&self.id, &pending_id)?;
            // One asked in the window is the fold's (or was elided there).
            if opened.is_none_or(|(opened, _)| opened >= window) {
                continue;
            }
            if let Some(item) = stored_question(&self.store, &self.id, &pending_id)?
                && item.version > after
            {
                // Never one left out (the security review's B-2).
                if stored == GROUP_MAX_QUESTIONS {
                    return Ok(None);
                }
                stored += 1;
                if !burst.add(self.messages(vec![item])?) {
                    return Ok(None);
                }
            }
        }
        // Once, after the items, if the window changed it past the anchor.
        if std::mem::take(&mut self.catalog)
            && let Some(message) = self.catalog_changed()?
            && !burst.add(vec![message])
        {
            return Ok(None);
        }
        Ok(Some(burst.messages))
    }

    /// SSE `catalog_changed`: the session's whole catalogue as it stands
    /// (as `GET /api/sessions/{id}/catalog` serves it); `None` once the
    /// session is gone.
    fn catalog_changed(&self) -> Result<Option<Out>> {
        let Some(catalog) = self.store.catalog(&self.id)? else {
            return Ok(None);
        };
        Ok(Some(Out::new("catalog_changed", serde_json::to_string(&catalog)?)))
    }

    /// Read and apply every event past the cursor, a batch at a time,
    /// sending after a batch that changed anything but chunks. Whether the
    /// stream still has a reader.
    async fn catch_up(&mut self, tx: &Outbox) -> Result<bool> {
        loop {
            let batch = self.store.events_between(&self.id, self.cursor, i64::MAX, BATCH)?;
            if batch.is_empty() {
                break;
            }
            for event in &batch {
                // A new group: the ended one's changes are sent first, so
                // the fold holds no more than one group's (plan 4a-i
                // decision 12). Every event up to the cursor is applied.
                if event.kind == "user_turn" && (self.urgent || self.chunks) && !self.send(tx).await? {
                    return Ok(false);
                }
                self.apply(event)?;
                if self.overflow {
                    return Ok(true);
                }
            }
            if self.urgent && !self.send(tx).await? {
                return Ok(false);
            }
        }
        // Only chunks since the last send: their items are held, from the
        // first read, for at most `COALESCE`.
        if self.chunks && self.held.is_none() {
            self.held = Some(Instant::now());
        }
        Ok(true)
    }

    fn apply(&mut self, event: &EventDto) -> Result<()> {
        if event.kind == SESSION_DELETED {
            self.cursor = event.event_id;
            self.deleted = true;
            return Ok(());
        }
        let changed = self.fold.apply(event, &unanswerable);
        self.cursor = event.event_id;
        if changes_catalogue(event) {
            self.catalog = true;
            self.urgent = true;
        }
        if event.kind == "user_turn" {
            self.group_start = event.event_id;
        }
        if changed {
            if is_chunk(event) {
                self.chunks = true;
            } else {
                self.urgent = true;
            }
        }
        // A question of an earlier group: not the fold's; its item is the
        // store's. One of this group that is not in the fold was elided.
        if let Some(pending_id) = question_event(event)
            && shown(pending_id)
            && self.fold.current().get(&question_id(pending_id)).is_none()
            && !self.outside.iter().any(|p| p == pending_id)
            && self
                .store
                .pending_opened_at(&self.id, pending_id)?
                .is_some_and(|(opened, _)| opened < self.group_start)
        {
            // Never one left out (the security review's B-2).
            if self.outside.len() == GROUP_MAX_QUESTIONS {
                self.overflow = true;
                return Ok(());
            }
            self.outside.push(pending_id.to_string());
            self.urgent = true;
        }
        Ok(())
    }

    /// Send what changed since the last send: the fold's items, the held
    /// chunks' first, and the outside questions. Whether the stream still
    /// has a reader.
    async fn send(&mut self, tx: &Outbox) -> Result<bool> {
        let mut changed = Changed::default();
        changed.extend(self.fold.take());
        for pending_id in std::mem::take(&mut self.outside) {
            if let Some(item) = stored_question(&self.store, &self.id, &pending_id)? {
                changed.upsert(item);
            }
        }
        self.urgent = false;
        self.chunks = false;
        self.held = None;
        let mut messages = self.messages(changed.items)?;
        // After the burst's items, so only the burst's last message has
        // the id.
        if std::mem::take(&mut self.catalog)
            && let Some(message) = self.catalog_changed()?
        {
            messages.push(message);
        }
        if messages.is_empty() {
            return Ok(true);
        }
        Ok(send_burst(tx, messages, anchor_now(self.cursor)).await)
    }

    /// Each item as an SSE `item`, its question overlaid with the store's
    /// record.
    fn messages(&self, items: Vec<Item>) -> Result<Vec<Out>> {
        let mut out = Vec::with_capacity(items.len());
        for mut item in items {
            overlay(&self.store, &self.id, &mut item)?;
            out.push(Out::new("item", serde_json::to_string(&item)?));
        }
        Ok(out)
    }
}

/// What a resume sends, as it is written, and its budget.
struct Burst {
    messages: Vec<Out>,
    bytes: usize,
    budget: usize,
}

impl Burst {
    /// Add `messages`; whether the burst is still within its budget.
    fn add(&mut self, messages: Vec<Out>) -> bool {
        for message in messages {
            self.bytes += message.bytes;
            self.messages.push(message);
        }
        self.bytes <= self.budget
    }
}

/// `<epoch>:<cursor>`.
fn anchor_now(cursor: i64) -> String {
    anchor(epoch(), cursor)
}

/// Sleep until `at`; never, without one (`select!` skips the branch then).
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// A message or thought chunk: what is held to be merged.
fn is_chunk(event: &EventDto) -> bool {
    event.kind == "acp_update"
        && matches!(
            event
                .body
                .pointer("/payload/update/sessionUpdate")
                .and_then(|k| k.as_str()),
            Some("agent_message_chunk" | "agent_thought_chunk")
        )
}

/// The question an event of a question's is about.
fn question_event(event: &EventDto) -> Option<&str> {
    matches!(
        event.kind.as_str(),
        "pending_opened" | "answer_submitted" | "answer_result" | "pending_resolved" | "pending_cancelled"
    )
    .then(|| event.body.get("pending_id")?.as_str())
    .flatten()
}

/// Items to send, each once, as it last stood, in the order each first
/// came.
#[derive(Default)]
struct Changed {
    items: Vec<Item>,
    at: HashMap<String, usize>,
}

impl Changed {
    fn upsert(&mut self, item: Item) {
        match self.at.get(&item.id) {
            Some(&at) => self.items[at] = item,
            None => {
                self.at.insert(item.id.clone(), self.items.len());
                self.items.push(item);
            }
        }
    }

    fn extend(&mut self, items: impl IntoIterator<Item = Item>) {
        for item in items {
            self.upsert(item);
        }
    }
}

/// `GET /api/stream/sessions` (A8; ACP core §9): SSE `session_upsert`, one
/// summary each, for every session with an event past the anchor, then as
/// events come, and `session_removed` for one deleted (plan 9a);
/// `waiting_changed`, the count of sessions waiting within `?hat=`, after
/// the resume and whenever it changes (4c's request);
/// `resync_required` when it cannot resume there. Every
/// burst's last message carries `<epoch>:<cursor>`, the owner's greatest
/// event id read before it.
async fn list_stream(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Query(params): Query<ListStreamParams>,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = refuse_empty_hat(params.hat.as_deref()) {
        return refused.into_response();
    }
    let ListStreamParams { after, hat } = params;
    let anchor = stream_anchor(&headers, StreamParams { after });
    let doorbell = state.hub.subscribe();
    let (tx, rx) = Outbox::new(QUEUE_BYTES);
    let store = state.store.clone();
    tokio::spawn(async move {
        if let Err(err) = list_follow(&store, anchor.as_deref(), hat.as_deref(), doorbell, &tx).await {
            tracing::error!(error = %err, "session list stream ended");
        }
    });
    sse(&state, operator_session, rx)
}

async fn list_follow(
    store: &Store,
    anchor: Option<&str>,
    hat: Option<&str>,
    mut doorbell: broadcast::Receiver<EventDto>,
    tx: &Outbox,
) -> Result<()> {
    // Read before the sessions that changed: an event stored between the
    // two is sent now and again later, never missed.
    let mut cursor = store.max_event_id()?;
    let Some(after) = revision_of(anchor).filter(|after| *after <= cursor) else {
        tx.send(resync()).await;
        return Ok(());
    };
    // Nothing since: nothing to replay, however old the anchor.
    if after < cursor {
        let cutoff = stamp_ago(LIST_WINDOW);
        if !store.event_ts(after)?.is_some_and(|ts| ts >= cutoff) {
            tx.send(resync()).await;
            return Ok(());
        }
    }
    // What this stream last sent of each session: one equal to it is not
    // sent again.
    let mut sent: HashMap<String, SessionSummary> = HashMap::new();
    // The count this stream last sent: none yet, so the resume's burst
    // ends with it, whatever changed.
    let mut counted: Option<u32> = None;
    let mut changed = store.sessions_changed_after(after)?;
    loop {
        // Every change to what it counts (an activity, a question's state,
        // a hat, a delete) is written with an event of the session's.
        let count = if counted.is_none() || !changed.is_empty() {
            Some(store.waiting_count(hat)?)
        } else {
            None
        };
        let waiting = if changed.is_empty() {
            HashSet::new()
        } else {
            store.sessions_waiting()?
        };
        let mut messages = Vec::new();
        for session_id in changed {
            let Some(item) = store.find_session_item(&session_id)? else {
                // Deleted (plan 9a; ACP core §9): its tombstone. It has
                // no event after its `session_deleted` (triggers refuse
                // them), so this is sent once, or twice when the delete
                // is committed between the cursor's read and this round's
                // (the re-confirmation's 1): a keyed delete, which a
                // client applies again harmlessly. A session neither
                // listed nor a tombstone is not the owner's to name.
                if store.session_host(&session_id)?.is_some() {
                    messages.push(removed(&session_id)?);
                }
                continue;
            };
            let summary = summary(item.bounded(), &waiting);
            if sent.get(&session_id) == Some(&summary) {
                continue;
            }
            messages.push(Out::new("session_upsert", serde_json::to_string(&summary)?));
            sent.insert(session_id, summary);
        }
        // Last, so it carries the id when it is sent: the count as of the
        // cursor.
        if let Some(count) = count.filter(|count| counted != Some(*count)) {
            messages.push(Out::new(
                "waiting_changed",
                serde_json::to_string(&WaitingChanged { count })?,
            ));
            counted = Some(count);
        }
        if !messages.is_empty() && !send_burst(tx, messages, anchor_now(cursor)).await {
            return Ok(());
        }
        // The next event, of any session's; a lag or a delete reads now,
        // anything else after the hold.
        let held = tokio::select! {
            () = tx.closed() => return Ok(()),
            rung = doorbell.recv() => match rung {
                Err(RecvError::Closed) => return Ok(()),
                Err(RecvError::Lagged(_)) => false,
                Ok(event) => event.kind != SESSION_DELETED,
            },
        };
        if held && !hold(&mut doorbell, tx).await {
            return Ok(());
        }
        let max = store.max_event_id()?;
        changed = if max > cursor {
            store.sessions_changed_after(cursor)?
        } else {
            Vec::new()
        };
        cursor = cursor.max(max);
    }
}

/// The list stream's hold (`LIST_COALESCE`): the rings meanwhile are
/// absorbed; it ends early on a lag or a delete, which are read now.
/// Whether the stream still has a reader.
async fn hold(doorbell: &mut broadcast::Receiver<EventDto>, tx: &Outbox) -> bool {
    let until = Instant::now() + LIST_COALESCE;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(until) => return true,
            () = tx.closed() => return false,
            rung = doorbell.recv() => match rung {
                Err(RecvError::Closed) => return false,
                Err(RecvError::Lagged(_)) => return true,
                Ok(event) if event.kind == SESSION_DELETED => return true,
                Ok(_) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_view::api::ItemPage;

    /// A store with the owner's session `s`, and a connection to write its
    /// events as ingest would.
    fn store() -> (tempfile::TempDir, Arc<Store>, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let store = Arc::new(Store::open(&db).unwrap());
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id)
             VALUES ('s', 'host-1', 'fake', '/tmp', 'active', '2026-10-01T00:00:00.000Z',
                     '2026-10-01T00:00:00.000Z', ?1)",
            [store.owner_id()],
        )
        .unwrap();
        (dir, store, conn)
    }

    fn write(conn: &rusqlite::Connection, store: &Store, kind: &str, body: serde_json::Value) -> EventDto {
        conn.execute(
            "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
             VALUES ('s', NULL, ?1, ?2, '2026-10-01T00:00:00.000Z', ?3)",
            rusqlite::params![kind, body.to_string(), store.owner_id()],
        )
        .unwrap();
        EventDto {
            event_id: conn.last_insert_rowid(),
            session_id: "s".into(),
            host_seq: None,
            kind: kind.into(),
            body,
            ts: String::new(),
        }
    }

    fn tool(call: &str) -> serde_json::Value {
        serde_json::json!({"kind": "acp_update", "indexed": {}, "payload": {"sessionId": "a",
            "update": {"sessionUpdate": "tool_call", "toolCallId": call, "title": "Run", "status": "pending"}}})
    }

    /// 4a-i's whole-branch review: a page past its byte budget leaves out
    /// its oldest groups, never its newest, and says `older`; the groups
    /// left out are the next page's.
    #[test]
    fn a_page_past_its_budget_keeps_its_newest_groups() {
        let (_dir, store, conn) = store();
        for turn in ["t1", "t2", "t3"] {
            write(
                &conn,
                &store,
                "user_turn",
                serde_json::json!({"turn_id": turn, "content": []}),
            );
            let chunk = serde_json::json!({"kind": "acp_update", "indexed": {}, "payload": {"sessionId": "a",
                "update": {"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": "x".repeat(1000)}}}});
            write(&conn, &store, "acp_update", chunk);
        }
        let page = |before: Option<&str>, budget: usize| -> ItemPage {
            serde_json::from_str(&read_page(&store, "s", before, 20, budget).unwrap().unwrap()).unwrap()
        };
        let turns = |page: &ItemPage| -> Vec<String> {
            let mut turns: Vec<String> = page.items.iter().filter_map(|i| i.turn_id.clone()).collect();
            turns.dedup();
            turns
        };
        let whole = page(None, usize::MAX);
        let events = store.events("s", 0, 100).unwrap();
        assert_eq!(whole.items, hennery_view::fold(&events, &|_: &str| false));
        assert_eq!(
            (turns(&whole), whole.older),
            (vec!["t1".into(), "t2".into(), "t3".into()], false)
        );
        let weight = |turn: &str| -> usize {
            whole
                .items
                .iter()
                .filter(|i| i.turn_id.as_deref() == Some(turn))
                .map(|i| serde_json::to_string(i).unwrap().len())
                .sum()
        };

        let two = page(None, weight("t2") + weight("t3"));
        assert_eq!((turns(&two), two.older), (vec!["t2".into(), "t3".into()], true));
        assert_eq!(&two.items[..], &whole.items[2..]);
        // The newest group, whatever it weighs.
        let one = page(None, 1);
        assert_eq!((turns(&one), one.older), (vec!["t3".into()], true));
        // What was left out is before the page's first group.
        let rest = page(Some("t2"), usize::MAX);
        assert_eq!((turns(&rest), rest.older), (vec!["t1".into()], false));
    }

    /// Plan 9a: a delete that lands while a page or a resume reads makes
    /// no item, and a resume that reads it ends the stream after what it
    /// read (the routes answer 404 for a tombstone before reading).
    #[tokio::test]
    async fn a_session_deleted_while_read_makes_no_item_and_ends_the_stream() {
        let (_dir, store, conn) = store();
        let start = write(
            &conn,
            &store,
            "user_turn",
            serde_json::json!({"turn_id": "t1", "content": []}),
        );
        write(&conn, &store, "acp_update", tool("c1"));
        let deleted = write(&conn, &store, SESSION_DELETED, serde_json::json!({}));
        let page: ItemPage =
            serde_json::from_str(&read_page(&store, "s", None, 20, usize::MAX).unwrap().unwrap()).unwrap();
        let ids: Vec<&str> = page.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["t1:0", "t1:tool:c1"]);

        let (_bell, doorbell) = broadcast::channel(1);
        let (tx, mut rx) = Outbox::new(QUEUE_BYTES);
        let mut items = ItemStream::new(store.clone(), "s".into());
        let anchor = anchor_now(start.event_id);
        let task = tokio::spawn(async move { items.run(Some(&anchor), doorbell, &tx).await });
        let sent = item(&mut rx).await;
        assert!(
            sent.contains("t1:tool:c1") && sent.contains(&format!(":{}", deleted.event_id)),
            "{sent}"
        );
        // Then, with no id, the session's removal (the security review's
        // B-3).
        let gone = item(&mut rx).await;
        assert!(
            gone.contains("session_removed")
                && gone.contains(r#"{\"session_id\":\"s\"}"#)
                && !gone.contains(&format!(":{}", deleted.event_id)),
            "{gone}"
        );
        tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the stream ends")
            .unwrap()
            .unwrap();
        assert!(rx.recv().await.is_none());
    }

    async fn item(rx: &mut mpsc::Receiver<Queued>) -> String {
        let (event, _room) = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("an item in time")
            .expect("the stream goes on");
        format!("{event:?}")
    }

    fn texts(messages: &[Out]) -> Vec<String> {
        messages.iter().map(|m| format!("{:?}", m.event)).collect()
    }

    /// The answerable pair: `answerable` is the overlay's alone. A
    /// question with a record is served answerable while it is open with
    /// no answer queued, and not once it is not open; one with no record (none exists for a question
    /// the fold holds, short of a delete racing the read) is served as
    /// folded: not answerable, at its own version.
    #[test]
    fn answerable_comes_from_the_record_and_never_without_one() {
        let (_dir, store, conn) = store();
        write(
            &conn,
            &store,
            "user_turn",
            serde_json::json!({"turn_id": "t1", "content": []}),
        );
        let opened = |pending_id: &str| {
            serde_json::json!({"kind": "pending_opened", "pending_id": pending_id,
                "indexed": {"turn_id": "t1", "pending": {"id": pending_id, "kind": "permission", "option_ids": ["allow"]}},
                "payload": {"toolCall": {"toolCallId": "c1", "title": "Write"},
                            "options": [{"optionId": "allow", "name": "Yes", "kind": "allow_once"}]}})
        };
        let recorded = write(&conn, &store, "pending_opened", opened("p1"));
        conn.execute(
            "INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at, owner_id)
             VALUES ('p1', 's', 'permission', 't1', '[\"allow\"]', '{}', 'open', '2026-10-01T00:00:00.000Z', ?1)",
            [store.owner_id()],
        )
        .unwrap();
        let unrecorded = write(&conn, &store, "pending_opened", opened("p2"));
        // Cancelled by the collector, with no verdict event the fold reads
        // as one: the record is not open, so it is not answerable.
        write(&conn, &store, "pending_opened", opened("p3"));
        conn.execute(
            "INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at, owner_id)
             VALUES ('p3', 's', 'permission', 't1', '[\"allow\"]', '{}', 'cancelled', '2026-10-01T00:00:00.000Z', ?1)",
            [store.owner_id()],
        )
        .unwrap();
        let page: ItemPage =
            serde_json::from_str(&read_page(&store, "s", None, 20, usize::MAX).unwrap().unwrap()).unwrap();
        let served = |id: &str| -> (bool, i64) {
            let item = page.items.iter().find(|i| i.id == id).unwrap();
            let Body::Question(q) = &item.body else {
                panic!("{item:?}")
            };
            (q.answerable, item.version)
        };
        assert_eq!(served("question:p1"), (true, recorded.event_id));
        assert_eq!(served("question:p2"), (false, unrecorded.event_id));
        assert!(!served("question:p3").0);
    }

    /// The security review's B-1: a resume counts what it writes against
    /// its budget; within it, it sends what it always did, past it, it
    /// resyncs.
    #[test]
    fn a_resume_past_its_budget_resyncs() {
        let (_dir, store, conn) = store();
        let start = write(
            &conn,
            &store,
            "user_turn",
            serde_json::json!({"turn_id": "t1", "content": []}),
        );
        for (n, turn) in ["t1", "t2", "t3"].iter().enumerate() {
            if n > 0 {
                write(
                    &conn,
                    &store,
                    "user_turn",
                    serde_json::json!({"turn_id": turn, "content": []}),
                );
            }
            write(&conn, &store, "acp_update", tool(&format!("c{n}")));
        }
        let anchor = anchor_now(start.event_id);
        let resume = |budget| {
            ItemStream::new(store.clone(), "s".into())
                .resume(Some(&anchor), budget)
                .unwrap()
        };
        let whole = resume(usize::MAX).unwrap();
        let ids: Vec<String> = texts(&whole);
        assert_eq!(ids.len(), 5, "{ids:?}");
        for (text, id) in ids
            .iter()
            .zip(["t1:tool:c0", "t2:0", "t2:tool:c1", "t3:0", "t3:tool:c2"])
        {
            assert!(text.contains(id), "{text} is not {id}");
        }
        let bytes: usize = whole.iter().map(|m| m.bytes).sum();
        assert_eq!(texts(&resume(bytes).unwrap()), ids);
        assert!(resume(bytes - 1).is_none());

        // Past it at a group's end, it reads no further: what it holds is
        // bounded as it goes, not only judged at the end.
        let first: usize = whole[0].bytes;
        for _ in 0..BATCH {
            write(&conn, &store, "git_state", serde_json::json!({}));
        }
        let reads = |budget| {
            let before = store.view_reads();
            assert!(resume(budget).is_none());
            store.view_reads() - before
        };
        // Past it in the first group, against past it only in the last.
        let (early, late) = (reads(first - 1), reads(bytes - 1));
        assert!(early < late, "{early} reads, then {late}");
    }

    /// The security review's B-7: a stream's queue holds `cap` bytes; a
    /// message waits for its room, so a slow reader holds the producer
    /// back; one heavier than the queue goes alone, once it is empty.
    #[tokio::test]
    async fn the_queue_holds_its_producer_back_by_bytes() {
        use futures::FutureExt;
        let (tx, mut rx) = Outbox::new(10);
        let message = |bytes: usize| Out::new("x", "y".repeat(bytes - 1));
        assert_eq!(tx.send(message(8)).now_or_never(), Some(true));
        // No room for 8 more: the producer waits.
        let mut waiting = Box::pin(tx.send(message(8)));
        assert_eq!((&mut waiting).now_or_never(), None);
        let taken = rx.recv().await.unwrap();
        drop(taken);
        assert!(waiting.await);
        drop(rx.recv().await.unwrap());
        // Heavier than the queue: alone, and nothing joins it.
        assert_eq!(tx.send(message(100)).now_or_never(), Some(true));
        let mut small = Box::pin(tx.send(message(1)));
        assert_eq!((&mut small).now_or_never(), None);
        drop(rx.recv().await.unwrap());
        assert!(small.await);
    }

    /// The response gives a message's room back as it takes it.
    #[tokio::test]
    async fn the_response_gives_the_queue_its_room_back() {
        use futures::FutureExt;
        let (tx, rx) = Outbox::new(10);
        let mut response = Box::pin(delivered(rx));
        for _ in 0..3 {
            assert_eq!(tx.send(Out::new("x", "y".repeat(7))).now_or_never(), Some(true));
            assert!(response.next().await.is_some());
        }
    }

    /// The review's A-2: a doorbell that lagged is no loss: the stream
    /// reads the store now, and sends what the skipped rings were for.
    #[tokio::test]
    async fn a_lagged_doorbell_reads_now() {
        let (_dir, store, conn) = store();
        let start = write(
            &conn,
            &store,
            "user_turn",
            serde_json::json!({"turn_id": "t1", "content": []}),
        );
        let (bell, doorbell) = broadcast::channel(1);
        let (tx, mut rx) = Outbox::new(QUEUE_BYTES);
        let mut items = ItemStream::new(store.clone(), "s".into());
        let anchor = anchor_now(start.event_id);
        tokio::spawn(async move { items.run(Some(&anchor), doorbell, &tx).await });
        // Live first: a ring for the session is followed.
        bell.send(write(&conn, &store, "acp_update", tool("c0"))).unwrap();
        assert!(item(&mut rx).await.contains("t1:tool:c0"));
        // Then a ring the stream misses: two rings of another session's
        // overrun the one-message doorbell before the stream runs again.
        write(&conn, &store, "acp_update", tool("c1"));
        let elsewhere = EventDto {
            session_id: "other".into(),
            ..start.clone()
        };
        bell.send(elsewhere.clone()).unwrap();
        bell.send(elsewhere).unwrap();
        assert!(item(&mut rx).await.contains("t1:tool:c1"));
    }

    /// The re-confirmation's ask, the review's A-2 and the plan review's
    /// 3b, for the list stream: a lagged doorbell is read now, not after
    /// the hold, whether the lag is the first ring or comes during a hold,
    /// and loses nothing. Each case has a stream of its own: after a lag
    /// the doorbell still holds its last ring, which would start a hold.
    #[tokio::test]
    async fn a_lagged_list_doorbell_reads_now() {
        let (_dir, store, conn) = store();
        write(
            &conn,
            &store,
            "user_turn",
            serde_json::json!({"turn_id": "t1", "content": []}),
        );
        // A stream from the owner's last event: its resume sends the
        // count alone.
        let follow = || {
            let (bell, doorbell) = broadcast::channel(1);
            let (tx, rx) = Outbox::new(QUEUE_BYTES);
            let (store, anchor) = (store.clone(), anchor_now(store.max_event_id().unwrap()));
            tokio::spawn(async move { list_follow(&store, Some(&anchor), None, doorbell, &tx).await });
            (bell, rx)
        };
        // `s` changed: each read sends its upsert.
        let change = |at: &str| {
            conn.execute("UPDATE sessions SET last_event_at = ?1 WHERE id = 's'", [at])
                .unwrap();
            write(&conn, &store, "acp_update", tool(at))
        };
        let quick = LIST_COALESCE / 2;

        // The first ring is a lag: two rings overrun the one-message
        // doorbell before the stream runs again.
        let (bell, mut rx) = follow();
        assert!(item(&mut rx).await.contains("waiting_changed"));
        let event = change("2026-10-01T00:00:01.000Z");
        let rung = Instant::now();
        bell.send(event.clone()).unwrap();
        bell.send(event).unwrap();
        let message = item(&mut rx).await;
        assert!(
            message.contains("session_upsert") && message.contains("00:00:01"),
            "{message}"
        );
        assert!(rung.elapsed() < quick, "sent after {:?}", rung.elapsed());

        // A lag during a hold: one ring starts the hold (the stream runs up
        // to it when this task yields: the test's runtime has one thread),
        // then two overrun the doorbell.
        let (bell, mut rx) = follow();
        assert!(item(&mut rx).await.contains("waiting_changed"));
        bell.send(change("2026-10-01T00:00:02.000Z")).unwrap();
        tokio::task::yield_now().await;
        let rung = Instant::now();
        let event = change("2026-10-01T00:00:03.000Z");
        bell.send(event.clone()).unwrap();
        bell.send(event).unwrap();
        let message = item(&mut rx).await;
        assert!(
            message.contains("session_upsert") && message.contains("00:00:03"),
            "{message}"
        );
        assert!(rung.elapsed() < quick, "sent after {:?}", rung.elapsed());
    }
}
