//! The host's connection loop against a fake collector: enforcing the read
//! deadline on a half-open socket (F1) and resetting the reconnect backoff
//! only once the collector has proven it is actually committing frames (an
//! `ack`), not merely on a successful `hello` handshake (F2) — a collector
//! that always accepts `hello` but never commits (e.g. disk full) must not
//! be hammered at `reconnect_min` forever.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_host::{HostConfig, run};
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
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

/// Accept a host's WebSocket the way the collector does: the upgrade
/// response carries a hello nonce (ACP core §3.5). These fake collectors
/// check no proof, so any nonce does.
async fn accept(
    tcp: tokio::net::TcpStream,
) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, tokio_tungstenite::tungstenite::Error> {
    tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await
}

/// The signature tungstenite's handshake callback requires.
#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
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
            let Ok(ws) = accept(tcp).await else {
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
        HostKey::from_seed([1; 32]),
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
            let Ok(ws) = accept(tcp).await else {
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
            let Ok(ws) = accept(tcp).await else {
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
        HostKey::from_seed([1; 32]),
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
        HostKey::from_seed([1; 32]),
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

type ServerSink = futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>;

/// Accept one host connection: read its `hello`, answer `hello_ack`, and
/// read up to its `resend_complete`. Returns the hello's attached sessions.
async fn accept_host(
    listener: &TcpListener,
) -> (ServerSink, ServerStream, Vec<hennery_proto::frames::AttachedSession>) {
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let ws = accept(tcp).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let HostFrame::Hello { attached_sessions, .. } = read_host_frame(&mut stream).await else {
        panic!("expected hello");
    };
    send_frame(&mut sink, &hello_ack()).await;
    read_until(&mut stream, |f| matches!(f, HostFrame::ResendComplete)).await;
    (sink, stream, attached_sessions)
}

async fn send_frame(sink: &mut ServerSink, frame: &CollectorFrame) {
    sink.send(Message::text(serde_json::to_string(frame).unwrap()))
        .await
        .unwrap();
}

/// Read frames until one matches (10 s bound).
async fn read_until(stream: &mut ServerStream, pred: impl Fn(&HostFrame) -> bool) -> HostFrame {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    let frame: HostFrame = serde_json::from_str(&text).unwrap();
                    if pred(&frame) {
                        return frame;
                    }
                }
                Some(Ok(_)) => {}
                other => panic!("connection ended while waiting: {other:?}"),
            }
        }
    })
    .await
    .expect("expected frame within 10s")
}

fn body_is(session: &str, kind: &str) -> impl Fn(&HostFrame) -> bool {
    let (session, kind) = (session.to_string(), kind.to_string());
    move |f| match f {
        HostFrame::Session { session_id, body, .. } => {
            session_id == &session && serde_json::to_value(body).unwrap()["kind"] == kind.as_str()
        }
        _ => false,
    }
}

fn error_for(request: &str) -> impl Fn(&HostFrame) -> bool {
    let request = request.to_string();
    move |f| matches!(f, HostFrame::Error { request_id, .. } if *request_id == request)
}

fn start(request_id: &str, session_id: &str) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: request_id.into(),
        session_id: session_id.into(),
        committed_seq: 0,
        agent: "fake".into(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        config: Default::default(),
    }
}

/// A host with the fake adapter (a slow 2 s turn) registered as `fake`.
fn host_with_fake(addr: std::net::SocketAddr, name: &str, program: hennery_host::AgentCommand) -> HostConfig {
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir(name),
    );
    cfg.reconnect_min = Duration::from_millis(50);
    cfg.reconnect_max = Duration::from_millis(200);
    cfg.agents.insert("fake".into(), program);
    cfg
}

fn slow_fake() -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        chunks: (1..=20).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 100,
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

#[tokio::test]
async fn hello_reports_live_sessions_with_their_open_turn_and_drops_ended_ones() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "hello-open-turn", slow_fake())));

    let (mut sink, mut stream, attached) = accept_host(&listener).await;
    assert!(attached.is_empty());
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::Prompt {
            request_id: "r2".into(),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            content: vec![serde_json::json!({"type": "text", "text": "go"})],
        },
    )
    .await;
    read_until(&mut stream, body_is("s1", "turn_started")).await;
    send_frame(&mut sink, &start("r3", "s2")).await;
    read_until(&mut stream, body_is("s2", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r4".into(),
            session_id: "s2".into(),
        },
    )
    .await;
    read_until(&mut stream, body_is("s2", "session_closed")).await;
    drop((sink, stream)); // the connection drops mid-turn

    let (_sink, _stream, attached) = accept_host(&listener).await;
    assert_eq!(attached.len(), 1, "{attached:?}");
    assert_eq!(attached[0].session_id, "s1");
    assert_eq!(attached[0].open_turn_id.as_deref(), Some("t1"));
    assert!(attached[0].last_seq >= 2, "{attached:?}");
}

