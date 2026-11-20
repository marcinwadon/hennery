//! The skills catalogue over HTTP (skills spec §5): upload behind step-up,
//! the refusals, host assignment with per-host delivery state, the hat's
//! candidate hosts and the `skills` feature flag.

use base64::Engine;
use flate2::{Compression, write::GzEncoder};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_kernel::skill_archive::MAX_COMPRESSED;
use hennery_kernel::skills::SkillHostReport;
use hennery_proto::frames::{AgentIsolation, Capabilities, Capability, CollectorFrame, SkillFailed, SkillFailure};
use hennery_proto::rest::{
    ApiError, CapabilitiesResponse, CreateSkillRequest, HatHostsResponse, SetSkillHostsRequest, SkillDetail,
    SkillHostStatus, SkillHostsResponse, SkillItem,
};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::PUBLIC_URL;
use std::net::SocketAddr;

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The owner's client, stepped up for five minutes.
    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    /// A request from a signed-in session whose step-up is too old.
    fn stale(&self, method: &str, path: &str) -> reqwest::RequestBuilder {
        self.client();
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        let token = self
            .state
            .operator
            .open_session("test", &phc, unix_now() - 301)
            .unwrap()
            .unwrap();
        reqwest::Client::new()
            .request(method.parse().unwrap(), self.url(path))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("__Host-hennery_session={token}"))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        self.client()
            .get(self.url(path))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn create(&self, archive: &[u8]) -> reqwest::Response {
        let body = serde_json::to_string(&CreateSkillRequest { data: b64(archive) }).unwrap();
        self.create_body(body).await
    }

    async fn create_body(&self, body: String) -> reqwest::Response {
        self.client()
            .post(self.url("/api/skills"))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap()
    }

    fn pair(&self, id: &str, key: &str, skills: bool) {
        let enrollment = Enrollment {
            public_key: key.into(),
            name: id.into(),
            host_version: "0".into(),
            platform: "linux".into(),
        };
        self.state.hosts.register(id, &enrollment, 1).unwrap();
        let capabilities = Capabilities(if skills { vec![Capability::Skills] } else { vec![] });
        self.state.hosts.record_hello(id, "0", &capabilities, 2).unwrap();
    }

    /// A ready hub connection for `id`, announcing `skills`; it lasts as
    /// long as the receiver.
    fn connect(&self, id: &str) -> tokio::sync::mpsc::UnboundedReceiver<CollectorFrame> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let registration = self
            .state
            .hub
            .register(
                id,
                tx,
                Capabilities(vec![Capability::Skills]),
                AgentIsolation::default(),
            )
            .unwrap();
        self.state.hub.mark_ready(id, registration.conn_id);
        rx
    }

    async fn set_hosts(&self, skill: &str, hosts: &[&str]) -> reqwest::Response {
        self.client()
            .put(self.url(&format!("/api/skills/{skill}/hosts")))
            .json(&SetSkillHostsRequest {
                host_ids: hosts.iter().map(|h| h.to_string()).collect(),
            })
            .send()
            .await
            .unwrap()
    }
}

const KEY_A: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const KEY_B: &str = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn tgz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        header.set_path(name).unwrap();
        header.set_cksum();
        builder.append(&header, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn pdf() -> Vec<u8> {
    tgz(&[("SKILL.md", b"---\nname: pdf\ndescription: Reads PDFs\n---\n# PDF")])
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

#[tokio::test]
async fn a_skill_is_created_listed_detailed_and_deleted_behind_step_up() {
    let c = Collector::start().await;
    let body = serde_json::to_string(&CreateSkillRequest { data: b64(&pdf()) }).unwrap();
    let resp = c
        .stale("POST", "/api/skills")
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "step_up_required".into()));

    let resp = c.create(&pdf()).await;
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.headers()["cache-control"], "private, no-store");
    let item: SkillItem = resp.json().await.unwrap();
    assert_eq!(item.name, "pdf");
    assert_eq!(item.description, "Reads PDFs");
    assert!(item.host_ids.is_empty());

    let list: Vec<SkillItem> = c.get("/api/skills").await;
    assert_eq!(list, std::slice::from_ref(&item));

    let path = format!("/api/skills/{}", item.id);
    let detail: SkillDetail = c.get(&path).await;
    assert_eq!(detail.item, item);
    assert!(detail.skill_md.contains("# PDF"));
    let resp = c.client().get(c.url("/api/skills/nope")).send().await.unwrap();
    assert_eq!(code_of(resp).await, (404, "skill_not_found".into()));

    let resp = c.stale("DELETE", &path).send().await.unwrap();
    assert_eq!(code_of(resp).await, (403, "step_up_required".into()));
    assert_eq!(c.client().delete(c.url(&path)).send().await.unwrap().status(), 204);
    let list: Vec<SkillItem> = c.get("/api/skills").await;
    assert!(list.is_empty());
}

