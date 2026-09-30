//! Host pairing over HTTP (kernel spec §4.1, §11): minting a code needs the
//! operator, enrollment needs only the code, and wrong codes lock the
//! client's address out without spoiling the codes that are still valid.

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
use hennery_proto::rest::{ApiError, EnrollRequest, EnrollResponse, PairingCodeResponse};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const TOKEN: &str = "dev-token-for-tests";
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
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn mint(&self) -> String {
        let resp = reqwest::Client::new()
            .post(self.url("/api/hosts/pairing-codes"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        resp.json::<PairingCodeResponse>().await.unwrap().code
    }

    async fn enroll(&self, code: &str, key: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url("/api/hosts/enroll"))
            .json(&EnrollRequest {
                code: code.into(),
                public_key: key.into(),
                name: "laptop".into(),
                host_version: "0.0.0".into(),
                platform: "linux-x86_64".into(),
            })
            .send()
            .await
            .unwrap()
    }

    /// Like `enroll`, but with a caller-supplied `X-Forwarded-For`: the
    /// limiter must key on the real peer address, not this header — a proxy
    /// header is forgeable, and trusting it would let a single real address
    /// spend a fresh budget on every request just by changing it.
    async fn enroll_claiming(&self, code: &str, key: &str, forwarded_for: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url("/api/hosts/enroll"))
            .header("x-forwarded-for", forwarded_for)
            .json(&EnrollRequest {
                code: code.into(),
                public_key: key.into(),
                name: "laptop".into(),
                host_version: "0.0.0".into(),
                platform: "linux-x86_64".into(),
            })
            .send()
            .await
            .unwrap()
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

#[tokio::test]
async fn a_minted_code_pairs_a_host_without_the_operator_bearer_once() {
    let collector = Collector::start().await;
    let code = collector.mint().await;
    let resp = collector.enroll(&code, KEYS[0]).await;
    assert_eq!(resp.status(), 201);
    let host_id = resp.json::<EnrollResponse>().await.unwrap().host_id;
    let record = collector.state.hosts.host(&host_id).unwrap().expect("host stored");
    assert_eq!(
        (record.name.as_str(), record.platform.as_str()),
        ("laptop", "linux-x86_64")
    );

    assert_eq!(
        code_of(collector.enroll(&code, KEYS[1]).await).await,
        (401, "invalid_code".into())
    );
}

#[tokio::test]
async fn minting_needs_the_operator_and_enrollment_does_not() {
    let collector = Collector::start().await;
    let mint = reqwest::Client::new()
        .post(collector.url("/api/hosts/pairing-codes"))
        .send()
        .await
        .unwrap();
    assert_eq!(mint.status(), 401);
    // Enrollment is reached without a bearer: the 401 is the code's, with
    // an API error body, not the bearer layer's.
    assert_eq!(
        code_of(collector.enroll("0000-0000", KEYS[0]).await).await,
        (401, "invalid_code".into())
    );
}

#[tokio::test]
async fn wrong_codes_lock_the_address_out_but_leave_valid_codes_unspent() {
    let collector = Collector::start().await;
    let code = collector.mint().await;
    for _ in 0..4 {
        assert_eq!(
            code_of(collector.enroll("0000-0000", KEYS[0]).await).await,
            (401, "invalid_code".into())
        );
    }
    // Four wrong codes are free, and a right one clears them.
    assert_eq!(collector.enroll(&code, KEYS[0]).await.status(), 201);

    let spare = collector.mint().await;
    for _ in 0..5 {
        assert_eq!(collector.enroll("0000-0000", KEYS[1]).await.status(), 401);
    }
    // Locked out: even the right code is not looked at.
    let locked = collector.enroll(&spare, KEYS[1]).await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(locked.json::<ApiError>().await.unwrap().code, "rate_limited");
    // The spare code was never spent: it still pairs a host.
    let enrollment = Enrollment {
        public_key: KEYS[1].into(),
        name: "desk".into(),
        host_version: "0.0.0".into(),
        platform: "linux-x86_64".into(),
    };
    assert!(matches!(
        collector
            .state
            .hosts
            .enroll(&spare, &enrollment, hennery_kernel::secret::unix_now())
            .unwrap(),
        EnrollOutcome::Enrolled { .. }
    ));
}

#[tokio::test]
async fn a_malformed_or_already_paired_enrollment_is_refused_and_keeps_the_code() {
    let collector = Collector::start().await;
    let first = collector.mint().await;
    assert_eq!(collector.enroll(&first, KEYS[0]).await.status(), 201);

    let code = collector.mint().await;
    assert_eq!(
        code_of(collector.enroll(&code, "not-a-key").await).await,
        (400, "invalid".into())
    );
    assert_eq!(
        code_of(collector.enroll(&code, KEYS[0]).await).await,
        (409, "already_paired".into())
    );
    assert_eq!(collector.enroll(&code, KEYS[2]).await.status(), 201);
    assert_eq!(collector.state.hosts.list().unwrap().len(), 2);
}

#[tokio::test]
async fn a_spoofed_x_forwarded_for_does_not_evade_the_rate_limit() {
    let collector = Collector::start().await;
    // Every one of the 5 free attempts claims a different address; since
    // the real peer is the same loopback socket every time, they all count
    // against the same limiter entry, and the 6th is still locked out.
    for n in 0..5 {
        assert_eq!(
            collector
                .enroll_claiming("0000-0000", KEYS[0], &format!("203.0.113.{n}"))
                .await
                .status(),
            401
        );
    }
    let locked = collector.enroll_claiming("0000-0000", KEYS[0], "203.0.113.99").await;
    assert_eq!(locked.status(), 429);
    assert!(locked.headers().contains_key("retry-after"));
}