#[tokio::test]
async fn park_reaches_the_actor_and_frames_for_a_detached_session_are_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "park", slow_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    let park = |request_id: &str| CollectorFrame::ParkSession {
        request_id: request_id.into(),
        session_id: "s1".into(),
    };
    send_frame(&mut sink, &park("r2")).await;
    let parked = read_until(&mut stream, body_is("s1", "session_parked")).await;
    assert!(
        matches!(&parked, HostFrame::Session { body: hennery_proto::frames::SessionBody::SessionParked { reason }, .. }
            if *reason == hennery_proto::frames::ParkReason::Operator),
        "{parked:?}"
    );
    // The actor is gone: a second park, a close and a prompt are all refused.
    send_frame(&mut sink, &park("r3")).await;
    let refused = read_until(&mut stream, error_for("r3")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r4".into(),
            session_id: "s1".into(),
        },
    )
    .await;
    let close_refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&close_refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{close_refused:?}"
    );
    send_frame(
        &mut sink,
        &CollectorFrame::Prompt {
            request_id: "r5".into(),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            content: vec![serde_json::json!({"type": "text", "text": "go"})],
        },
    )
    .await;
    let prompt_refused = read_until(&mut stream, error_for("r5")).await;
    assert!(
        matches!(&prompt_refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{prompt_refused:?}"
    );
}

/// After a session is parked, a repeated `start_session` for the same id must
/// not be routed to `Restart` (there is no live actor left to answer it) —
/// it spawns a fresh actor, whose `session_started` carries the new request.
#[tokio::test]
async fn a_start_after_park_spawns_a_fresh_actor() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "start-after-park", slow_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::ParkSession {
            request_id: "r2".into(),
            session_id: "s1".into(),
        },
    )
    .await;
    read_until(&mut stream, body_is("s1", "session_parked")).await;

    send_frame(&mut sink, &start("r5", "s1")).await;
    if let HostFrame::Session {
        body: hennery_proto::frames::SessionBody::SessionStarted { request_id, .. },
        ..
    } = read_until(&mut stream, body_is("s1", "session_started")).await
    {
        assert_eq!(request_id, "r5");
    }
}

#[tokio::test]
async fn a_repeated_start_session_re_emits_session_started_without_a_second_adapter() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    // Counts adapter launches, then becomes the fake adapter.
    let counting = hennery_host::AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "echo spawned >> {}; exec {}",
                spawns.display(),
                env!("CARGO_BIN_EXE_hennery-fake-acp")
            ),
        ],
        env: Vec::new(),
    };
    tokio::spawn(run(host_with_fake(addr, "restart", counting)));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    send_frame(&mut sink, &start("r2", "s1")).await;
    let mut request_ids = Vec::new();
    for _ in 0..2 {
        if let HostFrame::Session {
            body: hennery_proto::frames::SessionBody::SessionStarted { request_id, .. },
            ..
        } = read_until(&mut stream, body_is("s1", "session_started")).await
        {
            request_ids.push(request_id);
        }
    }
    assert_eq!(request_ids, ["r1", "r2"]);
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 1);
}

/// Acks `hello`, keeps the connection up for `hold`, then closes cleanly,
/// never acking a frame. Reports each hello's arrival time.
async fn hold_then_close_server(listener: TcpListener, hold: Duration, hellos: mpsc::UnboundedSender<Instant>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = accept(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(Instant::now()).ok();
            let _ = sink
                .send(Message::text(serde_json::to_string(&hello_ack()).unwrap()))
                .await;
            tokio::time::sleep(hold).await;
            let _ = sink.close().await;
        });
    }
}

