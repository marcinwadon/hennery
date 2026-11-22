//! Immutable per-group view seeds; raw ACP event bodies stay unchanged.
use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_proto::rest::EventDto;
use hennery_view::fold::{Fold, PREAMBLE, group_key};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn identity(event: &EventDto) -> Option<(String, String)> {
    if event.kind != "acp_update" {
        return None;
    }
    let payload = event.body.get("payload")?;
    let update = payload.get("update").unwrap_or(payload);
    let (kind, field) = match update["sessionUpdate"].as_str()? {
        "terminal_update" | "terminal_output_chunk" => ("terminal", "terminalId"),
        "tool_call_update" | "tool_call_content_chunk" => ("tool", "toolCallId"),
        "agent_message" | "agent_message_chunk" => ("message", "messageId"),
        "agent_thought" | "agent_thought_chunk" => ("thought", "messageId"),
        _ => return None,
    };
    let wire_id = update[field].as_str().filter(|v| !v.is_empty() && v.len() <= 256)?;
    let lineage = payload["sessionId"].as_str()?;
    let prefix: String = Sha256::digest(lineage.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some((format!("{prefix}:{kind}:{wire_id}"), prefix))
}

pub(crate) fn enrich(conn: &Connection, owner: &str, event: &mut EventDto) -> Result<()> {
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT prefix,seed,terminal_bytes FROM acp_view_refs WHERE owner_id=?1 AND session_id=?2 AND event_id=?3",
            params![owner, event.session_id, event.event_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((prefix, seed, bytes)) = row {
        decorate(event, &prefix, seed.as_deref(), bytes.as_deref())?;
    }
    Ok(())
}

fn decorate(event: &mut EventDto, prefix: &str, seed: Option<&str>, bytes: Option<&str>) -> Result<()> {
    event.body["_view_entity_prefix"] = json!(prefix);
    if !event.body.get("indexed").is_some_and(Value::is_object) {
        event.body["indexed"] = json!({});
    }
    event.body["indexed"]["acp_protocol"] = json!(2);
    if let Some(seed) = seed {
        event.body["_view_seed"] = serde_json::from_str(seed)?;
    }
    if let Some(bytes) = bytes {
        event.body["_view_terminal_bytes"] = json!(bytes);
    }
    Ok(())
}

pub(crate) fn project(tx: &Transaction<'_>, owner: &str, event: &mut EventDto) -> Result<()> {
    let Some((key, prefix)) = identity(event) else {
        return Ok(());
    };
    let version: Option<u16> = tx.query_row(
        "SELECT acp_protocol FROM sessions WHERE owner_id=?1 AND id=?2",
        params![owner, event.session_id],
        |r| r.get(0),
    )?;
    if version != Some(2) {
        return Ok(());
    }
    let group: Option<(i64,String)> = tx.query_row(
        "SELECT event_id,body FROM events WHERE owner_id=?1 AND session_id=?2 AND kind='user_turn' AND applied=1 AND event_id<?3 ORDER BY event_id DESC LIMIT 1",
        params![owner,event.session_id,event.event_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let group_id = match &group {
        Some((id, body)) => {
            let body: Value = serde_json::from_str(body)?;
            group_key(*id, body["turn_id"].as_str())
        }
        None => PREAMBLE.into(),
    };
    let previous: Option<(String, String)> = tx
        .query_row(
            "SELECT item,terminal_bytes FROM acp_view_entities WHERE owner_id=?1 AND session_id=?2 AND entity_key=?3",
            params![owner, event.session_id, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let first: bool = !tx.query_row("SELECT EXISTS(SELECT 1 FROM acp_view_refs WHERE owner_id=?1 AND session_id=?2 AND entity_key=?3 AND group_id=?4)",params![owner,event.session_id,key,group_id],|r|r.get::<_,bool>(0))?;
    tx.execute("INSERT INTO acp_view_refs(event_id,owner_id,session_id,entity_key,group_id,prefix,seed,terminal_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![event.event_id,owner,event.session_id,key,group_id,prefix,if first {previous.as_ref().map(|p|&p.0)}else{None},if first {previous.as_ref().map(|p|&p.1)}else{None}])?;
    // Fold one bounded entity from its pre-update state, in the current group.
    let mut fold = Fold::new();
    if let Some((event_id, body)) = group {
        fold.apply(
            &EventDto {
                event_id,
                session_id: event.session_id.clone(),
                host_seq: None,
                kind: "user_turn".into(),
                body: serde_json::from_str(&body)?,
                ts: event.ts.clone(),
            },
            &|_: &str| false,
        );
        fold.take();
    }
    let mut hydrated = event.clone();
    decorate(
        &mut hydrated,
        &prefix,
        previous.as_ref().map(|p| p.0.as_str()),
        previous.as_ref().map(|p| p.1.as_str()),
    )?;
    fold.apply(&hydrated, &|_: &str| false);
    if let Some((item, bytes)) = fold.entity_state(&hydrated) {
        tx.execute("INSERT INTO acp_view_entities(owner_id,session_id,entity_key,item,terminal_bytes,last_event) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(owner_id,session_id,entity_key) DO UPDATE SET item=excluded.item,terminal_bytes=excluded.terminal_bytes,last_event=excluded.last_event",params![owner,event.session_id,key,serde_json::to_string(&item)?,STANDARD.encode(bytes),event.event_id])?;
    }
    // Durable per-entity snapshots are individually bounded by Fold. Loading
    // one row keeps working memory bounded without evicting an ongoing entity
    // merely because many unrelated messages arrived. Session deletion scrubs
    // both this projection and the immutable historical seeds.
    enrich(tx, owner, event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use hennery_proto::frames::{Indexed, SessionBody};

    fn update(store: &Store, seq: u64, value: Value) -> Vec<EventDto> {
        store
            .ingest(
                "s",
                seq,
                &SessionBody::AcpUpdate {
                    indexed: Indexed::default(),
                    payload: json!({"sessionId":"lineage","update":value}),
                },
            )
            .unwrap()
    }

    #[test]
    fn late_terminal_patch_has_preupdate_seed_and_raw_replay_stays_identical() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        let started = SessionBody::SessionStarted {
            request_id: "r".into(),
            agent_session_id: "lineage".into(),
            indexed: Indexed {
                acp_protocol: Some(2),
                ..Default::default()
            },
            agent_home: None,
        };
        store.ingest("s", 1, &started).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"terminal_update","terminalId":"t","command":"sleep 8"}),
        );
        // A different user-turn group, with its own foreground lifecycle.
        assert!(store.open_turn("s", "turn-2", &[]).unwrap());
        store
            .ingest(
                "s",
                3,
                &SessionBody::TurnStarted {
                    request_id: "p".into(),
                    turn_id: "turn-2".into(),
                },
            )
            .unwrap();
        let patch = SessionBody::AcpUpdate {
            indexed: Indexed::default(),
            payload: json!({"sessionId":"lineage","update":{"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{"exitCode":0}}}),
        };
        let result = store.ingest("s", 4, &patch).unwrap();
        let event = result.iter().find(|e| e.kind == "acp_update").unwrap();
        assert_eq!(event.body["_view_seed"]["command"], "sleep 8");
        assert!(store.ingest("s", 4, &patch).unwrap().is_empty());
        let raw: String = store
            .conn()
            .query_row("SELECT body FROM events WHERE host_seq=4", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.contains("_view_seed"));
        let mut fold = Fold::new();
        fold.apply(event, &|_: &str| false);
        let (terminal, _) = fold.entity_state(event).unwrap();
        let terminal = serde_json::to_value(terminal).unwrap();
        assert_eq!(terminal["command"], "sleep 8");
        assert_eq!(terminal["exit_code"], 0);
    }

    #[test]
    fn many_intervening_entities_do_not_evict_ongoing_terminal_state() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store
            .ingest(
                "s",
                1,
                &SessionBody::SessionStarted {
                    request_id: "r".into(),
                    agent_session_id: "lineage".into(),
                    indexed: Indexed {
                        acp_protocol: Some(2),
                        ..Default::default()
                    },
                    agent_home: None,
                },
            )
            .unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"terminal_update","terminalId":"long","command":"sleep 300","output":{"data":"aGVsbG8="}}),
        );
        for index in 0..300 {
            update(
                &store,
                index + 3,
                json!({"sessionUpdate":"agent_message_chunk","messageId":format!("other-{index}"),"content":{"type":"text","text":"unrelated"}}),
            );
        }
        assert!(store.open_turn("s", "later-turn", &[]).unwrap());
        store
            .ingest(
                "s",
                303,
                &SessionBody::TurnStarted {
                    request_id: "p".into(),
                    turn_id: "later-turn".into(),
                },
            )
            .unwrap();
        let events = update(
            &store,
            304,
            json!({"sessionUpdate":"terminal_update","terminalId":"long","exitStatus":{"exitCode":0}}),
        );
        let event = events.iter().find(|event| event.kind == "acp_update").unwrap();
        let mut fold = Fold::new();
        fold.apply(event, &|_: &str| false);
        let (item, _) = fold.entity_state(event).unwrap();
        let item = serde_json::to_value(item).unwrap();
        assert_eq!(item["command"], "sleep 300");
        assert_eq!(item["output"], "hello");
        assert_eq!(item["exit_code"], 0);
    }
}
