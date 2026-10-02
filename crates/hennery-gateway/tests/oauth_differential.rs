//! Differential tests for plan 8f's checks on parsed input (lane L16, the
//! fleet rule of 2026-10-02): the authorization server's metadata, the
//! protected-resource document, a registration's answer, a token response
//! and the callback's query. Each vector goes through the real gateway,
//! against the fake authorization server; what the gateway did with it is
//! then held against what each modelled decoder reads in the same bytes:
//! either the gateway refused the input, or every decoder reads in it
//! exactly what the gateway used. Nothing a decoder could read otherwise is
//! ever acted on.
//!
//! The JSON decoders are plan 2026-10-15's six, as #93's
//! `tests/differential.rs` measured them (`Exact`, `First`, `GoV1`,
//! `GoV2Fold`, `JsonC`, `CJson`), from the shared test-support module
//! `support::differential`. The query decoders are the four ways a query
//! string is read: the first or the last of a name, a name matched whatever
//! its case, and a value cut at its first NUL.
//!
//! Each table is one line per vector; a failure names every vector that
//! failed, not only the first.

mod support;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::model::{CredKind, NewConnection};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use serde_json::Value;
use std::sync::Arc;
use support::differential::{DECODERS, Decoder, Node};
use support::oauth::{Config, FakeAs, PrAt};
use support::{Recorder, World};
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";

/// A string cut at its first NUL, as a C reader of the query hands it on.
fn cut_at_nul(s: &str) -> &str {
    s.split('\0').next().unwrap_or_default()
}