#[tokio::test]
async fn backoff_resets_after_a_healthy_connection_even_without_acks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut hellos) = mpsc::unbounded_channel();
    // Up for 150 ms each time, against a 100 ms healthy threshold.
    tokio::spawn(hold_then_close_server(listener, Duration::from_millis(150), tx));

    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir("healthy-reset"),
    );
    cfg.reconnect_min = Duration::from_millis(20);
    cfg.reconnect_max = Duration::from_secs(10);
    cfg.healthy_after = Duration::from_millis(100);
    tokio::spawn(run(cfg));

    let mut timestamps = Vec::new();
    for _ in 0..6 {
        let t = tokio::time::timeout(Duration::from_secs(3), hellos.recv())
            .await
            .expect("hello within 3s")
            .expect("hello channel open");
        timestamps.push(t);
    }
    // Each gap is the 150 ms hold plus the backoff; without the reset the
    // backoff alone would pass 300 ms by the fifth reconnect.
    let gaps: Vec<Duration> = timestamps.windows(2).map(|w| w[1] - w[0]).collect();
    for (i, gap) in gaps.iter().enumerate() {
        assert!(*gap < Duration::from_millis(350), "gap {i} was {gap:?}; gaps {gaps:?}");
    }
}

#[tokio::test]
async fn a_collector_that_never_completes_the_handshake_is_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir("connect-timeout"),
    );
    cfg.connect_timeout = Duration::from_millis(200);
    cfg.reconnect_min = Duration::from_millis(50);
    tokio::spawn(run(cfg));

    // Accept TCP but never answer the WebSocket upgrade.
    let mut held = Vec::new();
    for attempt in 0..2 {
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap_or_else(|_| panic!("connection attempt {attempt} within 2s"))
            .unwrap();
        held.push(tcp);
    }
}

fn resume(request_id: &str, session_id: &str, committed_seq: u64, agent_session_id: &str) -> CollectorFrame {
    CollectorFrame::ResumeSession {
        request_id: request_id.into(),
        session_id: session_id.into(),
        committed_seq,
        agent: "fake".into(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        agent_session_id: agent_session_id.into(),
        config: Default::default(),
    }
}

/// The fake adapter behind a shell that appends a line to `spawns` per launch.
fn counting_fake(spawns: &std::path::Path) -> hennery_host::AgentCommand {
    counting_fake_with(spawns, "")
}

/// `counting_fake` running `prelude` first (e.g. `trap '' TERM;`, which
/// survives the `exec`).
fn counting_fake_with(spawns: &std::path::Path, prelude: &str) -> hennery_host::AgentCommand {
    hennery_host::AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "{prelude} echo spawned >> {}; exec {}",
                spawns.display(),
                env!("CARGO_BIN_EXE_hennery-fake-acp")
            ),
        ],
        env: Vec::new(),
    }
}

/// `(seq, request_id, agent_session_id)` of a `session_started` frame.
fn started(frame: &HostFrame) -> (u64, String, String) {
    match frame {
        HostFrame::Session {
            seq,
            body:
                hennery_proto::frames::SessionBody::SessionStarted {
                    request_id,
                    agent_session_id,
                    ..
                },
            ..
        } => (*seq, request_id.clone(), agent_session_id.clone()),
        other => panic!("expected session_started, got {other:?}"),
    }
}

/// Read frames until one matches (10 s bound), returning all of them.
async fn read_through(stream: &mut ServerStream, pred: impl Fn(&HostFrame) -> bool) -> Vec<HostFrame> {
    let mut seen = Vec::new();
    loop {
        let frame = read_until(stream, |_| true).await;
        let done = pred(&frame);
        seen.push(frame);
        if done {
            return seen;
        }
    }
}

/// ACP core §5.1 and §12 scenario 17: a host whose outbox is gone (a fresh
/// data dir) resumes a session the collector has committed 40 frames of.
/// Its first frame must be 41, or the collector would discard it as a
/// duplicate of a frame it already has.
#[tokio::test]
async fn a_resume_after_the_outbox_was_lost_continues_past_the_collectors_seq() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "resume-fast-forward", slow_fake())));

    let (mut sink, mut stream, attached) = accept_host(&listener).await;
    assert!(attached.is_empty());
    send_frame(&mut sink, &resume("r1", "s9", 40, "agent-7")).await;
    let frame = read_until(&mut stream, body_is("s9", "session_started")).await;
    assert_eq!(started(&frame), (41, "r1".to_string(), "agent-7".to_string()));
}

#[tokio::test]
async fn a_resume_for_an_attached_session_re_emits_session_started_without_a_second_adapter() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    tokio::spawn(run(host_with_fake(addr, "resume-attached", counting_fake(&spawns))));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    let first = read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(&mut sink, &resume("r2", "s1", 0, "fake-session-1")).await;
    let second = read_until(&mut stream, body_is("s1", "session_started")).await;
    assert_eq!(started(&first).1, "r1");
    assert_eq!(started(&second).1, "r2");
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 1);
}

