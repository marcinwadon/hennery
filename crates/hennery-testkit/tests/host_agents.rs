//! A host's agents on the collector (plan 4d-B1-i): what a reconciled
//! `hello` stores, and `GET /api/hosts/{id}/agents[?refresh=1]`, against a
//! scripted host over a real WebSocket, so the test controls every frame,
//! its timing and the connection.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, AgentList, MaybeRuntime, RuntimeInfo, RuntimeSource};
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame};
use hennery_proto::rest::{AgentsSource, HostAgents};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::{AppState, store::Store};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HOST: &str = "host-1";

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// A collector whose state `tune` adjusts before it serves.
    async fn start_with(tune: impl FnOnce(&mut AppState)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        hosts.register(HOST, &enrollment, 0).unwrap();
        let operator = Operator::open(&db).unwrap();
        let client = hennery_testkit::operator_client(&operator);
        let mut state = AppState::new(Store::open(&db).unwrap(), hosts, operator);
        tune(&mut state);
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            client,
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(self.url(path)).send().await.unwrap()
    }

    /// `GET /api/hosts/host-1/agents`, with `query`, as the API answers it:
    /// never cached, since a note can name a path on the host.
    async fn agents(&self, query: &str) -> HostAgents {
        let resp = self.get(&format!("/api/hosts/{HOST}/agents{query}")).await;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()["cache-control"], "no-store");
        resp.json().await.unwrap()
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing a host.
struct ScriptedHost {
    ws: Ws,
}

impl ScriptedHost {
    /// `hello` with `capabilities`, `agents` and `runtime`, its
    /// `hello_ack`, and with `reconcile` its `resend_complete`, after which
    /// the host is waited for until it is listed as connected.
    async fn connect(
        collector: &Collector,
        capabilities: Vec<Capability>,
        agents: Vec<AgentInfo>,
        runtime: Option<RuntimeInfo>,
        reconcile: bool,
    ) -> Self {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities: Capabilities(capabilities),
            mcp_isolation: Default::default(),
            workspace_roots: vec![],
            attached_sessions: vec![],
            agents: AgentList(agents),
            runtime: MaybeRuntime(runtime),
        })
        .await;
        assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
        if reconcile {
            host.send(&HostFrame::ResendComplete).await;
            wait_for("host ready", || async {
                collector.state.hub.is_ready(HOST).then_some(())
            })
            .await;
        }
        host
    }

    /// `connect`, reconciled, as a host of this build: it can be probed.
    async fn current(collector: &Collector) -> Self {
        Self::connect(
            collector,
            vec![Capability::ProbeAgents],
            vec![agent("claude", AgentAuth::Unknown)],
            Some(managed()),
            true,
        )
        .await
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    /// The next collector frame that is not an `ack`.
    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text).unwrap() {
                        CollectorFrame::Ack { .. } => {}
                        frame => return frame,
                    },
                    Some(Ok(_)) => {}
                    other => panic!("collector connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a collector frame within 10s")
    }

    /// The request id of the `probe_agents` this host was sent next.
    async fn probed(&mut self) -> String {
        match self.next().await {
            CollectorFrame::ProbeAgents { request_id } => request_id,
            other => panic!("expected probe_agents, got {other:?}"),
        }
    }

    /// Nothing but pings reaches this host within `within`.
    async fn hears_nothing(&mut self, within: Duration) {
        let more = tokio::time::timeout(within, self.next()).await;
        assert!(more.is_err(), "the host was sent {more:?}");
    }

    async fn answer(&mut self, request_id: String, agents: Vec<AgentInfo>) {
        self.send(&HostFrame::Agents {
            request_id,
            agents: AgentList(agents),
            runtime: MaybeRuntime(Some(managed())),
        })
        .await;
    }

    /// Drop the connection and wait until the collector has noticed.
    async fn drop_connection(self, collector: &Collector) {
        drop(self.ws);
        wait_for("host gone", || async {
            (!collector.state.hub.is_ready(HOST)).then_some(())
        })
        .await;
    }
}

async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn agent(name: &str, auth: AgentAuth) -> AgentInfo {
    AgentInfo {
        agent: name.into(),
        available: true,
        auth,
        cli: AgentCli::Bundled,
        adapter_version: Some("1.0.0".into()),
        images: None,
        note: None,
    }
}

fn managed() -> RuntimeInfo {
    RuntimeInfo {
        source: RuntimeSource::Managed,
        set_id: Some("abc".into()),
        pinned: Some(true),
        held: Some(false),
    }
}

fn source_of(collector: &Collector) -> AgentsSource {
    collector.state.hosts.agents(HOST).unwrap().unwrap().source
}

