//! Differential tests for the proxy's filters (fleet rule: a security
//! filter on parsed input gets one; plan 8d's whole-branch review found a
//! case-insensitive upstream running a blocked tool).
//!
//! Each vector is sent through the real proxy, against the fake upstream.
//! What crossed the gateway is then decoded again, the ways real decoders
//! do, and the filter's verdict must hold under every one of them:
//! - **Up:** either the gateway refuses (400) or answers itself, or no
//!   decoder reads in what it forwarded a `tools/call` of a tool outside the
//!   allowlist, an `initialize` with a capability not forwarded, or a
//!   method the gateway reads that the gateway did not read there.
//! - **Down:** no decoder reads in what the client received a refused
//!   server request, or a tool outside the allowlist in an answer to the
//!   client's `tools/list`.
//!
//! The decoders, as measured (plan 2026-10-15 "gateway differential"):
//! - `Exact`: last of two keys wins, names compared exactly (serde_json,
//!   JavaScript's `JSON.parse`, Python's `json`).
//! - `First`: first wins, exactly (RapidJSON's `FindMember`, simdjson).
//! - `GoV1`: last wins, names matched ignoring case with `ſ` as `s` and the
//!   Kelvin sign as `k` (Go's `encoding/json`).
//! - `GoV2Fold`: as `GoV1`, also ignoring `_` and `-` (Go's
//!   `encoding/json/v2` asked to match case-insensitively).
//! - `JsonC`: last wins, keys and strings cut at the first NUL (json-c 0.18).
//! - `CJson`: first wins, names matched ignoring ASCII case, keys and
//!   strings cut at the first NUL (cJSON 1.7.19's `cJSON_GetObjectItem`).
//!
//! What no decoder conflates (a key with a trailing space, a zero-width
//! character, a fullwidth letter, NFD against NFC) is forwarded, and the
//! test says so with `Expect::Forwarded`.
//!
//! Each table is one line per vector; a failure names every vector that
//! failed, not only the first.

mod support;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use hennery_gateway::model::CredKind;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use support::upstream::{FakeUpstream, Harness};

const ALLOWED: &str = "search";
const READ_METHODS: &[&str] = &["tools/call", "tools/list", "initialize"];
const STRIPPED: &[&str] = &["sampling", "elicitation", "roots"];
const REFUSED: &[&str] = &["sampling/createMessage", "elicitation/create", "roots/list"];

// --- A JSON tree that keeps every key, in order, twice if twice -----------