#[tokio::test]
async fn a_duplicate_name_is_refused() {
    let c = Collector::start().await;
    assert_eq!(c.create(&pdf()).await.status(), 201);
    assert_eq!(code_of(c.create(&pdf()).await).await, (409, "skill_exists".into()));
}

#[tokio::test]
async fn an_invalid_archive_names_its_problem() {
    let c = Collector::start().await;
    let resp = c.create(&tgz(&[("readme.txt", b"hi")])).await;
    assert_eq!(resp.status(), 400);
    let error: ApiError = resp.json().await.unwrap();
    assert_eq!(error.code, "invalid_skill");
    assert!(error.message.contains("SKILL.md"), "{}", error.message);
    // The most common cause, a skill wrapped in its own directory, is named.
    let resp = c
        .create(&tgz(&[("pdf/SKILL.md", b"---\nname: pdf\ndescription: d\n---")]))
        .await;
    let error: ApiError = resp.json().await.unwrap();
    assert!(error.message.contains("top-level directory"), "{}", error.message);
    assert!(error.message.contains("tar czf pdf.tgz -C pdf ."), "{}", error.message);
}

#[tokio::test]
async fn an_oversized_upload_is_refused_before_decoding() {
    let c = Collector::start().await;
    let body = format!("{{\"data\":\"{}\"}}", "A".repeat(MAX_COMPRESSED * 2));
    assert_eq!(c.create_body(body).await.status(), 413);
}

#[tokio::test]
async fn hosts_are_assigned_and_reported() {
    let c = Collector::start().await;
    c.pair("host-a", KEY_A, true);
    c.pair("host-b", KEY_B, false);
    let _host_a = c.connect("host-a");
    let item: SkillItem = c.create(&pdf()).await.json().await.unwrap();

    let resp = c.set_hosts(&item.id, &["nope"]).await;
    assert_eq!(code_of(resp).await, (400, "unknown_host".into()));

    let resp = c.set_hosts(&item.id, &["host-a", "host-b"]).await;
    assert_eq!(resp.status(), 200);
    let put: SkillHostsResponse = resp.json().await.unwrap();
    let statuses: Vec<_> = put.hosts.iter().map(|h| (h.host_id.as_str(), h.status)).collect();
    assert_eq!(
        statuses,
        [
            ("host-a", SkillHostStatus::Unknown),
            ("host-b", SkillHostStatus::Unsupported)
        ]
    );
    let path = format!("/api/skills/{}/hosts", item.id);
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got, put);
    assert_eq!(got.skill_id, item.id);

    // A report that lists the skill makes it applied; one that omits it
    // leaves it pending.
    let revision = c.state.hosts.skills_revision().unwrap();
    let report = |applied: Vec<String>| SkillHostReport {
        revision,
        applied,
        failed: vec![],
        reported_at: 5,
    };
    c.state
        .hosts
        .record_skill_state("host-a", &report(vec![item.id.clone()]))
        .unwrap();
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Applied);
    assert_eq!(got.hosts[0].reported_at, Some(5));
    c.state.hosts.record_skill_state("host-a", &report(vec![])).unwrap();
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Pending);
}

