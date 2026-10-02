//! What the proxy reads of MCP's JSON-RPC (gateway spec §5.5, §5.6): the
//! tool allowlist on `tools/call` and `tools/list`, and the capabilities
//! not forwarded in v1. Everything else passes as it came.
//!
//! - A request body must be JSON without a duplicate key anywhere (plan 8d
//!   decision 6): a parser upstream that keeps the first of two `"method"`s
//!   or `"name"`s would otherwise run what the allowlist check never saw.
//!   Nor may it spell a key the gateway reads otherwise than exactly
//!   (`METHOD`, `Name`, `ſampling`): a decoder that matches names whatever
//!   their case, as Go's `encoding/json` does, would read it.
//! - `initialize` goes upstream without `sampling`, `elicitation` and
//!   `roots` in its client capabilities.
//! - With an allowlist, a `tools/call` for a tool outside it is answered
//!   here, `-32602`, and never reaches the upstream; the ids of `tools/list`
//!   requests are kept, and their responses filtered.
//! - A server-to-client request for one of those capabilities, arriving in
//!   an event stream, is answered here with an error and not passed on.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use std::collections::HashSet;

/// The client capabilities never sent upstream (gateway spec §5.6).
pub const STRIPPED_CAPABILITIES: &[&str] = &["sampling", "elicitation", "roots"];

/// Server-to-client requests answered with an error, never passed on: the
/// ones those capabilities would have allowed.
pub const REFUSED_SERVER_REQUESTS: &[&str] = &["sampling/createMessage", "elicitation/create", "roots/list"];

/// `tools/call` outside the allowlist (gateway spec §5.5).
pub const TOOL_NOT_AVAILABLE: i64 = -32602;
const TOOL_NOT_AVAILABLE_MESSAGE: &str = "tool not available through hennery";

/// The rest of a batch that held such a call (plan 8d decision 7).
const INVALID_REQUEST: i64 = -32600;
const BATCH_REFUSED_MESSAGE: &str = "batch refused: a tools/call in it is not available through hennery";

/// A server-to-client request the gateway does not forward.
const METHOD_NOT_FOUND: i64 = -32601;

/// A UTF-8 byte-order mark, which a client's event-stream parser strips
/// once, at the start of its stream.
pub const BOM: &[u8] = b"\xef\xbb\xbf";

