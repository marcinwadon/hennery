# MCP gateway: JSON answers read whole, every tools list filtered Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** close what plan 2026-10-15 "gateway differential" left open, before plan 8e mints tokens.
- **Server requests inside JSON answers** (8d's Q1, reversed by the lane parent and ruled by the fleet parent, 2026-10-02). MCP's TypeScript SDK and rmcp dispatch every message in an `application/json` answer, requests included, so §5.6's "in a response stream" failed against a hostile upstream.
- **A `GET` stream and its `Last-Event-ID` replay** went unfiltered.
- **The two gaps the fleet parent accepted** are documented and pinned by tests: an id of another type, and an answer on another stream.
- **The differential harness** is made reusable for 8e and 8f.

**Architecture:**
- Every JSON answer is read whole, capped at 8 MiB, and judged by one new classifier, `jsonrpc::inspect_answer`, with four outcomes: `Unchanged`, `Rewritten`, `Unreadable` and `ServerRequest`.
- `jsonrpc::filter_tools` replaces `filter_tools_lists`. It filters every `result.tools`, whatever the id. The `tools/list` ids the proxy kept, `same_id`, `Inspected::Forward`'s `tools_list` and the proxy's JSON `passthrough` are gone.
- The decoder models move from `tests/differential.rs` to `tests/support/differential.rs`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), serde_json, axum 0.8, reqwest, tokio. No new crate.