#[tokio::test]
async fn a_reconciled_hello_stores_the_agents_of_a_host_that_reports_them() {
    let collector = Collector::start().await;
    let _host = ScriptedHost::current(&collector).await;
    let got = collector.agents("").await;
    assert_eq!(got.host_id, HOST);
    assert_eq!(got.source, AgentsSource::Hello);
    assert_eq!(got.agents, [agent("claude", AgentAuth::Unknown)]);
    assert_eq!(got.runtime, Some(managed()));
    assert!(got.live);
    let at = got.reported_at.unwrap();
    assert!(at.ends_with('Z') && at.contains('T'), "{at}");
}

/// A host of an older build has no `probe_agents`, and reports nothing.
#[tokio::test]
async fn an_older_hosts_hello_stores_nothing() {
    let collector = Collector::start().await;
    let _host = ScriptedHost::connect(
        &collector,
        vec![Capability::Projects],
        vec![agent("claude", AgentAuth::Unknown)],
        Some(managed()),
        true,
    )
    .await;
    let got = collector.agents("").await;
    assert_eq!(got.source, AgentsSource::None);
    assert!(got.agents.is_empty() && got.runtime.is_none() && got.reported_at.is_none());
    assert!(got.live);
}

/// `hennery host join`'s and doctor's probes send a `hello` and never
/// reconcile: what it says is not stored.
#[tokio::test]
async fn a_hello_never_reconciled_stores_nothing() {
    let collector = Collector::start().await;
    let host = ScriptedHost::connect(
        &collector,
        vec![Capability::ProbeAgents],
        vec![agent("claude", AgentAuth::Unknown)],
        None,
        false,
    )
    .await;
    drop(host);
    let got = collector.agents("").await;
    assert_eq!(got.source, AgentsSource::None);
    assert!(!got.live);
}

#[tokio::test]
async fn an_unknown_host_is_not_found_and_a_bad_refresh_is_invalid() {
    let collector = Collector::start().await;
    let resp = collector.get("/api/hosts/host-9/agents").await;
    assert_eq!(resp.status(), 404);
    let resp = collector.get("/api/hosts/host-9/agents?refresh=1").await;
    assert_eq!(resp.status(), 404);
    for query in ["?refresh=2", "?refresh=yes", "?refresh="] {
        let resp = collector.get(&format!("/api/hosts/{HOST}/agents{query}")).await;
        assert_eq!(resp.status(), 400, "{query}");
    }
    assert_eq!(collector.agents("?refresh=0").await.source, AgentsSource::None);
}

/// No existence oracle: another owner's host, with a report stored, answers
/// as an unknown one, refreshed or not, and its report is never shown. Even
/// connected (the hub, which never checks owners, is given it directly),
/// it is never probed: the registry decides first (the review's A3).
#[tokio::test]
async fn another_owners_host_is_not_found() {
    const OTHER: &str = "owner-00000000000000b2";
    let collector = Collector::start().await;
    let conn = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, 0, 0)",
        [OTHER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES ('hat-00000000000000b2', ?1, 'Theirs', '#000000', 0)",
        [OTHER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at,
                           agents, agents_reported_at, agents_source)
         VALUES ('host-b2', ?1, 'theirs', 'b2', 'p', 'v', 'hat-00000000000000b2', 0,
                 '{\"agents\":[{\"agent\":\"theirs\",\"available\":true,\"cli\":\"bundled\"}]}', 1, 'probe')",
        [OTHER],
    )
    .unwrap();
    let (tx, mut sent) = tokio::sync::mpsc::unbounded_channel();
    let registration = collector
        .state
        .hub
        .register(
            "host-b2",
            tx,
            Capabilities(vec![Capability::ProbeAgents]),
            Default::default(),
        )
        .unwrap();
    collector.state.hub.mark_ready("host-b2", registration.conn_id);
    assert!(collector.state.hub.is_ready("host-b2"));
    for query in ["", "?refresh=1"] {
        let resp = collector.get(&format!("/api/hosts/host-b2/agents{query}")).await;
        assert_eq!(resp.status(), 404, "{query}");
        let body = resp.text().await.unwrap();
        assert!(!body.contains("theirs"), "{body}");
    }
    assert!(sent.try_recv().is_err(), "another owner's host was probed");
}

/// Concurrent refreshes share one probe; its answer is stored and each of
/// them answers from the store. A refresh after them probes again.
#[tokio::test]
async fn concurrent_refreshes_share_one_probe() {
    let collector = std::sync::Arc::new(Collector::start().await);
    let mut host = ScriptedHost::current(&collector).await;
    let refresh = || {
        let collector = collector.clone();
        tokio::spawn(async move { collector.agents("?refresh=1").await })
    };
    let (first, second) = (refresh(), refresh());
    let request_id = host.probed().await;
    host.hears_nothing(Duration::from_millis(300)).await;
    assert!(!first.is_finished() && !second.is_finished());
    host.answer(request_id, vec![agent("claude", AgentAuth::Ok)]).await;
    for got in [first.await.unwrap(), second.await.unwrap()] {
        assert_eq!(got.source, AgentsSource::Probe);
        assert_eq!(got.agents, [agent("claude", AgentAuth::Ok)]);
    }
    let third = refresh();
    let request_id = host.probed().await;
    host.answer(request_id, vec![agent("claude", AgentAuth::Missing)]).await;
    assert_eq!(third.await.unwrap().agents, [agent("claude", AgentAuth::Missing)]);
}