/// What to do with a request body.
#[derive(Debug, PartialEq)]
pub enum Inspected {
    /// Send it upstream: `body` is the client's bytes, or `initialize`
    /// rewritten. `tools_list` holds the ids of its `tools/list` requests
    /// when the connection has an allowlist, whose responses are filtered.
    Forward { body: Vec<u8>, tools_list: Vec<Value> },
    /// Answer it here and send nothing upstream: a JSON-RPC answer, or
    /// `None` when nothing in it has an id (202, no body).
    Answer(Option<Value>),
    /// Not JSON, a key twice in one object, or a key the gateway reads
    /// spelt otherwise: refused, 400.
    Invalid(&'static str),
}

/// What a `POST` body refused 400 is told.
const INVALID_BODY: &str =
    "the body is not JSON, has a key twice in one object, or spells a key the gateway reads otherwise";

/// The keys the gateway reads in a message, and in the `params` of the
/// methods it reads them for. A decoder that matches names whatever their
/// case (Go's `encoding/json`) takes `METHOD` or `Name` for them, so such
/// a spelling, at its level, is refused like a key twice (plan 8d decision
/// 6; the whole-branch review).
const MESSAGE_KEYS: &[&str] = &["jsonrpc", "id", "method", "params"];

/// A key as such a decoder compares it with the names the gateway reads:
/// ASCII case, and `ſ` as `s`. Unicode folds only one other letter to an
/// ASCII one, the Kelvin sign to `k`, and no name read here has a `k`.
/// Go's `encoding/json/v2`, matching names case-insensitively, ignores `_`
/// and `-` as well (the re-confirmation's F1).
fn folded(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .map(|c| match c {
            '\u{17f}' => 's',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// Whether `object` has a key that folds to one of `names` without being it.
fn respelt(object: &serde_json::Map<String, Value>, names: &[&str]) -> bool {
    object.keys().any(|key| {
        let folded = folded(key);
        names.iter().any(|name| folded == *name && key != name)
    })
}

/// Whether a message, or any in a batch, spells a key the gateway reads
/// otherwise than exactly: one of the message's own, `name` in a
/// `tools/call`'s `params`, or `capabilities` and those not forwarded in
/// an `initialize`'s.
pub fn respelt_keys(value: &Value) -> bool {
    let messages: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    messages.into_iter().any(|message| {
        let Some(object) = message.as_object() else {
            return false;
        };
        if respelt(object, MESSAGE_KEYS) {
            return true;
        }
        let Some(params) = object.get("params").and_then(Value::as_object) else {
            return false;
        };
        match method(message) {
            Some("tools/call") => respelt(params, &["name"]),
            Some("initialize") => {
                respelt(params, &["capabilities"])
                    || params
                        .get("capabilities")
                        .and_then(Value::as_object)
                        .is_some_and(|capabilities| respelt(capabilities, STRIPPED_CAPABILITIES))
            }
            _ => false,
        }
    })
}

/// Inspect a `POST` body under `allowlist` (`None`: every tool).
pub fn inspect_request(body: &[u8], allowlist: Option<&[String]>) -> Inspected {
    if serde_json::from_slice::<Unique>(body).is_err() {
        return Inspected::Invalid(INVALID_BODY);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Inspected::Invalid(INVALID_BODY);
    };
    if respelt_keys(&value) {
        return Inspected::Invalid(INVALID_BODY);
    }
    let mut rewritten = false;
    let mut blocked = false;
    let mut tools_list = Vec::new();
    {
        let messages: Vec<&mut Value> = match &mut value {
            Value::Array(items) => items.iter_mut().collect(),
            other => vec![other],
        };
        for message in messages {
            match method(message) {
                Some("initialize") => rewritten |= strip_capabilities(message),
                Some("tools/call") if allowlist.is_some_and(|tools| !call_allowed(message, tools)) => blocked = true,
                Some("tools/list") if allowlist.is_some() => {
                    if let Some(id) = message.get("id") {
                        tools_list.push(id.clone());
                    }
                }
                _ => {}
            }
        }
    }
    if blocked {
        return Inspected::Answer(refusal(&value, allowlist.unwrap_or_default()));
    }
    let body = if rewritten {
        serde_json::to_vec(&value).expect("a JSON value serialises")
    } else {
        body.to_vec()
    };
    Inspected::Forward { body, tools_list }
}

fn method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
}

/// Whether a `tools/call` names, as a string, a tool in `tools`.
fn call_allowed(message: &Value, tools: &[String]) -> bool {
    message
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|name| tools.iter().any(|tool| tool == name))
}

/// Remove the capabilities not forwarded from an `initialize`: true if one
/// was there.
fn strip_capabilities(message: &mut Value) -> bool {
    let Some(capabilities) = message
        .get_mut("params")
        .and_then(|params| params.get_mut("capabilities"))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut removed = false;
    for name in STRIPPED_CAPABILITIES {
        removed |= capabilities.remove(*name).is_some();
    }
    removed
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The answer to a body holding a `tools/call` outside the allowlist: for
/// one message, its error; for a batch, an error for each request in it,
/// that call's and the others' (plan 8d decision 7). Notifications get
/// none; with nothing to answer, `None`.
fn refusal(value: &Value, tools: &[String]) -> Option<Value> {
    let answer = |message: &Value| {
        let id = message.get("id")?;
        Some(
            if method(message) == Some("tools/call") && !call_allowed(message, tools) {
                error(id, TOOL_NOT_AVAILABLE, TOOL_NOT_AVAILABLE_MESSAGE)
            } else {
                error(id, INVALID_REQUEST, BATCH_REFUSED_MESSAGE)
            },
        )
    };
    match value {
        Value::Array(items) => {
            let answers: Vec<Value> = items.iter().filter_map(answer).collect();
            (!answers.is_empty()).then_some(Value::Array(answers))
        }
        message => answer(message),
    }
}

/// Whether a response's id answers a request's: equal, or the same number
/// however written (`1` and `1.0`), as a client's matching may read them
/// (the review's O1).
fn same_id(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Filter the `tools/list` responses in `value` (a message or a batch)
/// whose id is in `ids` to `tools`: true if one was found. A result without
/// a tools array gets an empty one, never `null`.
pub fn filter_tools_lists(value: &mut Value, ids: &[Value], tools: &[String]) -> bool {
    let messages: Vec<&mut Value> = match value {
        Value::Array(items) => items.iter_mut().collect(),
        other => vec![other],
    };
    let mut found = false;
    for message in messages {
        if !message
            .get("id")
            .is_some_and(|id| ids.iter().any(|want| same_id(id, want)))
        {
            continue;
        }
        let Some(result) = message.get_mut("result").and_then(Value::as_object_mut) else {
            continue;
        };
        found = true;
        let listed = match result.remove("tools") {
            Some(Value::Array(listed)) => listed,
            _ => Vec::new(),
        };
        let kept: Vec<Value> = listed
            .into_iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| tools.iter().any(|t| t == name))
            })
            .collect();
        result.insert("tools".into(), Value::Array(kept));
    }
    found
}

/// The server-to-client requests in `value` (a message or a batch) the
/// gateway refuses, removed from it, each with the error to send back.
/// What is left of `value` is `None` if nothing is.
pub fn take_refused_requests(value: Value) -> (Option<Value>, Vec<Value>) {
    let refused = |message: &Value| {
        let method = method(message)?;
        let id = message.get("id")?;
        REFUSED_SERVER_REQUESTS.contains(&method).then(|| {
            error(
                id,
                METHOD_NOT_FOUND,
                &format!("{method} is not available through hennery"),
            )
        })
    };
    match value {
        Value::Array(items) => {
            let mut answers = Vec::new();
            let mut kept = Vec::new();
            for item in items {
                match refused(&item) {
                    Some(answer) => answers.push(answer),
                    None => kept.push(item),
                }
            }
            if answers.is_empty() {
                (Some(Value::Array(kept)), answers)
            } else {
                ((!kept.is_empty()).then_some(Value::Array(kept)), answers)
            }
        }
        message => match refused(&message) {
            Some(answer) => (None, vec![answer]),
            None => (Some(message), Vec::new()),
        },
    }
}

/// Any JSON, refusing a key twice in one object at any depth.
struct Unique;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON without duplicate keys")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_str<E>(self, _: &str) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
        while seq.next_element::<Unique>()?.is_some() {}
        Ok(Unique)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(de::Error::custom("a key twice in one object"));
            }
            map.next_value::<Unique>()?;
        }
        Ok(Unique)
    }
}