/// A resume that arrives right behind a close for the same session (the
/// reconciliation's `close_session`, then an operator's resume) must not be
/// routed to the closing actor, which would answer `not_attached`. Nor may
/// it start a second adapter while the first is still being killed: the new
/// actor's `session_started` would overtake the old one's `session_closed`
/// and the collector would close the session it just resumed. It waits for
/// the old actor to finish. The adapter ignores SIGTERM, so that takes the
/// whole 5 s kill grace.
#[tokio::test]
async fn a_resume_queued_behind_a_close_attaches_a_fresh_adapter_after_the_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    let stubborn = counting_fake_with(&spawns, "trap '' TERM;");
    tokio::spawn(run(host_with_fake(addr, "resume-after-close", stubborn)));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r2".into(),
            session_id: "s1".into(),
        },
    )
    .await;
    send_frame(&mut sink, &resume("r3", "s1", 0, "fake-session-1")).await;
    let frames = read_through(&mut stream, body_is("s1", "session_started")).await;
    assert!(
        frames.iter().any(body_is("s1", "session_closed")),
        "the close ran first: {frames:?}"
    );
    assert!(
        !frames.iter().any(|f| matches!(f, HostFrame::Error { .. })),
        "the resume was refused: {frames:?}"
    );
    assert_eq!(started(frames.last().unwrap()).1, "r3");
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 2);
}

// Plan B2a: cancel and capabilities over the connection.

/// Plan 6a adds `images`: image prompts, refused per agent when its
/// `initialize` offers none (decision 2).
#[tokio::test]
async fn a_host_announces_that_it_can_park_and_take_images() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "capabilities", slow_fake())));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let (_sink, mut stream) = accept(tcp).await.unwrap().split();
    let HostFrame::Hello { capabilities, .. } = read_host_frame(&mut stream).await else {
        panic!("expected hello");
    };
    use hennery_proto::frames::{Capabilities, Capability};
    assert_eq!(capabilities, Capabilities(vec![Capability::Park, Capability::Images]));
}

#[tokio::test]
async fn cancel_turn_reaches_the_actor_and_a_cancel_for_a_detached_session_is_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "cancel", slow_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::Prompt {
            request_id: "r2".into(),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            content: vec![serde_json::json!({"type": "text", "text": "go"})],
        },
    )
    .await;
    read_until(&mut stream, body_is("s1", "turn_started")).await;
    let cancel = |request_id: &str, session_id: &str| CollectorFrame::CancelTurn {
        request_id: request_id.into(),
        session_id: session_id.into(),
        turn_id: "t1".into(),
    };
    send_frame(&mut sink, &cancel("r3", "s1")).await;
    let ended = read_until(&mut stream, body_is("s1", "turn_ended")).await;
    let HostFrame::Session {
        body: hennery_proto::frames::SessionBody::TurnEnded { turn_id, outcome, .. },
        ..
    } = ended
    else {
        panic!("expected turn_ended, got {ended:?}");
    };
    assert_eq!(
        (turn_id.as_str(), outcome),
        ("t1", hennery_proto::frames::TurnOutcome::Cancelled)
    );
    send_frame(&mut sink, &cancel("r4", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}

/// A resume waiting behind a close must not attach once host shutdown has
/// taken the session map: shutdown would never wait for that adapter, and
/// the runtime would SIGKILL it without its grace. The adapter ignores
/// SIGTERM, so the close takes the whole 5 s kill grace, and shutdown
/// begins while the resume still waits.
#[tokio::test]
async fn a_resume_waiting_behind_a_close_never_attaches_after_host_shutdown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    let stubborn = counting_fake_with(&spawns, "trap '' TERM;");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = tokio::spawn(hennery_host::run_until(
        host_with_fake(addr, "resume-after-shutdown", stubborn),
        async {
            let _ = stopped.await;
        },
    ));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r2".into(),
            session_id: "s1".into(),
        },
    )
    .await;
    send_frame(&mut sink, &resume("r3", "s1", 0, "fake-session-1")).await;
    // Nothing on the wire says the resume is queued; the close has most of
    // its 5 s grace left.
    tokio::time::sleep(Duration::from_secs(1)).await;
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), host)
        .await
        .expect("the host shut down")
        .unwrap()
        .unwrap();
    // Time for a wrongly attached adapter to launch.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 1);
}

// Plan B2b: config over the connection.

fn configurable_fake() -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