/// A report for an older revision says nothing about the current content
/// (final review, finding 5): after a Replace it is `pending`, not
/// `applied` or `failed`.
#[tokio::test]
async fn a_report_of_an_older_revision_is_pending() {
    let c = Collector::start().await;
    c.pair("host-a", KEY_A, true);
    let _host_a = c.connect("host-a");
    let item: SkillItem = c.create(&pdf()).await.json().await.unwrap();
    assert_eq!(c.set_hosts(&item.id, &["host-a"]).await.status(), 200);
    let path = format!("/api/skills/{}/hosts", item.id);
    let old = c.state.hosts.skills_revision().unwrap();
    let applied = SkillHostReport {
        revision: old,
        applied: vec![item.id.clone()],
        failed: vec![],
        reported_at: 5,
    };
    c.state.hosts.record_skill_state("host-a", &applied).unwrap();
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Applied);

    // Another skill is created: the revision moves on, the report does not.
    let other = tgz(&[("SKILL.md", b"---\nname: docx\ndescription: d\n---")]);
    assert_eq!(c.create(&other).await.status(), 201);
    assert!(c.state.hosts.skills_revision().unwrap() > old);
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Pending);
    assert_eq!(got.hosts[0].reported_at, Some(5));

    let failed = SkillHostReport {
        applied: vec![],
        failed: vec![SkillFailed {
            id: item.id.clone(),
            reason: SkillFailure::Io,
            agent: None,
        }],
        ..applied
    };
    c.state.hosts.record_skill_state("host-a", &failed).unwrap();
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Pending);
    assert_eq!(got.hosts[0].failure, None);
}

/// Revoking a host drops its assignments and bumps the revision; the
/// connected hosts get the new revision at once, or their states would read
/// `pending` until the next skills write (re-review of the fix wave).
#[tokio::test]
async fn revoking_a_host_sends_the_others_the_new_revision() {
    let c = Collector::start().await;
    c.pair("host-a", KEY_A, true);
    c.pair("host-b", KEY_B, true);
    let mut host_a = c.connect("host-a");
    let item: SkillItem = c.create(&pdf()).await.json().await.unwrap();
    assert_eq!(c.set_hosts(&item.id, &["host-a", "host-b"]).await.status(), 200);
    while host_a.try_recv().is_ok() {}
    let before = c.state.hosts.skills_revision().unwrap();

    let resp = c.client().delete(c.url("/api/hosts/host-b")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let revision = c.state.hosts.skills_revision().unwrap();
    assert!(revision > before);
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), host_a.recv())
        .await
        .expect("a frame within 5s")
        .expect("the connection is open");
    let CollectorFrame::SkillSync { revision: got, items } = frame else {
        panic!("expected skill_sync, got {frame:?}");
    };
    assert_eq!(got, revision);
    assert_eq!(items.len(), 1);
}

/// A host with no ready connection is `offline`, whatever it last reported.
#[tokio::test]
async fn a_disconnected_host_is_offline() {
    let c = Collector::start().await;
    c.pair("host-a", KEY_A, true);
    let item: SkillItem = c.create(&pdf()).await.json().await.unwrap();
    assert_eq!(c.set_hosts(&item.id, &["host-a"]).await.status(), 200);
    let report = SkillHostReport {
        revision: c.state.hosts.skills_revision().unwrap(),
        applied: vec![item.id.clone()],
        failed: vec![],
        reported_at: 5,
    };
    c.state.hosts.record_skill_state("host-a", &report).unwrap();
    let path = format!("/api/skills/{}/hosts", item.id);
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Offline);
    let host_a = c.connect("host-a");
    let got: SkillHostsResponse = c.get(&path).await;
    assert_eq!(got.hosts[0].status, SkillHostStatus::Applied);
    drop(host_a);
}

#[tokio::test]
async fn hat_hosts_lists_default_hat_hosts() {
    let c = Collector::start().await;
    c.pair("host-a", KEY_A, true);
    let hat = c.state.hosts.host("host-a").unwrap().unwrap().default_hat_id;
    let got: HatHostsResponse = c.get(&format!("/api/skills/hat-hosts/{hat}")).await;
    assert_eq!(got.host_ids, ["host-a"]);
}

#[tokio::test]
async fn capabilities_advertise_skills() {
    let c = Collector::start().await;
    let got: CapabilitiesResponse = c.get("/api/capabilities").await;
    assert!(got.features.iter().any(|f| f == "skills"));
}
