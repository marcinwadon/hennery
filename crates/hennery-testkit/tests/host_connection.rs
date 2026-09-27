//! The host's connection loop against a fake collector: enforcing the read
//! deadline on a half-open socket (F1) and resetting the reconnect backoff
//! only once the collector has proven it is actually committing frames (an
//! `ack`), not merely on a successful `hello` handshake (F2) — a collector
//! that always accepts `hello` but never commits (e.g. disk full) must not
//! be hammered at `reconnect_min` forever.

use futures::{SinkExt, StreamExt};
use hennery_host::{HostConfig, run};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

/// A minimal ack for `hello`: accepts every session with no committed backlog.
fn hello_ack() -> CollectorFrame {
    CollectorFrame::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        collector_version: "test".into(),
        committed: BTreeMap::new(),
    }
}

type ServerStream = futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>>;

/// Read one text frame off `stream` and parse it as a `HostFrame`. Panics if
/// the connection closes or sends anything else first.
async fn read_host_frame(stream: &mut ServerStream) -> HostFrame {
    match stream.next().await {
        Some(Ok(Message::Text(text))) => serde_json::from_str(&text).expect("valid HostFrame json"),
        other => panic!("expected a HostFrame, got {other:?}"),
    }
}

fn unique_data_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hennery-host-connection-test-{name}-{}-{nanos}-{n}",
        std::process::id()
    ))
}

/// Accepts connections, acks `hello`, then goes silent without closing the
/// socket (a half-open connection: nothing more ever arrives). Reports every
/// accepted connection's `hello` on `hellos`.
async fn silent_after_ack_server(listener: TcpListener, hellos: mpsc::UnboundedSender<()>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(()).ok();
            let _ = sink
                .send(Message::text(serde_json::to_string(&hello_ack()).unwrap()))
                .await;
            // Go silent forever: never send another frame, never close. This
            // is the half-open socket the read deadline exists to detect.
            std::future::pending::<()>().await;
        });
    }
}

#[tokio::test]
async fn a_silent_connection_is_dropped_and_reconnected_within_the_read_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut hellos) = mpsc::unbounded_channel();
    tokio::spawn(silent_after_ack_server(listener, tx));

    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        "token",
        unique_data_dir("read-deadline"),
    );
    cfg.ping_interval = Duration::from_millis(50);
    cfg.read_timeout = Duration::from_millis(200);
    cfg.reconnect_min = Duration::from_millis(50);
    cfg.reconnect_max = Duration::from_millis(200);
    tokio::spawn(run(cfg));

    // First hello: the initial connection.
    tokio::time::timeout(Duration::from_secs(1), hellos.recv())
        .await
        .expect("first hello within 1s")
        .expect("hello channel open");

    // The server never sends anything after hello_ack. Without the read
    // deadline firing, the host would ping into the dead socket forever and
    // never reconnect, so this second hello only arrives if `connect_once`
    // noticed the silence and gave up.
    tokio::time::timeout(Duration::from_secs(1), hellos.recv())
        .await
        .expect("second hello (reconnect) within 1s — the read deadline did not fire")
        .expect("hello channel open");
}

/// Accepts connections, acks `hello`, then acks one (fabricated) session
/// frame — proof it is actually committing, not just handshaking — and
/// immediately closes cleanly. Reports the instant each `hello` was
/// received on `hellos`.
async fn ack_then_close_server(listener: TcpListener, hellos: mpsc::UnboundedSender<Instant>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(Instant::now()).ok();
            let _ = sink
                .send(Message::text(serde_json::to_string(&hello_ack()).unwrap()))
                .await;
            // No session is actually attached, but any session_id is enough
            // to prove backoff resets on real commit evidence, not just the
            // handshake: `Outbox::ack` on an unknown session harmlessly
            // deletes zero rows.
            let ack = CollectorFrame::Ack {
                session_id: "s0".into(),
                ack_seq: 0,
            };
            let _ = sink.send(Message::text(serde_json::to_string(&ack).unwrap())).await;
            let _ = sink.close().await;
        });
    }
}

/// Accepts connections, acks `hello`, then immediately closes cleanly
/// WITHOUT ever acking a frame — the disk-full-collector scenario, where
/// only the handshake succeeds. Reports the instant each `hello` was
/// received on `hellos`.
async fn ack_hello_only_then_close_server(listener: TcpListener, hellos: mpsc::UnboundedSender<Instant>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(Instant::now()).ok();
            let _ = sink
                .send(Message::text(serde_json::to_string(&hello_ack()).unwrap()))
                .await;
            let _ = sink.close().await;
        });
    }
}

#[tokio::test]
async fn backoff_resets_to_reconnect_min_after_every_acked_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut hellos) = mpsc::unbounded_channel();
    tokio::spawn(ack_then_close_server(listener, tx));

    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        "token",
        unique_data_dir("backoff-reset"),
    );
    cfg.reconnect_min = Duration::from_millis(20);
    // Deliberately far above reconnect_min: if backoff never resets, repeated
    // doubling (20, 40, 80, 160, 320, 640ms, ...) would blow well past the
    // per-gap threshold below by the 6th reconnect.
    cfg.reconnect_max = Duration::from_secs(10);
    tokio::spawn(run(cfg));

    let mut timestamps = Vec::new();
    for _ in 0..7 {
        let t = tokio::time::timeout(Duration::from_secs(2), hellos.recv())
            .await
            .expect("hello within 2s")
            .expect("hello channel open");
        timestamps.push(t);
    }

    let gaps: Vec<Duration> = timestamps.windows(2).map(|w| w[1] - w[0]).collect();
    for (i, gap) in gaps.iter().enumerate() {
        assert!(
            *gap < Duration::from_millis(200),
            "gap {i} was {gap:?}; backoff should stay near reconnect_min (20ms) \
             after every successful handshake instead of doubling \
             (an unreset backoff would exceed 200ms well before the 6th reconnect)"
        );
    }
}

/// The disk-full-collector scenario: the handshake always succeeds, but
/// nothing is ever acked. A handshake alone must not be enough to reset
/// backoff, or the host would hammer such a collector at reconnect_min
/// forever instead of backing off.
#[tokio::test]
async fn backoff_keeps_growing_when_the_collector_never_acks_a_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut hellos) = mpsc::unbounded_channel();
    tokio::spawn(ack_hello_only_then_close_server(listener, tx));

    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        "token",
        unique_data_dir("backoff-no-ack"),
    );
    cfg.reconnect_min = Duration::from_millis(20);
    cfg.reconnect_max = Duration::from_secs(10);
    tokio::spawn(run(cfg));

    let mut timestamps = Vec::new();
    for _ in 0..6 {
        let t = tokio::time::timeout(Duration::from_secs(2), hellos.recv())
            .await
            .expect("hello within 2s")
            .expect("hello channel open");
        timestamps.push(t);
    }

    let gaps: Vec<Duration> = timestamps.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        *gaps.last().unwrap() > Duration::from_millis(100),
        "gaps were {gaps:?}; without any ack ever landing, backoff should have kept \
         doubling from reconnect_min (20ms) instead of resetting on the handshake alone"
    );
}