#[derive(Debug, Clone)]
enum Node {
    Null,
    Bool,
    Num(f64),
    Str(String),
    Arr(Vec<Node>),
    Obj(Vec<(String, Node)>),
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON")
    }
    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_bool<E>(self, _: bool) -> Result<Node, E> {
        Ok(Node::Bool)
    }
    fn visit_i64<E>(self, v: i64) -> Result<Node, E> {
        Ok(Node::Num(v as f64))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Node, E> {
        Ok(Node::Num(v as f64))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Node, E> {
        Ok(Node::Num(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Node, E> {
        Ok(Node::Str(v.to_owned()))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Node::Arr(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Node>()? {
            entries.push((key, value));
        }
        Ok(Node::Obj(entries))
    }
}

// --- The decoders ---------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Decoder {
    Exact,
    First,
    GoV1,
    GoV2Fold,
    JsonC,
    CJson,
}

const DECODERS: &[Decoder] = &[
    Decoder::Exact,
    Decoder::First,
    Decoder::GoV1,
    Decoder::GoV2Fold,
    Decoder::JsonC,
    Decoder::CJson,
];

fn cut_at_nul(s: &str) -> &str {
    s.split('\0').next().unwrap_or_default()
}

impl Decoder {
    /// A key as this decoder compares it with a name.
    fn key(self, key: &str) -> String {
        match self {
            Decoder::Exact | Decoder::First => key.to_owned(),
            Decoder::GoV1 => key.chars().map(go_fold).collect(),
            Decoder::GoV2Fold => key.chars().filter(|c| !matches!(c, '_' | '-')).map(go_fold).collect(),
            Decoder::JsonC => cut_at_nul(key).to_owned(),
            Decoder::CJson => cut_at_nul(key).to_ascii_lowercase(),
        }
    }

    fn first_wins(self) -> bool {
        matches!(self, Decoder::First | Decoder::CJson)
    }

    /// The member `name` of `node`, as this decoder finds it.
    fn get<'a>(self, node: &'a Node, name: &str) -> Option<&'a Node> {
        let Node::Obj(entries) = node else {
            return None;
        };
        let want = self.key(name);
        let mut found = entries.iter().filter(|(k, _)| self.key(k) == want).map(|(_, v)| v);
        if self.first_wins() {
            found.next()
        } else {
            found.next_back()
        }
    }

    /// A string as this decoder hands it on.
    fn str(self, node: Option<&Node>) -> Option<String> {
        match node? {
            Node::Str(s) => Some(match self {
                Decoder::JsonC | Decoder::CJson => cut_at_nul(s).to_owned(),
                _ => s.clone(),
            }),
            _ => None,
        }
    }
}

/// Go's case folding of a rune, as far as it reaches ASCII.
fn go_fold(c: char) -> char {
    match c {
        '\u{17f}' => 's',
        '\u{212a}' => 'k',
        c => c.to_ascii_lowercase(),
    }
}

fn messages(root: &Node) -> Vec<&Node> {
    match root {
        Node::Arr(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// What no decoder may read in a body the gateway forwarded.
fn forwarded_is_safe(body: &[u8]) -> Result<(), String> {
    let root: Node = serde_json::from_slice(body)
        .map_err(|e| format!("forwarded bytes no strict decoder reads ({e}); a lenient one might"))?;
    for (i, message) in messages(&root).into_iter().enumerate() {
        let gateway = Decoder::Exact.str(Decoder::Exact.get(message, "method"));
        for &d in DECODERS {
            let method = d.str(d.get(message, "method"));
            if let Some(m) = method.as_deref()
                && READ_METHODS.contains(&m)
                && gateway.as_deref() != Some(m)
            {
                return Err(format!(
                    "{d:?} reads method {m:?} in message {i}; the gateway read {gateway:?}"
                ));
            }
            match method.as_deref() {
                Some("tools/call") => {
                    let name = d.str(d.get(message, "params").and_then(|p| d.get(p, "name")));
                    if name.as_deref() != Some(ALLOWED) {
                        return Err(format!("{d:?} reads a tools/call of {name:?} in message {i}"));
                    }
                }
                Some("initialize") => {
                    let caps = d.get(message, "params").and_then(|p| d.get(p, "capabilities"));
                    for cap in STRIPPED {
                        if caps.and_then(|c| d.get(c, cap)).is_some() {
                            return Err(format!("{d:?} reads capability {cap} in message {i}"));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// What no decoder may read in a message the client received, answering a
/// `tools/list` of id `asked`: a refused request, and, with `allowlist`, a
/// tool outside it.
fn received_is_safe(root: &Node, asked: f64, allowlist: bool) -> Result<(), String> {
    for (i, message) in messages(root).into_iter().enumerate() {
        for &d in DECODERS {
            let method = d.str(d.get(message, "method"));
            if let Some(m) = method.as_deref()
                && REFUSED.contains(&m)
                && d.get(message, "id").is_some()
            {
                return Err(format!("{d:?} reads a {m} request in message {i}"));
            }
            let answers = matches!(d.get(message, "id"), Some(Node::Num(id)) if *id == asked);
            let tools = d.get(message, "result").and_then(|r| d.get(r, "tools"));
            if allowlist
                && answers
                && let Some(Node::Arr(tools)) = tools
            {
                for tool in tools {
                    let name = d.str(d.get(tool, "name"));
                    if name.as_deref() != Some(ALLOWED) {
                        return Err(format!("{d:?} reads tool {name:?} in message {i}"));
                    }
                }
            }
        }
    }
    Ok(())
}

/// A client's event-stream parser (WHATWG's rules, as `eventsource-parser`
/// in MCP's TypeScript SDK): one leading byte-order mark stripped, lines
/// ended by CRLF, LF or CR, comments ignored, data lines joined by LF, an
/// event dispatched at an empty line, an unended last one dropped.
fn client_events(text: &[u8]) -> Vec<String> {
    let text = text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(text);
    let text = String::from_utf8_lossy(text);
    let mut lines = Vec::new();
    let mut rest = &text[..];
    while let Some(at) = rest.find(['\r', '\n']) {
        lines.push(&rest[..at]);
        let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[at + skip..];
    }
    let mut out = Vec::new();
    let mut data: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            if let Some(d) = data.take() {
                out.push(d);
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        if field == "data" {
            match &mut data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => data = Some(value.to_owned()),
            }
        }
    }
    out
}

// --- The proxy, the upstream, one vector at a time ------------------------

struct Setup {
    h: Harness,
    upstream: FakeUpstream,
    token: String,
}

async fn setup() -> Setup {
    setup_with(Some(&[ALLOWED])).await
}

async fn setup_with(allowlist: Option<&[&str]>) -> Setup {
    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let hat = h.hat();
    let id = h.connection_in("linear", &upstream.url("/mcp"), CredKind::None, &hat, allowlist);
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &hat);
    Setup { h, upstream, token }
}

impl Setup {
    async fn post(&self, body: impl Into<Vec<u8>>) -> reqwest::Response {
        self.h
            .client
            .post(self.h.url("linear"))
            .bearer_auth(&self.token)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(body.into())
            .send()
            .await
            .unwrap()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// Whatever the gateway does, no decoder reads past its verdict.
    Safe,
    /// Safe, and it reaches the upstream: no decoder conflates it, so there
    /// is nothing to refuse.
    Forwarded,
    /// Refused, 400: nothing goes up.
    Refused,
}

use Expect::{Forwarded, Refused, Safe};

/// The request vectors: (what, body, expected).
#[rustfmt::skip]
const REQUESTS: &[(&str, &str, Expect)] = &[
    // Controls.
    ("allowed call", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search"}}"#, Forwarded),
    ("refused call", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete"}}"#, Safe),
    // 1. Key case and folding, at every level read.
    ("METHOD", r#"{"jsonrpc":"2.0","id":1,"METHOD":"tools/call","params":{"name":"delete"}}"#, Refused),
    ("Method beside method", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","Method":"tools/call","params":{"name":"delete"}}"#, Refused),
    ("Params", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","Params":{"name":"delete"}}"#, Refused),
    ("NAME beside name", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","NAME":"delete"}}"#, Refused),
    ("na_me (Go v2)", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","na_me":"delete"}}"#, Refused),
    ("long s in a capability", "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{\"\u{17f}ampling\":{}}}}", Refused),
    ("Capabilities", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"Capabilities":{"sampling":{}}}}"#, Refused),
    ("ROOTS", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"ROOTS":{}}}}"#, Refused),
    ("METHOD in a batch", r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"METHOD":"tools/call","params":{"name":"delete"}}]"#, Refused),
    // 2. Duplicate keys, either order, every level read.
    ("method twice, call last", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","method":"tools/call","params":{"name":"delete"}}"#, Refused),
    ("method twice, call first", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"tools/list","params":{"name":"delete"}}"#, Refused),
    ("params twice", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete"},"params":{"name":"search"}}"#, Refused),
    ("name twice, bad first", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete","name":"search"}}"#, Refused),
    ("name twice, bad last", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name":"delete"}}"#, Refused),
    ("capabilities twice", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"sampling":{}},"capabilities":{}}}"#, Refused),
    ("sampling twice", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"sampling":{},"sampling":{}}}}"#, Refused),
    ("duplicate in a batch element", r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"ping","method":"tools/call","params":{"name":"delete"}}]"#, Refused),
    // 3. Escaped keys and values: serde_json unescapes as every decoder does.
    ("escaped method key twice", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","\u006dethod":"tools/call","params":{"name":"delete"}}"#, Refused),
    ("escaped name key", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"\u006eame":"delete"}}"#, Safe),
    ("escaped method value", r#"{"jsonrpc":"2.0","id":1,"method":"tools\u002fcall","params":{"name":"delete"}}"#, Safe),
    ("escaped capability", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"\u0073ampling":{}}}}"#, Safe),
    ("escaped upper METHOD", r#"{"jsonrpc":"2.0","id":1,"\u004dETHOD":"tools/call","params":{"name":"delete"}}"#, Refused),
    // 4. A byte-order mark and whitespace JSON does not have (cJSON skips a mark).
    ("BOM", "\u{feff}{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"delete\"}}", Refused),
    ("NBSP", "{\u{a0}\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}", Refused),
    ("ideographic space", "{\"jsonrpc\":\"2.0\",\u{3000}\"id\":1,\"method\":\"ping\"}", Refused),
    ("line separator", "{\"jsonrpc\":\"2.0\",\u{2028}\"id\":1,\"method\":\"ping\"}", Refused),
    ("form feed", "{\"jsonrpc\":\"2.0\",\x0c\"id\":1,\"method\":\"ping\"}", Refused),
    ("vertical tab", "{\"jsonrpc\":\"2.0\",\x0b\"id\":1,\"method\":\"ping\"}", Refused),
    // 5. What a lenient decoder takes (json-c's default tokener: the first four).
    ("trailing comma", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete"},}"#, Refused),
    ("block comment", r#"{"jsonrpc":"2.0","id":1,/* x */"method":"ping"}"#, Refused),
    ("line comment", "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\" // x\n}", Refused),
    ("single quotes", r#"{'jsonrpc':'2.0','id':1,'method':'ping'}"#, Refused),
    ("NaN", r#"{"jsonrpc":"2.0","id":NaN,"method":"ping"}"#, Refused),
    ("Infinity", r#"{"jsonrpc":"2.0","id":Infinity,"method":"ping"}"#, Refused),
    ("leading plus", r#"{"jsonrpc":"2.0","id":+1,"method":"ping"}"#, Refused),
    ("hex number", r#"{"jsonrpc":"2.0","id":0x10,"method":"ping"}"#, Refused),
    ("leading zero", r#"{"jsonrpc":"2.0","id":01,"method":"ping"}"#, Refused),
    ("unquoted key", r#"{"jsonrpc":"2.0","id":1,method:"ping"}"#, Refused),
    ("raw control character", "{\"jsonrpc\":\"2.0\",\"id\":1,\"me\x01thod\":\"ping\"}", Refused),
    ("lone surrogate", r#"{"jsonrpc":"2.0","id":1,"method":"ping","x":"\ud800"}"#, Refused),
    // 6. NUL: json-c and cJSON cut keys and strings at it.
    ("NUL key after (json-c)", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","method\u0000x":"tools/call","params":{"name":"delete"}}"#, Refused),
    ("NUL key before (cJSON)", r#"{"jsonrpc":"2.0","id":1,"method\u0000x":"tools/call","method":"tools/list","params":{"name":"delete"}}"#, Refused),
    ("NUL params key", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search"},"params\u0000":{"name":"delete"}}"#, Refused),
    ("NUL name key", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name\u0000":"delete"}}"#, Refused),
    ("NUL capability key", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"sampling\u0000":{}}}}"#, Refused),
    ("NUL method value", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call\u0000x","params":{"name":"delete"}}"#, Refused),
    ("NUL initialize value", r#"{"jsonrpc":"2.0","id":1,"method":"initialize\u0000","params":{"capabilities":{"sampling":{}}}}"#, Refused),
    ("upper method value", r#"{"jsonrpc":"2.0","id":1,"method":"TOOLS/CALL","params":{"name":"delete"}}"#, Refused),
    ("NUL tools/list value", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list\u0000"}"#, Refused),
    ("NUL name value", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search\u0000x"}}"#, Safe),
    // Not the same name to any decoder measured (Go, cJSON, json-c).
    ("trailing space key", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name ":"delete"}}"#, Forwarded),
    ("zero-width key", "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"search\",\"na\u{200b}me\":\"delete\"}}", Forwarded),
    ("fullwidth key", "{\"jsonrpc\":\"2.0\",\"id\":1,\"\u{ff4d}ethod\":\"tools/call\",\"method\":\"ping\",\"params\":{\"name\":\"delete\"}}", Forwarded),
    ("combining mark key", "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"search\",\"na\u{301}me\":\"delete\"}}", Forwarded),
    // 7. The envelope.
    ("initialize capabilities array", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":["sampling","roots"]}}"#, Refused),
    ("initialize capabilities string", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":"sampling"}}"#, Refused),
    ("initialize params array", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":[{"capabilities":{"sampling":{}}}]}"#, Refused),
    ("object id", r#"{"jsonrpc":"2.0","id":{"a":1},"method":"tools/call","params":{"name":"delete"}}"#, Safe),
    ("array id", r#"{"jsonrpc":"2.0","id":[1],"method":"tools/call","params":{"name":"delete"}}"#, Safe),
    ("array method", r#"{"jsonrpc":"2.0","id":1,"method":["tools/call"],"params":{"name":"delete"}}"#, Refused),
    ("number method", r#"{"jsonrpc":"2.0","id":1,"method":7,"params":{"name":"delete"}}"#, Refused),
    ("array params", r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":["delete"]}"#, Safe),
    ("batch in a batch", r#"[[{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete"}}]]"#, Refused),
    ("scalar in a batch", r#"[1,{"jsonrpc":"2.0","id":1,"method":"ping"}]"#, Refused),
    ("empty batch", "[]", Forwarded),
    ("requests and responses", r#"[{"jsonrpc":"2.0","id":"s1","result":{}},{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search"}}]"#, Forwarded),
    ("a response and a refused call", r#"[{"jsonrpc":"2.0","id":"s1","result":{}},{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete"}}]"#, Safe),
];

#[tokio::test]
async fn no_decoder_reads_past_the_request_filter() {
    let s = setup().await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#))
            .unwrap()
    });
    let mut failures = Vec::new();
    for &(what, body, expect) in REQUESTS {
        let before = s.upstream.seen().len();
        let status = s.post(body).await.status();
        let sent: Vec<_> = s.upstream.seen().into_iter().skip(before).collect();
        if expect == Refused && status != StatusCode::BAD_REQUEST {
            failures.push(format!("{what}: {status}, not 400"));
        }
        if expect == Forwarded && sent.is_empty() {
            failures.push(format!("{what}: {status}, not forwarded"));
        }
        if status == StatusCode::BAD_REQUEST && !sent.is_empty() {
            failures.push(format!("{what}: 400, yet sent"));
        }
        for up in sent {
            if let Err(why) = forwarded_is_safe(&up.body) {
                failures.push(format!("{what}: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// How an upstream answers in a response vector.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Json,
    Sse,
}

/// The response vectors, answering the client's `tools/list` of id 1:
/// (what, how, body, expected). `Forwarded` here: the client receives a
/// message.
#[rustfmt::skip]
const RESPONSES: &[(&str, Kind, &str, Expect)] = &[
    // Controls.
    ("json list", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"search"},{"name":"delete"}]}}"#, Forwarded),
    ("sse list", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"search\"},{\"name\":\"delete\"}]}}\n\n", Forwarded),
    ("sse sampling", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"sampling/createMessage\"}\n\n", Safe),
    // 1. Key case and folding, going down.
    ("json Id", Kind::Json, r#"{"jsonrpc":"2.0","Id":1,"result":{"tools":[{"name":"delete"}]}}"#, Safe),
    ("json Result", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"Result":{"tools":[{"name":"delete"}]}}"#, Safe),
    ("json Tools", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"Tools":[{"name":"delete"}]}}"#, Safe),
    ("json tool Name", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"search","Name":"delete"}]}}"#, Safe),
    ("json Id in a batch", Kind::Json, r#"[{"jsonrpc":"2.0","id":2,"result":{}},{"jsonrpc":"2.0","ID":1,"result":{"tools":[{"name":"delete"}]}}]"#, Safe),
    ("sse Id", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"Id\":1,\"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n", Safe),
    ("sse Result", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"Result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n", Safe),
    ("sse Tools", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"Tools\":[{\"name\":\"delete\"}]}}\n\n", Safe),
    ("sse tool NAME", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"search\",\"NAME\":\"delete\"}]}}\n\n", Safe),
    ("sse METHOD", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"METHOD\":\"sampling/createMessage\"}\n\n", Safe),
    // 2. Duplicate keys.
    ("json id twice", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"id":2,"result":{"tools":[{"name":"delete"}]}}"#, Safe),
    ("json tools twice", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete"}],"tools":[]}}"#, Safe),
    ("json name twice", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete","name":"search"}]}}"#, Safe),
    ("sse method twice", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"sampling/createMessage\",\"method\":\"ping\"}\n\n", Safe),
    ("sse tools twice", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"delete\"}],\"tools\":[]}}\n\n", Safe),
    // 3. Escapes.
    ("sse escaped method", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"\\u0073ampling\\/createMessage\"}\n\n", Safe),
    ("sse escaped key", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"\\u006dethod\":\"roots/list\"}\n\n", Safe),
    // 4. Marks, whitespace, framing.
    ("json BOM", Kind::Json, "\u{feff}{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"delete\"}]}}", Safe),
    ("json NBSP", Kind::Json, "{\u{a0}\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"delete\"}]}}", Safe),
    ("sse BOM first", Kind::Sse, "\u{feff}data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse BOM later", Kind::Sse, ": hi\n\n\u{feff}data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse BOM behind a dropped event", Kind::Sse, "data: not json\n\n\u{feff}data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse BOM behind a stripped BOM", Kind::Sse, "\u{feff}\u{feff}data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse CR only", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\r\r", Safe),
    ("sse CRLF", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\r\n\r\n", Safe),
    ("sse CR inside", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\rdata: \"method\":\"roots/list\"}\n\n", Safe),
    ("sse split data", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n", Safe),
    ("sse split request", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\ndata: \"method\":\"elicitation/create\"}\n\n", Safe),
    ("sse comments between", Kind::Sse, ": a\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\n: b\ndata: \"method\":\"roots/list\"}\n\n", Safe),
    ("sse no space", Kind::Sse, "data:{\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse bare data line", Kind::Sse, "data\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse other event type", Kind::Sse, "event: x\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    ("sse NBSP", Kind::Sse, "data: {\u{a0}\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    // 5. Lenient forms.
    ("json trailing comma", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete"}]},}"#, Safe),
    ("sse trailing comma", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\",}\n\n", Safe),
    ("sse single quotes", Kind::Sse, "data: {'jsonrpc':'2.0','id':'s','method':'roots/list'}\n\n", Safe),
    ("sse comment in data", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",/**/\"id\":\"s\",\"method\":\"roots/list\"}\n\n", Safe),
    // 6. NUL.
    ("json NUL tools key", Kind::Json, r#"{"jsonrpc":"2.0","id":1,"result":{"tools\u0000":[{"name":"delete"}]}}"#, Safe),
    ("json NUL id key", Kind::Json, r#"{"jsonrpc":"2.0","id\u0000":1,"result":{"tools":[{"name":"delete"}]}}"#, Safe),
    ("sse NUL method key", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\\u0000\":\"roots/list\"}\n\n", Safe),
    ("sse NUL method value", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\\u0000x\"}\n\n", Safe),
    ("sse NUL sampling value", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"sampling/createMessage\\u0000\"}\n\n", Safe),
    ("sse upper method value", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"Sampling/CreateMessage\"}\n\n", Safe),
    ("sse NUL tool name key", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"search\",\"name\\u0000\":\"delete\"}]}}\n\n", Safe),
    // 7. The envelope.
    ("sse array method", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":[\"roots/list\"]}\n\n", Safe),
    ("sse batch in a batch", Kind::Sse, "data: [[{\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}]]\n\n", Safe),
    ("sse refused in a batch", Kind::Sse, "data: [{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"delete\"}]}},{\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}]\n\n", Forwarded),
];

/// Event streams that are not UTF-8 (the security review's finding 2):
/// (what, body). The gateway would read them with replacement characters;
/// a client decoding them otherwise (dropping the byte, or an overlong
/// `C0 A2` as `"`) reads a refused request.
#[rustfmt::skip]
const RAW_EVENTS: &[(&str, &[u8])] = &[
    ("sse invalid byte in a method", b"data: {\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\xff\"}\n\n"),
    ("sse overlong quotes", b"data: {\"id\":\"s\",\"x\":\"\xc0\xa2,\xc0\xa2method\xc0\xa2:\xc0\xa2roots/list\xc0\xa2,\xc0\xa2y\xc0\xa2:\xc0\xa2\",\"jsonrpc\":\"2.0\"}\n\n"),
];

/// What the client received, as messages, for a body of `content_type`.
/// Bytes that are not UTF-8 are a failure: a client may decode them
/// otherwise than any model here.
fn received(content_type: &str, body: &[u8]) -> Result<Vec<Node>, String> {
    std::str::from_utf8(body).map_err(|e| format!("received bytes that are not UTF-8 ({e})"))?;
    let datas = if content_type == "text/event-stream" {
        client_events(body)
    } else {
        vec![String::from_utf8_lossy(body).into_owned()]
    };
    datas
        .iter()
        .map(|d| serde_json::from_str(d).map_err(|e| format!("received data no strict decoder reads ({e}): {d:?}")))
        .collect()
}

/// One response vector through the proxy: the failures it shows, if any.
async fn answer_vector(
    s: &Setup,
    what: &str,
    kind: Kind,
    body: &'static [u8],
    expect: Expect,
    allowlist: bool,
) -> Vec<String> {
    let content_type = match kind {
        Kind::Json => "application/json",
        Kind::Sse => "text/event-stream",
    };
    s.upstream.reply(move |seen, _| {
        // The refused requests' answers.
        if seen.body.windows(7).any(|w| w == b"\"error\"") {
            return Response::builder()
                .status(StatusCode::ACCEPTED)
                .body(Body::empty())
                .unwrap();
        }
        Response::builder()
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap()
    });
    let resp = s
        .post(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string())
        .await;
    let status = resp.status();
    let got_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = resp.bytes().await.unwrap();
    let mut failures = Vec::new();
    if status == StatusCode::BAD_GATEWAY {
        if expect == Forwarded {
            failures.push(format!("{what}: 502, not delivered"));
        }
        return failures;
    }
    match received(&got_type, &bytes) {
        Err(why) => failures.push(format!("{what}: {why}")),
        Ok(messages) => {
            if expect == Forwarded && messages.is_empty() {
                failures.push(format!("{what}: nothing delivered"));
            }
            for message in &messages {
                if let Err(why) = received_is_safe(message, 1.0, allowlist) {
                    failures.push(format!("{what}: {why}"));
                }
            }
        }
    }
    failures
}

#[tokio::test]
async fn no_decoder_reads_past_the_response_filters() {
    let s = setup().await;
    let mut failures = Vec::new();
    for &(what, kind, body, expect) in RESPONSES {
        failures.extend(answer_vector(&s, what, kind, body.as_bytes(), expect, true).await);
    }
    for &(what, body) in RAW_EVENTS {
        failures.extend(answer_vector(&s, what, Kind::Sse, body, Safe, true).await);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The refusal of server requests does not depend on an allowlist: every
/// event-stream vector again, on a connection without one. (Without one, a
/// JSON answer is passed on unread, as 8d's Q1 ruling has it.)
#[tokio::test]
async fn no_decoder_reads_a_refused_request_without_an_allowlist() {
    let s = setup_with(None).await;
    let mut failures = Vec::new();
    for &(what, kind, body, expect) in RESPONSES {
        if matches!(kind, Kind::Sse) {
            failures.extend(answer_vector(&s, what, kind, body.as_bytes(), expect, false).await);
        }
    }
    for &(what, body) in RAW_EVENTS {
        failures.extend(answer_vector(&s, what, Kind::Sse, body, Safe, false).await);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Ids (vector 8): the gateway's own answer echoes an integer id digit for
/// digit, never through a float, and any other number as the same number
/// (`-0`, `1e0`); an upstream that answers a `tools/list` of id 2^53+1 as
/// 2^53, as a decoder into a double does (Go's `any`, JavaScript), or `-0`
/// as `0`, is still filtered.
#[tokio::test]
async fn ids_are_echoed_exactly_and_still_filtered() {
    let s = setup().await;
    for id in ["9007199254740993", "-9223372036854775808", "18446744073709551615"] {
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"delete"}}}}"#);
        let text = s.post(body).await.text().await.unwrap();
        assert!(text.contains(&format!(r#""id":{id}"#)), "{id}: {text}");
    }
    for (id, value) in [("-0", 0.0), ("1e0", 1.0)] {
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"delete"}}}}"#);
        let answer: Value = s.post(body).await.json().await.unwrap();
        assert_eq!(answer["id"].as_f64(), Some(value), "{id}: {answer}");
    }
    assert!(s.upstream.seen().is_empty());
    for (asked, answered) in [("9007199254740993", "9007199254740992"), ("-0", "0"), ("1e0", "1")] {
        let listed = format!(r#"{{"jsonrpc":"2.0","id":{answered},"result":{{"tools":[{{"name":"delete"}}]}}}}"#);
        s.upstream.reply(move |_, _| {
            Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(listed.clone()))
                .unwrap()
        });
        let answer: Value = s
            .post(format!(r#"{{"jsonrpc":"2.0","id":{asked},"method":"tools/list"}}"#))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(answer["result"]["tools"], json!([]), "{asked} answered as {answered}");
    }
}

/// A `tools/list` without an id (the security review's finding 3): an
/// upstream that answers it with `"id": null` is filtered too.
#[tokio::test]
async fn a_tools_list_without_an_id_is_filtered_as_null() {
    let s = setup().await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":null,"result":{"tools":[{"name":"search"},{"name":"delete"}]}}"#,
            ))
            .unwrap()
    });
    let answer: Value = s
        .post(r#"{"jsonrpc":"2.0","method":"tools/list"}"#)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(answer["result"]["tools"], json!([{"name": "search"}]));
}
