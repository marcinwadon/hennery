//! Hats over HTTP (kernel spec §5, §8): setup names the default hat, the
//! owner adds and changes hats, gives a host another default hat, and
//! replaces a host's path rules. Changing a host or its rules needs a
//! fresh step-up (plan 5a decisions 7 and 8; `step_up.rs` covers the
//! refusals).

use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{
    ApiError, CreateHatRequest, HatItem, HostItem, PathRuleInput, PathRuleItem, PathRulesRequest, SetupRequest,
    UpdateHatRequest, UpdateHostRequest,
};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use std::net::SocketAddr;

/// Valid Ed25519 public keys (RFC 8032 §7.1, tests 1 to 3).
const KEYS: [&str; 3] = [
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
];

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    /// A collector on one database file, so the operator, the registry and
    /// the store see the same hats. Not set up.
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

    /// The owner's client: set up if need be, stepped up for five minutes.
    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    /// Pair a host under `host_id` with the `n`th of `KEYS`.
    fn pair(&self, host_id: &str, n: usize) {
        let enrollment = Enrollment {
            public_key: KEYS[n].into(),
            name: "laptop".into(),
            host_version: "0.0.0".into(),
            platform: "linux-x86_64".into(),
        };
        self.state.hosts.register(host_id, &enrollment, unix_now()).unwrap();
    }

    async fn hats(&self) -> Vec<HatItem> {
        let resp = self.client().get(self.url("/api/hats")).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        resp.json().await.unwrap()
    }

    async fn create(&self, name: &str, colour: Option<&str>) -> reqwest::Response {
        self.client()
            .post(self.url("/api/hats"))
            .json(&CreateHatRequest {
                name: name.into(),
                colour: colour.map(Into::into),
            })
            .send()
            .await
            .unwrap()
    }

    async fn put_rules(&self, host_id: &str, rules: &[(&str, &str)]) -> reqwest::Response {
        let rules = rules
            .iter()
            .map(|(prefix, hat_id)| PathRuleInput {
                prefix: (*prefix).into(),
                hat_id: (*hat_id).into(),
            })
            .collect();
        self.client()
            .put(self.url(&format!("/api/hosts/{host_id}/path-rules")))
            .json(&PathRulesRequest { rules })
            .send()
            .await
            .unwrap()
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

/// Kernel spec §3.1: the setup form names the default hat, which the host
/// `up` paired before setup already has.
#[tokio::test]
async fn setup_names_the_default_hat() {
    let c = Collector::start().await;
    c.pair("host-up", 0);
    let token = c.state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
    let resp = reqwest::Client::new()
        .post(c.url("/api/setup"))
        .header("origin", PUBLIC_URL)
        .json(&SetupRequest {
            token,
            password: OWNER_PASSWORD.into(),
            public_url: PUBLIC_URL.into(),
            default_hat_name: Some("Acme".into()),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let hats = c.hats().await;
    assert_eq!(hats.len(), 1, "{hats:?}");
    assert_eq!((hats[0].name.as_str(), hats[0].default_for_new_hosts), ("Acme", true));
    let resp = c.client().get(c.url("/api/hosts")).send().await.unwrap();
    let hosts: Vec<HostItem> = resp.json().await.unwrap();
    assert_eq!(hosts[0].default_hat_id, hats[0].id);
}

#[tokio::test]
async fn hats_are_listed_created_and_changed() {
    let c = Collector::start().await;
    let hats = c.hats().await;
    assert_eq!(hats.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["Personal"]);
    let resp = c.create("Acme", Some("#FF0000")).await;
    assert_eq!(resp.status(), 201);
    let acme: HatItem = resp.json().await.unwrap();
    assert_eq!(
        (acme.name.as_str(), acme.colour.as_str(), acme.default_for_new_hosts),
        ("Acme", "#ff0000", false)
    );
    assert_eq!(code_of(c.create("acme", None).await).await, (409, "name_taken".into()));
    assert_eq!(
        code_of(c.create("Other", Some("red; x")).await).await,
        (400, "invalid".into())
    );

    let patch = |id: &str, req: UpdateHatRequest| c.client().patch(c.url(&format!("/api/hats/{id}"))).json(&req).send();
    let resp = patch(
        &acme.id,
        UpdateHatRequest {
            name: Some("Acme Corp".into()),
            colour: None,
            default_for_new_hosts: Some(true),
        },
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let changed: HatItem = resp.json().await.unwrap();
    assert_eq!(
        (
            changed.name.as_str(),
            changed.colour.as_str(),
            changed.default_for_new_hosts
        ),
        ("Acme Corp", "#ff0000", true)
    );
    // One default: the other hat is not it any more. (Both were made in the
    // same second, so their order is the ids'.)
    let mut flags: Vec<(String, bool)> = c
        .hats()
        .await
        .into_iter()
        .map(|h| (h.name, h.default_for_new_hosts))
        .collect();
    flags.sort();
    assert_eq!(flags, [("Acme Corp".into(), true), ("Personal".into(), false)]);
    let resp = patch(
        "hat-nope",
        UpdateHatRequest {
            name: Some("x".into()),
            colour: None,
            default_for_new_hosts: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
    // A hat newly paired hosts get.
    c.pair("host-new", 1);
    assert_eq!(c.state.hosts.host("host-new").unwrap().unwrap().default_hat_id, acme.id);
}

#[tokio::test]
async fn a_host_is_renamed_and_given_another_default_hat() {
    let c = Collector::start().await;
    c.pair("host-1", 0);
    let acme: HatItem = c.create("Acme", None).await.json().await.unwrap();
    let patch = |req: UpdateHostRequest| c.client().patch(c.url("/api/hosts/host-1")).json(&req).send();
    let resp = patch(UpdateHostRequest {
        name: Some("work".into()),
        default_hat_id: Some(acme.id.clone()),
    })
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let host: HostItem = resp.json().await.unwrap();
    assert_eq!(
        (host.name.as_str(), host.default_hat_id.as_str()),
        ("work", acme.id.as_str())
    );
    let resp = patch(UpdateHostRequest {
        name: None,
        default_hat_id: Some("hat-nope".into()),
    })
    .await
    .unwrap();
    assert_eq!(code_of(resp).await, (400, "invalid".into()));
    let resp = c
        .client()
        .patch(c.url("/api/hosts/host-nope"))
        .json(&UpdateHostRequest {
            name: Some("x".into()),
            default_hat_id: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
}

/// Kernel spec §5.2, §8: the full set replaces the host's rules; each
/// prefix is normalised by its text and stored unverified until the host
/// can resolve it (plan 5a decision 6).
#[tokio::test]
async fn path_rules_are_replaced_as_a_set_normalised_and_unverified() {
    let c = Collector::start().await;
    c.pair("host-1", 0);
    let acme: HatItem = c.create("Acme", None).await.json().await.unwrap();
    let resp = c
        .put_rules("host-1", &[("/p/acme/", &acme.id), ("/p//acme/./secret", &acme.id)])
        .await;
    assert_eq!(resp.status(), 200);
    let stored: Vec<PathRuleItem> = resp.json().await.unwrap();
    let shown: Vec<(&str, bool)> = stored.iter().map(|r| (r.prefix.as_str(), r.verified)).collect();
    assert_eq!(shown, [("/p/acme/secret", false), ("/p/acme", false)]);
    let resp = c
        .client()
        .get(c.url("/api/hosts/host-1/path-rules"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.json::<Vec<PathRuleItem>>().await.unwrap(), stored);
    let resolved = c.state.hosts.resolve_hat("host-1", "/p/acme/x").unwrap().unwrap();
    assert_eq!(resolved.hat_id, acme.id);

    // A bad set changes nothing.
    for bad in [
        vec![("p/acme", acme.id.as_str())],
        vec![("~/acme", acme.id.as_str())],
        vec![("/p/acme", "hat-nope")],
        vec![("/p/a", acme.id.as_str()), ("/p/a/", acme.id.as_str())],
        vec![("/p/a\u{0}b", acme.id.as_str())],
        vec![("/p/x/../a", acme.id.as_str())],
        vec![("/", acme.id.as_str())],
    ] {
        assert_eq!(
            code_of(c.put_rules("host-1", &bad).await).await,
            (400, "invalid".into()),
            "{bad:?}"
        );
    }
    assert_eq!(c.state.hosts.path_rules("host-1").unwrap().unwrap().len(), 2);
    assert_eq!(
        code_of(c.put_rules("host-nope", &[]).await).await,
        (404, "not_found".into())
    );
    let resp = c
        .client()
        .get(c.url("/api/hosts/host-nope/path-rules"))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (404, "not_found".into()));
    // The empty set removes them all.
    let resp = c.put_rules("host-1", &[]).await;
    assert_eq!(resp.json::<Vec<PathRuleItem>>().await.unwrap(), []);
}

/// Every hat route needs the operator's session.
#[tokio::test]
async fn hat_routes_need_the_operator() {
    let c = Collector::start().await;
    c.client();
    for (method, path) in [
        ("GET", "/api/hats"),
        ("POST", "/api/hats"),
        ("PATCH", "/api/hats/hat-1"),
        ("GET", "/api/hosts/host-1/path-rules"),
        ("PUT", "/api/hosts/host-1/path-rules"),
        ("PATCH", "/api/hosts/host-1"),
    ] {
        let resp = reqwest::Client::new()
            .request(method.parse().unwrap(), c.url(path))
            .header("origin", PUBLIC_URL)
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(resp).await, (401, "unauthenticated".into()), "{method} {path}");
    }
}