/// Where the first complete event in `buf` ends (gateway spec §5.5's SSE
/// framing): `Ok`, just past the empty line that ends it. A line ends in
/// `\r\n`, `\n` or `\r`; a `\r` as the last byte may be half of a `\r\n`
/// still to come, so it ends nothing yet. The scan starts at `from`, the
/// start of a line; without an end, `Err` is where the next scan of the same
/// event, with more bytes, starts (the review's O2: no rescan of a long
/// event from its beginning).
pub fn event_end(buf: &[u8], from: usize) -> Result<usize, usize> {
    let mut line_start = from;
    let mut i = from;
    while i < buf.len() {
        let end = match buf[i] {
            b'\n' => i + 1,
            b'\r' if i + 1 == buf.len() => return Err(line_start),
            b'\r' if buf[i + 1] == b'\n' => i + 2,
            b'\r' => i + 1,
            _ => {
                i += 1;
                continue;
            }
        };
        if i == line_start {
            return Ok(end);
        }
        line_start = end;
        i = end;
    }
    Err(line_start)
}

/// One complete event, as its fields: the data lines' values joined by
/// `\n` (`None` without one), and every other line as it was.
fn event_parts(event: &[u8]) -> (Option<String>, Vec<&[u8]>) {
    let mut data: Option<String> = None;
    let mut others = Vec::new();
    let text_lines = event
        .split(|b| *b == b'\n')
        .flat_map(|line| line.split(|b| *b == b'\r'))
        .filter(|line| !line.is_empty());
    for line in text_lines {
        let value = if line == b"data" {
            Some(&b""[..])
        } else {
            line.strip_prefix(b"data:")
                .map(|rest| rest.strip_prefix(b" ").unwrap_or(rest))
        };
        match value {
            Some(value) => {
                let value = String::from_utf8_lossy(value);
                match &mut data {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(&value);
                    }
                    None => data = Some(value.into_owned()),
                }
            }
            None => others.push(line),
        }
    }
    (data, others)
}

