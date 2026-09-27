use hennery_host::outbox::Outbox;
use hennery_proto::frames::{HostFrame, Indexed, SessionBody};
use serde_json::json;

fn update(n: u32) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload: json!({ "n": n }),
    }
}

fn seq_of(frame: &HostFrame) -> u64 {
    match frame {
        HostFrame::Session { seq, .. } => *seq,
        other => panic!("not a session frame: {other:?}"),
    }
}

#[test]
fn seqs_are_per_session_and_start_at_one() {
    let mut ob = Outbox::open_in_memory().unwrap();
    assert_eq!(seq_of(&ob.enqueue("a", update(1)).unwrap()), 1);
    assert_eq!(seq_of(&ob.enqueue("a", update(2)).unwrap()), 2);
    assert_eq!(seq_of(&ob.enqueue("b", update(1)).unwrap()), 1);
}

#[test]
fn ack_removes_only_frames_up_to_the_acked_seq() {
    let mut ob = Outbox::open_in_memory().unwrap();
    for n in 1..=3 {
        ob.enqueue("a", update(n)).unwrap();
    }
    ob.enqueue("b", update(1)).unwrap();
    assert_eq!(ob.ack("a", 2).unwrap(), 2);
    let left: Vec<u64> = ob.pending().unwrap().iter().map(seq_of).collect();
    assert_eq!(left, vec![3, 1]);
}

#[test]
fn seq_numbering_survives_reopen_even_after_everything_is_acked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("outbox.db");
    {
        let mut ob = Outbox::open(&path).unwrap();
        ob.enqueue("a", update(1)).unwrap();
        ob.enqueue("a", update(2)).unwrap();
        ob.ack("a", 2).unwrap();
    }
    let mut ob = Outbox::open(&path).unwrap();
    assert!(ob.pending().unwrap().is_empty());
    assert_eq!(ob.last_seq("a").unwrap(), 2);
    assert_eq!(seq_of(&ob.enqueue("a", update(3)).unwrap()), 3);
}

#[test]
fn unacked_frames_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("outbox.db");
    {
        let mut ob = Outbox::open(&path).unwrap();
        ob.enqueue("a", update(1)).unwrap();
    }
    let ob = Outbox::open(&path).unwrap();
    assert_eq!(ob.pending().unwrap().len(), 1);
}

#[test]
fn fast_forward_never_moves_the_counter_backwards() {
    let mut ob = Outbox::open_in_memory().unwrap();
    ob.enqueue("a", update(1)).unwrap();
    ob.fast_forward("a", 10).unwrap();
    assert_eq!(seq_of(&ob.enqueue("a", update(2)).unwrap()), 11);
    ob.fast_forward("a", 3).unwrap();
    assert_eq!(seq_of(&ob.enqueue("a", update(3)).unwrap()), 12);
}
