//! Generated sequences (client view spec §7): an update kind the fold does
//! not know becomes an `unrecognised` item, one for each, and never leaves
//! fewer items than the sequence without it.

use hennery_proto::rest::EventDto;
use hennery_view::{Body, fold};
use serde_json::{Value, json};

/// A small deterministic generator (xorshift64), so a failure replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn acp(update: Value) -> (String, Value) {
    (
        "acp_update".into(),
        json!({"kind": "acp_update", "indexed": {}, "payload": {"sessionId": "a", "update": update}}),
    )
}

/// One known event, picked by `rng`.
fn known(rng: &mut Rng) -> (String, Value) {
    let call = format!("c{}", rng.below(4));
    match rng.below(9) {
        0 => (
            "user_turn".into(),
            json!({"turn_id": format!("t{}", rng.next()), "content": [{"type": "text", "text": "go"}]}),
        ),
        1 | 2 => acp(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "m"}})),
        3 => acp(json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "t"}})),
        4 => acp(json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": "Run"})),
        5 => acp(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": call, "status": "completed", "rawOutput": "ok"}),
        ),
        6 => acp(json!({"sessionUpdate": "usage_update", "used": 1, "size": 2})),
        7 => acp(json!({"sessionUpdate": "plan", "entries": [{"content": "step"}]})),
        _ => (
            "session_parked".into(),
            json!({"kind": "session_parked", "reason": "idle"}),
        ),
    }
}

/// One update or event kind no fold knows.
fn unknown(rng: &mut Rng) -> (String, Value) {
    let name = format!("future_{}", rng.below(1000));
    if rng.below(2) == 0 {
        acp(json!({"sessionUpdate": name, "anything": [1, 2, 3]}))
    } else {
        (name, json!({"anything": true}))
    }
}

fn events(kinds: &[(String, Value)]) -> Vec<EventDto> {
    kinds
        .iter()
        .enumerate()
        .map(|(i, (kind, body))| EventDto {
            event_id: i as i64 + 1,
            session_id: "s".into(),
            host_seq: None,
            kind: kind.clone(),
            body: body.clone(),
            ts: "2026-10-02T08:00:00.000Z".into(),
        })
        .collect()
}

fn none(_: &str) -> bool {
    false
}

#[test]
fn unknown_kinds_become_items_and_never_fewer() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for round in 0..500 {
        let len = 1 + rng.below(40) as usize;
        let base: Vec<(String, Value)> = (0..len).map(|_| known(&mut rng)).collect();
        let mut mixed = base.clone();
        let inserted = 1 + rng.below(5) as usize;
        let mut unknown_kinds = Vec::new();
        for _ in 0..inserted {
            let at = rng.below(mixed.len() as u64 + 1) as usize;
            let (kind, body) = unknown(&mut rng);
            let label = if kind == "acp_update" {
                format!(
                    "acp_update/{}",
                    body["payload"]["update"]["sessionUpdate"].as_str().unwrap()
                )
            } else {
                kind.clone()
            };
            unknown_kinds.push(label);
            mixed.insert(at, (kind, body));
        }
        let without = fold(&events(&base), &none);
        let with = fold(&events(&mixed), &none);
        let mut seen: Vec<String> = with
            .iter()
            .filter_map(|i| match &i.body {
                Body::Unrecognised(u) => Some(u.update_kind.clone()),
                _ => None,
            })
            .collect();
        seen.sort();
        unknown_kinds.sort();
        assert_eq!(
            seen, unknown_kinds,
            "round {round}: one unrecognised item per unknown update"
        );
        assert!(
            with.len() >= without.len() + inserted,
            "round {round}: {} items with {inserted} unknown, {} without",
            with.len(),
            without.len()
        );
    }
}