**Spec:** gateway §5.3, §5.5, §5.6 and §11, written back in Task 3. It builds on plan 2026-10-15 "gateway differential" (PR #93), and supersedes 8d's decision 17 and that plan's decision 8: no id matching is left.

**Base:** `main` at `ecc50cd` (PR #93 merged). Anchors are taken from `ecc50cd`.

**Status:** written and executed 2026-10-02 (see "Execution status"). Amended after the security review of 2026-10-02. See "The security review's answers".

**How the code blocks were made and checked:**
- Every block below was generated from the branch's commits by a script, one region of a file per block.
- The plan was replayed from its own text onto `ecc50cd` in a scratch worktree, task by task; each commit's tree matched the branch's, byte for byte.
- The 11 revert-probes of Task 2 were run, and each failed as expected.

## Execution status (2026-10-02)

**Executed** on `main` at `ecc50cd` as the branch `fix/gateway-json-answers`. It was first stacked on #93, then rebased when #93 merged. The tests came first, and fail on `ecc50cd` as Task 1's Step 2 says; then the fix; then the security review's amendments. The plan was generated from the final commits and replayed onto `ecc50cd`: 53 blocks, 7 files, each task's tree byte for byte.

| Area | As built | Why |
|---|---|---|
| The tables | 69 request vectors, 61 response vectors, 2 raw-byte events. The response vectors run twice, with and without an allowlist, JSON ones too now. | JSON answers are now judged without an allowlist as well. |
| `#[path]` from `src` | The drift check includes the harness from `jsonrpc.rs` at file level (`#[path = "../tests/support/differential.rs"]`), not from inside `mod tests`. | Inside an inline module the path starts at `src/jsonrpc/tests/`, which does not exist, so `..` cannot climb out of it. |
| Signing | Commits are signed (the operator's decision of 2026-10-02), with the gmail identity. | |

Checks:
- fmt, both clippy runs and `gen --check` passed. The workspace tests passed: 1404, from 1399.
- The 11 revert-probes each failed as expected. `q8`'s first form did not compile and was rewritten as a streaming passthrough.
- The run was macOS only, so ubuntu CI is the Linux check.

## Scope

1. Every JSON answer read whole (8 MiB), and judged as an event's data is.
2. A refused server request in a JSON answer: the answer refused, 502.
3. Every `result.tools` filtered under an allowlist, whatever its id: JSON, SSE, `GET`, replays.
4. The id-type and other-stream gaps, accepted: documented in §5.5 and pinned by tests.
5. The differential harness, shared.

**Out:** nothing else of the proxy changes.

## Decisions this plan makes where the spec is silent

1. **Every JSON answer is read whole, with an allowlist or without** (the lane parent's ruling).
   - The cap is 8 MiB, the one `tools/list` had; past it, 502 `upstream_too_large`, never truncated. An empty body (a 202 typed JSON) passes, empty.
   - It goes on as it came, unless the tools filter touched it.
   - Cost:
     - A JSON answer no longer reaches the client before its end. A client can use none of it before then, so only the latency of the last byte counts.
     - A `POST` waits on a JSON body that trickles, holding its request permit, as the streamed body did.
     - The memory bound per request in flight is the 8 MiB read, plus the parse that refuses a key twice, the `Value` tree, and a copy when rewritten: several times 8 MiB at worst, for at most 64 requests in flight per connection (§5.7). 8d's `tools/list` path had the same bound, under an allowlist only.
     - The upstream body has no deadline: `head_timeout` ends at the head, and §5.7 sets no idle timeout. A trickling upstream holds the request's permit until the client gives up, as the streamed body did; but now the client also has no head meanwhile. A body deadline is a candidate for 8e or 8f (the security review's finding 4).
   - The first-chunk guarantee is now for event streams (§5.3). `the_first_chunk_arrives_before_the_upstream_finishes` asserts both: a JSON answer is held while its upstream blocks, an event is not.
2. **A server request the gateway refuses inside a JSON answer makes the whole answer 502 `upstream_invalid`** (an `ApiError`), and nothing is answered upstream. Why refuse rather than filter it out, as an event stream's is:
   - A JSON body is the `POST`'s response. MCP's streamable HTTP sends server requests only in an event stream, so the upstream broke the transport.
   - Taking the request out would hand the client a partial answer, or for a lone request an empty body where it awaits its response. Either way it is a wrong answer.
   - Answering upstream would take the event-stream path's `Answerer` into the JSON path, for an upstream that is already misbehaving.

   A notification of those names (no id) passes, as in an event stream. The 502's message names no method.
3. **Every `result.tools` is filtered, whatever the id** (the lane parent's (b) and the fleet parent's (a)).
   - That covers `GET` streams, `Last-Event-ID` replays, ids of another type, answers on another stream, and those with no id.
   - A result without `tools` is untouched. Before, an answer to a `tools/list` without one got `[]`. A `tools` that is not an array becomes `[]`.
   - Accepted: a non-`tools/list` result with a `tools` key is filtered too, under an allowlist. No MCP result but `tools/list`'s has one.
   - Accepted (the security review's finding 5): a tool list inside an error (`error.data.tools`) is passed on. No client reads tools from an error, and the `tools/call` refusal still enforces.
   - Accepted (finding 6): a 2xx JSON answer that ends as 502 still sets the connection `ok` (§7), since `mark_ok` runs at the head. The status speaks for the credential, which the upstream accepted, not for the body.
   - So the proxy keeps no request ids. 8d's decision 17 (`same_id`) and plan "gateway differential"'s decision 8 (a `tools/list` without an id kept as `null`) are gone with them. Their tests still pass, and now hold for every id.
4. **An id of another type is accepted** (the fleet parent's (c)). The gateway's own answers echo an id as sent: integers digit for digit, a string as a string. A client's `Number(id)` is the client's. Pinned: `ids_are_echoed_exactly_and_still_filtered` (the string `"1"` comes back a string) and the rows `json id of another type`, `sse id of another type`.
5. **An answer on another stream is accepted** (the fleet parent's (d)), because the proxy keeps nothing between requests. Each `POST` or `GET` is its own upstream exchange.
   - Pinned: `each_session_sees_only_its_own_upstream_stream`. Two sessions on one connection open their `GET` streams at once, each with its own upstream session, and each receives only its own 20 events and its own `Mcp-Session-Id`.
   - The upstream streams each event as its own chunk with a pause between them, so the two streams interleave inside the proxy. It answers with its own `Mcp-Session-Id` (`srv-…`), so the header assertion cannot pass on the client's (the security review's finding 3).
   - There is no line to revert-probe: the property is the absence of shared state.
   - **Open for 8e (the security review's finding 2):** the upstream `Mcp-Session-Id` is all that separates two sessions' streams. The gateway forwards the client's, does not bind it to the token, and all tokens on a connection share its credential. So a token that presents another session's id gets what the upstream serves for it. Written into §5.5.
6. **The harness is shared.** `tests/support/differential.rs` holds:
   - `Node`, a JSON tree keeping every key, duplicates and order;
   - `Decoder` and `DECODERS`, the six measured decoders;
   - `forwarded_is_safe(body, allowed)`;
   - `received_is_safe(message, allowed)`, now checking every `result.tools` whatever its id;
   - `client_events` (WHATWG framing) and `received` (strict UTF-8).

   Use:
   - from this crate's tests, `mod support; use support::differential::…`;
   - from another crate's, `#[path = "…/hennery-gateway/tests/support/differential.rs"] mod differential;`, needing only `serde` and `serde_json`.

   - The file has its own `#![allow(dead_code)]`, so it builds under `-D warnings` wherever it is included.
   - Its `REFUSED`, `STRIPPED` and `READ_METHODS` are checked against the gateway's own by a `jsonrpc` unit test that includes it the same way (`the_differential_harness_reads_the_gateway_s_names`), so they cannot drift (finding 7).

   A new filter adds rows to its own table and asserts with these.

## The security review's answers

**The security review (opus, 2026-10-02), on the maintainer's behalf: approve after amendments**, all taken.
- Method: it ran the gateway's suite at `3ad58e9` in a scratch worktree, and its own vectors through the proxy. Every one of these was 502 `upstream_invalid`, with and without an allowlist:
  - a 500 holding `sampling/createMessage`;
  - a 400 batch of tool results and `roots/list`;
  - a 202 holding `elicitation/create`;
  - `"id": null` holding `roots/list`.
- It found the code correct and failing closed, decision 2 sound, and the 502's message free of upstream content.
- Removing the id tracking lost nothing.
- Every outcome of `Answered` is probed.

| Finding | Taken how |
|---|---|
| **1 (blocking)** The plan's execution status was a placeholder | Filled in |
| 2 §5.5 claimed more separation between sessions than the code gives | §5.5 reworded; the unbound `Mcp-Session-Id` is open for 8e (decision 5) |
| 3 The two-session test sent each stream in one chunk, and its header check could pass on the client's own header | Interleaved streams, the upstream's own `srv-…` header (decision 5) |
| 4 The memory bound and the missing body deadline were understated | Decision 1 states both; a body deadline handed to 8e or 8f |
| 5 `error.data.tools` passes | Accepted, decision 3 and §5.5 |
| 6 `mark_ok` runs before the JSON body is judged | Accepted, decision 3 |
| 7 The harness needed its own `allow(dead_code)`, and its constants could drift from the gateway's | Both taken (decision 6) |
| 8 No row mixed tool results with a refused request; no non-2xx status | Row `json tools and a refused request`; `a_server_request_in_a_json_answer_is_502` now runs 200, 202, 400 and 500. The id-respelling JSON rows now pass for two reasons, which is why `q4` does not name them |

**The scoped re-confirmation (a fresh opus reviewer, 2026-10-02), scoped to the amendments: confirmed with notes, none blocking.** It ran the gateway's tests at `cd6ae88` in a scratch worktree. Its notes:
1. The execution status is accurate. Five new tests take the count from 1399 to 1404.
2. The §5.5 wording on `Mcp-Session-Id` matches the code; it is open for 8e.
3. The two-session test is now meaningful: a proxy that swapped or shared sessions fails it. It does not prove that the streams overlapped in time.
4. Decision 1's memory and deadline wording is accurate.
5. Accepting `error.data.tools` and `mark_ok` is right. Calling `mark_ok` only after the body is judged would be a reasonable later cleanup.
6. The `#[cfg(test)]` `#[path]` include is sound: never in a non-test build, and it resolves from `src/`. `q11` is real, and the check fails if either side changes.
7. `json tools and a refused request` is `Safe`, which a strip would also satisfy. The 502 itself is pinned by `a_server_request_in_a_json_answer_is_502` (statuses 200, 202, 400 and 500).
8. Nothing was loosened: all 11 probes fail as stated.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`. After every task the five checks pass.
- **No new crates, no wire types, no migration.**
- Fail closed: what the gateway cannot judge is 502 (JSON) or dropped (an event), never passed on.
- Every outcome of `Answered` has its own positive test and its own revert-probe (fleet rule).
- Commits: Conventional Commits, the gmail identity (signing allowed since 2026-10-02; `git log --format='%ae' origin/main..HEAD` before every push).

## Review Focus

1. **A server request inside a JSON answer.**
   - Expected: 502 `upstream_invalid`, alone or in a batch, with an allowlist or without; nothing answered upstream.
   - Tests: `a_server_request_in_a_json_answer_is_502`; the `json sampling`, `json refused in a batch`, `json METHOD`, `json NUL method value` and `json method twice` rows, with and without an allowlist; `a_json_answer_has_one_outcome_of_four`.
2. **Every tools list filtered.**
   - Expected: a `GET`, a replay, an id of another type, another id, and no id are all filtered.
   - Tests: `a_get_stream_and_its_replay_are_filtered`; the `… id …` rows; `every_tools_list_is_filtered_and_an_empty_one_is_an_array`.
3. **JSON read whole.**
   - Expected: 8 MiB cap, an empty body passes, JSON held while its upstream blocks, the body-failure log line kept.
   - Tests: `a_json_answer_is_read_whole_and_capped`, `the_first_chunk_arrives_before_the_upstream_finishes`, `proxy_log`.
4. **The accepted gaps,** pinned: decisions 4 and 5.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-gateway/tests/support/differential.rs` (new), `tests/support/mod.rs` | the shared harness | 1 |
| `crates/hennery-gateway/tests/differential.rs`, `tests/proxy.rs` | the vectors and tests | 1 |
| `crates/hennery-gateway/src/jsonrpc.rs` | `filter_tools`, `Answered`, `inspect_answer`; ids gone | 2 |
| `crates/hennery-gateway/src/proxy.rs` | every JSON answer read whole and judged; `passthrough` gone | 2 |
| `docs/specs/2026-09-26-mcp-gateway-design.md` | §5.3, §5.5, §5.6, §11 | 3 |

**Reading the steps:** as in plan "gateway differential": "Create" is a whole file; "In `path`, replace: … with: …" replaces its one occurrence, in order.

---
### Task 1: The tests, and the shared harness

**Files:** Create `crates/hennery-gateway/tests/support/differential.rs`; modify `tests/differential.rs`, `tests/proxy.rs`, `tests/support/mod.rs`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
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
```

with:

```rust
//! - **Down:** no decoder reads in what the client received a refused
//!   server request, in JSON or an event stream, or a tool outside the
//!   allowlist in any `result.tools`, whatever its id.
//!
//! The decoders are `support::differential`'s, each measured against a
//! real one (Go, json-c, cJSON).
//!
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
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

```

with:

```rust
use hennery_gateway::model::CredKind;
use serde_json::{Value, json};
use support::differential::{forwarded_is_safe, received, received_is_safe};
use support::upstream::{FakeUpstream, Harness};

const ALLOWED: &str = "search";

```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
        for up in sent {
            if let Err(why) = forwarded_is_safe(&up.body) {
                failures.push(format!("{what}: {why}"));
```

with:

```rust
        for up in sent {
            if let Err(why) = forwarded_is_safe(&up.body, &[ALLOWED]) {
                failures.push(format!("{what}: {why}"));
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
    ("sse batch in a batch", Kind::Sse, "data: [[{\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}]]\n\n", Safe),
    ("sse refused in a batch", Kind::Sse, "data: [{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"delete\"}]}},{\"jsonrpc\":\"2.0\",\"id\":\"s\",\"method\":\"roots/list\"}]\n\n", Forwarded),
```

with:

```rust
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
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
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

```

with:

```rust
];

```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
            for message in &messages {
                if let Err(why) = received_is_safe(message, 1.0, allowlist) {
                    failures.push(format!("{what}: {why}"));
```

with:

```rust
            for message in &messages {
                if let Err(why) = received_is_safe(message, allowlist.then_some(&[ALLOWED][..])) {
                    failures.push(format!("{what}: {why}"));
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
/// The refusal of server requests does not depend on an allowlist: every
/// event-stream vector again, on a connection without one. (Without one, a
/// JSON answer is passed on unread, as 8d's Q1 ruling has it.)
#[tokio::test]
```

with:

```rust
/// The refusal of server requests does not depend on an allowlist: every
/// vector again, in JSON and event streams, on a connection without one
/// (gateway spec §5.6: in any answer).
#[tokio::test]
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
    for &(what, kind, body, expect) in RESPONSES {
        if matches!(kind, Kind::Sse) {
            failures.extend(answer_vector(&s, what, kind, body.as_bytes(), expect, false).await);
        }
    }
```

with:

```rust
    for &(what, kind, body, expect) in RESPONSES {
        failures.extend(answer_vector(&s, what, kind, body.as_bytes(), expect, false).await);
    }
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
    }
    for (id, value) in [("-0", 0.0), ("1e0", 1.0)] {
```

with:

```rust
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
```

In `crates/hennery-gateway/tests/differential.rs`, replace:

```rust
    assert_eq!(answer["result"]["tools"], json!([{"name": "search"}]));
}
```

with:

```rust
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
#[tokio::test]
async fn each_session_sees_only_its_own_upstream_stream() {
    let s = setup().await;
    let hat = s.h.hat();
    let other = s.h.mint("s2", "host-a", &hat);
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
    let open = |token: String, upstream_session: &'static str| {
        let client = s.h.client.clone();
        let url = s.h.url("linear");
        async move {
            let resp = client
                .get(url)
                .bearer_auth(token)
                .header(header::ACCEPT, "text/event-stream")
                .header("mcp-session-id", upstream_session)
                .send()
                .await
                .unwrap();
            assert_eq!(
                resp.headers()["mcp-session-id"],
                format!("srv-{upstream_session}").as_str()
            );
            resp.text().await.unwrap()
        }
    };
    let (one, two) = tokio::join!(open(s.token.clone(), "up-1"), open(other, "up-2"));
    for (text, mine) in [(one, "up-1"), (two, "up-2")] {
        let events = support::differential::client_events(text.as_bytes());
        assert_eq!(events.len(), 20, "{mine}: {text}");
        for event in events {
            let message: Value = serde_json::from_str(&event).unwrap();
            assert_eq!(message["params"]["for"], mine, "{event}");
        }
    }
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
```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

```rust
async fn the_first_chunk_arrives_before_the_upstream_finishes() {
    for (content_type, first) in [
        ("application/json", r#"{"jsonrpc":"2.0","#.to_string()),
        (
            "text/event-stream",
            event(&json!({"jsonrpc": "2.0", "method": "notifications/progress"})),
        ),
    ] {
        let s = setup(CredKind::None, None).await;
```

with:

```rust
async fn the_first_chunk_arrives_before_the_upstream_finishes() {
    // JSON is read whole (plan 2026-10-15 "gateway JSON answers"): a client
    // can use none of it before its end, and a server request may be in it.
    // While the upstream blocks, nothing of it comes down.
    {
        let s = setup(CredKind::None, None).await;
        s.upstream
            .reply(|_, hold| first_then_block("application/json", r#"{"jsonrpc":"2.0","#, hold));
        let held = tokio::time::timeout(Duration::from_secs(1), s.h.post("linear", &s.token, &ping(1))).await;
        assert!(held.is_err(), "a JSON answer came down before its end");
    }
    for (content_type, first) in [(
        "text/event-stream",
        event(&json!({"jsonrpc": "2.0", "method": "notifications/progress"})),
    )] {
        let s = setup(CredKind::None, None).await;
```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

```rust
    s.upstream
        .reply(|_, hold| first_then_block("application/json", "{", hold));
    let first = s.h.post("linear", &s.token, &ping(1)).await;
```

with:

```rust
    s.upstream
        .reply(|_, hold| first_then_block("text/event-stream", ": open\n\n", hold));
    let first = s.h.post("linear", &s.token, &ping(1)).await;
```

Create `crates/hennery-gateway/tests/support/differential.rs`:

```rust
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

fn cut_at_nul(s: &str) -> &str {
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
```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

```rust

pub mod upstream;
```

with:

```rust

pub mod differential;
pub mod upstream;
```

- [ ] **Step 2: Run them, and watch them fail**

Run: `nix develop -c cargo test -p hennery-gateway --locked --no-fail-fast`
Expected on `ecc50cd`, six failures:
- `differential`'s `a_get_stream_and_its_replay_are_filtered`, `a_json_answer_is_read_whole_and_capped` and `a_server_request_in_a_json_answer_is_502`;
- `no_decoder_reads_past_the_response_filters`, naming `json sampling`, `json refused in a batch` and the five id rows (`json`/`sse id of another type`, `json`/`sse another id`, `json no id`);
- `no_decoder_reads_a_refused_request_without_an_allowlist`, naming the JSON server-request rows and `json BOM`, `json NBSP`, `json trailing comma`;
- `proxy`'s `the_first_chunk_arrives_before_the_upstream_finishes`: "a JSON answer came down before its end".

- [ ] **Step 3: Commit**

```bash
git add crates/hennery-gateway/tests
git commit -m "test(gateway): JSON answers read whole, every tools list filtered, the harness shared"
```

---
### Task 2: Read JSON answers whole, refuse server requests in them, filter every tools list

**Files:** Modify `crates/hennery-gateway/src/jsonrpc.rs`, `crates/hennery-gateway/src/proxy.rs`.

- [ ] **Step 1: The fix**

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
//! - With an allowlist, a `tools/call` for a tool outside it is answered
//!   here, `-32602`, and never reaches the upstream; the ids of `tools/list`
//!   requests are kept, and their responses filtered.
//! - A server-to-client request for one of those capabilities, arriving in
//!   an event stream, is answered here with an error and not passed on.

```

with:

```rust
//! - With an allowlist, a `tools/call` for a tool outside it is answered
//!   here, `-32602`, and never reaches the upstream; and every
//!   `result.tools` coming down is filtered to it, whatever its id, in any
//!   answer (plan 2026-10-15 "gateway JSON answers").
//! - A server-to-client request for one of those capabilities, arriving in
//!   an event stream, is answered here with an error and not passed on; in
//!   a JSON answer, the answer is refused whole (`inspect_answer`).

```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
pub enum Inspected {
    /// Send it upstream: `body` is the client's bytes, or `initialize`
    /// rewritten. `tools_list` holds the ids of its `tools/list` requests
    /// when the connection has an allowlist, whose responses are filtered.
    Forward { body: Vec<u8>, tools_list: Vec<Value> },
    /// Answer it here and send nothing upstream: a JSON-RPC answer, or
```

with:

```rust
pub enum Inspected {
    /// Send it upstream: the client's bytes, or `initialize` rewritten.
    Forward(Vec<u8>),
    /// Answer it here and send nothing upstream: a JSON-RPC answer, or
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
    let mut blocked = false;
    let mut tools_list = Vec::new();
    {
```

with:

```rust
    let mut blocked = false;
    {
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
                Some("tools/call") if allowlist.is_some_and(|tools| !call_allowed(message, tools)) => blocked = true,
                Some("tools/list") if allowlist.is_some() => {
                    // Without an id, as `null`: an upstream that answers it
                    // with `"id": null` is filtered too (the security
                    // review's finding 3).
                    tools_list.push(message.get("id").cloned().unwrap_or(Value::Null));
                }
                _ => {}
```

with:

```rust
                Some("tools/call") if allowlist.is_some_and(|tools| !call_allowed(message, tools)) => blocked = true,
                _ => {}
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
    };
    Inspected::Forward { body, tools_list }
}
```

with:

```rust
    };
    Inspected::Forward(body)
}
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust

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
```

with:

```rust

/// Filter every `result.tools` in `value` (a message or a batch) to
/// `tools`, whatever the message's id: true if one was there. Not only the
/// answers to this exchange's `tools/list` (plan 8d decision 17): a `GET`
/// stream, a replay with `Last-Event-ID`, an answer on another stream or
/// with an id of another type carry a list too (plan 2026-10-15 "gateway
/// JSON answers" decision 3). A `tools` that is not an array is `[]`.
pub fn filter_tools(value: &mut Value, tools: &[String]) -> bool {
    let messages: Vec<&mut Value> = match value {
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
    for message in messages {
        let Some(result) = message.get_mut("result").and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(listed) = result.get_mut("tools") else {
            continue;
        };
        found = true;
        let kept: Vec<Value> = match listed.take() {
            Value::Array(listed) => listed
                .into_iter()
                .filter(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| tools.iter().any(|t| t == name))
                })
                .collect(),
            _ => Vec::new(),
        };
        *listed = Value::Array(kept);
    }
    found
}

/// What to do with a JSON answer, read whole (plan 2026-10-15 "gateway
/// JSON answers").
#[derive(Debug, PartialEq)]
pub enum Answered {
    /// Passed on as it came (or empty).
    Unchanged,
    /// Passed on as these bytes instead: its tools filtered.
    Rewritten(Vec<u8>),
    /// Not passed on: not JSON, a key twice, or `ambiguous` (as an event's
    /// data is judged).
    Unreadable,
    /// Not passed on: it holds a server request the gateway refuses. A
    /// JSON answer is the `POST`'s response, and server requests come only
    /// in an event stream (MCP's streamable HTTP), so the answer is refused
    /// whole rather than passed on without it (decision 2).
    ServerRequest,
}

/// Judge a JSON answer under `allowlist`: the refusal of server requests
/// in any answer (gateway spec §5.6), and the tools filter.
pub fn inspect_answer(bytes: &[u8], allowlist: Option<&[String]>) -> Answered {
    if bytes.is_empty() {
        return Answered::Unchanged;
    }
    let Some(mut value) = read(bytes) else {
        return Answered::Unreadable;
    };
    let messages: Vec<&Value> = match &value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    if messages.into_iter().any(|message| refused_request(message).is_some()) {
        return Answered::ServerRequest;
    }
    match allowlist {
        Some(tools) if filter_tools(&mut value, tools) => {
            Answered::Rewritten(serde_json::to_vec(&value).expect("a JSON value serialises"))
        }
        _ => Answered::Unchanged,
    }
}

/// A server request the gateway refuses (one with an id), as the error to
/// send back for it.
fn refused_request(message: &Value) -> Option<Value> {
    let method = method(message)?;
    let id = message.get("id")?;
    REFUSED_SERVER_REQUESTS.contains(&method).then(|| {
        error(
            id,
            METHOD_NOT_FOUND,
            &format!("{method} is not available through hennery"),
        )
    })
}
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
pub fn take_refused_requests(value: Value) -> (Option<Value>, Vec<Value>) {
    let refused = refused_request;
    match value {
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
/// Apply the gateway's rules to one complete event: refused server
/// requests taken out (their errors pushed to `answers`), every
/// `result.tools` filtered to `allowlist`. An event without
/// data (a comment, a ping) or one no rule touches passes as it came, byte
/// for byte; one whose data is not JSON, or has a key twice, is
/// `Unreadable`.
pub fn rewrite_event(event: &[u8], allowlist: Option<&[String]>, answers: &mut Vec<Value>) -> EventOutcome {
    // A line that starts with a byte-order mark: if this event is the first
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
    let filtered = match allowlist {
        Some(tools) if !ids.is_empty() => filter_tools_lists(&mut kept, ids, tools),
        _ => false,
```

with:

```rust
    let filtered = match allowlist {
        Some(tools) => filter_tools(&mut kept, tools),
        _ => false,
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust

#[cfg(test)]
mod tests {
    use super::*;

```

with:

```rust

/// The differential harness the integration tests share (plan 2026-10-15
/// "gateway JSON answers" decision 6), for the check below.
#[cfg(test)]
#[path = "../tests/support/differential.rs"]
mod harness;

#[cfg(test)]
mod tests {
    use super::*;

    /// Its names are the gateway's, so the two cannot drift apart (the
    /// security review's finding 7).
    #[test]
    fn the_differential_harness_reads_the_gateway_s_names() {
        assert_eq!(harness::REFUSED, REFUSED_SERVER_REQUESTS);
        assert_eq!(harness::STRIPPED, STRIPPED_CAPABILITIES);
        assert_eq!(harness::READ_METHODS, READ_METHODS);
    }

```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
        // Without an allowlist, or for a listed tool, it is forwarded as it came.
        assert_eq!(inspect_request(body, None), Inspected::Forward(body.to_vec()));
        let ok = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search"}}"#;
        assert!(matches!(inspect_request(ok, Some(&tools())), Inspected::Forward(_)));
        // A name that is not a string is not on any list.
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
            "clientInfo":{"name":"c","version":"1"}}}"#;
        let Inspected::Forward { body, .. } = inspect_request(body, None) else {
            panic!("not forwarded");
```

with:

```rust
            "clientInfo":{"name":"c","version":"1"}}}"#;
        let Inspected::Forward(body) = inspect_request(body, None) else {
            panic!("not forwarded");
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
        let plain = br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"capabilities":{}}}"#;
        assert_eq!(inspect_request(plain, None), Inspected::Forward(plain.to_vec()));
    }

    #[test]
    fn every_tools_list_is_filtered_and_an_empty_one_is_an_array() {
        let mut value = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [
            { "name": "search" }, { "name": "delete" }, { "nameless": true }
        ], "nextCursor": "c" } });
        assert!(filter_tools(&mut value, &tools()));
        assert_eq!(value["result"]["tools"], json!([{ "name": "search" }]));
        assert_eq!(value["result"]["nextCursor"], "c");
        let mut none = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": null } });
        assert!(filter_tools(&mut none, &[]));
        assert_eq!(none["result"]["tools"], json!([]));
        // Whatever the id, a string one or none.
        let mut other = json!({ "jsonrpc": "2.0", "id": "1", "result": { "tools": [{ "name": "delete" }] } });
        assert!(filter_tools(&mut other, &tools()));
        assert_eq!(other["result"]["tools"], json!([]));
        // A result without tools, or an error: untouched.
        let mut plain = json!({ "jsonrpc": "2.0", "id": 2, "result": { "content": [] } });
        assert!(!filter_tools(&mut plain, &tools()));
        assert_eq!(plain, json!({ "jsonrpc": "2.0", "id": 2, "result": { "content": [] } }));
        // A batch, element by element.
        let mut batch = json!([
            { "jsonrpc": "2.0", "id": 1, "result": { "tools": [{ "name": "delete" }] } },
            { "jsonrpc": "2.0", "id": 2, "result": {} }
        ]);
        assert!(filter_tools(&mut batch, &tools()));
        assert_eq!(batch[0]["result"]["tools"], json!([]));
        assert_eq!(batch[1]["result"], json!({}));
    }

    #[test]
    fn a_json_answer_has_one_outcome_of_four() {
        let tools = tools();
        let listed = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"search"},{"name":"delete"}]}}"#;
        // Unchanged: without an allowlist, nothing touched; and empty.
        assert_eq!(inspect_answer(listed, None), Answered::Unchanged);
        assert_eq!(inspect_answer(b"", Some(&tools)), Answered::Unchanged);
        assert_eq!(
            inspect_answer(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#, Some(&tools)),
            Answered::Unchanged
        );
        // Rewritten: its tools filtered.
        let Answered::Rewritten(out) = inspect_answer(listed, Some(&tools)) else {
            panic!("not rewritten");
        };
        let out: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out["result"]["tools"], json!([{ "name": "search" }]));
        // Unreadable: as an event's data.
        for bytes in [&b"{"[..], br#"{"id":1,"id":2}"#, br#"{"Id":1,"result":{}}"#] {
            assert_eq!(inspect_answer(bytes, None), Answered::Unreadable);
        }
        // A server request the gateway refuses, alone or in a batch; one
        // without an id is a notification and passes.
        for bytes in [
            &br#"{"jsonrpc":"2.0","id":"s","method":"roots/list"}"#[..],
            br#"[{"jsonrpc":"2.0","id":1,"result":{}},{"jsonrpc":"2.0","id":"s","method":"elicitation/create"}]"#,
        ] {
            assert_eq!(inspect_answer(bytes, None), Answered::ServerRequest);
        }
        assert_eq!(
            inspect_answer(br#"{"jsonrpc":"2.0","method":"roots/list"}"#, None),
            Answered::Unchanged
        );
    }
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
        assert_eq!(
            rewrite_event(plain, Some(&tools()), &mut answers),
            EventOutcome::Unchanged
        );
        assert_eq!(
            rewrite_event(b": comment\n\n", None, &mut answers),
            EventOutcome::Unchanged
        );
        assert_eq!(
            rewrite_event(b"data: not json\n\n", None, &mut answers),
            EventOutcome::Unreadable
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
                b"data: {\"id\":1,\"method\":\"x\",\"p\":\"\\ud800\"}\n\n",
                &[],
                None,
```

with:

```rust
                b"data: {\"id\":1,\"method\":\"x\",\"p\":\"\\ud800\"}\n\n",
                None,
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
            b"id: 5\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n";
        let EventOutcome::Rewritten(out) = rewrite_event(list, &[json!(1)], Some(&tools()), &mut answers) else {
            panic!("not rewritten");
```

with:

```rust
            b"id: 5\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n";
        let EventOutcome::Rewritten(out) = rewrite_event(list, Some(&tools()), &mut answers) else {
            panic!("not rewritten");
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
            b"data: {\"jsonrpc\":\"2.0\",\"id\":\"s1\",\"method\":\"sampling/createMessage\",\"params\":{}}\n\n";
        assert_eq!(rewrite_event(sampling, &[], None, &mut answers), EventOutcome::Dropped);
        assert_eq!(answers.len(), 1);
```

with:

```rust
            b"data: {\"jsonrpc\":\"2.0\",\"id\":\"s1\",\"method\":\"sampling/createMessage\",\"params\":{}}\n\n";
        assert_eq!(rewrite_event(sampling, None, &mut answers), EventOutcome::Dropped);
        assert_eq!(answers.len(), 1);
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
        let marked = b"\xef\xbb\xbfdata: {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(marked, &[], None, &mut answers), EventOutcome::Unreadable);
        let later = b"id: 1\n\xef\xbb\xbf: x\ndata: {}\n\n";
        assert_eq!(rewrite_event(later, &[], None, &mut answers), EventOutcome::Unreadable);
        // A notification of that name has no id: nothing to answer, passed on.
        let note = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(note, &[], None, &mut answers), EventOutcome::Unchanged);
    }
```

with:

```rust
        let marked = b"\xef\xbb\xbfdata: {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(marked, None, &mut answers), EventOutcome::Unreadable);
        let later = b"id: 1\n\xef\xbb\xbf: x\ndata: {}\n\n";
        assert_eq!(rewrite_event(later, None, &mut answers), EventOutcome::Unreadable);
        // A notification of that name has no id: nothing to answer, passed on.
        let note = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(note, None, &mut answers), EventOutcome::Unchanged);
    }
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
//!   one type the gateway judged it by (the review's B1).
//! - **Streaming** (§5.3): every body is passed on chunk by chunk, except a
//!   `tools/list` answer in JSON under an allowlist, which is read whole
//!   (8 MiB at most) and filtered. An event stream is passed on event by
//!   event: a complete event is never held, a partial one waits for its end
//!   (plan 8d decision 5).
//! - **401** (§5.4) is never passed on: `502 upstream_auth`. The one place
```

with:

```rust
//!   one type the gateway judged it by (the review's B1).
//! - **Streaming** (§5.3): a JSON answer is read whole (8 MiB at most) and
//!   judged before any of it goes on: a client can use none of it before its
//!   end, and a server request may be in it (plan 2026-10-15 "gateway JSON
//!   answers"). An event stream is passed on event by event: a complete
//!   event is never held, a partial one waits for its end (plan 8d decision
//!   5).
//! - **401** (§5.4) is never passed on: `502 upstream_auth`. The one place
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust

use crate::jsonrpc::{self, BOM, EventOutcome, Inspected};
use crate::key::MasterKey;
```

with:

```rust

use crate::jsonrpc::{self, Answered, BOM, EventOutcome, Inspected};
use crate::key::MasterKey;
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
    };
    let (body, tools_list) = if method == Method::POST {
        let declared = headers
```

with:

```rust
    };
    let body = if method == Method::POST {
        let declared = headers
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
        match jsonrpc::inspect_request(&bytes, connection.tool_allowlist.as_deref()) {
            Inspected::Forward { body, tools_list } => (Some(body), tools_list),
            Inspected::Answer(Some(answer)) => {
```

with:

```rust
        match jsonrpc::inspect_request(&bytes, connection.tool_allowlist.as_deref()) {
            Inspected::Forward(body) => Some(body),
            Inspected::Answer(Some(answer)) => {
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
    } else {
        (None, Vec::new())
    };
```

with:

```rust
    } else {
        None
    };
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
        }
        BodyKind::Json if allowlist.is_some() && !tools_list.is_empty() => {
            let read = read_capped(response.bytes_stream(), MAX_FILTERED_BODY).await;
```

with:

```rust
        }
        BodyKind::Json => {
            let read = read_capped(response.bytes_stream(), MAX_FILTERED_BODY).await;
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
                Err(ReadError::Failed) => {
                    return refuse(
```

with:

```rust
                Err(ReadError::Failed) => {
                    tracing::debug!(connection_id = %connection.id, "gateway proxy: the upstream body failed");
                    return refuse(
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
            };
            // Read as an event's data is (plan 2026-10-15): the filter
            // re-serialises what it read, so a key twice is resolved for
            // the client, but a key spelt otherwise goes on, and a client
            // that ignores case or cuts at a NUL reads it.
            let Some(mut value) = jsonrpc::read(&bytes) else {
                return refuse(
                    StatusCode::BAD_GATEWAY,
                    "upstream_invalid",
                    format!(
                        "connection {} answered with JSON the gateway cannot read",
                        connection.label
                    ),
                );
            };
            jsonrpc::filter_tools_lists(&mut value, &tools_list, allowlist.as_deref().unwrap_or_default());
            Body::from(serde_json::to_vec(&value).expect("a JSON value serialises"))
        }
        BodyKind::Json => Body::from_stream(passthrough(response, permits, connection.id.clone())),
        BodyKind::EventStream => {
```

with:

```rust
            };
            // Read as an event's data is (plan 2026-10-15 "gateway
            // differential"), and judged as one: no server request the
            // gateway refuses, every tools list filtered ("gateway JSON
            // answers").
            match jsonrpc::inspect_answer(&bytes, allowlist.as_deref()) {
                Answered::Unchanged => Body::from(bytes),
                Answered::Rewritten(bytes) => Body::from(bytes),
                Answered::Unreadable => {
                    tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a JSON answer the gateway cannot read refused");
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_invalid",
                        format!(
                            "connection {} answered with JSON the gateway cannot read",
                            connection.label
                        ),
                    );
                }
                Answered::ServerRequest => {
                    tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a JSON answer holding a server request refused");
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_invalid",
                        format!(
                            "connection {} answered with a server request inside a JSON answer",
                            connection.label
                        ),
                    );
                }
            }
        }
        BodyKind::EventStream => {
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
            };
            Body::from_stream(events(response, permits, allowlist, tools_list, answerer))
        }
```

with:

```rust
            };
            Body::from_stream(events(response, permits, allowlist, answerer))
        }
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
    }
}

/// The upstream's body as it comes, chunk by chunk, holding `permits`. A
/// failure mid-body ends it; its error carries no URL.
fn passthrough(
    response: reqwest::Response,
    permits: Permits,
    connection_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    response.bytes_stream().map(move |chunk| {
        let _held = &permits;
        chunk.map_err(|err| {
            let err = err.without_url();
            tracing::debug!(connection_id = %connection_id, error = %err, "gateway proxy: the upstream body failed");
            std::io::Error::other(err)
        })
    })
}
```

with:

```rust
    }
}
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
    allowlist: Option<Vec<String>>,
    ids: Vec<Value>,
    answerer: Answerer,
```

with:

```rust
    allowlist: Option<Vec<String>>,
    answerer: Answerer,
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
        allowlist: Option<Vec<String>>,
        ids: Vec<Value>,
        answerer: Answerer,
```

with:

```rust
        allowlist: Option<Vec<String>>,
        answerer: Answerer,
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
        allowlist,
        ids,
        answerer,
```

with:

```rust
        allowlist,
        answerer,
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
                        let event = &state.pending[start..start + end];
                        match jsonrpc::rewrite_event(event, &state.ids, state.allowlist.as_deref(), &mut answers) {
                            EventOutcome::Unchanged => out.extend_from_slice(event),
```

with:

```rust
                        let event = &state.pending[start..start + end];
                        match jsonrpc::rewrite_event(event, state.allowlist.as_deref(), &mut answers) {
                            EventOutcome::Unchanged => out.extend_from_slice(event),
```

- [ ] **Step 2: Run the tests**

Run: `nix develop -c cargo test -p hennery-gateway --locked`. Expected: all pass.

- [ ] **Step 3: Revert-probe every fix line**

Change the line as below, run the named test binary (`nix develop -c cargo test -p hennery-gateway --test <binary> --locked`), see it fail, restore.

| Probe | The change | Binary | Fails, naming |
|---|---|---|---|
| `q1-empty-json` | drop `inspect_answer`'s empty-body pass | `differential` | `a_json_answer_is_read_whole_and_capped` |
| `q2-json-server-request` | `ServerRequest` never returned | `differential` | `a_server_request_in_a_json_answer_is_502`; `json sampling`, `json refused in a batch`, `json tools and a refused request`, with and without an allowlist |
| `q3-json-filter` | `Rewritten` never returned | `differential` | `json list`, `json another id`, `json id of another type`, `json no id`; the id and null-id tests |
| `q4-json-read` | `inspect_answer` reads with serde_json alone (`Unreadable` never for a key twice or spelt otherwise) | `differential` | `json METHOD`, `json NUL method value`, `json NUL tools key`, `json Result`, `json Tools`, `json method twice`, `json tool Name` |
| `q5-event-filter` | an event's tools never filtered | `differential` | `a_get_stream_and_its_replay_are_filtered`; `sse list`, `sse another id`, `sse id of another type`, `sse refused in a batch`, `sse split data` |
| `q6-get-filtered` | no allowlist for a `GET` stream (8d's `!ids.is_empty()` guard, in effect) | `differential` | `a_get_stream_and_its_replay_are_filtered` |
| `q7-json-cap` | no cap on a JSON answer | `differential` | `a_json_answer_is_read_whole_and_capped` |
| `q8-json-whole` | a JSON answer streamed through unread (8d's code) | `proxy` | `the_first_chunk_arrives_before_the_upstream_finishes`, `tools_list_is_filtered_in_json_batches_and_event_streams`, `a_filtered_tools_list_over_8_mib_is_502`, `a_filtered_tools_list_that_does_not_parse_is_502` |
| `q9-body-failed-log` | drop the cut body's log line | `proxy_log` | `no_token_credential_or_url_secret_is_logged_or_answered` |
| `q10-unchanged-as-it-came` | `Unchanged` sent as an empty body | `proxy` | `without_an_allowlist_tools_list_passes_as_it_came`, `a_request_goes_up_with_the_credential_and_the_allowed_headers_only` |
| `q11-harness-names` | `REFUSED_SERVER_REQUESTS` without `roots/list` | `--lib` | `the_differential_harness_reads_the_gateway_s_names` |

The JSON rows that spell `id` otherwise (`json Id`, `json id twice`, `json NUL id key`, `json Id in a batch`) are held twice over: by the strict reading and by the filter that ignores ids. So `q4` does not name them.

Decision 5 has no probe: what it pins is that no state is shared.

- [ ] **Step 4: The five checks, and commit**

```bash
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
nix develop -c cargo clippy -p hennery --locked -- -D warnings
nix develop -c cargo test --workspace --locked
nix develop -c cargo run -p hennery-proto --bin gen -- --check
git add crates/hennery-gateway/src
git commit -m "fix(gateway): read JSON answers whole, refuse server requests in them, filter every tools list"
```

---
### Task 3: The gateway spec's §5.3, §5.5, §5.6 and §11

**Files:** Modify `docs/specs/2026-09-26-mcp-gateway-design.md`.

- [ ] **Step 1: Write it back**

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown

- JSON responses are streamed chunk by chunk with no buffering, event
  streams event by event (below), and there is **no compression layer on the
  proxy route** (compression middleware delays SSE).
- **Guarantee:** the first chunk of an upstream response reaches the client
  before the upstream finishes writing. A test asserts it against an upstream
  that writes one chunk and then blocks; the test must fail within seconds,
  not hang, if the proxy buffers. For an event stream the guarantee holds
  for every chunk that ends an event; a partial event waits for its end.
- Exception: a `tools/list` response in JSON for a connection with an
  allowlist is read fully (cap 8 MiB, error if exceeded — never truncated)
  and rewritten; in an event stream, its event is. It is read as an event's
  data is (below): JSON with a key twice, or spelling a key or method the
  gateway reads otherwise, is 502 `upstream_invalid`.
- An event stream is passed on **event by event**: every complete event at
```

with:

```markdown

- Event streams are passed on event by event (below), and there is **no
  compression layer on the proxy route** (compression middleware delays
  SSE).
- **Guarantee:** for an event stream, every chunk that ends an event
  reaches the client before the upstream finishes writing; a partial event
  waits for its end. A test asserts it against an upstream that writes one
  event and then blocks; the test must fail within seconds, not hang, if the
  proxy buffers.
- **A JSON answer is read whole** (cap 8 MiB, past it 502
  `upstream_too_large`, never truncated) before any of it goes on, with an
  allowlist or without: a client can use none of it before its end, and it
  may hold a server request (§5.6). It is read as an event's data is
  (below): JSON with a key twice, or spelling a key or method the gateway
  reads otherwise, is 502 `upstream_invalid`. It goes on as it came, or
  rewritten when the tools filter touched it (§5.5); an empty one passes,
  empty.
- An event stream is passed on **event by event**: every complete event at
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown

- `tools/list` responses are filtered to the allowlist, in JSON and in SSE
  framing (per event, other events passed through byte for byte); an allowlist
  matching nothing yields `"tools": []`, never `null`. JSON-RPC batches are
  filtered element by element.
- **`tools/call` for a tool outside the allowlist is rejected** by the gateway
```

with:

```markdown

- **Every `result.tools` coming down is filtered to the allowlist,
  whatever the message's id**: in JSON and in SSE framing (per event, other
  events passed through byte for byte), on a `POST`'s answer, a `GET`
  stream, and a `GET` replaying one with `Last-Event-ID`. An allowlist
  matching nothing, or a `tools` that is not an array, yields `"tools":
  []`, never `null`. JSON-RPC batches are filtered element by element. A
  result without `tools` is untouched.
- **`tools/call` for a tool outside the allowlist is rejected** by the gateway
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  in it `-32600` ("batch refused"); notifications get nothing, and a body
  with nothing to answer is 202. A response's id matches a `tools/list`
  request's if equal or the same number however written (`1`, `1.0`); a
  `tools/list` without an id is kept as `null`, so an answer with
  `"id": null` is filtered too.
- **The filter hides; the `tools/call` refusal enforces.** The filter
  knows only the ids of the `tools/list` requests of the same exchange, so
  an unfiltered list can still reach a client: an id answered as another
  type (`"1"` for `1`), an interrupted `tools/list` replayed on a `GET`
  with `Last-Event-ID`, or a response the upstream sends on another
  stream. A tool seen that way still cannot be called. A response whose
  `id`, `result`, `tools` or a tool's `name` is spelt otherwise or twice
  is not passed on (§5.3).

```

with:

```markdown
  in it `-32600` ("batch refused"); notifications get nothing, and a body
  with nothing to answer is 202.
- **The filter hides; the `tools/call` refusal enforces.** Since the
  filter reads every `result.tools`, it needs no request's id, and these
  are accepted (the fleet parent's ruling of 2026-10-02):
  - *An id of another type.* The gateway's own answers echo an id as the
    client sent it, integers digit for digit and a string as a string; a
    client that coerces ids (MCP's TypeScript SDK matches with
    `Number(id)`) does so on its own. An upstream's answer is filtered
    whatever its id.
  - *An answer on another stream.* The proxy routes nothing between
    requests: each `POST` or `GET` is its own upstream exchange, and every
    stream is filtered alike. The upstream's `Mcp-Session-Id` is the only
    thing that separates two sessions' streams: the gateway forwards the
    client's, does not bind it to the token, and every token on a connection
    uses the same upstream credential, so a token presenting another
    session's id gets what the upstream serves for it (open for 8e).
  - *A tool list in an error* (`error.data.tools`) is passed on: no client
    reads tools from an error, and the refusal still enforces.
  - A response whose `id`, `result`, `tools` or a tool's `name` is spelt
    otherwise or twice is not passed on (§5.3).

```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
`sampling`, `elicitation` and `roots`. Server-to-client requests of those kinds
arriving in a response stream are answered by the gateway with a JSON-RPC
error. *(G-19: the predecessor's spec said these were not forwarded, but its
code passed the client's `initialize` through verbatim.)* The requests are
`sampling/createMessage`, `elicitation/create` and `roots/list`; each is
answered `-32601` with a `POST` on the same upstream session
(`Mcp-Session-Id`, the answer's or else the request's) and credential, in the
```

with:

```markdown
`sampling`, `elicitation` and `roots`. Server-to-client requests of those kinds
arriving in any answer are refused by the gateway and never reach the
client. *(G-19: the predecessor's spec said these were not forwarded, but its
code passed the client's `initialize` through verbatim.)* The requests are
`sampling/createMessage`, `elicitation/create` and `roots/list`. In an
event stream (a `POST`'s answer or a `GET`), each is answered `-32601` with
a `POST` on the same upstream session
(`Mcp-Session-Id`, the answer's or else the request's) and credential, in the
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
passes. At the connection's request cap (§5.7) the answer is skipped and
logged. These requests are refused in
event streams only: a JSON body answering a `POST` is that request's
response, so inside one they pass in v1.

```

with:

```markdown
passes. At the connection's request cap (§5.7) the answer is skipped and
logged. **In a JSON answer**, one of them (with an id) makes the
whole answer 502 `upstream_invalid`, and nothing is answered upstream: a
JSON body is the `POST`'s response, MCP's streamable HTTP sends server
requests only in an event stream, and MCP's TypeScript SDK and rmcp would
both dispatch it. Passing the rest on without it would hand the client a
partial answer.

```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown

- **Streaming:** first chunk before upstream completion; fails fast on a
  buffering implementation. SSE through the proxy with a long `tools/call`.
- **401 handling:** static token rejected → 502 `upstream_auth`, no
```

with:

```markdown

- **Streaming:** an event stream's first event before upstream completion;
  fails fast on a buffering implementation. SSE through the proxy with a
  long `tools/call`. A JSON answer comes down only whole (§5.3).
- **Differential:** every filter on parsed input, through the proxy, under
  the decoders of `tests/support/differential.rs` (§5.2, §5.3, §5.5, §5.6).
- **401 handling:** static token rejected → 502 `upstream_auth`, no
```

- [ ] **Step 2: Commit**

```bash
git add docs/specs/2026-09-26-mcp-gateway-design.md
git commit -m "docs(spec): JSON answers read whole, server requests refused in any answer, every tools list filtered (gateway §5.3, §5.5, §5.6, §11)"
```

## After this plan

**Obligations this plan hands on:**
- **8e, 8f and 8g** test each filter they add on parsed input with `tests/support/differential.rs` (decision 6): a table of vectors, one line each, asserted with `forwarded_is_safe` and `received_is_safe`. A new decoder measured elsewhere joins `DECODERS`.
- **8e:** nothing in the proxy depends on a request's id any more. A revoke that ends open streams (8d's open question) needs no filter state either.

---

_Generated with Claude AI — please review before distribution._
