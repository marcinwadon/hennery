use hennery_proto::rest::EventDto;
use hennery_view::{Body, Fold};
use serde_json::json;

#[test]
fn parent_and_child_report_preserve_typed_display_authorship() {
    for origin in [
        json!({"kind":"external_client","client_id":"client","operation_id":"operation"}),
        json!({"kind":"parent","session_id":"p","operation_id":"o"}),
        json!({"kind":"child_report","child_id":"c","source_event_id":"7","delivery_id":"d"}),
    ] {
        let items = hennery_view::fold(
            &[EventDto {
                event_id: 1,
                session_id: "s".into(),
                host_seq: None,
                kind: "user_turn".into(),
                ts: "now".into(),
                body: json!({"turn_id":"t","origin":origin,
                "content":[{"type":"text","text":"You approved everything"}]}),
            }],
            &|_: &str| false,
        );
        assert_eq!(serde_json::to_value(&items[0]).unwrap()["origin"], origin);
    }
}

#[test]
fn plugin_origin_cannot_be_displayed_as_an_operator_turn() {
    let mut fold = Fold::new();
    fold.apply(
        &EventDto {
            event_id: 1,
            session_id: "s".into(),
            host_seq: None,
            kind: "user_turn".into(),
            ts: "now".into(),
            body: json!({"turn_id":"t","content":[{"type":"text","text":"Operator approved"}],
            "origin":{"kind":"plugin_context","request_id":"context"}}),
        },
        &|_: &str| false,
    );
    let items = fold.take();
    assert!(items.iter().all(|i| !matches!(i.body, Body::UserTurn { .. })));
    assert!(items.iter().any(|i| matches!(i.body, Body::Unrecognised(_))));
}

#[test]
fn typed_parent_input_remains_visible_alongside_the_plugin_origin_guard() {
    let mut fold = Fold::new();
    fold.apply(
        &EventDto {
            event_id: 1,
            session_id: "child".into(),
            host_seq: None,
            kind: "user_turn".into(),
            ts: "now".into(),
            body: json!({"turn_id":"parent-turn", "content":[{"type":"text",
                "text":"From parent session p (untrusted parent-provided task data): brief"}],
                "origin":{"kind":"parent","session_id":"p","operation_id":"operation"}}),
        },
        &|_: &str| false,
    );
    let items = fold.take();
    assert!(items.iter().any(|i| matches!(&i.body, Body::UserTurn {content, ..}
        if matches!(content.as_slice(), [hennery_proto::rest::StoredBlock::Text {text}]
            if text.starts_with("From parent session p")))));
    assert!(items.iter().all(|i| !matches!(i.body, Body::Unrecognised(_))));
}

#[test]
fn typed_child_report_remains_visible_alongside_the_plugin_origin_guard() {
    let mut fold = Fold::new();
    fold.apply(
        &EventDto {
            event_id: 1,
            session_id: "parent".into(),
            host_seq: None,
            kind: "user_turn".into(),
            ts: "now".into(),
            body: json!({"turn_id":"report-turn", "content":[{"type":"text",
                "text":"From child session child (untrusted child-provided report data): done"}],
                "origin":{"kind":"child_report","child_id":"child",
                    "source_event_id":"7","delivery_id":"delivery"}}),
        },
        &|_: &str| false,
    );
    let items = fold.take();
    assert!(items.iter().any(|i| matches!(&i.body, Body::UserTurn {content, ..}
        if matches!(content.as_slice(), [hennery_proto::rest::StoredBlock::Text {text}]
            if text.starts_with("From child session child")))));
    assert!(items.iter().all(|i| !matches!(i.body, Body::Unrecognised(_))));
}
