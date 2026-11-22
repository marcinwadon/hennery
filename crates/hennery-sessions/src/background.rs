//! Persisted background groups. Applied inside the host event transaction.
use anyhow::Result;
use hennery_proto::{
    frames::AcpProtocolPreference,
    rest::{BackgroundInfo, BackgroundItem, EventDto},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Serialize, Deserialize)]
struct State {
    idle: bool,
    reported: bool,
    members: BTreeMap<String, Entry>,
    retired: BTreeSet<String>,
    entities: BTreeMap<String, Entry>,
    group: u64,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct Entry {
    title: String,
    kind: String,
    status: String,
    outcome: Option<String>,
    active: bool,
    #[serde(default)]
    terminal_ids: BTreeSet<String>,
}

fn terminal_links(kind: &str, content: Option<&Value>, terminals: &mut BTreeSet<String>) {
    let terminal = |item: &Value| {
        (item["type"].as_str() == Some("terminal"))
            .then(|| item["terminalId"].as_str().map(str::to_owned))
            .flatten()
    };
    if kind == "tool_call_content_chunk" {
        if let Some(id) = content.and_then(terminal) {
            terminals.insert(id);
        }
    } else if let Some(content) = content {
        if content.is_null() {
            terminals.clear();
        } else if let Some(items) = content.as_array() {
            *terminals = items.iter().filter_map(terminal).collect();
        }
    }
}

fn merge_outcome(previous: &mut Option<String>, next: &str) {
    // Once explicit failure/cancellation evidence is seen, a later generic
    // completion or missing Claude exit code must not turn it into success.
    if previous.as_deref() == Some("cancelled") {
        return;
    }
    if previous.as_deref() == Some("failed") && next != "cancelled" {
        return;
    }
    *previous = Some(next.into());
}

pub(crate) fn root_payload(tx: &Transaction<'_>, owner: &str, id: &str, payload: &Value) -> Result<bool> {
    let root: Option<String> = tx.query_row(
        "SELECT agent_session_id FROM sessions WHERE owner_id=?1 AND id=?2",
        params![owner, id],
        |r| r.get(0),
    )?;
    Ok(!payload
        .get("sessionId")
        .and_then(Value::as_str)
        .zip(root.as_deref())
        .is_some_and(|(actual, root)| actual != root))
}

pub(crate) fn requested_protocol(conn: &Connection, owner: &str, id: &str) -> Result<AcpProtocolPreference> {
    let config: Option<String> = conn
        .query_row(
            "SELECT requested_config FROM session_attach_evidence WHERE owner_id=?1 AND session_id=?2",
            params![owner, id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(config
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| serde_json::from_value(v.get("protocol")?.clone()).ok())
        .unwrap_or_default())
}

pub(crate) fn apply(tx: &Transaction<'_>, owner: &str, id: &str, payload: &Value, ts: &str) -> Result<Vec<EventDto>> {
    if !root_payload(tx, owner, id, payload)? {
        return Ok(vec![]);
    }
    let u = payload.get("update").unwrap_or(payload);
    let kind = u["sessionUpdate"].as_str().unwrap_or("");
    if !matches!(
        kind,
        "state_update" | "terminal_update" | "tool_call_update" | "tool_call_content_chunk"
    ) {
        return Ok(vec![]);
    }
    let (raw, agent, version): (Option<String>, String, Option<u16>) = tx.query_row(
        "SELECT background_state,agent,acp_protocol FROM sessions WHERE owner_id=?1 AND id=?2",
        params![owner, id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if raw.is_none() && kind != "state_update" && version != Some(2) {
        return Ok(vec![]);
    }
    let mut state: State = raw.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
    if kind == "state_update" {
        let activity = match u["state"].as_str() {
            Some("running") => "running",
            Some("requires_action") => "blocked",
            Some("idle") => "idle",
            _ => "running",
        };
        state.idle = activity == "idle";
        tx.execute(
            "UPDATE sessions SET activity=?3 WHERE owner_id=?1 AND id=?2 AND lifecycle='active'",
            params![owner, id, activity],
        )?;
        state.reported = false;
        if let Some(list) = u.get("background").and_then(Value::as_array) {
            let parsed: Option<Vec<(String, String)>> = list
                .iter()
                .map(|v| {
                    let kind = v["type"].as_str()?;
                    let key = match kind {
                        "terminal" => format!("terminal:{}", v["terminalId"].as_str()?),
                        "tool_call" => format!("tool_call:{}", v["toolCallId"].as_str()?),
                        _ => format!("other:{}", serde_json::to_string(v).ok()?),
                    };
                    Some((key, kind.into()))
                })
                .collect();
            if let Some(mut parsed) = parsed {
                state.reported = true;
                let keys: BTreeSet<_> = parsed.iter().map(|(key, _)| key.clone()).collect();
                let duplicate_tools: BTreeSet<_> = state
                    .entities
                    .iter()
                    .chain(state.members.iter())
                    .filter(|&(key, entry)| {
                        key.starts_with("tool_call:")
                            && entry
                                .terminal_ids
                                .iter()
                                .any(|terminal| keys.contains(&format!("terminal:{terminal}")))
                    })
                    .map(|(key, _)| key.clone())
                    .collect();
                parsed.retain(|(key, _)| !duplicate_tools.contains(key));
                // A launching tool and its still-running terminal are one
                // process, even if an earlier snapshot only listed the tool.
                state.members.retain(|key, _| !duplicate_tools.contains(key));
                for entry in state.members.values_mut() {
                    entry.active = false;
                }
                for (key, kind) in parsed {
                    if state.retired.contains(&key) {
                        continue;
                    }
                    let mut entry = state
                        .members
                        .remove(&key)
                        .or_else(|| state.entities.get(&key).cloned())
                        .unwrap_or_default();
                    entry.kind = kind;
                    entry.active = true;
                    if entry.title.is_empty() {
                        entry.title = key.clone();
                    }
                    if entry.status.is_empty() {
                        entry.status = "running".into();
                    }
                    state.members.insert(key, entry);
                }
                for entry in state.members.values_mut() {
                    if !entry.active && !matches!(entry.kind.as_str(), "terminal" | "tool_call") {
                        merge_outcome(&mut entry.outcome, "unknown");
                    }
                }
            }
        }
    } else {
        let (prefix, field) = if kind == "terminal_update" {
            ("terminal", "terminalId")
        } else {
            ("tool_call", "toolCallId")
        };
        if let Some(entity) = u[field].as_str() {
            let key = format!("{prefix}:{entity}");
            let previous = state.members.get(&key).cloned().unwrap_or_default();
            let entry = state.entities.entry(key.clone()).or_insert(previous);
            entry.kind = prefix.into();
            terminal_links(kind, u.get("content"), &mut entry.terminal_ids);
            if let Some(title) = u.get("title").or_else(|| u.get("command")).and_then(Value::as_str) {
                entry.title = title.chars().take(512).collect();
            }
            if let Some(status) = u["status"].as_str() {
                entry.status = status.into();
                match status {
                    "completed" => merge_outcome(&mut entry.outcome, "succeeded"),
                    "failed" => merge_outcome(&mut entry.outcome, "failed"),
                    "cancelled" => merge_outcome(&mut entry.outcome, "cancelled"),
                    _ => {}
                }
            }
            if let Some(exit) = u.get("exitStatus").filter(|v| v.is_object()) {
                let outcome = if exit.get("signal").is_some_and(|v| !v.is_null()) {
                    "failed"
                } else if let Some(code) = exit["exitCode"].as_i64() {
                    if code == 0 { "succeeded" } else { "failed" }
                } else if agent.contains("claude") {
                    "succeeded"
                } else {
                    "unknown"
                };
                merge_outcome(&mut entry.outcome, outcome);
                entry.status = "completed".into();
            }
            if let Some(member) = state.members.get_mut(&key) {
                let active = member.active;
                *member = entry.clone();
                member.active = active;
            }
        }
    }
    let mut events = vec![];
    if state.idle
        && state.reported
        && !state.members.is_empty()
        && state.members.values().all(|e| !e.active && e.outcome.is_some())
    {
        let mut totals = BTreeMap::<String, u32>::new();
        for (key, e) in std::mem::take(&mut state.members) {
            *totals.entry(e.outcome.unwrap()).or_default() += 1;
            state.retired.insert(key.clone());
            state.entities.remove(&key);
        }
        state.group += 1;
        events.push(crate::store::collector_event(tx,owner,id,"background_tasks_finished",json!({"group":state.group,"succeeded":totals.get("succeeded").copied().unwrap_or(0),"failed":totals.get("failed").copied().unwrap_or(0),"cancelled":totals.get("cancelled").copied().unwrap_or(0),"unknown":totals.get("unknown").copied().unwrap_or(0)}),ts)?);
    }
    // Keep a bounded pre-membership cache: agents announce terminals/tools
    // before listing them, including while the foreground is idle.
    // Host event sequence deduplication remains the primary replay guard.
    while state.entities.len() > 4096 {
        let Some(key) = state
            .entities
            .keys()
            .find(|key| !state.members.contains_key(*key))
            .cloned()
        else {
            break;
        };
        state.entities.remove(&key);
    }
    while state.retired.len() > 4096 {
        state.retired.pop_first();
    }
    let info = if state.members.is_empty() {
        None
    } else {
        let done = state
            .members
            .values()
            .filter(|e| !e.active && e.outcome.is_some())
            .count() as u32;
        Some(BackgroundInfo {
            active: state.members.len() as u32 - done,
            done,
            all: state.members.len() as u32,
            stale: !state.reported,
            items: state
                .members
                .iter()
                .map(|(id, e)| BackgroundItem {
                    id: id.clone(),
                    title: e.title.clone(),
                    kind: e.kind.clone(),
                    status: if e.active {
                        e.status.clone()
                    } else {
                        e.outcome.clone().unwrap_or_else(|| "unknown".into())
                    },
                })
                .collect(),
        })
    };
    tx.execute(
        "UPDATE sessions SET background_state=?3,background_info=?4 WHERE owner_id=?1 AND id=?2",
        params![
            owner,
            id,
            serde_json::to_string(&state)?,
            info.map(|v| serde_json::to_string(&v)).transpose()?
        ],
    )?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use hennery_proto::frames::{Indexed, SessionBody};

    fn update(store: &Store, seq: u64, update: Value) -> Vec<EventDto> {
        store
            .ingest(
                "s",
                seq,
                &SessionBody::AcpUpdate {
                    indexed: Indexed::default(),
                    payload: json!({"sessionId":"a","update":update}),
                },
            )
            .unwrap()
    }
    #[test]
    fn persisted_group_counts_and_summary_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "claude", "/tmp", "hat", None).unwrap();
        store.ingest("s", 1, &SessionBody::session_started("r", "a")).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"state_update","state":"idle","background":[{"type":"terminal","terminalId":"t"}]}),
        );
        update(
            &store,
            3,
            json!({"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{}}),
        );
        let ended = update(
            &store,
            4,
            json!({"sessionUpdate":"state_update","state":"idle","background":[]}),
        );
        let summary = ended.iter().find(|e| e.kind == "background_tasks_finished").unwrap();
        assert_eq!(summary.body["succeeded"], 1);
        assert!(
            update(
                &store,
                4,
                json!({"sessionUpdate":"state_update","state":"idle","background":[]})
            )
            .is_empty()
        );
        let repeated = update(
            &store,
            5,
            json!({"sessionUpdate":"state_update","state":"idle","background":[]}),
        );
        assert!(!repeated.iter().any(|e| e.kind == "background_tasks_finished"));
    }
    #[test]
    fn missing_snapshot_does_not_close_group_and_autonomous_state_is_visible() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store.ingest("s", 1, &SessionBody::session_started("r", "a")).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"state_update","state":"running","background":[{"type":"terminal","terminalId":"t"}]}),
        );
        assert_eq!(
            store.find_session("s").unwrap().unwrap().activity.as_deref(),
            Some("running")
        );
        assert!(!store.open_turn("s", "autonomous-race", &[]).unwrap());
        let missing = update(&store, 3, json!({"sessionUpdate":"state_update","state":"idle"}));
        assert!(!missing.iter().any(|e| e.kind == "background_tasks_finished"));
        let conn = store.conn();
        let raw: String = conn
            .query_row("SELECT background_info FROM sessions WHERE id='s'", [], |r| r.get(0))
            .unwrap();
        let info: BackgroundInfo = serde_json::from_str(&raw).unwrap();
        assert!(info.stale);
        assert_eq!((info.active, info.done, info.all), (1, 0, 1));
    }

    #[test]
    fn native_child_state_does_not_replace_root_activity() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store.ingest("s", 1, &SessionBody::session_started("r", "a")).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"state_update","state":"running","background":[]}),
        );
        store.ingest("s",3,&SessionBody::AcpUpdate { indexed:Indexed::default(), payload:json!({"sessionId":"native-child","update":{"sessionUpdate":"state_update","state":"idle","background":[]}})}).unwrap();
        assert_eq!(
            store.find_session("s").unwrap().unwrap().activity.as_deref(),
            Some("running")
        );
    }

    #[test]
    fn explicit_failure_wins_over_claude_missing_exit_fallback() {
        let mut outcome = Some("failed".into());
        merge_outcome(&mut outcome, "succeeded");
        assert_eq!(outcome.as_deref(), Some("failed"));
        let mut outcome = Some("cancelled".into());
        merge_outcome(&mut outcome, "unknown");
        assert_eq!(outcome.as_deref(), Some("cancelled"));
    }

    #[test]
    fn tool_and_its_terminal_count_as_one_background_process() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store.ingest("s", 1, &SessionBody::session_started("r", "a")).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"state_update","state":"running","background":[]}),
        );
        update(
            &store,
            3,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"call","status":"completed","content":[{"type":"terminal","terminalId":"term"}]}),
        );
        update(
            &store,
            4,
            json!({"sessionUpdate":"state_update","state":"idle","background":[{"type":"tool_call","toolCallId":"call"},{"type":"terminal","terminalId":"term"}]}),
        );
        let info = store.find_session_item("s").unwrap().unwrap().background.unwrap();
        assert_eq!((info.active, info.done, info.all), (1, 0, 1));
        update(
            &store,
            5,
            json!({"sessionUpdate":"terminal_update","terminalId":"term","exitStatus":{"exitCode":0}}),
        );
        let events = update(
            &store,
            6,
            json!({"sessionUpdate":"state_update","state":"idle","background":[]}),
        );
        let summary = events.iter().find(|e| e.kind == "background_tasks_finished").unwrap();
        assert_eq!(summary.body["succeeded"], 1);
    }

    #[test]
    fn streamed_terminal_reference_deduplicates_the_invoking_tool() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store.ingest("s", 1, &SessionBody::session_started("r", "a")).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"state_update","state":"running","background":[]}),
        );
        update(
            &store,
            3,
            json!({"sessionUpdate":"tool_call_content_chunk","toolCallId":"call","content":{"type":"terminal","terminalId":"term"}}),
        );
        update(
            &store,
            4,
            json!({"sessionUpdate":"state_update","state":"idle","background":[{"type":"tool_call","toolCallId":"call"},{"type":"terminal","terminalId":"term"}]}),
        );
        let info = store.find_session_item("s").unwrap().unwrap().background.unwrap();
        assert_eq!((info.active, info.all), (1, 1));
    }

    #[test]
    fn terminal_relation_chunks_append_and_array_or_null_replaces() {
        let mut links = BTreeSet::new();
        terminal_links(
            "tool_call_content_chunk",
            Some(&json!({"type":"terminal","terminalId":"a"})),
            &mut links,
        );
        terminal_links(
            "tool_call_content_chunk",
            Some(&json!({"type":"terminal","terminalId":"b"})),
            &mut links,
        );
        terminal_links("tool_call_update", None, &mut links);
        assert_eq!(links.len(), 2);
        terminal_links(
            "tool_call_update",
            Some(&json!([{"type":"terminal","terminalId":"c"}])),
            &mut links,
        );
        assert_eq!(links, BTreeSet::from(["c".into()]));
        terminal_links("tool_call_update", Some(&Value::Null), &mut links);
        assert!(links.is_empty());
    }

    #[test]
    fn completion_before_first_snapshot_is_preserved_and_signal_is_failure() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store
            .ingest(
                "s",
                1,
                &SessionBody::SessionStarted {
                    request_id: "r".into(),
                    agent_session_id: "a".into(),
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
            json!({"sessionUpdate":"terminal_update","terminalId":"t","exitStatus":{"signal":"SIGSEGV"}}),
        );
        update(
            &store,
            3,
            json!({"sessionUpdate":"state_update","state":"idle","background":[{"type":"terminal","terminalId":"t"}]}),
        );
        let events = update(
            &store,
            4,
            json!({"sessionUpdate":"state_update","state":"idle","background":[]}),
        );
        assert_eq!(
            events
                .iter()
                .find(|e| e.kind == "background_tasks_finished")
                .unwrap()
                .body["failed"],
            1
        );
    }

    #[test]
    fn unknown_future_work_can_finish_with_unknown_outcome() {
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store.ingest("s", 1, &SessionBody::session_started("r", "a")).unwrap();
        update(
            &store,
            2,
            json!({"sessionUpdate":"state_update","state":"idle","background":[{"type":"future_work","id":"x"}]}),
        );
        let events = update(
            &store,
            3,
            json!({"sessionUpdate":"state_update","state":"idle","background":[]}),
        );
        assert_eq!(
            events
                .iter()
                .find(|e| e.kind == "background_tasks_finished")
                .unwrap()
                .body["unknown"],
            1
        );
    }

    #[test]
    fn v2_background_permission_does_not_override_foreground_state() {
        use hennery_proto::frames::{PendingExtract, PendingKind, PendingResolution};
        let store = Store::open_in_memory().unwrap();
        store.create_session("s", "h", "codex", "/tmp", "hat", None).unwrap();
        store
            .ingest(
                "s",
                1,
                &SessionBody::SessionStarted {
                    request_id: "r".into(),
                    agent_session_id: "a".into(),
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
            json!({"sessionUpdate":"state_update","state":"running","background":[]}),
        );
        store
            .ingest(
                "s",
                3,
                &SessionBody::PendingOpened {
                    pending_id: "p".into(),
                    indexed: Indexed {
                        pending: Some(Box::new(PendingExtract {
                            id: "p".into(),
                            kind: PendingKind::Permission,
                            option_ids: Some(vec!["allow".into()]),
                            title: None,
                        })),
                        ..Default::default()
                    },
                    payload: json!({}),
                },
            )
            .unwrap();
        assert_eq!(
            store.find_session("s").unwrap().unwrap().activity.as_deref(),
            Some("running")
        );
        update(
            &store,
            4,
            json!({"sessionUpdate":"state_update","state":"requires_action","background":[]}),
        );
        store
            .ingest(
                "s",
                5,
                &SessionBody::PendingResolved {
                    pending_id: "p".into(),
                    resolution: PendingResolution::Cancelled,
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(
            store.find_session("s").unwrap().unwrap().activity.as_deref(),
            Some("blocked")
        );
    }
}
