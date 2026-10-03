//! The differential harness (fleet rule: a security filter on parsed input
//! gets a differential test): a JSON tree that keeps every key, the
//! decoders the gateway's verdicts must hold under, and a client's
//! event-stream parser. Plans 8e and 8f reuse it for their own filters:
//! from this crate's tests, `mod support;` and `support::differential::…`;
//! from another crate's, `#[path = "…/hennery-gateway/tests/support/differential.rs"] mod differential;`.
//! It needs only `serde` and `serde_json`. Its constants are the
//! gateway's, checked by `jsonrpc`'s unit tests, which include this file.

#![allow(dead_code)]
//!
//! The decoders, as measured (plan 2026-10-15 "gateway differential"):
//! - `Exact`: last of two keys wins, names compared exactly (serde_json,
//!   JavaScript's `JSON.parse`, Python's `json`).
//! - `First`: first wins, exactly (RapidJSON's `FindMember`, simdjson).
//! - `GoV1`: last wins, names matched ignoring case with `ſ` as `s` and the
//!   Kelvin sign as `k` (Go's `encoding/json`).
//! - `GoV2Fold`: as `GoV1`, also ignoring `_` and `-` (Go's
//!   `encoding/json/v2` asked to match case-insensitively; not run).
//! - `JsonC`: last wins, keys and strings cut at the first NUL (json-c 0.18).
//! - `CJson`: first wins, names matched ignoring ASCII case, keys and
//!   strings cut at the first NUL (cJSON 1.7.19's `cJSON_GetObjectItem`).

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

pub const READ_METHODS: &[&str] = &["tools/call", "tools/list", "initialize"];
pub const STRIPPED: &[&str] = &["sampling", "elicitation", "roots"];
pub const REFUSED: &[&str] = &["sampling/createMessage", "elicitation/create", "roots/list"];

// --- A JSON tree that keeps every key, in order, twice if twice -----------

#[derive(Debug, Clone)]
pub enum Node {
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
pub enum Decoder {
    Exact,
    First,
    GoV1,
    GoV2Fold,
    JsonC,
    CJson,
}

pub const DECODERS: &[Decoder] = &[
    Decoder::Exact,
    Decoder::First,
    Decoder::GoV1,
    Decoder::GoV2Fold,
    Decoder::JsonC,
    Decoder::CJson,
];

/// A string cut at its first NUL, as a C reader hands it on.
pub fn cut_at_nul(s: &str) -> &str {
    s.split('\0').next().unwrap_or_default()
}

impl Decoder {
    /// A key as this decoder compares it with a name.
    pub fn key(self, key: &str) -> String {
        match self {
            Decoder::Exact | Decoder::First => key.to_owned(),
            Decoder::GoV1 => key.chars().map(go_fold).collect(),
            Decoder::GoV2Fold => key.chars().filter(|c| !matches!(c, '_' | '-')).map(go_fold).collect(),
            Decoder::JsonC => cut_at_nul(key).to_owned(),
            Decoder::CJson => cut_at_nul(key).to_ascii_lowercase(),
        }
    }

    pub fn first_wins(self) -> bool {
        matches!(self, Decoder::First | Decoder::CJson)
    }

    /// The member `name` of `node`, as this decoder finds it.
    pub fn get<'a>(self, node: &'a Node, name: &str) -> Option<&'a Node> {
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
    pub fn str(self, node: Option<&Node>) -> Option<String> {
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

pub fn messages(root: &Node) -> Vec<&Node> {
    match root {
        Node::Arr(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// What no decoder may read in a body the gateway forwarded under the
/// allowlist `allowed`: a `tools/call` of another tool, an `initialize`
/// with a capability not forwarded, or a method the gateway reads that the
/// gateway did not read there.
pub fn forwarded_is_safe(body: &[u8], allowed: &[&str]) -> Result<(), String> {
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
                    if !name.as_deref().is_some_and(|n| allowed.contains(&n)) {
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

/// What no decoder may read in a message the client received: a refused
/// server request, and, under the allowlist `allowed`, a tool outside it in
/// any `result.tools`, whatever the message's id (gateway spec §5.5).
pub fn received_is_safe(root: &Node, allowed: Option<&[&str]>) -> Result<(), String> {
    for (i, message) in messages(root).into_iter().enumerate() {
        for &d in DECODERS {
            let method = d.str(d.get(message, "method"));
            if let Some(m) = method.as_deref()
                && REFUSED.contains(&m)
                && d.get(message, "id").is_some()
            {
                return Err(format!("{d:?} reads a {m} request in message {i}"));
            }
            let tools = d.get(message, "result").and_then(|r| d.get(r, "tools"));
            if let Some(allowed) = allowed
                && let Some(Node::Arr(tools)) = tools
            {
                for tool in tools {
                    let name = d.str(d.get(tool, "name"));
                    if !name.as_deref().is_some_and(|n| allowed.contains(&n)) {
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
pub fn client_events(text: &[u8]) -> Vec<String> {
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

/// What the client received, as messages, for a body of `content_type`.
/// Bytes that are not UTF-8 are a failure: a client may decode them
/// otherwise than any model here.
pub fn received(content_type: &str, body: &[u8]) -> Result<Vec<Node>, String> {
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