/// What became of one event.
#[derive(Debug, PartialEq)]
pub enum EventOutcome {
    /// Passed on as it came.
    Unchanged,
    /// Passed on as these bytes instead.
    Rewritten(Vec<u8>),
    /// Not passed on.
    Dropped,
    /// Not passed on: its data is not JSON, has a key twice in an object
    /// or spells a key the gateway reads otherwise, or a line of it starts
    /// with a byte-order mark, so the gateway cannot read what a client
    /// might (the review's B2, the re-confirmation's note 1, the Task 2
    /// review's finding 3, the whole-branch review).
    Unreadable,
}

/// Apply the gateway's rules to one complete event: refused server
/// requests taken out (their errors pushed to `answers`), `tools/list`
/// responses whose id is in `ids` filtered to `allowlist`. An event without
/// data (a comment, a ping) or one no rule touches passes as it came, byte
/// for byte; one whose data is not JSON, or has a key twice, is
/// `Unreadable`.
pub fn rewrite_event(
    event: &[u8],
    ids: &[Value],
    allowlist: Option<&[String]>,
    answers: &mut Vec<Value>,
) -> EventOutcome {
    // A line that starts with a byte-order mark: if this event is the first
    // the client sees, its parser strips the mark and reads a field the
    // gateway read as another (the re-confirmation's note 1).
    if event
        .split(|b| *b == b'\n' || *b == b'\r')
        .any(|line| line.starts_with(BOM))
    {
        return EventOutcome::Unreadable;
    }
    let (Some(data), others) = event_parts(event) else {
        return EventOutcome::Unchanged;
    };
    // Not JSON to serde_json, or a key twice in an object: serde_json keeps
    // the last, a client may keep the first (decision 6's reason, for what
    // comes down).
    let read = serde_json::from_str::<Unique>(&data).and_then(|_| serde_json::from_str::<Value>(&data));
    let value = match read {
        Ok(value) => value,
        Err(_) => return EventOutcome::Unreadable,
    };
    // A refused request's `method` spelt otherwise (the whole-branch
    // review): a client whose decoder ignores case would run it.
    if respelt_keys(&value) {
        return EventOutcome::Unreadable;
    }
    let (kept, refused) = take_refused_requests(value);
    let changed = !refused.is_empty();
    answers.extend(refused);
    let Some(mut kept) = kept else {
        return EventOutcome::Dropped;
    };
    let filtered = match allowlist {
        Some(tools) if !ids.is_empty() => filter_tools_lists(&mut kept, ids, tools),
        _ => false,
    };
    if !changed && !filtered {
        return EventOutcome::Unchanged;
    }
    let mut out = Vec::new();
    for line in others {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(&serde_json::to_vec(&kept).expect("a JSON value serialises"));
    out.extend_from_slice(b"\n\n");
    EventOutcome::Rewritten(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<String> {
        vec!["search".into()]
    }

    #[test]
    fn a_body_with_a_key_twice_or_not_json_is_invalid() {
        for body in [
            &br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"ping"}"#[..],
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name":"delete"}}"#,
            br#"[{"a":1},{"b":{"c":1,"c":2}}]"#,
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"x","method":"y"}"#,
            br#"{"jsonrpc":"2.0","id":1,"method":"ping",}"#,
            b"",
            b"not json",
        ] {
            assert!(
                matches!(inspect_request(body, None), Inspected::Invalid(_)),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_tools_call_outside_the_allowlist_is_answered_here() {
        let body = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"delete"}}"#;
        let Inspected::Answer(Some(answer)) = inspect_request(body, Some(&tools())) else {
            panic!("forwarded");
        };
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["error"]["code"], TOOL_NOT_AVAILABLE);
        // Without an allowlist, or for a listed tool, it is forwarded as it came.
        assert_eq!(
            inspect_request(body, None),
            Inspected::Forward {
                body: body.to_vec(),
                tools_list: vec![]
            }
        );
        let ok = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search"}}"#;
        assert!(matches!(inspect_request(ok, Some(&tools())), Inspected::Forward { .. }));
        // A name that is not a string is not on any list.
        let odd = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":["search"]}}"#;
        assert!(matches!(
            inspect_request(odd, Some(&tools())),
            Inspected::Answer(Some(_))
        ));
        // A notification of one: nothing to answer, nothing sent.
        let note = br#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"delete"}}"#;
        assert_eq!(inspect_request(note, Some(&tools())), Inspected::Answer(None));
    }

    #[test]
    fn a_batch_with_a_refused_call_is_answered_element_by_element() {
        let body = br#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"},
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"delete"}},
            {"jsonrpc":"2.0","method":"notifications/x"}]"#;
        let Inspected::Answer(Some(Value::Array(answers))) = inspect_request(body, Some(&tools())) else {
            panic!("forwarded");
        };
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0]["id"], 1);
        assert_eq!(answers[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(answers[1]["id"], 2);
        assert_eq!(answers[1]["error"]["code"], TOOL_NOT_AVAILABLE);
    }

    #[test]
    fn initialize_loses_the_capabilities_not_forwarded() {
        let body = br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18",
            "capabilities":{"sampling":{},"elicitation":{},"roots":{"listChanged":true},"experimental":{"x":1}},
            "clientInfo":{"name":"c","version":"1"}}}"#;
        let Inspected::Forward { body, .. } = inspect_request(body, None) else {
            panic!("not forwarded");
        };
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["params"]["capabilities"], json!({ "experimental": { "x": 1 } }));
        assert_eq!(sent["params"]["clientInfo"]["name"], "c");
        // Nothing to strip: the bytes as they came.
        let plain = br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"capabilities":{}}}"#;
        assert_eq!(
            inspect_request(plain, None),
            Inspected::Forward {
                body: plain.to_vec(),
                tools_list: vec![]
            }
        );
    }

    #[test]
    fn tools_list_ids_are_kept_only_under_an_allowlist() {
        let body = br#"[{"jsonrpc":"2.0","id":"a","method":"tools/list"},{"jsonrpc":"2.0","id":3,"method":"ping"}]"#;
        let Inspected::Forward { tools_list, .. } = inspect_request(body, Some(&tools())) else {
            panic!("not forwarded");
        };
        assert_eq!(tools_list, vec![json!("a")]);
        let Inspected::Forward { tools_list, .. } = inspect_request(body, None) else {
            panic!("not forwarded");
        };
        assert!(tools_list.is_empty());
    }

    #[test]
    fn a_tools_list_response_is_filtered_and_an_empty_one_is_an_array() {
        let mut value = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [
            { "name": "search" }, { "name": "delete" }, { "nameless": true }
        ], "nextCursor": "c" } });
        assert!(filter_tools_lists(&mut value, &[json!(1)], &tools()));
        assert_eq!(value["result"]["tools"], json!([{ "name": "search" }]));
        assert_eq!(value["result"]["nextCursor"], "c");
        let mut none = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": null } });
        assert!(filter_tools_lists(&mut none, &[json!(1)], &[]));
        assert_eq!(none["result"]["tools"], json!([]));
        // Another id, or an error: untouched.
        let mut other = json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [{ "name": "delete" }] } });
        assert!(!filter_tools_lists(&mut other, &[json!(1)], &tools()));
        assert_eq!(other["result"]["tools"][0]["name"], "delete");
        // A batch, element by element.
        let mut batch = json!([
            { "jsonrpc": "2.0", "id": 1, "result": { "tools": [{ "name": "delete" }] } },
            { "jsonrpc": "2.0", "id": 2, "result": { "tools": [{ "name": "delete" }] } }
        ]);
        assert!(filter_tools_lists(&mut batch, &[json!(1)], &tools()));
        assert_eq!(batch[0]["result"]["tools"], json!([]));
        assert_eq!(batch[1]["result"]["tools"][0]["name"], "delete");
    }

    #[test]
    fn events_end_at_an_empty_line_of_any_line_ending() {
        assert_eq!(event_end(b"data: a\n\nrest", 0), Ok(9));
        assert_eq!(event_end(b"data: a\r\n\r\nrest", 0), Ok(11));
        assert_eq!(event_end(b"data: a\r\rrest", 0), Ok(9));
        assert_eq!(event_end(b"data: a\n", 0), Err(8));
        assert_eq!(event_end(b"data: a\r\n\r", 0), Err(9));
        assert_eq!(event_end(b"data: a\n\r", 0), Err(8));
        assert_eq!(event_end(b": ping\n\n", 0), Ok(8));
        assert_eq!(event_end(b"\n", 0), Ok(1));
        assert_eq!(event_end(b"data: a", 0), Err(0));
        // Going on from where the last scan stopped finds the same end.
        let event = b"id: 1\ndata: a\r\n\r\nrest";
        for cut in 0..event.len() {
            let mut from = 0;
            if let Err(resume) = event_end(&event[..cut], 0) {
                from = resume;
            }
            assert_eq!(event_end(event, from), Ok(17), "cut at {cut}");
        }
    }

    #[test]
    fn an_event_is_rewritten_only_when_a_rule_applies() {
        let mut answers = Vec::new();
        let plain = b"id: 4\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n\n";
        assert_eq!(
            rewrite_event(plain, &[json!(1)], Some(&tools()), &mut answers),
            EventOutcome::Unchanged
        );
        assert_eq!(
            rewrite_event(b": comment\n\n", &[], None, &mut answers),
            EventOutcome::Unchanged
        );
        assert_eq!(
            rewrite_event(b"data: not json\n\n", &[], None, &mut answers),
            EventOutcome::Unreadable
        );
        // A lone surrogate: JSON to some parsers, not to serde_json.
        assert_eq!(
            rewrite_event(
                b"data: {\"id\":1,\"method\":\"x\",\"p\":\"\\ud800\"}\n\n",
                &[],
                None,
                &mut answers
            ),
            EventOutcome::Unreadable
        );
        let list =
            b"id: 5\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n";
        let EventOutcome::Rewritten(out) = rewrite_event(list, &[json!(1)], Some(&tools()), &mut answers) else {
            panic!("not rewritten");
        };
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("id: 5\ndata: "), "{out}");
        assert!(out.ends_with("\n\n"), "{out}");
        let data: Value = serde_json::from_str(out.trim_end().strip_prefix("id: 5\ndata: ").unwrap()).unwrap();
        assert_eq!(data["result"]["tools"], json!([]));
        assert!(answers.is_empty());
        let sampling =
            b"data: {\"jsonrpc\":\"2.0\",\"id\":\"s1\",\"method\":\"sampling/createMessage\",\"params\":{}}\n\n";
        assert_eq!(rewrite_event(sampling, &[], None, &mut answers), EventOutcome::Dropped);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0]["id"], "s1");
        assert_eq!(answers[0]["error"]["code"], METHOD_NOT_FOUND);
        // A line that starts with a byte-order mark: unreadable, wherever it is.
        let marked = b"\xef\xbb\xbfdata: {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(marked, &[], None, &mut answers), EventOutcome::Unreadable);
        let later = b"id: 1\n\xef\xbb\xbf: x\ndata: {}\n\n";
        assert_eq!(rewrite_event(later, &[], None, &mut answers), EventOutcome::Unreadable);
        // A notification of that name has no id: nothing to answer, passed on.
        let note = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(note, &[], None, &mut answers), EventOutcome::Unchanged);
    }
}
