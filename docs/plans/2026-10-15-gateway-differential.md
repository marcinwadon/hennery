# MCP gateway: differential tests for the proxy's filters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** a differential test for every filter of plan 8d's proxy that reads parsed input, and the smallest fail-closed fix for each vector that passed one. The fleet rule: the same bytes, decoded the ways an upstream or a client might, never yield a different verdict. It lands before plan 8e mints session tokens, since nothing reaches the proxy before then.

**Architecture:** one new test file and two small changes to the gateway.
- `crates/hennery-gateway/tests/differential.rs` drives the real proxy against the fake upstream. One table row per vector. What crossed the gateway is decoded again by six model decoders, each modelled on one measured below. The filter's verdict must hold under every one of them.
- `jsonrpc::ambiguous` (it was `respelt_keys`; renamed because its meaning widened) is the one rule for what a decoder could read otherwise. `jsonrpc::read` is the one reading, used three times: a request body, an event's data, and a `tools/list` answer read whole in JSON.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), serde, serde_json, axum 0.8, reqwest, tokio. No new crate.

**Spec:** gateway §5.2 (a `POST` body), §5.3 (streaming, an event stream failing closed), §5.5 (tool allowlist), §5.6 (capabilities not forwarded). Plan 8d's decisions 5, 6, 7 and 17, and its whole-branch review's blocker: a key spelt in another case was read by a case-insensitive upstream decoder.

