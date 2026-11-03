use hennery_proto::frames::{ParkReason, SessionBody};
use hennery_sessions::store::{Cursor, ListQuery, Store};

#[test]
fn native_session_ids_are_searchable_literally_across_lifecycles_with_hat_scoping_and_pagination() {
    let store = Store::open_in_memory().unwrap();
    let native = "runtime-42%_\\Native";
    for (id, hat, agent_id) in [
        ("open", "hat-a", native),
        ("parked", "hat-a", native),
        ("closed", "hat-a", native),
        ("elsewhere", "hat-b", native),
        ("decoy", "hat-a", "runtime-42XXNative"),
    ] {
        store.create_session(id, "host", "claude", "/tmp", hat, None).unwrap();
        store
            .ingest(id, 1, &SessionBody::session_started("start", agent_id))
            .unwrap();
    }
    store
        .ingest(
            "parked",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    store.close_now("closed").unwrap();
    store
        .create_session("unstarted", "host", "claude", "/tmp", "hat-a", None)
        .unwrap();

    for query in [native, "42%_\\nAT", "%_\\"] {
        let mut ids: Vec<_> = store
            .list(&ListQuery {
                search: Some(query),
                hat: Some("hat-a"),
                lifecycles: Some(&["starting"]),
                ..ListQuery::default()
            })
            .unwrap()
            .sessions
            .into_iter()
            .map(|item| item.session_id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["closed", "open", "parked"]);
    }
    let query = ListQuery {
        search: Some(native),
        hat: Some("hat-a"),
        limit: 1,
        ..ListQuery::default()
    };
    let mut ids = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .list(&ListQuery {
                after: cursor.as_ref(),
                ..query
            })
            .unwrap();
        ids.extend(page.sessions.into_iter().map(|item| item.session_id));
        cursor = page.next_cursor.as_deref().map(|value| Cursor::decode(value).unwrap());
        if cursor.is_none() {
            break;
        }
    }
    ids.sort();
    assert_eq!(ids, ["closed", "open", "parked"]);
    assert_eq!(
        store
            .list(&ListQuery {
                hat: Some("hat-b"),
                limit: 50,
                ..query
            })
            .unwrap()
            .sessions[0]
            .session_id,
        "elsewhere"
    );
    assert!(
        store
            .list(&ListQuery {
                hat: Some("hat-missing"),
                ..query
            })
            .unwrap()
            .sessions
            .is_empty()
    );
}