/// The host that cannot be probed is not asked: one gone, one of an older
/// build. Either answers at once, from the store.
#[tokio::test]
async fn a_refresh_of_a_host_that_cannot_be_probed_answers_the_last_report() {
    let collector = Collector::start().await;
    let host = ScriptedHost::current(&collector).await;
    host.drop_connection(&collector).await;
    let began = std::time::Instant::now();
    let got = collector.agents("?refresh=1").await;
    assert!(began.elapsed() < Duration::from_secs(2), "{:?}", began.elapsed());
    assert_eq!((got.source, got.live), (AgentsSource::Hello, false));
    assert_eq!(got.agents, [agent("claude", AgentAuth::Unknown)]);

    let mut older = ScriptedHost::connect(&collector, vec![Capability::Projects], vec![], None, true).await;
    let got = collector.agents("?refresh=1").await;
    assert_eq!((got.source, got.live), (AgentsSource::Hello, true));
    older.hears_nothing(Duration::from_millis(300)).await;
}

/// A probe the host does not answer in time, or refuses, changes nothing:
/// the last report stands.
#[tokio::test]
async fn an_unanswered_or_refused_probe_leaves_the_last_report() {
    let collector = Collector::start_with(|state| state.agents_probe_timeout = Duration::from_millis(500)).await;
    let mut host = ScriptedHost::current(&collector).await;
    let began = std::time::Instant::now();
    let call = {
        let url = collector.url(&format!("/api/hosts/{HOST}/agents?refresh=1"));
        let client = collector.client.clone();
        tokio::spawn(async move {
            client
                .get(url)
                .send()
                .await
                .unwrap()
                .json::<HostAgents>()
                .await
                .unwrap()
        })
    };
    let _ignored = host.probed().await;
    let got = call.await.unwrap();
    assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
    assert_eq!(got.source, AgentsSource::Hello);
    assert!(
        collector.state.hub.is_ready(HOST),
        "a probe's timeout keeps the connection"
    );

    let call = {
        let url = collector.url(&format!("/api/hosts/{HOST}/agents?refresh=1"));
        let client = collector.client.clone();
        tokio::spawn(async move {
            client
                .get(url)
                .send()
                .await
                .unwrap()
                .json::<HostAgents>()
                .await
                .unwrap()
        })
    };
    let request_id = host.probed().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "busy".into(),
        message: "a probe of the agents is running".into(),
    })
    .await;
    assert_eq!(call.await.unwrap().source, AgentsSource::Hello);
}

/// The probe outlives the request that started it: an answer that comes
/// after its caller went away is still stored.
#[tokio::test]
async fn an_answer_after_its_caller_left_is_stored() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::current(&collector).await;
    let gone = tokio::time::timeout(Duration::from_millis(200), collector.agents("?refresh=1")).await;
    assert!(gone.is_err(), "the refresh waits for the probe");
    let request_id = host.probed().await;
    host.answer(request_id, vec![agent("claude", AgentAuth::Ok)]).await;
    wait_for("the probe's answer stored", || async {
        (source_of(&collector) == AgentsSource::Probe).then_some(())
    })
    .await;
}

/// A host's answer is bounded and escaped before it is stored: it may
/// lie, but only about itself.
#[tokio::test]
async fn a_hostile_answer_is_bounded_before_it_is_stored() {
    let collector = std::sync::Arc::new(Collector::start().await);
    let mut host = ScriptedHost::current(&collector).await;
    let call = {
        let collector = collector.clone();
        tokio::spawn(async move { collector.agents("?refresh=1").await })
    };
    let request_id = host.probed().await;
    let mut loud = agent("claude", AgentAuth::Ok);
    loud.note = Some(format!("<script>\u{202E}{}", "x".repeat(10_000)));
    let mut many = vec![
        loud,
        agent("claude", AgentAuth::Missing),
        agent("bad name", AgentAuth::Ok),
    ];
    many.extend((0..40).map(|i| agent(&format!("a{i}"), AgentAuth::Ok)));
    host.answer(request_id, many).await;
    let got = call.await.unwrap();
    assert_eq!(got.agents.len(), hennery_proto::agents::MAX_AGENTS);
    let note = got.agents[0].note.clone().unwrap();
    assert!(note.len() <= hennery_proto::agents::MAX_AGENT_NOTE, "{}", note.len());
    assert!(note.starts_with("<script>\u{fffd}x"), "{note}");
    assert_eq!(got.agents[0].auth, AgentAuth::Ok, "the first of a name stands");
    assert_eq!(got.agents[1].agent, "a0");
}