**Base:** `main` at `6162a54`: plan 8d merged as `953d0e9` (PR #83), where the work began. The branch was rebased onto `6162a54` without conflicts. `main` changed none of the gateway's files or its spec in between, so the anchors are the same on both.

**Status:** written and executed 2026-10-02 (see "Execution status"). Amended after the security review of 2026-10-02 (approve after amendments, two findings blocking, all taken) and its scoped re-confirmation (confirmed with notes). See "The security review's answers".

**How the code blocks were made and checked:**
- Every block below was generated from the branch's commits by a script, one region of a file per block.
- The plan was replayed from its own text onto `6162a54` in a scratch worktree, task by task. The extractor applied 20 blocks to 4 files over the 3 commits, and each commit's tree matched the branch's, byte for byte.
- The five checks passed: 1385 tests, from 1380.
- The 22 revert-probes of Task 2 were run, and each failed as expected: 16 of this plan's lines, and 6 of 8d's guards that the vectors found safe.

## Execution status (2026-10-02)

**Executed** on `main` at `953d0e9` as the branch `fix/gateway-differential`, then rebased onto `6162a54`. This is a fix-forward, so the code came first:
- the tests were written first, and run on `953d0e9`: four of the five failed, naming 16 request vectors and 16 response vectors, 5 of them again without an allowlist (Task 1's Step 2);
- then the fixes, then the security review's amendments;
- the plan was generated from the final commits and replayed onto `6162a54`. Each task's tree matched, byte for byte.

| Area | As built | Why |
|---|---|---|
| The tables | 69 request vectors, 50 response vectors and 2 raw-byte events, one line each. The event-stream vectors run twice, with and without an allowlist. | Adding a vector is one line, and a failure names every vector that failed, not only the first. |
| The real decoders | Go 1.26.1, json-c 0.18 and cJSON 1.7.19 were run on the same bytes (scratch programs, not committed). The model decoders follow their measured behaviour. | The brief asked for citations, not recall. Two of the first Go rows proved nothing and were redone (the security review). |
| The test client | The event-stream parser follows WHATWG's rules, and the client refuses any byte that is not UTF-8. | A lossy client shared the gateway's own assumption, so it could not catch finding 2. |
| Clippy | `next_back()` for the last of two keys, in the model decoder. | `double_ended_iterator_last`. |

Checks:
- After the rebase, fmt, both clippy runs and `gen --check` passed, since `main` had touched `hennery-proto`. The workspace tests passed: 1385, from 1380, the five of `differential.rs`.
- The 22 revert-probes of Task 2's Step 3 each failed as expected. `p20-bom-line` was inert at first: no row put a mark where a client would strip it. Two rows in the shape of 8d's re-confirmation note 1 were added, and then it failed.
- The run was macOS only, so ubuntu CI is the Linux check. Nothing here touches processes, files or sockets beyond the existing loopback harness.

## Scope

The filters of plan 8d that read parsed input, and every vector the lane parent named:
- the method checks (`tools/call`, `tools/list`, `initialize`, the refused server requests);
- the tool allowlist on `tools/call`;
- capability stripping on `initialize`;
- the `tools/list` response filter, in JSON and in SSE;
- the refusal of server-to-client requests in streams.

The vectors, each a table row with its expected outcome:
1. key case and folding;
2. duplicate keys;
3. `\u`-escaped keys and values;
4. a byte-order mark and whitespace JSON does not have;
5. lenient-parser forms;
6. NUL and control characters, trailing spaces, zero-width characters and Unicode normalization in keys;
7. the JSON-RPC envelope;
8. large ids.

**Out:** the gaps gateway §5.5 accepts, which close only if decision 5 or the lane parent's ruling on 8d's Q1 is reopened ("After this plan"):
- an id answered as another type;
- a `tools/list` replayed on a `GET`;
- an answer sent on another stream;
- a server request inside a JSON answer.

## The decoders, as measured

The model decoders in `differential.rs` are each taken from a run of the real decoder on the same bytes, on 2026-10-02 (scratch programs, not committed):
- **Go 1.26.1 `encoding/json`** (into a struct):
  - names are matched ignoring case, and the last of two wins: `{"method":"tools/list","method":"tools/call"}` and `{"METHOD":"tools/call"}` both read `tools/call`;
  - `ſ` folds to `s`: `paramſ` is `params`. The dotless `ı` and the dotted `İ` fold to nothing ASCII;
  - an escaped key is unescaped first: `"\u006dethod"` is `method`;
  - a key with a NUL (`method\u0000x`), a fullwidth letter (`ｍethod`), a zero-width space, a trailing space or NFD is no match for `method`;
  - a NUL in a value is kept;
  - an array `method` is an error;
  - an id decoded into `any` is a float64, so `9007199254740993` reads as `9007199254740992`;
  - a trailing comma, a byte-order mark and NBSP are errors.
- **json-c 0.18** (`json_tokener_parse`):
  - the last of two keys wins;
  - **keys are cut at their first NUL**: `{"method":"tools/list","method\u0000x":"tools/call"}` reads `tools/call`;
  - `json_object_get_string` cuts a value at a NUL: `"tools/call\u0000x"` reads `tools/call`;
  - the default tokener accepts a trailing comma, comments, single quotes and `NaN`; `JSON_TOKENER_STRICT` refuses all four (`NaN` excepted).
- **cJSON 1.7.19:**
  - `cJSON_GetObjectItem` matches names ignoring ASCII case, and the first of two wins;
  - keys and values are cut at a NUL;
  - a leading byte-order mark is skipped;
  - a trailing comma, comments, single quotes, `NaN`, `+1`, `0x10` and NBSP are errors.
- serde_json, JavaScript's `JSON.parse` and Python's `json`: the last of two wins, and names are exact. RapidJSON's `FindMember` and simdjson take the first. The test's `Exact` and `First` stand for these; they were not run.
- `GoV2Fold` was not run either. It models `encoding/json/v2` asked to match names case-insensitively, which also ignores `_` and `-`, as 8d's re-confirmation (F1) found.

Go folds only `ſ` and the Kelvin sign to ASCII letters. No decoder measured applies NFKC or strips zero-width characters or spaces. So a trailing space, a zero-width character, a fullwidth letter or NFD is **not applicable** as a bypass. Those rows assert that the vector reaches the upstream, and that no model decoder reads past the verdict there.

## Decisions this plan makes where the spec is silent

1. **A key is folded up to its first NUL** (vector 6). A NUL cut at that level is refused like `METHOD`, with 400 going up and the event dropped going down: `method\u0000x`, `params\u0000`, `name\u0000`, `sampling\u0000`, `id\u0000`, `tools\u0000`. json-c and cJSON do this cut.
2. **A `method` that spells one the gateway reads otherwise is refused.** These are `tools/call`, `tools/list` and `initialize`, and §5.6's three refused requests. The value is folded as keys are, and the name too, since `sampling/createMessage` has a capital letter.
   - The NUL cut is measured: json-c and cJSON read `tools/call\u0000x` as `tools/call`.
   - Folding the case of a value is a precaution: no decoder measured dispatches a method ignoring its case. It costs no legitimate client anything.
3. **A `method` that is not a string is refused.** In JavaScript, `["tools/call"]` used as a property key is `"tools/call"`, so a dispatcher written as `handlers[msg.method]` would run it. Go and the TypeScript SDK's schema refuse it, so the gateway's verdict would otherwise depend on the decoder.
4. **A message must be an object, and so must an `initialize`'s `params` and `capabilities` where present.**
   - Messages: each element of a batch, and a body that is not a batch. A batch in a batch, or a scalar in one, is refused: 400 going up, unreadable going down. An empty batch has nothing to filter and goes up as it came.
   - `initialize`: an array or a string has nothing to strip, so it went up as it came. A server testing `"sampling" in caps` finds it in `["sampling"]`, and in `"sampling"` as a substring (the security review's finding 1). A `tools/call` with `params` that is not an object was already answered here, outside every allowlist. `"params": null` on an `initialize` is 400 too (the re-confirmation's note 1), as JSON-RPC asks for structured `params` where present.
5. **Going down, a `result`'s `tools` and a tool's `name` are keys the gateway reads,** and `result` joins the message's keys. Spelt otherwise, the message is unreadable: the event is dropped, and a JSON `tools/list` answer is 502 `upstream_invalid`.
   - That JSON answer is now read as an event's data is (`jsonrpc::read`: no key twice, nothing `ambiguous`). Before, it was read by serde_json alone. The filter re-serialises what it read, so a key twice was already resolved for the client; but `{"Id":1,…}`, `"Result"`, `"Tools"` and `"Name"` went on as they came, for Go (any case, last wins) or cJSON (any case, first wins) to read the unfiltered list.
   - This closes the part of §5.5's listed gap that is a spelling. The rest of that gap stays (Scope, "Out").
   - §5.2, §5.3 and §5.5 are written back (Task 3).
6. **Large ids** (vector 8): no change.
   - The gateway's own answers echo an integer id digit for digit, since serde_json reads every integer within `i64` and `u64` exactly. Any other number comes back as the same number, written otherwise: `-0` as `-0.0`, `1e0` as `1.0` (the security review's finding 4).
   - The filter matches those too: `-0` answered as `0`, and `1e0` as `1`.
   - The filter compares ids as doubles (8d decision 17). So an upstream that decodes `9007199254740993` into a double and answers `9007199254740992`, as Go's `any` and JavaScript do, is still filtered. Matching more ids than it should fails closed.
   - Beyond `u64`, serde_json reads a double, as those decoders do. No MCP client issues such ids, and JSON-RPC asks for integers.

7. **An event that is not UTF-8 is dropped** (the security review's finding 2, blocking).
   - Before, `event_parts` read data with replacement characters, and an event no rule touched went on as its raw bytes: `"method":"roots/list\xff"`, or `C0 A2` (an overlong `"`) around `method` and `roots/list`. A client that drops the byte, or decodes the overlong form, reads a refused request.
   - Every other reading already refuses it: `jsonrpc::read` and a request body (400).
   - This reverses 8d's re-confirmation note 3, which let invalid UTF-8 go on in an event no rule touches. The stated reason of the byte-order-mark rule covers it: the gateway cannot read what a client might.
8. **A `tools/list` without an id is kept as `null`** (the security review's finding 3). An upstream that answers a notification with `"id": null` would otherwise send an unfiltered list; `same_id` already matches `null` with `null`.
   - Accepted (the re-confirmation's note 3): a `tools/list` notification now takes the JSON read-whole path, so an upstream answering it 202 with `Content-Type: application/json` and an empty body is 502 `upstream_invalid`, where the same answer to any other notification passes as 202. A `tools/list` without an id means nothing, and the answer fails closed.

Accepted:
- a body that is a lone scalar is now 400, and an event whose data is a scalar is dropped. Neither is an MCP message.
- folding values also refuses, say, `tools/ca-ll`. No key or method an MCP peer sends folds onto one the gateway reads, even with `_` and `-` dropped: `_meta`, `nextCursor`, `notifications/initialized` and `notifications/tools/list_changed` all stay distinct (the security review).
- a server request without an id is a notification and passes, by §5.6 and 8d's unit test.
- under an allowlist, a JSON `tools/list` answer with a key twice anywhere, deep in a tool's `inputSchema` say, is now 502, where 8d re-serialised it. Re-serialising already made a duplicate safe, so this costs availability only, for an upstream that sends invalid JSON-RPC.

## The security review's answers

**The security review (opus, 2026-10-02), on the maintainer's behalf: approve after amendments.** Two findings were blocking; all were taken.
- Method: it read the change and checked the twelve revert-probes. It probed with throwaway tests and its own Go 1.26.1 run in a scratch worktree, and checked the model decoders against `c.out` and `go.out`.
- It confirmed decisions 1–5, and 6 after rewording.
- On the fixes: each is correct, minimal, and fails closed.

| Finding | Taken how |
|---|---|
| **1 (blocking)** An `initialize` whose `params` or `capabilities` is not an object went up as it came | Taken: refused, 400 (decision 4). Rows `initialize capabilities array`, `… string`, `initialize params array`; probes `p13-init-params-non-object`, `p14-init-capabilities-non-object` |
| **2 (blocking)** Invalid UTF-8 in an event no rule touched reached the client as raw bytes | Taken: the event is dropped (decision 7). Table `RAW_EVENTS`; the test client now refuses any non-UTF-8 byte it receives, since its parser decoded lossily, as the gateway did; probe `p15-event-utf8`; §5.3 written back |
| 3 A `tools/list` without an id was forwarded but not kept | Taken: kept as `null` (decision 8). Test `a_tools_list_without_an_id_is_filtered_as_null`; probe `p16-null-id` |
| 4 "Digit for digit" holds for integers only | Taken: decision 6 reworded; `-0` and `1e0` echoed and matched, in `ids_are_echoed_exactly_and_still_filtered` |
| 5 A refused server request without an id passes | By design (§5.6); kept |
| Evidence | Two rows of the Go probe proved nothing: `naſ` does not fold to `name`, and the "escaped" key had no escape. Rerun with `paramſ` and a real `\u006d`; `ı` and `İ` added. `GoV2Fold` noted as not run. The "NFD key" row is relabelled "combining mark key", since every key the gateway reads is ASCII |
| Coverage | Every response row ran under an allowlist. The event-stream rows now run again without one, in `no_decoder_reads_a_refused_request_without_an_allowlist` |

**Its view on the four accepted gaps** (the explicit question, which it did not decide) is in "After this plan".

**The scoped re-confirmation (a fresh opus reviewer, 2026-10-02), scoped to the amendments: confirmed with notes, none blocking.** It ran the gateway's tests at `2606a02` in a scratch worktree, with one throwaway probe of its own, and reran the Go evidence; the output matched `go.out` byte for byte. Its notes:
1. Finding 1's fix is correct and fails closed. `"params": null` on an `initialize` is now 400, which is right under JSON-RPC (written into decision 4).
2. Finding 2's check runs before every pass-through. `events()` only judges whole events, cut at `\n` and `\r` (ASCII), so a chunk boundary inside a character never trips it. Probe `p15` bites because the test client refuses non-UTF-8 bytes. The §5.3 write-back is accurate.
3. Finding 3 is correct. A `tools/list` notification answered 202 with a JSON type and an empty body is now 502. Accepted, as decision 8 says.
4. Decision 6's wording matches what it measured (`-0.0`, `1.0`). The test pins the number, not the `-0.0` spelling.
5. `p13`–`p16` are real probes of the new lines, and each failed.
6. The evidence and the plan's citations are correct.
7. Nothing the first review approved was loosened.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`.
- After every task the five checks pass (fmt, both clippy runs, the workspace tests, `gen --check`).
- **No new crates, no wire types, no migration.**
- Fail closed: a vector is refused (400), answered by the gateway, or dropped (an event) or 502 (a JSON answer). Nothing is ever forwarded "because it was unparseable".
- Each table row is one line (`#[rustfmt::skip]` on the tables), so adding a vector is a single line.
- Commits: Conventional Commits, gmail identity, unsigned. Push after every task; never push `main`.

## Review Focus

1. **A NUL in a key or a method value** (json-c, cJSON).
   - Expected: 400 up, the event dropped down, and a JSON `tools/list` answer 502.
   - Tests: the `NUL …` rows of both tables; probes `p1-nul-fold`, `p4-read-method-value`, `p5-refused-method-value`, `p9-fold-both-sides`.
2. **The JSON `tools/list` answer read whole** with `Id`, `Result`, `Tools` or a tool's `Name` spelt otherwise.
   - Expected: 502 `upstream_invalid`, never the unfiltered list.
   - Tests: the `json …` rows; probes `p2-result-key`, `p7-tools-key`, `p8-tool-name-key`, `p12-json-answer-read`.
3. **The envelope.**
   - Expected: a non-string `method`, a batch in a batch, a scalar in a batch: 400. An object or array id, or array `params`, on a refused call: answered by the gateway, nothing up.
   - Tests: the rows under "7."; probes `p3-non-object`, `p6-non-string-method`.
4. **Not UTF-8, and `initialize`'s shape** (the security review's findings 1 and 2).
   - Expected: an event with an invalid or overlong byte is dropped; an `initialize` whose `params` or `capabilities` is not an object is 400.
   - Tests: `RAW_EVENTS`; the `initialize …` rows; probes `p13`–`p15`.
5. **What is not applicable is still checked.** Rows marked `Forwarded` must reach the upstream, so a harness that refuses everything fails.
6. **What stays open** ("After this plan"): §5.5's accepted gaps, and Q1.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-gateway/tests/differential.rs` | the vector tables, the model decoders, a WHATWG event-stream parser | 1 |
| `crates/hennery-gateway/src/jsonrpc.rs` | `folded` to the first NUL; `ambiguous`; `read`; a `tools/list` without an id kept as `null`; an event not UTF-8 unreadable | 2 |
| `crates/hennery-gateway/src/proxy.rs` | the JSON `tools/list` answer read with `jsonrpc::read` | 2 |
| `docs/specs/2026-09-26-mcp-gateway-design.md` | §5.2, §5.3, §5.5 written back | 3 |

**Reading the steps:** each block is the exact text. "Create" is a whole new file. "In `path`, replace: … with: …" replaces the one occurrence of the first block, in order.

---
### Task 1: The differential tests

**Files:**
- Create: `crates/hennery-gateway/tests/differential.rs`

**Interfaces:**
- Consumes: `tests/support`'s `Harness` and `FakeUpstream`, and the proxy as served.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-gateway/tests/differential.rs`:

```rust
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
```

- [ ] **Step 2: Run them, and watch them fail**

Run: `nix develop -c cargo test -p hennery-gateway --test differential --locked`
Expected on `953d0e9`: `ids_are_echoed_exactly_and_still_filtered` passes. The other four fail:
- `no_decoder_reads_past_the_request_filter`, naming 16 vectors:
  - the `NUL …` rows: "JsonC reads method \"tools/call\" … the gateway read Some(\"tools/list\")" and the like;
  - `upper method value`, `array method`, `number method`, `batch in a batch`, `scalar in a batch` and the three `initialize …` rows: "200 OK, not 400".
- `no_decoder_reads_past_the_response_filters`, naming 16:
  - `json Id`, `json Result`, `json Tools`, `json tool Name`, `json Id in a batch`, `sse Result`, `sse Tools`, `sse tool NAME`: "GoV1 (or CJson) reads tool Some(\"delete\")";
  - the `… NUL …` rows;
  - the two `RAW_EVENTS`: "received bytes that are not UTF-8".
- `no_decoder_reads_a_refused_request_without_an_allowlist`, naming 5: the three `sse NUL` method rows and the two `RAW_EVENTS`.
- `a_tools_list_without_an_id_is_filtered_as_null`: the list comes back with `delete`.

- [ ] **Step 3: Commit**

```bash
git add crates/hennery-gateway/tests/differential.rs
git -c commit.gpgsign=false commit -m "test(gateway): differential tests for the proxy's filters"
```

---
### Task 2: Refuse what a decoder reads otherwise

**Files:**
- Modify: `crates/hennery-gateway/src/jsonrpc.rs`, `crates/hennery-gateway/src/proxy.rs`

**Interfaces:**
- Produces:
  - `jsonrpc::ambiguous(&Value) -> bool`, which replaces `respelt_keys`;
  - `jsonrpc::read(&[u8]) -> Option<Value>`.
- Consumes: 8d's `Unique`, `respelt`, `inspect_request`, `rewrite_event`, and the proxy's JSON `tools/list` path.

- [ ] **Step 1: The fix**

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
//!   Nor may it spell a key the gateway reads otherwise than exactly
//!   (`METHOD`, `Name`, `ſampling`): a decoder that matches names whatever
//!   their case, as Go's `encoding/json` does, would read it.
//! - `initialize` goes upstream without `sampling`, `elicitation` and
```

with:

```rust
//!   Nor may it spell a key the gateway reads otherwise than exactly
//!   (`METHOD`, `Name`, `ſampling`, `method\0x`): a decoder that matches
//!   names whatever their case, as Go's `encoding/json` does, or cuts them
//!   at a NUL, as json-c and cJSON do, would read it. Nor may a method the
//!   gateway reads be spelt otherwise (`tools/call\0x`), nor `method` be
//!   anything but a string, nor a batch hold anything but objects (plan
//!   2026-10-15, the differential tests).
//! - `initialize` goes upstream without `sampling`, `elicitation` and
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
    Answer(Option<Value>),
    /// Not JSON, a key twice in one object, or a key the gateway reads
    /// spelt otherwise: refused, 400.
    Invalid(&'static str),
```

with:

```rust
    Answer(Option<Value>),
    /// Not JSON, a key twice in one object, or `ambiguous`: refused, 400.
    Invalid(&'static str),
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
const INVALID_BODY: &str =
    "the body is not JSON, has a key twice in one object, or spells a key the gateway reads otherwise";

```

with:

```rust
const INVALID_BODY: &str =
    "the body is not JSON, has a key twice in one object, or spells a key or method the gateway reads otherwise";

```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
/// a spelling, at its level, is refused like a key twice (plan 8d decision
/// 6; the whole-branch review). `result` is read going down, in a
/// `tools/list` answer.
const MESSAGE_KEYS: &[&str] = &["jsonrpc", "id", "method", "params", "result"];

/// The methods the gateway reads, besides the refused server requests.
const READ_METHODS: &[&str] = &["tools/call", "tools/list", "initialize"];

/// A key as such a decoder compares it with the names the gateway reads:
/// up to its first NUL, as json-c and cJSON keep keys and strings in C
/// strings; ASCII case, and `ſ` as `s`. Unicode folds only one other letter
/// to an ASCII one, the Kelvin sign to `k`, and no name read here has a
/// `k`. Go's `encoding/json/v2`, matching names case-insensitively, ignores
/// `_` and `-` as well (the re-confirmation's F1).
fn folded(key: &str) -> String {
    key.chars()
        .take_while(|c| *c != '\0')
        .filter(|c| !matches!(c, '_' | '-'))
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust

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
```

with:

```rust

/// Whether `word` folds to one of `names` without being it.
fn respelt_word(word: &str, names: &[&str]) -> bool {
    let folded = folded(word);
    names.iter().any(|name| folded == self::folded(name) && word != *name)
}

/// Whether `object` has a key that folds to one of `names` without being it.
fn respelt(object: &serde_json::Map<String, Value>, names: &[&str]) -> bool {
    object.keys().any(|key| respelt_word(key, names))
}

/// Whether a message, or any in a batch, could be read by some decoder
/// otherwise than the gateway reads it (plan 8d decision 6; plan
/// 2026-10-15's differential tests):
/// - a message that is not an object: a batch in a batch, a scalar;
/// - a key the gateway reads spelt otherwise than exactly: one of the
///   message's own, `name` in a `tools/call`'s `params`, `capabilities`
///   and those not forwarded in an `initialize`'s, `tools` in a `result`
///   and `name` in each of its tools;
/// - a `method` that is not a string (JavaScript reads `["tools/call"]` as
///   a property key `tools/call`), or that spells a method the gateway
///   reads otherwise (`tools/call\0x` is `tools/call` to json-c and cJSON);
/// - an `initialize` whose `params` or `capabilities` is not an object.
pub fn ambiguous(value: &Value) -> bool {
    let messages: Vec<&Value> = match value {
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
        let Some(object) = message.as_object() else {
            return false;
        };
```

with:

```rust
        let Some(object) = message.as_object() else {
            return true;
        };
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
        }
        match object.get("method") {
            None => {}
            Some(Value::String(method)) => {
                if respelt_word(method, READ_METHODS) || respelt_word(method, REFUSED_SERVER_REQUESTS) {
                    return true;
                }
            }
            Some(_) => return true,
        }
        if let Some(result) = object.get("result").and_then(Value::as_object) {
            let tools = result.get("tools").and_then(Value::as_array);
            if respelt(result, &["tools"])
                || tools.is_some_and(|tools| {
                    tools
                        .iter()
                        .filter_map(Value::as_object)
                        .any(|tool| respelt(tool, &["name"]))
                })
            {
                return true;
            }
        }
        let params = object.get("params");
        match method(message) {
            Some("tools/call") => params
                .and_then(Value::as_object)
                .is_some_and(|params| respelt(params, &["name"])),
            // An `initialize` whose `params` or `capabilities` is not an
            // object: nothing to strip, yet a server testing `"sampling" in
            // caps` finds it in `["sampling"]` (the security review's
            // finding 1).
            Some("initialize") => match params {
                None => false,
                Some(Value::Object(params)) => {
                    respelt(params, &["capabilities"])
                        || match params.get("capabilities") {
                            None => false,
                            Some(Value::Object(capabilities)) => respelt(capabilities, STRIPPED_CAPABILITIES),
                            Some(_) => true,
                        }
                }
                Some(_) => true,
            },
            _ => false,
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
pub fn inspect_request(body: &[u8], allowlist: Option<&[String]>) -> Inspected {
    let Some(mut value) = read(body) else {
        return Inspected::Invalid(INVALID_BODY);
    };
    let mut rewritten = false;
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
                Some("tools/list") if allowlist.is_some() => {
                    if let Some(id) = message.get("id") {
                        tools_list.push(id.clone());
                    }
                }
```

with:

```rust
                Some("tools/list") if allowlist.is_some() => {
                    // Without an id, as `null`: an upstream that answers it
                    // with `"id": null` is filtered too (the security
                    // review's finding 3).
                    tools_list.push(message.get("id").cloned().unwrap_or(Value::Null));
                }
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust

/// Any JSON, refusing a key twice in one object at any depth.
```

with:

```rust

/// A body, or an event's data, as the gateway reads it: `None` if it is not
/// JSON to serde_json, has a key twice in one object at any depth (serde_json
/// keeps the last, a decoder may keep the first), or is `ambiguous`. The one
/// reading for both ways: a request, an event, and a `tools/list` answer
/// read whole (plan 2026-10-15).
pub fn read(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Unique>(bytes).ok()?;
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    (!ambiguous(&value)).then_some(value)
}

/// Any JSON, refusing a key twice in one object at any depth.
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
    Dropped,
    /// Not passed on: its data is not JSON, has a key twice in an object
    /// or spells a key the gateway reads otherwise, or a line of it starts
    /// with a byte-order mark, so the gateway cannot read what a client
    /// might (the review's B2, the re-confirmation's note 1, the Task 2
    /// review's finding 3, the whole-branch review).
    Unreadable,
```

with:

```rust
    Dropped,
    /// Not passed on: it is not UTF-8, its data is not JSON, has a key
    /// twice in an object or is `ambiguous`, or a line of it starts with a
    /// byte-order mark, so the gateway cannot read what a client might (the
    /// review's B2, the re-confirmation's note 1, the Task 2 review's
    /// finding 3, the whole-branch review, plan 2026-10-15).
    Unreadable,
```

In `crates/hennery-gateway/src/jsonrpc.rs`, replace:

```rust
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
```

with:

```rust
    }
    // Not UTF-8: the gateway would read it with replacement characters, a
    // client may decode it otherwise, an overlong `"` as a quote (the
    // security review's finding 2).
    if std::str::from_utf8(event).is_err() {
        return EventOutcome::Unreadable;
    }
    let (Some(data), others) = event_parts(event) else {
        return EventOutcome::Unchanged;
    };
    // Not JSON to serde_json, a key twice in an object (decision 6's reason,
    // for what comes down), or a refused request's `method` spelt otherwise
    // (the whole-branch review): a client whose decoder ignores case, or
    // cuts at a NUL, would run it.
    let Some(value) = read(data.as_bytes()) else {
        return EventOutcome::Unreadable;
    };
    let (kept, refused) = take_refused_requests(value);
```

In `crates/hennery-gateway/src/proxy.rs`, replace:

```rust
            };
            let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
                return refuse(
                    StatusCode::BAD_GATEWAY,
                    "upstream_invalid",
                    format!("connection {} answered with JSON that does not parse", connection.label),
                );
```

with:

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
```

- [ ] **Step 2: Run the tests**

Run: `nix develop -c cargo test -p hennery-gateway --locked`
Expected: every test passes, the five of `differential` too.

- [ ] **Step 3: Revert-probe every fix line**

Remove one line, or make it inert, as below. Run `nix develop -c cargo test -p hennery-gateway --test differential --locked`, see it fail with the vectors named, then restore the line.

| Probe | The change | Fails, naming |
|---|---|---|
| `p1-nul-fold` | drop `.take_while(\|c\| *c != '\0')` from `folded` | every `NUL …` row of both tables (8 request rows; 6 response rows) |
| `p2-result-key` | `MESSAGE_KEYS` without `"result"` | `json Result`, `sse Result` |
| `p3-non-object` | a message that is not an object: `return false` | `batch in a batch`, `scalar in a batch` |
| `p4-read-method-value` | drop `respelt_word(method, READ_METHODS) \|\|` | `NUL method value`, `NUL initialize value`, `upper method value`, `NUL tools/list value` |
| `p5-refused-method-value` | drop `\|\| respelt_word(method, REFUSED_SERVER_REQUESTS)` | `sse NUL method value`, `sse NUL sampling value` |
| `p6-non-string-method` | `Some(_) => {}` | `array method`, `number method` |
| `p7-tools-key` | `if false` for `respelt(result, &["tools"])` | `json Tools`, `sse Tools`, `json NUL tools key` |
| `p8-tool-name-key` | `.any(\|_\| false)` for a tool's `name` | `json tool Name`, `sse tool NAME`, `sse NUL tool name key` |
| `p9-fold-both-sides` | `folded == *name` in `respelt_word` | `sse NUL sampling value` (the capital `M`) |
| `p10-request-read` | `inspect_request` reads with serde_json alone | 35 request rows: every case, duplicate, NUL, method-value and envelope row |
| `p11-event-read` | `rewrite_event` reads with serde_json alone | 10 `sse …` rows |
| `p12-json-answer-read` | the JSON `tools/list` answer read with serde_json alone (8d's code) | the 7 `json …` spelling and NUL rows |
| `p13-init-params-non-object` | an `initialize`'s `params` not an object: `false` | `initialize params array` |
| `p14-init-capabilities-non-object` | its `capabilities` not an object: `false` | `initialize capabilities array`, `initialize capabilities string` |
| `p15-event-utf8` | `if false` for the UTF-8 check in `rewrite_event` | `sse invalid byte in a method`, `sse overlong quotes`, with and without an allowlist |
| `p16-null-id` | keep a `tools/list`'s id only if it has one (8d's code) | `a_tools_list_without_an_id_is_filtered_as_null` |

8d's own guards, which the vectors found safe, are probed the same way. No test had watched them fail for these vectors:

| Probe | The change | Fails, naming |
|---|---|---|
| `p17-request-lenient` | `inspect_request` forwards a body serde_json cannot read | 18 request rows: the byte-order mark, NBSP, U+3000, U+2028, form feed, vertical tab, and every lenient form, control character and lone surrogate |
| `p18-event-lenient` | `rewrite_event` passes an event whose data serde_json cannot read | `sse NBSP`, `sse trailing comma`, `sse single quotes`, `sse comment in data`, with and without an allowlist |
| `p19-json-answer-lenient` | the JSON `tools/list` answer passed on when serde_json cannot read it | `json BOM`, `json NBSP`, `json trailing comma` |
| `p20-bom-line` | `.any(\|_\| false)` for a line that starts with a byte-order mark | `sse BOM behind a dropped event`, `sse BOM behind a stripped BOM`, with and without an allowlist |
| `p21-error-echo` | `error()` echoes the id through `as_f64()` | `ids_are_echoed_exactly_and_still_filtered` (the digits) |
| `p22-same-id` | `same_id` is `a == b` | `ids_are_echoed_exactly_and_still_filtered` (2^53+1 as 2^53, `-0` as `0`, `1e0` as `1`) |

- [ ] **Step 4: The five checks, and commit**

```bash
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
nix develop -c cargo clippy -p hennery --locked -- -D warnings
nix develop -c cargo test --workspace --locked
nix develop -c cargo run -p hennery-proto --bin gen -- --check
git add crates/hennery-gateway/src/jsonrpc.rs crates/hennery-gateway/src/proxy.rs
git -c commit.gpgsign=false commit -m "fix(gateway): refuse what a NUL-cutting or lenient decoder reads otherwise"
```

---
### Task 3: The gateway spec's §5.2, §5.3 and §5.5

**Files:**
- Modify: `docs/specs/2026-09-26-mcp-gateway-design.md`

- [ ] **Step 1: Write it back**

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  than exactly, folding ASCII case and `ſ` to `s` and ignoring `_` and `-`,
  as Go's `encoding/json` (v1 and v2) can match names: a message's
  `jsonrpc`, `id`, `method` and `params`, a `tools/call`'s `name`, an
  `initialize`'s `capabilities` and the capabilities not forwarded.
  `METHOD` or `Name` would be read upstream as what the allowlist never
  saw. The bytes go up as they came, except an
  `initialize` rewritten (§5.6). `GET` and `DELETE` bodies are neither read
```

with:

```markdown
  than exactly, folding ASCII case and `ſ` to `s` and ignoring `_` and `-`,
  as Go's `encoding/json` (v1 and v2) can match names, and cutting the key
  at its first NUL, as json-c and cJSON keep keys in C strings: a message's
  `jsonrpc`, `id`, `method`, `params` and `result`, a `tools/call`'s
  `name`, an `initialize`'s `capabilities` and the capabilities not
  forwarded. `METHOD`, `Name` or `method\u0000x` would be read upstream as
  what the allowlist never saw. Nor may a `method` be anything but a
  string, or spell a method the gateway reads (`tools/call`, `tools/list`,
  `initialize`, and §5.6's refused requests) otherwise, folded the same
  way (`tools/call\u0000x` is `tools/call` to json-c and cJSON); nor may a
  batch hold anything but objects, nor an `initialize` have `params` or
  `capabilities` that is not an object (`["sampling"]` holds `sampling` to
  a server that tests membership). The bytes go up as they came, except an
  `initialize` rewritten (§5.6). `GET` and `DELETE` bodies are neither read
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  allowlist is read fully (cap 8 MiB, error if exceeded — never truncated)
  and rewritten; in an event stream, its event is.
- An event stream is passed on **event by event**: every complete event at
```

with:

```markdown
  allowlist is read fully (cap 8 MiB, error if exceeded — never truncated)
  and rewritten; in an event stream, its event is. It is read as an event's
  data is (below): JSON with a key twice, or spelling a key or method the
  gateway reads otherwise, is 502 `upstream_invalid`.
- An event stream is passed on **event by event**: every complete event at
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  ends. It fails closed: a byte-order mark at its start is dropped, as a
  client's parser would; an event whose data is not JSON (to serde_json: a
  lone surrogate, say), has a key twice in an object, spells a key the
  gateway reads otherwise (§5.2), or has a line that starts with a
  byte-order mark, is dropped, since a client's parser might read what the
```

with:

```markdown
  ends. It fails closed: a byte-order mark at its start is dropped, as a
  client's parser would; an event that is not UTF-8 (a client may decode
  an invalid or overlong byte otherwise than as a replacement character),
  or whose data is not JSON (to serde_json: a
  lone surrogate, say), has a key twice in an object, spells a key or
  method the gateway reads otherwise (§5.2; going down also a `result`'s
  `tools` and a tool's `name`), or has a line that starts with a
  byte-order mark, is dropped, since a client's parser might read what the
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  fields (`id:`, `event:`) and gets one `data:` line.
  Events without data (comments, pings) pass byte for byte, and invalid
  UTF-8 in an event no rule touches goes on as it came, as clients decode
  it with replacement characters.

```

with:

```markdown
  fields (`id:`, `event:`) and gets one `data:` line.
  Events without data (comments, pings) pass byte for byte.

```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  with nothing to answer is 202. A response's id matches a `tools/list`
  request's if equal or the same number however written (`1`, `1.0`).
- **The filter hides; the `tools/call` refusal enforces.** The filter
```

with:

```markdown
  with nothing to answer is 202. A response's id matches a `tools/list`
  request's if equal or the same number however written (`1`, `1.0`); a
  `tools/list` without an id is kept as `null`, so an answer with
  `"id": null` is filtered too.
- **The filter hides; the `tools/call` refusal enforces.** The filter
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  type (`"1"` for `1`), an interrupted `tools/list` replayed on a `GET`
  with `Last-Event-ID`, a response the upstream sends on another stream,
  or one whose `result`, `tools` or a tool's `name` is spelt otherwise or
  twice, for a client that reads it so. A tool seen that way still cannot
  be called.

```

with:

```markdown
  type (`"1"` for `1`), an interrupted `tools/list` replayed on a `GET`
  with `Last-Event-ID`, or a response the upstream sends on another
  stream. A tool seen that way still cannot be called. A response whose
  `id`, `result`, `tools` or a tool's `name` is spelt otherwise or twice
  is not passed on (§5.3).

```

- [ ] **Step 2: Commit**

```bash
git add docs/specs/2026-09-26-mcp-gateway-design.md
git -c commit.gpgsign=false commit -m "docs(spec): write back the differential tests' fixes into gateway §5.2, §5.3, §5.5"
```

## After this plan

**Open for the lane parent (a conflict between this task's brief and a ruling, not decided here):** the brief asks that a client never receive a tool the filter removed, or a server request it refused, "under any interpretation the client might use". Four vectors still do, and gateway §5.5 and the parent's ruling on 8d's Q1 accept them. Closing any of them means reading every JSON answer whole, or filtering every answer whatever its id. Either reopens decision 5's streaming of JSON and Q1's "event streams only".
- **An id answered as another type.** MCP's TypeScript SDK matches a response with `Number(response.id)`, so `"1"`, `"0x1"`, `" 1 "` or `true` answer `tools/list` 1. The filter, by decision 17, matches only equal ids and the same number however written.
- **A `tools/list` replayed on a `GET`** with `Last-Event-ID`: the `GET` stream knows no `tools/list` ids.
- **An answer sent on another stream:** a `tools/list` answer in another `POST`'s body, or on the `GET` channel.
- **A server request inside a JSON answer** (8d's Q1): rmcp and the TypeScript SDK both hand one to their handlers.

The `tools/call` refusal still enforces the allowlist in each case: a tool seen that way cannot be called. The server requests are the sharper one, since nothing enforces behind their refusal. A cheap middle way exists: under an allowlist, filter every `result.tools` in every event and every JSON answer already read, whatever its id. That closes the first three for SSE and leaves JSON streaming as it is.

**Obligations this plan hands on:**
- **Every later filter on parsed input** (8e's `servers_for`, 8f's OAuth metadata, 8g's standalone clients) adds rows to `differential.rs`'s tables rather than a test of its own. A new key the gateway reads joins `MESSAGE_KEYS` or `ambiguous`, with a row that fails without it.
- **8e** mints the first tokens. From then on these filters face real traffic.

---

_Generated with Claude AI — please review before distribution._