fn current_mode(frame: &HostFrame) -> Option<String> {
    match frame {
        HostFrame::Session {
            body: hennery_proto::frames::SessionBody::SessionStarted { indexed, .. },
            ..
        } => indexed.current_mode.clone(),
        other => panic!("expected session_started, got {other:?}"),
    }
}

#[tokio::test]
async fn start_and_resume_carry_their_config_to_the_adapter() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "start-config", configurable_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    let plan = hennery_proto::frames::SessionConfig {
        mode: Some("plan".into()),
        ..Default::default()
    };
    let mut frame = start("r1", "s1");
    if let CollectorFrame::StartSession { config, .. } = &mut frame {
        *config = plan.clone();
    }
    send_frame(&mut sink, &frame).await;
    let started = read_until(&mut stream, body_is("s1", "session_started")).await;
    assert_eq!(current_mode(&started).as_deref(), Some("plan"));

    let mut frame = resume("r2", "s2", 0, "agent-7");
    if let CollectorFrame::ResumeSession { config, .. } = &mut frame {
        *config = hennery_proto::frames::SessionConfig {
            mode: Some("bypass".into()),
            ..Default::default()
        };
    }
    send_frame(&mut sink, &frame).await;
    let resumed = read_until(&mut stream, body_is("s2", "session_started")).await;
    assert_eq!(current_mode(&resumed).as_deref(), Some("bypass"));
}

#[tokio::test]
async fn set_config_reaches_the_actor_and_one_for_a_detached_session_is_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "set-config", configurable_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    let set = |request_id: &str, session_id: &str| CollectorFrame::SetConfig {
        request_id: request_id.into(),
        session_id: session_id.into(),
        config_id: "mode".into(),
        value: hennery_proto::frames::ConfigValue::Id("plan".into()),
    };
    send_frame(&mut sink, &set("r2", "s1")).await;
    let applied = read_until(&mut stream, body_is("s1", "config_applied")).await;
    let HostFrame::Session {
        body: hennery_proto::frames::SessionBody::ConfigApplied { request_id, indexed },
        ..
    } = applied
    else {
        panic!("expected config_applied, got {applied:?}");
    };
    assert_eq!(
        (request_id.as_str(), indexed.current_mode.as_deref()),
        ("r2", Some("plan"))
    );
    send_frame(&mut sink, &set("r3", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r3")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}

// Plan B2b, review fix round 1: launching while the old actor is still
// ending by itself.

/// A `session_parked` frame with this `reason`, matched via its plain JSON
/// (`SessionBody`/`ParkReason` are not otherwise imported here).
fn parked_reason(session: &str, reason: &str) -> impl Fn(&HostFrame) -> bool {
    let (session, reason) = (session.to_string(), reason.to_string());
    move |f| match f {
        HostFrame::Session { session_id, body, .. } => {
            let value = serde_json::to_value(body).unwrap();
            session_id == &session && value["kind"] == "session_parked" && value["reason"] == reason.as_str()
        }
        _ => false,
    }
}

/// An idle reap ends the actor by itself (task 9: `begin_ending` fires
/// before the adapter is killed, not only behind a queued park/close). A
/// resume arriving while it is still killing its adapter (ignoring SIGTERM,
/// so it needs the whole 5 s kill grace) must not be routed to it and
/// answered `not_attached`, nor start a second adapter alongside the one
/// still being killed.
///
/// Fix round 1: `attach`'s own `is_ending` check and `spawn_or_restart`'s
/// launch decision used to be two separate locked reads of the session map.
/// The actor's own `begin_ending` (set with no lock held, from inside the
/// actor's task) could land in the gap between them: `attach` read
/// `is_ending() == false` and took the direct path into `spawn_or_restart`,
/// which then re-read the map and found the handle no longer restartable
/// (`is_ending() == true` by then) — falling through to a launch while the
/// old actor was still tearing down. Collapsing both checks into
/// `spawn_or_restart`'s single locked read closes that *launch* gap — a
/// `Restart` send can still race `begin_ending` (it takes no lock), but that
/// only ever costs a retryable `not_attached`, never a second adapter.
///
/// The ordering assertion below (`session_parked` before the new
/// `session_started`) is the one that actually proves "one adapter at a
/// time": the idle-reap arm only emits `session_parked{idle}` after
/// `teardown` has killed the old adapter, so seeing it first means the old
/// adapter was already dead before the new one started.
#[tokio::test]
async fn a_resume_during_an_idle_reaps_kill_grace_waits_for_the_old_actor_to_finish() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    let stubborn = counting_fake_with(&spawns, "trap '' TERM;");
    let mut cfg = host_with_fake(addr, "resume-during-idle-reap", stubborn);
    cfg.idle_timeout = Duration::from_millis(200);
    tokio::spawn(run(cfg));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;

    // Past the idle timeout: the actor has begun reaping and (ignoring
    // SIGTERM) is still killing its adapter, well within the 5 s grace.
    tokio::time::sleep(Duration::from_millis(500)).await;
    send_frame(&mut sink, &resume("r3", "s1", 0, "fake-session-1")).await;

    // A direct sample well inside the kill grace (it runs 5 s; the old
    // actor's SIGTERM-ignoring adapter cannot possibly be dead yet): a
    // launch racing the old actor's teardown would already show a second
    // spawn here, long before either frame below lands.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        std::fs::read_to_string(&spawns).unwrap().lines().count(),
        1,
        "a second adapter was spawned while the first was still being killed"
    );

    let frames = read_through(&mut stream, body_is("s1", "session_started")).await;
    assert!(
        !frames.iter().any(|f| matches!(f, HostFrame::Error { .. })),
        "the resume must not be answered not_attached while the old actor is still ending: {frames:?}"
    );
    let parked_at = frames
        .iter()
        .position(parked_reason("s1", "idle"))
        .unwrap_or_else(|| panic!("the new session_started arrived before the old actor's session_parked: {frames:?}"));
    let started_at = frames.len() - 1;
    assert_eq!(started(frames.last().unwrap()).1, "r3");
    assert!(
        parked_at < started_at,
        "the old actor's session_parked must land before the new session_started: {frames:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&spawns).unwrap().lines().count(),
        2,
        "one spawn for the start, one for the resume — a launch racing the old \
         actor's teardown would show a third"
    );
}