/// A field as `decoder` hands it on: a string (cut at NUL where it cuts), a
/// number, or a list of strings, as text.
fn field(decoder: Decoder, node: &Node, name: &str) -> Option<String> {
    let found = decoder.get(node, name);
    match found? {
        Node::Str(_) => decoder.str(found),
        Node::Num(n) => Some(n.to_string()),
        Node::Arr(items) => Some(
            items
                .iter()
                .map(|item| match item {
                    Node::Str(s) => s.clone(),
                    _ => "?".into(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// What every decoder reads of `fields` in `bytes`, if all read the same;
/// `Err` names the first field two of them read apart. Bytes no strict
/// decoder reads are `Err` too: a lenient one might.
fn agreed(bytes: &[u8], fields: &[&str]) -> Result<Vec<Option<String>>, String> {
    let root: Node = serde_json::from_slice(bytes).map_err(|e| format!("no strict decoder reads it ({e})"))?;
    let exact: Vec<Option<String>> = fields.iter().map(|f| field(Decoder::Exact, &root, f)).collect();
    for &d in DECODERS {
        for (i, f) in fields.iter().enumerate() {
            let read = field(d, &root, f);
            if read != exact[i] {
                return Err(format!("{d:?} reads {f} as {read:?}, Exact as {:?}", exact[i]));
            }
        }
    }
    Ok(exact)
}

// --- The gateway, in-process -----------------------------------------------

struct Harness {
    world: World,
    app: axum::Router,
    operator: Arc<Operator>,
    phc: String,
}

impl Harness {
    fn new() -> Self {
        let world = World::new();
        let operator = Arc::new(Operator::open(&world.db).unwrap());
        let now = unix_now();
        let setup = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap()
        else {
            panic!("setup failed");
        };
        let runtime = world.runtime(support::test_egress(), Arc::new(Recorder::default()));
        let app = router(GatewayState {
            runtime,
            operator: operator.clone(),
        });
        Self {
            world,
            app,
            operator,
            phc,
        }
    }

    fn connection(&self, url: &str) -> String {
        self.world.connection_with(NewConnection {
            slug: "linear".into(),
            label: "Linear".into(),
            url: url.into(),
            hat_id: self.world.hat(),
            cred_kind: CredKind::OauthDcr,
            static_header: None,
            static_prefix: None,
            tool_allowlist: None,
            internal_network: true,
        })
    }

    async fn authorize(&self, id: &str) -> (StatusCode, HeaderMap, Value) {
        let session = self
            .operator
            .open_session("test", &self.phc, unix_now())
            .unwrap()
            .unwrap();
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/mcp/connections/{id}/authorize"))
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn callback(&self, path_and_query: &str, cookie: &str) -> String {
        let req = Request::builder()
            .uri(path_and_query)
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        let at = html.find("data-result=\"").unwrap() + "data-result=\"".len();
        html[at..].split('"').next().unwrap().to_string()
    }
}

fn flow_cookie(headers: &HeaderMap) -> String {
    headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// Whether a vector is to be taken (every decoder reads it alike) or
/// refused (some decoder reads it otherwise, or it is not one document).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Taken,
    Refused,
}

/// The authorization server's metadata the gateway trusts.
const SERVER_FIELDS: &[&str] = &[
    "issuer",
    "authorization_endpoint",
    "token_endpoint",
    "registration_endpoint",
    "code_challenge_methods_supported",
];

/// The metadata vectors: `{o}` is the fake's origin.
fn server_vectors() -> Vec<(&'static str, Expect, String)> {
    let base = r#""issuer":"{o}","authorization_endpoint":"{o}/authorize","token_endpoint":"{o}/token","registration_endpoint":"{o}/register","code_challenge_methods_supported":["S256"]"#;
    let doc = |extra: &str| format!("{{{base}{extra}}}");
    vec![
        ("the baseline", Expect::Taken, doc("")),
        (
            "a key twice",
            Expect::Refused,
            doc(r#","token_endpoint":"https://evil.example/token""#),
        ),
        (
            "a key twice, first",
            Expect::Refused,
            format!(r#"{{"token_endpoint":"https://evil.example/token",{base}}}"#),
        ),
        (
            "an escaped key twice",
            Expect::Refused,
            doc(r#","token_endpoint":"https://evil.example/token""#),
        ),
        (
            "a key in another case",
            Expect::Refused,
            doc(r#","TOKEN_ENDPOINT":"https://evil.example/token""#),
        ),
        (
            "a key with ſ for s",
            Expect::Refused,
            doc(r#","iſſuer":"https://evil.example""#),
        ),
        (
            "a key with the Kelvin sign for k",
            Expect::Refused,
            doc(r#","toKen_endpoint":"https://evil.example/token""#),
        ),
        (
            "a key without its _",
            Expect::Refused,
            doc(r#","tokenendpoint":"https://evil.example/token""#),
        ),
        (
            "a key with - for _",
            Expect::Refused,
            doc(r#","token-endpoint":"https://evil.example/token""#),
        ),
        (
            "a key with a NUL",
            Expect::Refused,
            doc(r#","token_endpoint\u0000x":"https://evil.example/token""#),
        ),
        (
            "a NUL in a value",
            Expect::Refused,
            base.replace("{o}/token\"", "{o}/token\\u0000.evil\"")
                .pipe(|b| format!("{{{b}}}")),
        ),
        (
            "a NUL in a list",
            Expect::Refused,
            base.replace("[\"S256\"]", "[\"S256\\u0000x\"]")
                .pipe(|b| format!("{{{b}}}")),
        ),
        ("a byte-order mark", Expect::Refused, format!("\u{feff}{{{base}}}")),
        ("odd whitespace", Expect::Refused, format!("{{\u{a0}{base}}}")),
        (
            "a key with a trailing space",
            Expect::Taken,
            doc(r#","token_endpoint ":"https://evil.example/token""#),
        ),
        (
            "a key with a zero-width space",
            Expect::Taken,
            doc(r#","token_endpoint\u200b":"https://evil.example/token""#),
        ),
    ]
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl Pipe for String {}

/// Lane L16: the authorization server's metadata is taken only when every
/// decoder reads in it the issuer, the endpoints and PKCE the gateway
/// used.
#[tokio::test]
async fn server_metadata_is_taken_only_as_every_decoder_reads_it() {
    let mut failures = Vec::new();
    for (what, expect, template) in server_vectors() {
        let h = Harness::new();
        let fake = FakeAs::start(Config::default()).await;
        let document = template.replace("{o}", &fake.origin());
        fake.configure(|c| c.raw_meta = Some(document.clone()));
        let id = h.connection(&fake.mcp_url());
        let (status, _, answer) = h.authorize(&id).await;
        let taken = status == StatusCode::OK;
        let outcome = if taken { Expect::Taken } else { Expect::Refused };
        if outcome != expect {
            failures.push(format!("{what}: {outcome:?}, expected {expect:?} ({answer})"));
            continue;
        }
        if taken {
            match agreed(document.as_bytes(), SERVER_FIELDS) {
                Err(why) => failures.push(format!("{what}: taken, but {why}")),
                Ok(read) => {
                    let consent = answer["consent_url"].as_str().unwrap();
                    let authorize = read[1].as_deref().unwrap();
                    if !consent.starts_with(&format!("{authorize}?")) {
                        failures.push(format!("{what}: consent at {consent}, the decoders read {authorize}"));
                    }
                }
            }
            if fake.paths().iter().any(|p| p.contains("evil")) {
                failures.push(format!("{what}: something went to evil"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Lane L16: the protected-resource document is taken only when every
/// decoder reads its `resource` and servers as the gateway did.
#[tokio::test]
async fn a_resource_document_is_taken_only_as_every_decoder_reads_it() {
    // The server named is the fake at `/tenant`, so a document passed over
    // (the resource's origin taken instead, which serves no metadata) is
    // told from one used.
    let base = r#""resource":"{r}","authorization_servers":["{o}/tenant"]"#;
    let doc = |extra: &str| format!("{{{base}{extra}}}");
    let vectors = vec![
        ("the baseline", Expect::Taken, doc("")),
        (
            "servers twice",
            Expect::Refused,
            doc(r#","authorization_servers":["https://evil.example"]"#),
        ),
        (
            "resource twice, escaped",
            Expect::Refused,
            doc(r#","resource":"https://evil.example/mcp""#),
        ),
        (
            "servers in another case",
            Expect::Refused,
            doc(r#","Authorization_Servers":["https://evil.example"]"#),
        ),
        (
            "servers with a NUL key",
            Expect::Refused,
            doc(r#","authorization_servers\u0000":["https://evil.example"]"#),
        ),
        (
            "a NUL in a server",
            Expect::Refused,
            base.replace("[\"{o}/tenant\"]", "[\"{o}/tenant\\u0000.evil\"]")
                .pipe(|b| format!("{{{b}}}")),
        ),
    ];
    let mut failures = Vec::new();
    for (what, expect, template) in vectors {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            pr_at: PrAt::Origin,
            as_path: "/tenant".into(),
            ..Config::default()
        })
        .await;
        let document = template.replace("{r}", &fake.mcp_url()).replace("{o}", &fake.origin());
        fake.configure(|c| c.raw_pr = Some(document.clone()));
        let id = h.connection(&fake.mcp_url());
        let (status, _, answer) = h.authorize(&id).await;
        let outcome = match (status, answer["code"].as_str()) {
            (StatusCode::OK, _) => Expect::Taken,
            (_, Some("discovery_failed")) => Expect::Refused,
            _ => {
                failures.push(format!("{what}: {status} {answer}"));
                continue;
            }
        };
        if fake.paths().iter().any(|p| p.contains("evil")) {
            failures.push(format!("{what}: something went to evil"));
        }
        if outcome != expect {
            failures.push(format!("{what}: {outcome:?}, expected {expect:?}"));
        }
        if expect == Expect::Taken
            && let Err(why) = agreed(document.as_bytes(), &["resource", "authorization_servers"])
        {
            failures.push(format!("{what}: taken, but {why}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Lane L16: a token response is stored only when every decoder reads its
/// tokens, type and lifetime as the gateway did.
#[tokio::test]
async fn a_token_response_is_taken_only_as_every_decoder_reads_it() {
    let vectors = vec![
        (
            "the baseline",
            Expect::Taken,
            r#"{"access_token":"good-1","token_type":"Bearer","expires_in":3600,"refresh_token":"r-1"}"#,
        ),
        (
            "a token twice",
            Expect::Refused,
            r#"{"access_token":"good-1","access_token":"evil-1"}"#,
        ),
        (
            "a token twice, escaped",
            Expect::Refused,
            r#"{"access_token":"good-1","access_token":"evil-1"}"#,
        ),
        (
            "a token in another case",
            Expect::Refused,
            r#"{"access_token":"good-1","ACCESS_TOKEN":"evil-1"}"#,
        ),
        (
            "a token without its _",
            Expect::Refused,
            r#"{"access_token":"good-1","accesstoken":"evil-1"}"#,
        ),
        (
            "a refresh token with the Kelvin sign",
            Expect::Refused,
            r#"{"access_token":"good-1","refresh_toKen":"evil-r"}"#,
        ),
        (
            "a NUL in a key",
            Expect::Refused,
            r#"{"access_token":"good-1","access_token\u0000":"evil-1"}"#,
        ),
        (
            "a NUL in a token",
            Expect::Refused,
            r#"{"access_token":"good\u0000evil"}"#,
        ),
        (
            "expires_in twice",
            Expect::Refused,
            r#"{"access_token":"good-1","expires_in":3600,"expires_in":1}"#,
        ),
        (
            "a type twice",
            Expect::Refused,
            r#"{"access_token":"good-1","token_type":"Bearer","token_type":"mac"}"#,
        ),
        (
            "a byte-order mark",
            Expect::Refused,
            "\u{feff}{\"access_token\":\"good-1\"}",
        ),
        (
            "a key with a trailing space",
            Expect::Taken,
            r#"{"access_token":"good-1","access_token ":"evil-1"}"#,
        ),
    ];
    let mut failures = Vec::new();
    for (what, expect, raw) in vectors {
        let h = Harness::new();
        let fake = FakeAs::start(Config {
            raw_token: Some(raw.into()),
            ..Config::default()
        })
        .await;
        let id = h.connection(&fake.mcp_url());
        let (_, headers, answer) = h.authorize(&id).await;
        let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
        let result = h
            .callback(location.strip_prefix(ORIGIN).unwrap(), &flow_cookie(&headers))
            .await;
        let outcome = if result == "connected" {
            Expect::Taken
        } else {
            Expect::Refused
        };
        if outcome != expect {
            failures.push(format!("{what}: {result}, expected {expect:?}"));
            continue;
        }
        if outcome == Expect::Taken {
            let fields = ["access_token", "refresh_token", "token_type", "expires_in"];
            match agreed(raw.as_bytes(), &fields) {
                Err(why) => failures.push(format!("{what}: taken, but {why}")),
                Ok(read) => {
                    let grant = h.world.store.oauth_credential(&id, &h.world.key).unwrap().unwrap();
                    if Some(grant.tokens.access_token.to_string()) != read[0] {
                        failures.push(format!("{what}: stored another access token than the decoders read"));
                    }
                    if grant.tokens.refresh_token.as_ref().map(|t| t.to_string()) != read[1] {
                        failures.push(format!("{what}: stored another refresh token than the decoders read"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Lane L16: a registration's answer is taken only when every decoder
/// reads its client id as the gateway did.
#[tokio::test]
async fn a_registration_is_taken_only_as_every_decoder_reads_it() {
    // The fake's own registration, as raw answers it cannot build: through
    // the authorization server's metadata pointing at a raw document.
    let vectors = vec![
        ("the baseline", Expect::Taken, r#"{"client_id":"pre-good"}"#),
        (
            "an id twice",
            Expect::Refused,
            r#"{"client_id":"pre-good","client_id":"pre-evil"}"#,
        ),
        (
            "an id in another case",
            Expect::Refused,
            r#"{"client_id":"pre-good","Client_Id":"pre-evil"}"#,
        ),
        ("a NUL in an id", Expect::Refused, r#"{"client_id":"pre-good\u0000x"}"#),
        (
            "a secret twice",
            Expect::Refused,
            r#"{"client_id":"pre-good","client_secret":"a","client_secret":"b"}"#,
        ),
    ];
    let mut failures = Vec::new();
    for (what, expect, raw) in vectors {
        let h = Harness::new();
        let fake = FakeAs::start(Config::default()).await;
        let registration = support::upstream::FakeUpstream::start().await;
        let raw = raw.to_string();
        registration.reply(move |_, _| {
            axum::response::Response::builder()
                .status(StatusCode::CREATED)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(raw.clone()))
                .unwrap()
        });
        let o = fake.origin();
        let meta = format!(
            r#"{{"issuer":"{o}","authorization_endpoint":"{o}/authorize","token_endpoint":"{o}/token","registration_endpoint":"{}","code_challenge_methods_supported":["S256"]}}"#,
            registration.url("/register")
        );
        fake.configure(|c| c.raw_meta = Some(meta));
        let id = h.connection(&fake.mcp_url());
        let (status, _, answer) = h.authorize(&id).await;
        let outcome = if status == StatusCode::OK {
            Expect::Taken
        } else {
            Expect::Refused
        };
        if outcome != expect {
            failures.push(format!("{what}: {status} {answer}, expected {expect:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The ways a query string is read: the first or the last of a name, the
/// last of a name whatever its case, a value cut at its first NUL.
#[derive(Debug, Clone, Copy)]
enum Query {
    First,
    Last,
    AnyCase,
    NulCut,
}

fn query_reads(decoder: Query, query: &str, name: &str) -> Option<String> {
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let matching = pairs.iter().filter(|(k, _)| match decoder {
        Query::AnyCase => k.eq_ignore_ascii_case(name),
        _ => k == name,
    });
    let value = match decoder {
        Query::First | Query::NulCut => matching.clone().next(),
        // As Go's `url.Values` read by a case-insensitive lookup: the last.
        Query::Last | Query::AnyCase => matching.clone().next_back(),
    }
    .map(|(_, v)| v.clone());
    match decoder {
        Query::NulCut => value.map(|v| cut_at_nul(&v).to_owned()),
        _ => value,
    }
}

/// Lane L16: the callback acts on `state` and `code` only when every way
/// of reading the query reads them alike; else it reads neither, and the
/// flow is untouched.
#[tokio::test]
async fn the_callback_acts_only_on_a_query_every_reader_reads_alike() {
    let h = Harness::new();
    let fake = FakeAs::start(Config::default()).await;
    let id = h.connection(&fake.mcp_url());
    let (_, headers, answer) = h.authorize(&id).await;
    let cookie = flow_cookie(&headers);
    let location = fake.consent(answer["consent_url"].as_str().unwrap()).await;
    let query = location.split_once('?').unwrap().1.to_string();
    let q: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let (state, code) = (&q["state"], &q["code"]);
    let vectors = [
        ("state twice", format!("state={state}&state=other&code={code}")),
        (
            "state twice, first other",
            format!("state=other&state={state}&code={code}"),
        ),
        ("code twice", format!("state={state}&code={code}&code=other")),
        (
            "state in another case",
            format!("state={state}&STATE=other&code={code}"),
        ),
        ("code in another case", format!("state={state}&code={code}&Code=other")),
        ("iss twice", format!("state={state}&code={code}&iss=a&iss=b")),
        ("a NUL in code", format!("state={state}&code={code}%00x")),
        ("a NUL in state", format!("state={state}%00x&code={code}")),
    ];
    let mut failures = Vec::new();
    for (what, vector) in &vectors {
        let readers_agree = ["state", "code", "iss"].iter().all(|name| {
            let first = query_reads(Query::First, vector, name);
            [Query::Last, Query::AnyCase, Query::NulCut]
                .iter()
                .all(|&d| query_reads(d, vector, name) == first)
        });
        assert!(!readers_agree, "{what} is no differential");
        let result = h.callback(&format!("/api/mcp/oauth/callback?{vector}"), &cookie).await;
        if result != "flow_unknown" {
            failures.push(format!("{what}: {result}"));
        }
    }
    // The flow was untouched by all of them: the vendor's own redirect
    // still completes.
    let result = h.callback(location.strip_prefix(ORIGIN).unwrap(), &cookie).await;
    if result != "connected" {
        failures.push(format!("the real redirect: {result}"));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
