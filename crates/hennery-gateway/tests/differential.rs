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
//!   server request, in JSON or an event stream, or a tool outside the
//!   allowlist in any `result.tools`, whatever its id.
//!
//! The decoders are `support::differential`'s, each measured against a
//! real one (Go, json-c, cJSON).
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
use serde_json::{Value, json};
use support::differential::{forwarded_is_safe, received, received_is_safe};
use support::upstream::{FakeUpstream, Harness};

const ALLOWED: &str = "search";

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
            if let Err(why) = forwarded_is_safe(&up.body, &[ALLOWED]) {
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
    ("json sampling", Kind::Json, r#"{"jsonrpc":"2.0","id":"s","method":"sampling/createMessage","params":{}}"#, Safe),
    ("json refused in a batch", Kind::Json, r#"[{"jsonrpc":"2.0","id":1,"result":{}},{"jsonrpc":"2.0","id":"s","method":"roots/list"}]"#, Safe),
    ("json tools and a refused request", Kind::Json, r#"[{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"search"}]}},{"jsonrpc":"2.0","id":"s","method":"roots/list"}]"#, Safe),
    ("json METHOD", Kind::Json, r#"{"jsonrpc":"2.0","id":"s","METHOD":"elicitation/create"}"#, Safe),
    ("json NUL method value", Kind::Json, r#"{"jsonrpc":"2.0","id":"s","method":"roots/list\u0000x"}"#, Safe),
    ("json method twice", Kind::Json, r#"{"jsonrpc":"2.0","id":"s","method":"roots/list","method":"ping"}"#, Safe),
    // Ids: the filter reads every result.tools, whatever its id.
    ("json id of another type", Kind::Json, r#"{"jsonrpc":"2.0","id":"1","result":{"tools":[{"name":"delete"}]}}"#, Safe),
    ("json another id", Kind::Json, r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[{"name":"search"},{"name":"delete"}]}}"#, Forwarded),
    ("json no id", Kind::Json, r#"{"jsonrpc":"2.0","result":{"tools":[{"name":"delete"}]}}"#, Safe),
    ("sse id of another type", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n", Safe),
    ("sse another id", Kind::Sse, "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[{\"name\":\"search\"},{\"name\":\"delete\"}]}}\n\n", Forwarded),
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
                if let Err(why) = received_is_safe(message, allowlist.then_some(&[ALLOWED][..])) {
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
/// vector again, in JSON and event streams, on a connection without one
/// (gateway spec §5.6: in any answer).
#[tokio::test]
async fn no_decoder_reads_a_refused_request_without_an_allowlist() {
    let s = setup_with(None).await;
    let mut failures = Vec::new();
    for &(what, kind, body, expect) in RESPONSES {
        failures.extend(answer_vector(&s, what, kind, body.as_bytes(), expect, false).await);
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
    // A string id stays a string: a client that coerces it (`Number("1")`)
    // does so on its own (gateway spec §5.5).
    let answer: Value = s
        .post(r#"{"jsonrpc":"2.0","id":"1","method":"tools/call","params":{"name":"delete"}}"#)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(answer["id"], json!("1"));
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

/// A `GET` stream, and a `GET` replaying one with `Last-Event-ID`, are
/// filtered as a live answer is: they carry no `tools/list` id of their
/// own (gateway spec §5.5).
#[tokio::test]
async fn a_get_stream_and_its_replay_are_filtered() {
    let s = setup().await;
    let replayed = "id: 9\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"search\"},{\"name\":\"delete\"}]}}\n\n";
    s.upstream.reply(move |_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(replayed))
            .unwrap()
    });
    for last_event_id in [None, Some("8")] {
        let mut get =
            s.h.client
                .get(s.h.url("linear"))
                .bearer_auth(&s.token)
                .header(header::ACCEPT, "text/event-stream");
        if let Some(id) = last_event_id {
            get = get.header("last-event-id", id);
        }
        let text = get.send().await.unwrap().text().await.unwrap();
        let events = support::differential::client_events(text.as_bytes());
        assert_eq!(events.len(), 1, "{last_event_id:?}: {text}");
        let answer: Value = serde_json::from_str(&events[0]).unwrap();
        assert_eq!(
            answer["result"]["tools"],
            json!([{"name": "search"}]),
            "{last_event_id:?}"
        );
    }
    let seen = s.upstream.seen();
    assert_eq!(seen[1].header("last-event-id"), Some("8"));
}

/// An answer on another stream (accepted, gateway spec §5.5): the proxy
/// keeps nothing between requests, so one session's upstream stream never
/// reaches another's. Two sessions on one connection, each with its own
/// upstream session, open their streams at once; each sees only its own.
/// Each gets its session id from a `POST`, wrapped for its token (plan 8e
/// decision 13), and the upstream only ever sees the bare ids.
#[tokio::test]
async fn each_session_sees_only_its_own_upstream_stream() {
    let s = setup().await;
    let hat = s.h.hat();
    let other = s.h.mint("s2", "host-a", &hat);
    let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    let theirs = s.h.session_id(&s.upstream, "linear", &other, "up-2").await;
    // Each event its own chunk, a pause between them, so the two streams
    // interleave inside the proxy; the answer's session id is the
    // upstream's own (`srv-…`), not the one the client sent.
    s.upstream.reply(|seen, _| {
        let who = seen.header("mcp-session-id").unwrap_or("none").to_owned();
        let header = format!("srv-{who}");
        let events = futures::stream::unfold(0, move |n| {
            let who = who.clone();
            async move {
                if n == 20 {
                    return None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let event = format!(
                    "data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{{\"for\":\"{who}\",\"n\":{n}}}}}\n\n"
                );
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(event)), n + 1))
            }
        });
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header("mcp-session-id", header)
            .body(Body::from_stream(events))
            .unwrap()
    });
    let open = |token: String, session: String, upstream_session: &'static str| {
        let client = s.h.client.clone();
        let url = s.h.url("linear");
        async move {
            let resp = client
                .get(url)
                .bearer_auth(token)
                .header(header::ACCEPT, "text/event-stream")
                .header("mcp-session-id", session)
                .send()
                .await
                .unwrap();
            let id = resp.headers()["mcp-session-id"].to_str().unwrap().to_owned();
            assert!(id.starts_with(&format!("srv-{upstream_session}.")), "{id}");
            resp.text().await.unwrap()
        }
    };
    let (one, two) = tokio::join!(open(s.token.clone(), mine, "up-1"), open(other, theirs, "up-2"));
    let streams: Vec<_> = s
        .upstream
        .seen()
        .into_iter()
        .filter(|seen| seen.method == "GET")
        .collect();
    assert_eq!(streams.len(), 2);
    let mut bare: Vec<_> = streams
        .iter()
        .map(|seen| seen.header("mcp-session-id").unwrap())
        .collect();
    bare.sort();
    assert_eq!(bare, ["up-1", "up-2"], "the upstream sees the bare ids only");
    for (text, mine) in [(one, "up-1"), (two, "up-2")] {
        let events = support::differential::client_events(text.as_bytes());
        assert_eq!(events.len(), 20, "{mine}: {text}");
        for event in events {
            let message: Value = serde_json::from_str(&event).unwrap();
            assert_eq!(message["params"]["for"], mine, "{event}");
        }
    }
}

/// Session ids as a client or an intermediary may spell them (plan 8e
/// decision 13): another token's id, joined to this token's with a comma
/// either way round (how a list header is folded), or wrapped once more.
/// Each is the same 404, and nothing goes up.
#[tokio::test]
async fn no_spelling_of_another_tokens_session_id_goes_up() {
    let s = setup().await;
    let other = s.h.mint("s2", "host-a", &s.h.hat());
    let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
    let theirs = s.h.session_id(&s.upstream, "linear", &other, "up-2").await;
    let before = s.upstream.seen().len();
    for spelling in [
        theirs.clone(),
        format!("{mine},{theirs}"),
        format!("{theirs}, {mine}"),
        format!("up-1.{mine}"),
        format!("{theirs}.{}", &mine["up-1.".len()..]),
    ] {
        let resp =
            s.h.client
                .post(s.h.url("linear"))
                .bearer_auth(&s.token)
                .header(header::CONTENT_TYPE, "application/json")
                .header("mcp-session-id", &spelling)
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
                .send()
                .await
                .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{spelling}");
    }
    assert_eq!(s.upstream.seen().len(), before, "nothing went up");
}

/// A server request inside a JSON answer (gateway spec §5.6, plan
/// 2026-10-15 "gateway JSON answers" decision 2): the whole answer is 502
/// `upstream_invalid`, nothing of it reaches the client, and nothing is
/// answered upstream. With an allowlist or without.
#[tokio::test]
async fn a_server_request_in_a_json_answer_is_502() {
    for allowlist in [Some(&[ALLOWED][..]), None] {
        let s = setup_with(allowlist).await;
        // Whatever the status: a JSON body is judged on a 4xx or 5xx too.
        for status in [
            StatusCode::OK,
            StatusCode::ACCEPTED,
            StatusCode::BAD_REQUEST,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let before = s.upstream.seen().len();
            s.upstream.reply(move |_, _| {
                Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"[{"jsonrpc":"2.0","id":1,"result":{}},{"jsonrpc":"2.0","id":"s","method":"sampling/createMessage","params":{}}]"#,
                    ))
                    .unwrap()
            });
            let resp = s.post(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).await;
            assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "{allowlist:?} {status}");
            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["code"], "upstream_invalid", "{allowlist:?} {status}");
            assert!(!body["message"].as_str().unwrap().contains("sampling/createMessage"));
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            assert_eq!(
                s.upstream.seen().len(),
                before + 1,
                "{allowlist:?} {status}: something was answered upstream"
            );
        }
    }
}

/// Every JSON answer is read whole (gateway spec §5.3): one over 8 MiB is
/// 502 `upstream_too_large`, never truncated, with an allowlist or not; an
/// empty one passes, empty.
#[tokio::test]
async fn a_json_answer_is_read_whole_and_capped() {
    for allowlist in [Some(&[ALLOWED][..]), None] {
        let s = setup_with(allowlist).await;
        s.upstream.reply(|_, _| {
            let pad = "x".repeat(8 * 1024 * 1024);
            Response::builder()
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"pad":"{pad}"}}}}"#
                )))
                .unwrap()
        });
        let resp = s.post(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "{allowlist:?}");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["code"], "upstream_too_large");
        // A 202 typed JSON, with a body that is empty but not declared so.
        s.upstream.reply(|_, _| {
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from_stream(futures::stream::iter(Vec::<
                    Result<axum::body::Bytes, std::io::Error>,
                >::new())))
                .unwrap()
        });
        let resp = s
            .post(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "{allowlist:?}");
        assert_eq!(resp.text().await.unwrap(), "");
    }
}