// Plan (2): answers over the connection.

fn asking_fake() -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        asks: vec![hennery_testkit::FakeAsk::Permission],
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

/// A session frame's body as JSON.
fn body_of(frame: &HostFrame) -> serde_json::Value {
    match frame {
        HostFrame::Session { body, .. } => serde_json::to_value(body).unwrap(),
        other => panic!("expected a session frame, got {other:?}"),
    }
}

#[tokio::test]
async fn answers_reach_the_actor_and_one_for_a_detached_session_is_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "answers", asking_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    let prompt = CollectorFrame::Prompt {
        request_id: "r2".into(),
        session_id: "s1".into(),
        turn_id: "t1".into(),
        content: vec![serde_json::json!({"type": "text", "text": "hi"})],
    };
    send_frame(&mut sink, &prompt).await;
    let opened = body_of(&read_until(&mut stream, body_is("s1", "pending_opened")).await);
    let pending_id = opened["pending_id"].as_str().unwrap().to_string();
    let answer = |request_id: &str, session_id: &str| CollectorFrame::AnswerPermission {
        request_id: request_id.into(),
        session_id: session_id.into(),
        pending_id: pending_id.clone(),
        option_id: "allow".into(),
    };
    send_frame(&mut sink, &answer("r3", "s1")).await;
    let result = body_of(&read_until(&mut stream, body_is("s1", "answer_result")).await);
    assert_eq!(
        (&result["request_id"], &result["delivered"]),
        (&serde_json::json!("r3"), &serde_json::json!(true))
    );
    send_frame(&mut sink, &answer("r4", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}

// Plan 3a: the hello proof.

#[tokio::test]
async fn a_host_signs_its_hello_over_the_connections_nonce() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "proof", slow_fake())));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let (_sink, mut stream) = accept(tcp).await.unwrap().split();
    let HostFrame::Hello { host_id, proof, .. } = read_host_frame(&mut stream).await else {
        panic!("expected hello");
    };
    let key = HostKey::from_seed([1; 32]);
    assert_eq!(proof, key.sign_hello(&[5u8; 32], &host_id, PROTOCOL_VERSION));
}

#[tokio::test]
async fn a_collector_that_sends_no_nonce_gets_no_hello() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let host = tokio::spawn(run(host_with_fake(addr, "no-nonce", slow_fake())));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    // A plain upgrade, without the nonce header.
    let (_sink, mut stream) = tokio_tungstenite::accept_async(tcp).await.unwrap().split();
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("the host gives up on the connection");
    assert!(
        !matches!(first, Some(Ok(Message::Text(_)))),
        "the host sent a frame without a nonce to sign: {first:?}"
    );
    host.abort();
}
