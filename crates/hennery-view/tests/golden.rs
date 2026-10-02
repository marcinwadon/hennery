//! Golden tests over real adapter output (client view spec §7; frontend
//! §6.1): a live session of each pinned adapter, recorded on 2026-10-02 and
//! scrubbed (`fixtures/scrub.py`), folded and compared with the items it
//! made then. A pin bump that changes a shape changes these items: re-record
//! the session with the new adapter, look at the difference, and bless it
//! with `HENNERY_BLESS=1 cargo test -p hennery-view --test golden`.

use hennery_proto::rest::EventDto;
use hennery_view::{Body, Fold, Item, Items, MarkerKind, fold};
use std::path::PathBuf;

/// The recorded sessions: the pinned adapter's package and version
/// (`adapters/manifest.json`).
const SESSIONS: [&str; 2] = ["claude-agent-acp-0.81.0", "codex-acp-1.13.0"];

/// The prompts each recorded session sent.
fn prompts(session: &str) -> usize {
    match session {
        "claude-agent-acp-0.81.0" => 9,
        "codex-acp-1.13.0" => 10,
        _ => panic!("{session}: not a recording"),
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn events(session: &str) -> Vec<EventDto> {
    let text = std::fs::read_to_string(fixture(&format!("{session}.jsonl"))).expect("a fixture");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("an event"))
        .collect()
}

/// Every question of a recorded session has been answered or withdrawn.
fn none_answerable(_: &str) -> bool {
    false
}

#[test]
fn each_recorded_session_folds_to_its_golden_items() {
    for session in SESSIONS {
        let items = fold(&events(session), &none_answerable);
        let mut text = serde_json::to_string_pretty(&items).expect("items serialize");
        text.push('\n');
        let golden = fixture(&format!("{session}.items.json"));
        if std::env::var_os("HENNERY_BLESS").is_some() {
            std::fs::write(&golden, &text).expect("write the golden file");
            continue;
        }
        let expected = std::fs::read_to_string(&golden).expect("a golden file");
        assert!(
            text == expected,
            "{session}: the fold changed; bless it once the difference is understood"
        );
    }
}

/// Which group an item is in: its turn, or the preamble.
fn group_of(item: &Item) -> Option<&str> {
    item.turn_id.as_deref()
}

/// Locality (plan 4a-i decision 2): folding from any group's start gives
/// the same items, ids and versions included, for the groups it covers as
/// folding the whole session.
#[test]
fn a_fold_from_any_turn_gives_the_same_items_for_its_turns() {
    for session in SESSIONS {
        let events = events(session);
        let whole = fold(&events, &none_answerable);
        let starts: Vec<usize> = (0..events.len()).filter(|&i| events[i].kind == "user_turn").collect();
        assert_eq!(starts.len(), prompts(session), "{session}: every turn of the recording");
        for start in starts {
            let window = fold(&events[start..], &none_answerable);
            let turns: Vec<Option<&str>> = window.iter().map(group_of).collect();
            let expected: Vec<&Item> = whole.iter().filter(|item| turns.contains(&group_of(item))).collect();
            assert_eq!(
                window.iter().collect::<Vec<_>>(),
                expected,
                "{session} from event {start}"
            );
        }
    }
}

/// The item stream's view: an event at a time, taking what changed after
/// each, gives the whole fold's items.
#[test]
fn taking_after_every_event_gives_the_same_items() {
    for session in SESSIONS {
        let events = events(session);
        let mut fold_state = Fold::new();
        let mut items = Items::default();
        let mut last_version = std::collections::HashMap::new();
        for event in &events {
            fold_state.apply(event, &none_answerable);
            for item in fold_state.take() {
                // Every upsert is newer than the one before it.
                let before = last_version.insert(item.id.clone(), item.version);
                assert!(before.is_none_or(|v| v < item.version), "{}", item.id);
                assert_eq!(item.version, event.event_id);
                items.upsert(item);
            }
        }
        assert_eq!(items.into_vec(), fold(&events, &none_answerable), "{session}");
    }
}

#[test]
fn the_recordings_make_what_their_sessions_did() {
    for session in SESSIONS {
        let items = fold(&events(session), &none_answerable);
        let count = |f: &dyn Fn(&Body) -> bool| items.iter().filter(|i| f(&i.body)).count();
        // Each prompt; nothing the fold did not know.
        assert_eq!(
            count(&|b| matches!(b, Body::UserTurn { .. })),
            prompts(session),
            "{session}"
        );
        assert_eq!(count(&|b| matches!(b, Body::Unrecognised(_))), 0, "{session}");
        // Three questions, each answered and over, none answerable.
        let questions: Vec<_> = items
            .iter()
            .filter_map(|i| match &i.body {
                Body::Question(q) => Some(q),
                _ => None,
            })
            .collect();
        assert_eq!(questions.len(), 3, "{session}");
        assert!(questions.iter().all(|q| q.answered && !q.answerable), "{session}");
        // The park, the resume, the host's restart and the close.
        for marker in [
            MarkerKind::Parked,
            MarkerKind::Resumed,
            MarkerKind::HostRestarted,
            MarkerKind::Closed,
        ] {
            assert!(
                items
                    .iter()
                    .any(|i| matches!(&i.body, Body::Marker(m) if m.marker == marker)),
                "{session}: {marker:?}"
            );
        }
        // No tool output fabricated.
        assert!(
            !items
                .iter()
                .any(|i| matches!(&i.body, Body::ToolCall(c) if c.fabricated.is_some())),
            "{session}"
        );
    }
}

/// A long reply streamed as one chunk per event folds into one message,
/// though usage updates arrive between its chunks: the count the operator
/// cancelled at 331, and the one a host restart cut at 180.
#[test]
fn a_streamed_reply_is_one_message() {
    let events = events("claude-agent-acp-0.81.0");
    let chunks = events
        .iter()
        .filter(|e| {
            e.body.pointer("/payload/update/sessionUpdate").and_then(|u| u.as_str()) == Some("agent_message_chunk")
        })
        .count();
    let items = fold(&events, &none_answerable);
    let messages: Vec<&str> = items
        .iter()
        .filter_map(|i| match &i.body {
            Body::Message(m) => Some(m.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(chunks > 150);
    assert_eq!(messages.len(), 9);
    let counting: Vec<&str> = messages.into_iter().filter(|m| m.starts_with("1\n2\n3\n")).collect();
    assert_eq!(counting.len(), 2, "two counting replies");
    let numbers = |m: &str| {
        m.lines()
            .map(|l| l.parse::<u32>().expect("a number"))
            .collect::<Vec<_>>()
    };
    assert_eq!(numbers(counting[0]), (1..=331).collect::<Vec<_>>());
    assert_eq!(numbers(counting[1]), (1..=180).collect::<Vec<_>>());
}
