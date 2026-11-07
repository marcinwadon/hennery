//! One read model for UI and MCP. Observations do not invent execution state.
use crate::store::Store;
use anyhow::Result;
use hennery_proto::rest::EventDto;
use hennery_view::work::{
    BackgroundWork, ExternalQueueScope, SessionWork, WorkContent, WorkMessage, WorkPlan, WorkQueue, WorkTurn,
    WorkWaiting,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

const CONTENT_EVENTS: usize = 512;
const CONTENT_BYTES: usize = 128 * 1024;
const MESSAGE_BYTES: usize = 1024;
const PLAN_ENTRIES: usize = 4;
const PLAN_LABEL_BYTES: usize = 128;

/// Queue counts retain the caller's existing visibility. No prompt text or
/// operation identifiers belonging to other clients are exposed.
pub(crate) fn snapshot_in(
    conn: &Connection,
    owner: &str,
    session: &str,
    transcript: bool,
    external_client: Option<&str>,
) -> Result<Option<SessionWork>> {
    let row = conn
        .query_row(
            "SELECT s.open_turn_id,COALESCE((SELECT MAX(e.event_id) FROM events e
         WHERE e.owner_id = ?1 AND e.session_id=s.id AND e.applied=1),0)
         ,s.lifecycle,s.activity,s.presumed_parked,s.close_requested,s.configuration_error,s.prompt_queue_held
         FROM sessions s WHERE s.owner_id = ?1 AND s.id=?2 AND s.lifecycle<>'deleted'
         AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=s.hat_id)",
            params![owner, session],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, bool>(4)?,
                    r.get::<_, bool>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, bool>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        current_turn_id,
        revision,
        lifecycle,
        activity,
        presumed_parked,
        close_requested,
        configuration_error,
        held,
    )) = row
    else {
        return Ok(None);
    };
    let last_activity_at = conn
        .query_row(
            "SELECT e.ts FROM events e WHERE e.owner_id = ?1 AND e.session_id=?2 AND e.applied=1
         AND e.kind IN ('acp_update','turn_started','turn_ended','pending_opened','pending_resolved',
             'pending_cancelled','start_failed','session_started','session_parked','session_closed')
         ORDER BY e.event_id DESC LIMIT 1",
            params![owner, session],
            |r| r.get(0),
        )
        .optional()?;
    let last_message_at: Option<String> = conn
        .query_row(
            "SELECT e.ts FROM events e WHERE e.owner_id = ?1 AND e.session_id=?2 AND e.applied=1
         AND e.kind='acp_update' AND json_extract(e.body,'$.payload.update.sessionUpdate')='agent_message_chunk'
         AND json_extract(e.body,'$.payload.update.content.type')='text'
         AND length(json_extract(e.body,'$.payload.update.content.text'))>0
         ORDER BY e.event_id DESC LIMIT 1",
            params![owner, session],
            |r| r.get(0),
        )
        .optional()?;
    let last_turn = conn
        .query_row(
            "SELECT json_extract(e.body,'$.turn_id'),json_extract(e.body,'$.outcome'),e.ts
         FROM events e WHERE e.owner_id = ?1 AND e.session_id=?2 AND e.applied=1
         AND e.kind='turn_ended' ORDER BY e.event_id DESC LIMIT 1",
            params![owner, session],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .optional()?
        .map(|(turn_id, outcome, ended_at)| -> Result<_> {
            Ok(WorkTurn {
                turn_id,
                outcome: serde_json::from_value(outcome.into())?,
                ended_at,
            })
        })
        .transpose()?;
    let waiting = conn.query_row(
        "SELECT COALESCE(sum(p.kind='permission'),0),COALESCE(sum(p.kind<>'permission'),0),
         COALESCE(sum(EXISTS(SELECT 1 FROM answer_queue a WHERE a.owner_id = ?1
             AND a.pending_id=p.pending_id AND a.delivered IS NULL)),0)
         FROM pending p WHERE p.owner_id = ?1 AND p.session_id=?2 AND p.state='open'",
        params![owner, session],
        |r| {
            Ok(WorkWaiting {
                permissions: r.get(0)?,
                questions: r.get(1)?,
                answered: r.get(2)?,
            })
        },
    )?;
    let (operator, delegation) = conn.query_row(
        "SELECT COALESCE(sum(q.origin='operator'),0),COALESCE(sum(q.origin='parent'),0)
         FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=?2
         AND q.state IN ('queued','waiting_config','awaiting_reconciliation')",
        params![owner, session],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let external = conn.query_row(
        "SELECT count(*) FROM external_operations o WHERE o.owner_id = ?1 AND o.session_id=?2
         AND o.tool='session_prompt' AND o.state='queued' AND (?3 IS NULL OR o.client_id=?3)",
        params![owner, session, external_client],
        |r| r.get(0),
    )?;
    let mut content = transcript.then(|| content_in(conn, owner, session)).transpose()?;
    if let Some(message) = content.as_mut().and_then(|content| content.last_message.as_mut())
        && let Some(ts) = &last_message_at
    {
        message.updated_at = ts.clone();
    }
    Ok(Some(SessionWork {
        lifecycle,
        activity,
        presumed_parked,
        close_requested,
        configuration_error,
        revision,
        current_turn_id,
        last_activity_at,
        last_message_at,
        last_turn,
        waiting,
        queue: WorkQueue {
            held,
            operator,
            delegation,
            external,
            external_scope: if external_client.is_some() {
                ExternalQueueScope::Client
            } else {
                ExternalQueueScope::Session
            },
        },
        background: BackgroundWork::NotReported,
        content,
    }))
}

/// The canonical transcript fold supplies message boundaries and plan parsing.
/// Both the input window and the output are bounded, even for hostile payloads.
fn content_in(conn: &Connection, owner: &str, session: &str) -> Result<WorkContent> {
    let mut stmt = conn.prepare(
        "SELECT e.event_id,e.host_seq,e.kind,CASE WHEN length(CAST(e.body AS BLOB))<=?3 THEN e.body END,e.ts
         FROM events e WHERE e.owner_id = ?1 AND e.session_id=?2 AND e.applied=1
         ORDER BY e.event_id DESC LIMIT ?4",
    )?;
    let mut rows = stmt.query(params![
        owner,
        session,
        CONTENT_BYTES as i64,
        (CONTENT_EVENTS + 1) as i64
    ])?;
    let mut events = Vec::new();
    let mut bytes = 0;
    let mut history_truncated = false;
    while let Some(row) = rows.next()? {
        let raw: Option<String> = row.get(3)?;
        let Some(raw) = raw else {
            history_truncated = true;
            break;
        };
        bytes += raw.len();
        if bytes > CONTENT_BYTES || events.len() == CONTENT_EVENTS {
            history_truncated = true;
            break;
        }
        events.push(EventDto {
            event_id: row.get(0)?,
            session_id: session.into(),
            host_seq: row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
            kind: row.get(2)?,
            body: serde_json::from_str(&raw)?,
            ts: row.get(4)?,
        });
    }
    events.reverse();
    let mut times = HashMap::new();
    let mut fold = hennery_view::Fold::new();
    for event in &events {
        times.insert(event.event_id, event.ts.clone());
        fold.apply(event, &|_: &str| false);
    }
    let mut last_message = None;
    let mut plan = None;
    for item in fold.take() {
        let updated_at = times.get(&item.version).cloned().unwrap_or(item.ts);
        // A window that begins mid-turn lacks the original boundary and may
        // begin mid-message. Never present that prefix as a complete response.
        let partial = history_truncated && item.turn_id.is_none();
        match item.body {
            hennery_view::Body::Message(text) => {
                let truncated = text.truncated;
                let (text, cut) = clip_json(&text.text, MESSAGE_BYTES);
                last_message = Some(WorkMessage {
                    turn_id: item.turn_id,
                    text,
                    updated_at,
                    truncated: partial || cut || truncated,
                });
            }
            hennery_view::Body::Plan { mut entries, truncated } => {
                let mut cut = truncated || entries.len() > PLAN_ENTRIES;
                entries.truncate(PLAN_ENTRIES);
                for entry in &mut entries {
                    let (text, shortened) = clip_json(&entry.content, PLAN_LABEL_BYTES);
                    entry.content = text;
                    cut |= shortened;
                    for value in [&mut entry.priority, &mut entry.status].into_iter().flatten() {
                        let (text, shortened) = clip_json(value, 64);
                        *value = text;
                        cut |= shortened;
                    }
                }
                plan = Some(WorkPlan {
                    turn_id: item.turn_id,
                    entries,
                    updated_at,
                    truncated: cut,
                });
            }
            _ => {}
        }
    }
    Ok(WorkContent {
        last_message,
        plan,
        history_truncated,
    })
}

fn clip_json(text: &str, budget: usize) -> (String, bool) {
    let mut bytes = 0;
    for (at, ch) in text.char_indices() {
        bytes += hennery_proto::rest::json_char_width(ch);
        if bytes > budget {
            return (text[..at].into(), true);
        }
    }
    (text.into(), false)
}

impl Store {
    pub fn session_work(&self, session: &str) -> Result<Option<SessionWork>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let snapshot = snapshot_in(&tx, self.owner_id(), session, true, None)?;
        tx.commit()?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::SessionBody;
    use serde_json::{Value, json};

    fn store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_session("session", "host", "fake", "/tmp", "hat", None)
            .unwrap();
        store
            .ingest("session", 1, &SessionBody::session_started("start", "native"))
            .unwrap();
        store
    }
    fn event(store: &Store, kind: &str, body: Value, ts: &str, applied: bool) -> i64 {
        let conn = store.conn();
        conn.execute(
            "INSERT INTO events(owner_id,session_id,kind,body,ts,applied) VALUES(?1,'session',?2,?3,?4,?5)",
            params![store.owner_id(), kind, body.to_string(), ts, applied],
        )
        .unwrap();
        conn.last_insert_rowid()
    }
    fn update(store: &Store, update: Value, ts: &str) -> i64 {
        event(store, "acp_update", json!({"payload":{"update":update}}), ts, true)
    }
    fn message(store: &Store, text: &str, ts: &str) -> i64 {
        update(
            store,
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}),
            ts,
        )
    }
    fn turn(store: &Store, id: &str) {
        event(
            store,
            "user_turn",
            json!({"turn_id":id,"content":[{"type":"text","text":"work"}]}),
            "prompt",
            true,
        );
    }

    #[test]
    fn shared_observations_keep_message_boundaries_timestamps_and_completion_distinct() {
        let store = store();
        turn(&store, "first");
        message(&store, "Before tool", "message-1");
        update(
            &store,
            json!({"sessionUpdate":"tool_call","toolCallId":"tool","title":"Inspect","status":"completed"}),
            "tool",
        );
        message(&store, "Final ", "message-2");
        message(&store, "response", "message-3");
        update(
            &store,
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private reasoning"}}),
            "thought",
        );
        event(
            &store,
            "turn_ended",
            json!({"turn_id":"first","outcome":"completed"}),
            "end",
            true,
        );
        turn(&store, "next");
        let revision = event(&store, "session_seen", json!({}), "receipt", true);
        event(
            &store,
            "turn_ended",
            json!({"turn_id":"late","outcome":"failed"}),
            "unapplied",
            false,
        );
        let before = store.events("session", 0, 100).unwrap();
        let work = store.session_work("session").unwrap().unwrap();
        assert_eq!(work.revision, revision);
        assert_eq!(work.last_activity_at.as_deref(), Some("end"));
        assert_eq!(work.last_message_at.as_deref(), Some("message-3"));
        let last_turn = work.last_turn.unwrap();
        assert_eq!(last_turn.turn_id, "first");
        assert_eq!(last_turn.outcome, hennery_proto::frames::TurnOutcome::Completed);
        let message = work.content.unwrap().last_message.unwrap();
        assert_eq!(message.text, "Final response");
        assert_eq!(message.turn_id.as_deref(), Some("first"));
        assert_eq!(message.updated_at, "message-3");
        assert!(!message.truncated);
        assert_eq!(store.events("session", 0, 100).unwrap(), before);
    }

    #[test]
    fn plan_is_advisory_and_metadata_does_not_include_transcript() {
        let store = store();
        turn(&store, "turn");
        message(&store, "answer", "message");
        update(
            &store,
            json!({"sessionUpdate":"plan","entries":[{"content":"private plan","status":"in_progress","priority":"high"}]}),
            "plan",
        );
        let summary = store.family_summary("session").unwrap().unwrap();
        let work = summary.work.unwrap();
        assert_eq!(work.activity.as_deref(), Some("idle"));
        assert_eq!(work.background, BackgroundWork::NotReported);
        assert!(work.content.is_none());
        let value = serde_json::to_value(&work).unwrap();
        assert!(value.get("content").is_none());
        let content = store.session_work("session").unwrap().unwrap().content.unwrap();
        let plan = content.plan.unwrap();
        assert_eq!(plan.entries[0].content, "private plan");
        assert_eq!(plan.entries[0].status.as_deref(), Some("in_progress"));
        assert_eq!(plan.turn_id.as_deref(), Some("turn"));
        assert_eq!(plan.updated_at, "plan");
    }

    #[test]
    fn open_questions_and_answers_are_observations_without_acknowledgement() {
        let store = store();
        let conn = store.conn();
        for (id, kind) in [("permission", "permission"), ("question", "elicitation")] {
            conn.execute(
                "INSERT INTO pending(owner_id,pending_id,session_id,kind,payload,state,opened_at)
                VALUES(?1,?2,'session',?3,'{}','open','now')",
                params![store.owner_id(), id, kind],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO answer_queue(owner_id,pending_id,session_id,request_id,answer,submitted_at)
            VALUES(?1,'question','session','request','{}','now')",
            [store.owner_id()],
        )
        .unwrap();
        drop(conn);
        let work = store.session_work("session").unwrap().unwrap();
        assert_eq!(
            work.waiting,
            WorkWaiting {
                permissions: 1,
                questions: 1,
                answered: 1
            }
        );
        assert_eq!(store.session_work("session").unwrap().unwrap().waiting, work.waiting);
    }

    #[test]
    fn partial_message_window_and_oversized_body_are_reported_honestly() {
        let store = store();
        turn(&store, "turn");
        for _ in 0..CONTENT_EVENTS + 2 {
            message(&store, "x", "message");
        }
        let work = store.session_work("session").unwrap().unwrap();
        let content = work.content.unwrap();
        assert!(content.history_truncated);
        assert!(content.last_message.as_ref().unwrap().truncated);
        message(&store, &"z".repeat(CONTENT_BYTES + 1), "huge");
        let work = store.session_work("session").unwrap().unwrap();
        assert_eq!(work.last_message_at.as_deref(), Some("huge"));
        let content = work.content.unwrap();
        assert!(content.history_truncated);
        assert!(content.last_message.is_none());
    }

    #[test]
    fn excerpts_bound_json_escaping_and_plan_size_without_invalid_utf8() {
        let store = store();
        turn(&store, "turn");
        message(&store, &"😀\n\"\\".repeat(1024), "message");
        update(
            &store,
            json!({"sessionUpdate":"plan","entries":(0..12).map(|_|json!({
            "content":"😀\n".repeat(512),"status":"pending"
        })).collect::<Vec<_>>()}),
            "plan",
        );
        let content = store.session_work("session").unwrap().unwrap().content.unwrap();
        let message = content.last_message.unwrap();
        assert!(message.truncated);
        assert!(serde_json::to_string(&message.text).unwrap().len() <= MESSAGE_BYTES + 2);
        let plan = content.plan.unwrap();
        assert!(plan.truncated);
        assert_eq!(plan.entries.len(), PLAN_ENTRIES);
        assert!(
            plan.entries
                .iter()
                .all(|e| serde_json::to_string(&e.content).unwrap().len() <= PLAN_LABEL_BYTES + 2)
        );
    }

    #[test]
    fn unknown_foreign_deleted_and_purged_sessions_have_no_snapshot() {
        let store = store();
        assert!(store.session_work("missing").unwrap().is_none());
        assert!(
            snapshot_in(&store.conn(), "foreign", "session", true, None)
                .unwrap()
                .is_none()
        );
        store
            .conn()
            .execute(
                "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES(?1,'hat',0)",
                [store.owner_id()],
            )
            .unwrap();
        assert!(store.session_work("session").unwrap().is_none());
        store
            .conn()
            .execute("DELETE FROM purged_hats WHERE owner_id=?1", [store.owner_id()])
            .unwrap();
        store
            .conn()
            .execute(
                "UPDATE sessions SET lifecycle='deleted' WHERE owner_id=?1 AND id='session'",
                [store.owner_id()],
            )
            .unwrap();
        assert!(store.session_work("session").unwrap().is_none());
    }
}
