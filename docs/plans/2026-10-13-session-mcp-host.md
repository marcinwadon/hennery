# MCP gateway (plan 8c): per-session MCP servers on the host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The host side of delivering a session's MCP servers (ACP core §1, §3.3, §4.3, §6, §8; gateway spec §3.2, §3.4):
- `start_session` and `resume_session` carry the session's `mcp_servers` and its `hat_id`. Both are left out when empty, so an older host and an older collector still talk to each other.
- A host announces a new capability, `mcp_servers`, and how it isolates each agent (`hello.mcp_isolation`). The collector's hub sends servers only to a connection that announced them, and only for an agent it isolates, unless the collector waives isolation.
- The host passes the servers into ACP `session/new` and every `session/load`. For Claude it sends `_meta.claudeCode.options.extraArgs["strict-mcp-config"] = ""` on every one, servers or not. It refuses servers for an agent it cannot isolate, unless waived (`mcp_isolation_unavailable`), before anything is spawned.
- No token reaches a log: frames' `Debug` hides header and env values, stdio arguments and an HTTP URL past its origin (the lane's L11); the process caps the libraries that trace whole messages; the host redacts a session's secret values from the text it writes itself.

The collector sends no servers yet: minting and the delivery decision are plan 8e's.

**Architecture:**
- **Wire** (`hennery-proto`): `NameValue`, `McpServer {Http, Stdio}`, `McpDelivery {mcp_servers, isolation_waived}` flattened into both frames beside `hat_id`; `Capability::McpServers`; `McpIsolation {ClaudeStrict, None}` and `AgentIsolation` (`hello.mcp_isolation`), read leniently.
- **Collector** (`hennery-sessions`): the hub keeps each connection's `mcp_isolation` and refuses (`RequestError::McpUndeliverable`, sending nothing, on a request or a `notify` alike) a start or resume whose servers the connection cannot take; `ws.rs` passes the `hello`'s map in; `api.rs` sends the session's hat and no servers.
- **Host** (`hennery-host`): `profile.rs` (`Profile` from where an agent came, its `_meta`, its isolation, the refusal, the ACP entries, `Secrets`); `connection.rs` announces and refuses; `session.rs` passes servers and `_meta` on new and load and redacts; `runtime/agents.rs` gives each installed agent its profile; `logging.rs` caps the message-tracing targets, and `hennery`'s `log.rs` installs every subscriber through it.
- **Tests:** the fake adapter records each `session/new` and `session/load` it parsed (`session_log`) and can quote its servers in an error (`new_session_error_echoes`).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, agent-client-protocol =2.2.0, tracing-subscriber 0.3 (already in the workspace; now a dependency of `hennery-host` too, for the cap). No new crates.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md), [`docs/specs/2026-09-26-mcp-gateway-design.md`](../specs/2026-09-26-mcp-gateway-design.md), the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md) and the [spike](../spikes/2026-09-25-per-session-mcp.md):
- ACP core §3.3: `start_session` and `resume_session` carry `mcp_servers[], hat`; "The collector never sends a frame … to a host that lacks the capability".
- ACP core §6: "Per-session MCP isolation: `_meta.claudeCode.options.extraArgs["strict-mcp-config"] = ""` on **every** `session/new` and `session/load`"; "Any override (a custom adapter command, `CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`) drops that agent to the fallback, visibly, unless the operator explicitly accepts unverified isolation in the host's config."
- ACP core §8: "Gateway tokens and the `headers` of `mcp_servers` entries are never written to the events table and never sent over SSE."
- Gateway spec §3.2: the servers are `{type: "http", name: "hennery-<slug>", url, headers: [{name: "Authorization", value: "Bearer <session token>"}]}`, "followed by the stdio servers for that (host, hat) (§3.4)".
- Umbrella §8.5: "Fallback: on a mixed host, sessions in the host's default hat get the default hat's mounts; sessions in other hats run without gateway MCP servers … Isolation is never silently weakened to make a feature work."
- The spike: "It is not persisted across resume … hennery must send the same `_meta` (and the same `mcpServers`) on every `session/new` **and** every `session/load`."

It builds on plan 5c's `sessions.hat_id` and plan 7b's installed set and `--use-cli`. Every anchor was taken from `main` at `befa372`.

**Status:** written 2026-10-02; reviewed on the maintainer's behalf, approved after amendments and re-confirmed (see "The security review's answers"). One product question is with the maintainer: decision 8's ACP payloads (Q2), needed before 8e. Decision 4's window between 8c and 8e (Q1) was decided by the lane parent from umbrella §8.5 and the spike: proceed; release sequencing flagged to the fleet parent (see "After this plan").

**How the code blocks were made and checked:**
- Every block below was generated from the built code, on a scratch branch, task by task.
- The plan was replayed from its own text onto `befa372`. The tree matched each tests-only and task commit, byte for byte.
- After every task the five checks passed: 1393, 1408 and 1410 tests, from 1378.
- Every side-effect line was revert-probed (each task's Step 5).

## Execution status (2026-10-02)

**Executed** with subagent-driven development: one implementer per task, an opus reviewer per task (Task 4's a sonnet one), a whole-branch opus review. It ran on `fc00485`, where each task's tests-only and task commits matched the plan's replayed trees byte for byte. Main then moved twice, and the branch was rebased each time, every fix folded into the commit that needs it:
- onto `e4e2ca3` (8a, the purge plans, push delivery and others): `mcp_delivery()` names plan 9c's new `ForgetHat` (its exhaustive match refused to compile without it), the hub's `register` in a new `api.rs` test gets the isolation, the `resolve` test reads the renamed `find_session`, and plan 9c's `purge` test sends `mcp_isolation` in its `hello`;
- onto `befa372` (8d, plan 9d's forget): `Capability` gains `mcp_servers` after 9d's `forget_session` (the host announces both); the host's `attach` keeps 9d's agent-home recorder and passes the profile; the actor keeps 9d's `agent_session_id` and 8c's `secrets`; the fake answers with 9d's scripted session id and logs and echoes 8c's servers; `mcp_delivery()` names `ForgetSession`; the forget's probe match folds `McpUndeliverable` into `Unsupported` (a probe carries no servers), as the other probes do; 9d's `forget_host` frames carry the hat and no servers. The reviews' amendments came as follow-up commits, and the plan was amended to match (its blocks are the final code), so the history is:

| Commit | What |
|---|---|
| `b409730` | the plan |
| `8ba78f5`, `b0d016e` | Task 1's tests, then the wire and the hub's gate |
| `f5b7799`, `eaf537b` | Task 2's tests, then the host |
| `d63674a` | Task 2's review: the test value `scrub` cuts in part used `:`, which `Secrets` also cuts at, so the redact-before-`scrub` order was untested (four probes passed: the stderr tail, `redact_with`, the config note, the replay note); it now uses `,` and all four fail their test |
| `7128f75`, `76b1812` | Task 3's tests, then the cap |
| `44e7121`, `139be2b` | Task 3's review (important): at `warn` the ACP crate quotes an adapter's stdout line that is not JSON-RPC (`Invalid transport input`), so a token printed there reached the log at the default `RUST_LOG`; the fake's `stdout_lines`, a test that shows it uncapped, and the ACP crate held at `error` (tungstenite stays at `info`) |
| `dd7f84e` | the plan amended with both |
| `ceab87d` | Task 4, the spec write-back |
| `94d57ec` | the whole-branch review's minors: the start's and resume's catch-all comments say a refused delivery leaves the session `starting`; the `hello` test's accept is bounded; ACP core §3.3 lists every capability the host announces |
| `8aafdfa` | the fleet's rule that every outcome a classifier or verdict can give has a test and a probe of its own: an audit (opus) found a resume's servers never checked through the hub, the 409 mapping untested and the host's delivered path untested; four tests, and probes P46–P80 |

| Area | As built | Why |
|---|---|---|
| The security review (opus, on the maintainer's behalf) and its re-confirmation | As recorded in "The security review's answers". | |
| Q1, decision 4's window | Decided by the lane parent: proceed; the release blocker below. | |
| Q2, decision 8's ACP payloads | Open, with the maintainer, before 8e. | |
| Task 1's review (opus) | Approved; minors recorded (two `impl HostConn` blocks, the generated TS's required lists, as elsewhere in the protocol). | |
| Task 2's review (opus) | One important finding, plan-mandated: the test value above. Fixed, re-reviewed: addressed. | |
| Task 3's review (opus) | One important finding, plan-mandated: the ACP crate's `warn`. Fixed, re-reviewed: addressed, and none of the crate's `error` events quotes a message. Its comments were made descriptive in a second round. | |
| Task 4's review (sonnet) | Approved. | |
| The whole-branch review (opus) | Ready to merge; six minors: three fixed (`94d57ec`), three added to "After this plan" (an agent's HTTP MCP capability, 8e's waived-flag test, a held rollback set's isolation). | |
| The outcome audit (opus) and its tests' review (sonnet) | Four tests (`8aafdfa`), approved; the 409-versus-502 status of `mcp_isolation_unavailable` left to 8e ("After this plan"). | The fleet rule of 2026-10-02. |

Checks:
- The five checks passed after every task: 1393, 1408 and 1410 tests, from 1378; at the branch's tip, 1410.
- Every revert-probe was run on the built code (P1–P45 on `fc00485`, P46–P80 on `e4e2ca3`, all again on `befa372`): P1–P79 each failed its test (P18 by the compiler; P38 with its line as P43 left it; P13 with the capability list 9d extended). The first full run found the four the test value hid; earlier results had predated the `:@` cut that hid them. P80 (`tokio_tungstenite` held at `info`) is caught by nothing: that crate traces no message at its target, so its cap is defence in depth.
- The four test binaries this plan adds or extends (`hub_mcp`, `host_connection`, `session_mcp`, `session_mcp_log`), four copies of each in parallel, three rounds, on `fc00485`, `e4e2ca3` and `befa372`, while the other lanes built: 48 runs each time, no failure.
- macOS only; ubuntu CI is the Linux check. No test reads another process's state.

## Scope

This is sub-plan 8c of plan 8, the MCP gateway (the lane's split: 8a the gateway crate, 8b egress, 8d the proxy, 8e the sessions wiring, 8f OAuth, 8g standalone and renderers, 8h the composed `CODEX_HOME`). It needs none of them.

That is **4 tasks**:
1. the wire, and the collector carrying the hat and gating servers in the hub;
2. the host: profiles, servers and Claude's strict flag on new and load, the refusal, redaction;
3. the process's log never shows a token;
4. writing the spec back.

**Out:**
- the `hennery-gateway` crate, session tokens and minting (8a, 8d, 8e);
- the delivery decision, mixedness and the fallback (8e, the lane's L2): the collector sends no servers in 8c;
- the composed `CODEX_HOME` (8h): Codex is not isolated in 8c;
- the operator's opt-in to unverified isolation for an override (decision 7);
- recording `mcp_isolation` in the host registry (decision 2).

## Decisions this plan makes where the spec is silent

Put to the security review on the maintainer's behalf (below); its answers are recorded there.

1. **The wire.**
   - `McpServer` is tagged by `type`: `http` `{name, url, headers[{name, value}]}`, `stdio` `{name, command, args[], env[{name, value}]}`. That is ACP's `mcpServers` entry, except that ACP leaves stdio untagged; the host builds ACP's form. Absent lists are empty ones.
   - On both frames, `McpDelivery {mcp_servers, isolation_waived}` is flattened beside `hat_id` (a `String`, empty for a session from before hats). Each is left out when empty, false or blank. So a frame with no servers is byte for byte what an older host reads, and an older collector's frame decodes with none.
   - `hat_id`, not the spec table's `hat`: it is `sessions.hat_id`. The host carries it unused; plan 8h composes `CODEX_HOME` by it.
   - `CollectorFrame::mcp_delivery()` is exhaustive over the frames, as `probe_capability` is: a new frame must say whether it carries servers.
2. **What a host announces, and where the collector keeps it.**
   - A capability, `mcp_servers`: "I pass a start's or resume's servers into `session/new` / `session/load` with each agent's isolation, and refuse those I cannot isolate unless waived". Without it the hub sends no servers: serde would drop an unknown field on an older host unseen (the lane's L3).
   - `hello.mcp_isolation`: per agent id, `claude_strict` or `none`. It is a map of its own, not ACP core §3.3's `agents[]`, which is reserved for the picker (version, auth, catalogue) and not on the wire yet: a half-filled one would mislead its eventual consumer.
   - Read leniently like `capabilities`: an unknown value (plan 8h's) reads as `none`, never as isolated and never a reason to refuse the `hello`; an agent left out is `none`. Only values the host produces are defined.
   - Kept in the hub, with the live connection, like the capabilities the gates read (ACP core §3.3). `Hub::mcp_isolation(host, agent) -> Option<(announced, McpIsolation)>` is for 8e; a connection that did not announce `mcp_servers` reports `none` for every agent, so 8e cannot read an isolation it would not get. Not in the host registry: that needs a kernel migration while 8a adds one of its own, and nothing displays it yet.
3. **Profiles come from where an agent came from, never from its name alone.**
   - The installed set's `claude` (the pinned `claude-agent-acp` with its bundled CLI) is `Profile::Claude`: the strict flag, and `claude_strict`.
   - The same adapter with the operator's CLI (`--use-cli`, which sets `CLAUDE_CODE_EXECUTABLE`, an override ACP core §6 names) is `ClaudeOwnCli`: the strict flag is still sent, since it narrows what loads, but the host reports `none`.
   - The installed set's `codex` is `Generic` until 8h. So is any `--agent` command, even one named `claude` (a custom adapter command is an override too): no `_meta`, `none`.
4. **The strict flag goes on every Claude `session/new` and `session/load`, with servers or without.**
   - The spike: the adapter keeps it across no load. Without it, the user's own MCP servers and claude.ai connectors load beside hennery's.
   - **This changes every Claude session hennery runs, from this plan on, before any gateway server exists: the user's own MCP servers and claude.ai connectors disappear from them.** That is the spec's intent (umbrella §8.5; the spike's conclusion 2: the gateway is the one place MCP is managed). The spec write-back (Task 4) says so in ACP core §6.
   - The window between 8c and 8e, in which Claude sessions have neither the user's MCP nor hennery's (Q1): decided by the lane parent from umbrella §8.5 and the spike: proceed; release sequencing flagged to the fleet parent. No release carries 8c without 8e ("After this plan").
   - The host sends no other `_meta` today; `Profile::session_meta` is the one object any later key joins.
5. **The host refuses servers it cannot isolate, unless the collector waives isolation.** *(Amends the lane's L3, which has the host refuse such servers outright.)*
   - An outright refusal would make umbrella §8.5's fallback impossible: it gives the default hat's mounts to a session in the default hat whatever its agent, and a single-hat host full mounts. Only the collector knows whether a host is mixed (the lane's L2), and the umbrella wins over a lane note.
   - So the frame says so when the collector knowingly delivers to an agent the host cannot isolate: `isolation_waived`. Absent, the host refuses. Isolation is never lost by omission, only by a decision 8e makes and the frame records.
   - The refusal is `HostFrame::Error{code: mcp_isolation_unavailable}`, before anything is spawned, as `cwd_not_canonical` is (plan 5c). Not a `start_failed` (the brief's example): nothing was started, so nothing is sequenced, and the collector's `Undo::Start` fails the session with the code. Its message names the agent, never a server.
   - The hub enforces the same rule collector-side (`HostConn::takes`), under the hosts lock, on the connection the frame would go out on. A host that reconnects on an older build cannot slip in between, the gap plan 6a recorded for `images`. A frame it refuses is `RequestError::McpUndeliverable`, a variant of its own (not `Unsupported`, which the probes answer as `projects_unsupported`), and nothing is sent; `Hub::notify` checks the same. `api.rs` would answer it 409 `mcp_isolation_unavailable`; the probes fold it into their `Unsupported` arm (a probe carries no servers).
6. **8c's collector sends `hat_id` and no servers.** `api.rs` sends the hat it just stored (a start) or re-resolved (a resume). No other change in `hennery-sessions`.
7. **The operator's opt-in to unverified isolation is deferred.** ACP core §6 lets an operator accept an override's isolation in the host's config. Nothing reads it until 8e delivers, and its form (per agent? per override?) is best settled with the fallback. So `ClaudeOwnCli` and every `Generic` agent report `none` in 8c, and the opt-in is listed for 8e.
8. **Token hygiene** (ACP core §8, gateway spec §11, the lane's L11).
   - `NameValue`'s and `McpServer`'s `Debug` show names, the URL's origin and the command, never a header's or env variable's value, the URL's path, query or userinfo, nor a stdio server's arguments (a key can be one). So does every frame's `Debug`, which `bail!("… {other:?}")` and friends log. The origin is `frames::url_origin` (`scheme://host[:port]`), the gateway lane's rule L11; 8a, 8d and 8f can use it too. `Secrets`' own `Debug` shows only how many it holds.
   - `url_origin` and `url_secrets` fail closed: a URL that does not read one way only shows as `<redacted>` and is a secret whole. That is: a scheme other than `http` or `https` (an MCP server's only ones; else a value put before `://` would show as the origin), a backslash, an `@` past the authority (`https://u:ab/cd@h/x`, a base64 userinfo with a `/`), a host of other characters than `[A-Za-z0-9._-]` or a bracketed IPv6 literal, a port of other than digits. Not a URL parser: a URL it refuses is only shown less.
   - Found while building: at `trace` the ACP crate logs every JSON-RPC line it sends (`Sending JSON-RPC message`, the whole `session/new`). At `debug` it logs the adapter's answers, so an error that quotes the config reaches the log too. tungstenite, through the `log` bridge, logs every WebSocket message on both ends (`Received message`, `Sending frame`). So `RUST_LOG=trace` would print every session's token, and every server's URL.
   - Found in the task review: at `warn` the ACP crate logs an adapter's stdout line that is not JSON-RPC whole (`Invalid transport input`, the parse error's `data`), so an adapter printing its config there would leak a token at the default `RUST_LOG`.
   - `hennery_host::logging::capped` holds `agent_client_protocol` at `error` and `tungstenite` and `tokio_tungstenite` at `info` (each target its own level; the ACP crate's own `error` events quote no message). It is a global filter over the subscriber, so no `RUST_LOG` lifts it. `hennery`'s `log::init` installs each of its three subscribers through one `install`, and a source audit test pins that.
   - A frame the host cannot decode was logged with serde's error text, which quotes a string of the frame (a header value, say). It is now logged by the error's kind, line and column only.
   - The host redacts a session's secret values from the text it writes itself: a `start_failed` message (before its log line too), a `turn_ended` error, a `host_note` (the start's or resume's config note and the replay note before `scrub`, which would otherwise cut a value in part so it no longer matches whole), an `adapter_exited` stderr tail (before `scrub` too), and the actor's `error` answers (`reject`, a refused `set_config` quoting the adapter). `Secrets::redact_body` is exhaustive over `SessionBody`, as `mcp_delivery()` is over the frames: a new body must say whether it carries such text. The actor's log lines that quote an adapter's error go through `Actor::redacted` (defense in depth: their paths, a send to a closing connection, have no test that reaches them, so they are not revert-probed). The `cancel_unanswered` note is not redacted again: its text is the host's own and a tail `stderr_tail` already redacted.
   - A secret value is what `McpServer::secret_values` names (a header's or env variable's value, a stdio server's argument, an HTTP URL's userinfo and everything past its authority), each of its parts between whitespace or the URL delimiters `/?#&=:@` (a bare token echoed without `Bearer `, one segment of a URL's path, the password of its `user:password`), and the JSON-escaped form of each (an adapter may echo its config as JSON), of at least 8 bytes (`MIN_SECRET_LEN`: shorter ones are likely ordinary words). It errs on the safe side: an ordinary word of 8 bytes or more in a stdio argument (`Projects` in a path) is redacted too, which costs diagnostic text, never a secret.
   - **Left open, for the maintainer (8e):** a token an agent prints into an ACP payload (a tool's output). ACP core §2.3 keeps payloads verbatim; §8 says gateway tokens never reach the events table. 8c delivers no tokens, so it changes neither; `redact_body` leaves `acp_update` and `pending_opened` as they are.
   - L11's canary: the host tests put a key in the HTTP server's URL path and assert it absent from the log at `trace`, from `start_failed`, its log line and `adapter_exited`, as they do the token.
9. **The fake adapter's record.**
   - `session_log` appends each `session/new` and `session/load` as the fake parsed it. An entry the schema drops is then missing from the record, as from a real adapter's view; the tests compare whole lists.
   - It is written to a file, never stderr, which reaches `adapter_exited`.
   - `new_session_error_echoes` makes the scripted `session/new` error quote the servers, as an adapter refusing its config might; `prompt_error` and `config_error` answer every prompt and every `set_config_option` with an error of the test's text.

## The security review's answers

**Round 0 (record lost).** An earlier round ran on a previous draft, in a session lost to machine restarts; its answers were never recorded. Its amendments were in the code (a variant of its own for the hub's refusal, `notify` checked too, a non-map `mcp_isolation` read as empty, stdio arguments and the URL past its origin as secret values, redaction before `scrub`, an undecodable frame logged by kind and place), and the code comments that cited its numbered findings were made descriptive. Round 1 reviewed them as code.

**Round 1** (an opus security reviewer, agent `ac71ecd43efa64d9f`, 2026-10-02, on this plan and the scratch diff): **approve after amendments.** Its answers to the spec-gap decisions:
- A (capability shape): sound; a per-connection map in the hub, read leniently and failing closed, with an exhaustive `mcp_delivery()`; out of the registry until something displays it.
- B (per-agent isolation): sound; the profile comes from where the agent came from (`from_set`, `run_host`), and `INHERITED_OVERRIDE_VARS` strips an inherited `CLAUDE_CODE_EXECUTABLE`, so a plain `claude` is the pinned one.
- C (`--use-cli`): an override, as ACP core §6 names `CLAUDE_CODE_EXECUTABLE`; the opt-in can wait, as 8c delivers nothing.
- D (the refusal, `isolation_waived`): a sound reading of umbrella §8.5, not a weakening; 8e must test that the flag is never set for a non-default hat on a mixed host.
- E (`_meta` on every Claude new and load): the end state is the spec's (ACP core §6, the spike's conclusion 2). The window between 8c and 8e, in which Claude sessions lose the user's own MCP with nothing in its place, is a product decision: put to the maintainer (decision 4).
- F (token and URL hygiene): the `Debug` impls, the cap and the decode logging hold; gaps F1, F3, F4, F6 below.

Its findings, and what was done:
1. F1 (blocking) `turn_ended.error` quoted the adapter's error unredacted, and `redact_body` ended in a catch-all: redacted, `redact_body` made exhaustive; an end-to-end test through `emit` with new fake knobs (`prompt_error`, `config_error`). The same test pins `reject`'s answers, which were also unredacted (found while fixing F1).
2. F2 (should-fix) the probe "drop `redact_body` in `emit`" was caught by no test: it now fails `an_adapters_errors_quoting_its_secrets_are_redacted_in_every_answer_and_fact`; the probe text is corrected.
3. F3 (should-fix) notes were scrubbed before `emit` redacted them: the config note and the replay note are redacted first, each pinned by a value `scrub` cuts in part. The `cancel_unanswered` note is left: its text is the host's own plus an already-redacted tail.
4. F4 (should-fix) `url_origin` / `url_secrets` did not fail closed: `url_parts` refuses an `@` past the authority, a backslash, a bad host, IPv6 literal or port; canaries for each.
5. F5 (nit) `Hub::mcp_isolation` reported an isolation without the capability: now `(false, None)`.
6. F6 (nit) the actor's log lines quoting an adapter's error: through `Actor::redacted`; not revert-probed (no test reaches those paths), recorded as defense in depth.
7. F7 (nit) `log_switch`'s doc comment sat on `log_session`: moved.
Found while fixing the L11 canary: one segment of a URL's path echoed alone was not redacted; `Secrets` now also cuts values at `/?#&=`.

**Escalated:** (Q1) decision 4's window between 8c and 8e: decided by the lane parent from umbrella §8.5 and the spike: proceed; release sequencing flagged to the fleet parent (no release with 8c but without 8e). (Q2) a token an agent prints into an ACP payload (ACP core §2.3 verbatim vs §8): open, with the maintainer, needed before 8e. The §8 write-back no longer says payloads are "outside this".

**Re-confirmation** (a fresh opus reviewer, agent `a9a410d3199b94113`, 2026-10-02, scoped to F1–F7, the `/?#&=` cut, the descriptive comments and the §8 write-back): **confirmed with nits**; F1–F7 ok; the `cancel_unanswered` exception sound (the message is the host's, the tail already redacted before `scrub`); F6 acceptable as defense in depth (every adapter-error log line in `session.rs` redacted; the ACP and tungstenite logs capped). Its nits, all applied: only `http`/`https` schemes read (a value before `://` showed as the origin), with a canary; `Secrets` also cuts at `:@` (a userinfo's password echoed alone), with a canary; the over-redaction of ordinary 8-byte words in stdio arguments said in the doc comment; the §8 write-back says the payload question is open. Each new line revert-probed (P42–P44).

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task the five checks pass (fmt, both clippy runs, the workspace tests, `gen --check`).
- **No new crates.** `tracing-subscriber` (in the workspace) becomes a dependency of `hennery-host` (Task 3), and `tracing` and `tracing-subscriber` dev-dependencies of `hennery-testkit` (Task 2); `Cargo.lock` changes with each, as the blocks show.
- **Wire types change in Task 1**; regenerate there.
- **No Linux-only code.** No test reads another process's state; the ones that wait poll with a deadline.
- No token, header value or env value in any log, event or `Debug`.
- Commits: Conventional Commits, gmail identity, unsigned; a tests-only commit, then the task's. Push after every task; never push `main`.

## Review Focus

1. **A start or resume with servers for an agent its host cannot isolate.**
   - Expected: the hub sends nothing (`McpUndeliverable`); a host that gets it anyway refuses it, `mcp_isolation_unavailable`, before any spawn; waived, it passes.
   - Tests: Task 1 `servers_go_only_where_they_are_announced_and_isolated_or_waived`, `a_reconnect_without_the_capability_takes_no_servers`, `notify_sends_no_servers_a_connection_cannot_take`; Task 2 `servers_an_agent_cannot_be_kept_to_are_refused_before_any_spawn`.
2. **A resume losing Claude's isolation.**
   - Expected: the same servers and the strict flag on every `session/load`, as on `session/new`; the flag without servers too, and with the operator's CLI.
   - Tests: Task 2 `claudes_servers_and_strict_flag_go_on_new_and_on_every_load`, `claude_is_strict_without_servers_too_and_its_own_cli_as_well`.
3. **An agent claiming isolation it does not have.**
   - Expected: only the pinned Claude with its bundled CLI reports `claude_strict`; `--use-cli`, Codex and any `--agent` report `none`; an unknown value from a newer host reads as `none`.
   - Tests: Task 1 `a_hello_announces_per_agent_isolation_read_leniently`; Task 2 `hello_announces_mcp_servers_and_how_each_agent_is_isolated`, `only_the_pinned_claude_with_its_own_cli_is_isolated`, the profile asserts in `runtime.rs`.
4. **A token in a log or an event.**
   - Expected: none at `RUST_LOG=trace`; none in a frame's `Debug`; none in `start_failed`, `host_note`, `adapter_exited`, nor the host's own log line. The same for a key in the URL past its origin (L11).
   - Tests: Task 1 `debug_never_shows_header_or_env_values_or_arguments`, `a_url_shows_only_its_origin`; Task 2 `the_sessions_secret_values_never_reach_what_it_reports`, `a_start_failure_quoting_the_servers_is_redacted_in_the_fact_and_the_log`, `an_adapters_errors_quoting_its_secrets_are_redacted_in_every_answer_and_fact`, `a_replay_note_quoting_a_secret_is_redacted_before_scrub`, `the_text_hennery_writes_is_redacted_in_every_body_that_has_some`; Task 3 `a_sessions_token_is_never_logged_even_at_trace`, `every_subscriber_is_installed_capped`.
5. **An older host or collector.**
   - Expected: frames without the new fields decode; frames with none leave them out; a `hello` without `mcp_isolation` isolates nothing.
   - Tests: Task 1 `the_new_fields_are_left_out_when_empty_and_default_when_absent`, `a_hello_announces_per_agent_isolation_read_leniently`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/{frames,codegen}.rs`, generated files | the wire types and fields | 1 |
| `crates/hennery-sessions/src/{hub,ws,api,projects,resolve}.rs` | isolation per connection, the hub's guard, the hat on the frames | 1 |
| `crates/hennery-host/src/connection.rs` | compiles against the new fields (1); announces, refuses, profiles (2) | 1, 2 |
| `crates/hennery-host/src/profile.rs` (new) | `Profile`, `mcp_refusal`, `acp_servers`, `Secrets` | 2 |
| `crates/hennery-host/src/{session,adapter,lib}.rs`, `runtime/agents.rs`; `crates/hennery/src/{main,runtime}.rs` | servers and `_meta` on new and load, redaction, profiles from the set | 2 |
| `crates/hennery-host/src/logging.rs` (new), `crates/hennery/src/log.rs`, `crates/hennery-host/Cargo.toml` | the log cap | 3 |
| `crates/hennery-testkit/src/{lib.rs,bin/hennery-fake-acp.rs}` | `session_log`, `new_session_error_echoes` | 2 |
| `docs/specs/2026-09-26-{acp-core,mcp-gateway}-design.md` | the write-back | 4 |
| Tests: `crates/hennery-proto/tests/{mcp,frames}.rs`; `crates/hennery-sessions/tests/{hub_mcp,hub}.rs`; `crates/hennery-testkit/tests/{session_mcp,session_mcp_log,host_connection,resolve,host_session,…}.rs`; `crates/hennery-host/tests/runtime.rs` | | 1–3 |

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

"Run:" lines only check or regenerate: `cargo run -p hennery-proto --bin gen` rewrites the generated files and changes no other file. The plan was replayed exactly this way, from its own text, onto `befa372`.

---

### Task 1: The wire, and the collector carrying the hat and gating servers

**Files:**
- Modify: `crates/hennery-proto/src/{frames,codegen}.rs`, the generated files; `crates/hennery-sessions/src/{hub,ws,api,projects,resolve}.rs`; `crates/hennery-host/src/connection.rs` (to compile only)
- Test: `crates/hennery-proto/tests/mcp.rs` (new), `crates/hennery-sessions/tests/hub_mcp.rs` (new), `crates/hennery-testkit/tests/resolve.rs`; the new fields in the existing literals of `crates/hennery-proto/tests/frames.rs`, `crates/hennery-sessions/tests/hub.rs` and `crates/hennery-testkit/tests/{auth,e2e,images,projects,reconcile,ws_ingest_error,host_connection}.rs`

**Interfaces:**
- Produces, in `hennery_proto::frames`:
  - `NameValue {name, value}` (`Debug` hides `value`), `NameValue::new`;
  - `McpServer {Http {name, url, headers}, Stdio {name, command, args, env}}` (`Debug` hides values, arguments and the URL past its origin), `McpServer::secret_values`; `url_origin`, `url_secrets`;
  - `McpDelivery {mcp_servers, isolation_waived}`; `hat_id: String` and `mcp: McpDelivery` on `CollectorFrame::StartSession` and `ResumeSession`; `CollectorFrame::mcp_delivery()`, `CollectorFrame::agent()`;
  - `Capability::McpServers`; `McpIsolation {ClaudeStrict, None}`; `AgentIsolation(BTreeMap<String, McpIsolation>)` with `get`, `isolates`, lenient `Deserialize`; `mcp_isolation: AgentIsolation` on `HostFrame::Hello`.
- Produces, in `hennery_sessions::hub`: `Hub::register(host, tx, capabilities, mcp_isolation)` (one argument more); `Hub::mcp_isolation(host, agent) -> Option<(bool, McpIsolation)>`, `(false, None)` for a connection without `mcp_servers` whatever its `hello` said; `RequestError::McpUndeliverable`, which a request whose servers the connection cannot take answers, while `Hub::notify` returns `false` for such a frame.
- Consumes: plan 5c's `sessions.hat_id`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            cwd: "/tmp".into(),
            config: Default::default(),
        },
```

with:

```rust
            cwd: "/tmp".into(),
            config: Default::default(),
            hat_id: String::new(),
            mcp: Default::default(),
        },
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            config: Default::default(),
        },
```

with:

```rust
            config: Default::default(),
            hat_id: String::new(),
            mcp: Default::default(),
        },
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        config: Default::default(),
```

with:

```rust
        config: Default::default(),
        hat_id: String::new(),
        mcp: Default::default(),
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        attached_sessions: vec![],
```

with:

```rust
        attached_sessions: vec![],
        mcp_isolation: Default::default(),
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        config: config(),
```

with:

```rust
        config: config(),
        hat_id: String::new(),
        mcp: Default::default(),
```

Create `crates/hennery-proto/tests/mcp.rs`:

```rust
//! Plan 8c: a session's MCP servers and hat on `start_session` /
//! `resume_session`, and what a host announces it can isolate (ACP core
//! §3.3, §6, §8; gateway spec §3.2, §3.4).

use hennery_proto::frames::{
    AgentIsolation, Capabilities, Capability, CollectorFrame, HostFrame, McpDelivery, McpIsolation, McpServer,
    NameValue,
};
use serde_json::json;

const TOKEN: &str = "hst_0123456789abcdef";

fn http() -> McpServer {
    McpServer::Http {
        name: "hennery-notes".into(),
        url: "https://hennery.example/mcp/notes".into(),
        headers: vec![NameValue::new("Authorization", format!("Bearer {TOKEN}"))],
    }
}

fn stdio() -> McpServer {
    McpServer::Stdio {
        name: "hennery-files".into(),
        command: "/usr/local/bin/files-mcp".into(),
        args: vec!["--root".into(), "/srv/secret-root".into()],
        env: vec![NameValue::new("FILES_KEY", "files-key-0123456789")],
    }
}

fn start(hat_id: &str, mcp: McpDelivery) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        config: Default::default(),
        hat_id: hat_id.into(),
        mcp,
    }
}

#[test]
fn servers_use_the_acp_entry_shapes_with_a_type_tag() {
    assert_eq!(
        serde_json::to_value(http()).unwrap(),
        json!({
            "type": "http", "name": "hennery-notes", "url": "https://hennery.example/mcp/notes",
            "headers": [{"name": "Authorization", "value": format!("Bearer {TOKEN}")}]
        })
    );
    assert_eq!(
        serde_json::to_value(stdio()).unwrap(),
        json!({
            "type": "stdio", "name": "hennery-files", "command": "/usr/local/bin/files-mcp",
            "args": ["--root", "/srv/secret-root"],
            "env": [{"name": "FILES_KEY", "value": "files-key-0123456789"}]
        })
    );
    // Absent lists are empty ones.
    let bare: McpServer = serde_json::from_value(json!({"type": "stdio", "name": "n", "command": "/c"})).unwrap();
    assert_eq!(
        bare,
        McpServer::Stdio {
            name: "n".into(),
            command: "/c".into(),
            args: vec![],
            env: vec![]
        }
    );
}

#[test]
fn start_and_resume_carry_the_hat_and_the_servers() {
    let mcp = McpDelivery {
        mcp_servers: vec![http(), stdio()],
        isolation_waived: false,
    };
    let frame = start("hat-1", mcp.clone());
    let value = serde_json::to_value(&frame).unwrap();
    assert_eq!(value["hat_id"], json!("hat-1"));
    assert_eq!(value["mcp_servers"].as_array().unwrap().len(), 2);
    assert!(value.get("isolation_waived").is_none(), "{value}");
    assert_eq!(serde_json::from_value::<CollectorFrame>(value).unwrap(), frame);
    let resume = CollectorFrame::ResumeSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 3,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        agent_session_id: "a1".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: McpDelivery {
            isolation_waived: true,
            ..mcp
        },
    };
    let value = serde_json::to_value(&resume).unwrap();
    assert_eq!(
        (&value["hat_id"], &value["isolation_waived"]),
        (&json!("hat-1"), &json!(true))
    );
    assert_eq!(serde_json::from_value::<CollectorFrame>(value).unwrap(), resume);
}

/// An older collector sends none of the new fields, and an older host
/// reads none: empty ones are left out, absent ones are empty.
#[test]
fn the_new_fields_are_left_out_when_empty_and_default_when_absent() {
    let value = serde_json::to_value(start("", McpDelivery::default())).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "start_session", "request_id": "r", "session_id": "s", "committed_seq": 0,
            "agent": "claude", "cwd": "/tmp"
        })
    );
    let old: CollectorFrame = serde_json::from_value(json!({
        "type": "resume_session", "request_id": "r", "session_id": "s", "committed_seq": 3,
        "agent": "claude", "cwd": "/tmp", "agent_session_id": "a1"
    }))
    .unwrap();
    let CollectorFrame::ResumeSession { hat_id, mcp, .. } = old else {
        panic!("{old:?}");
    };
    assert_eq!((hat_id.as_str(), mcp), ("", McpDelivery::default()));
    assert_eq!(
        start("", McpDelivery::default()).mcp_delivery(),
        Some(&McpDelivery::default())
    );
}

/// ACP core §8: a frame's `Debug` (logged by `{other:?}` and friends) never
/// shows a header's or an env variable's value, nor a stdio server's
/// arguments.
#[test]
fn debug_never_shows_header_or_env_values_or_arguments() {
    let frame = start(
        "hat-1",
        McpDelivery {
            mcp_servers: vec![http(), stdio()],
            isolation_waived: false,
        },
    );
    let shown = format!("{frame:?} {frame:#?}");
    for secret in [TOKEN, "files-key-0123456789", "/srv/secret-root", "/mcp/notes"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    for visible in [
        "hennery-notes",
        "Authorization",
        "FILES_KEY",
        "\"https://hennery.example\"",
    ] {
        assert!(shown.contains(visible), "{shown}");
    }
    let (http, stdio) = (http(), stdio());
    let secrets: Vec<&str> = http.secret_values().into_iter().chain(stdio.secret_values()).collect();
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(
        secrets,
        [
            bearer.as_str(),
            "/mcp/notes",
            "files-key-0123456789",
            "--root",
            "/srv/secret-root"
        ]
    );
    use hennery_proto::frames::url_secrets;
    assert_eq!(url_secrets("https://u:pw@h.example:1/p?q#f"), ["u:pw", "/p?q#f"]);
    assert_eq!(url_secrets("https://h.example"), Vec::<&str>::new());
    assert_eq!(url_secrets("no scheme"), ["no scheme"]);
    // Read more than one way: all of it is secret.
    for ambiguous in [
        "https://u:ab/cd@h.example/x",
        "https://ab/cd@h.example/x",
        "https://h.example\\@evil/x",
        "https://h.example:8a/x",
    ] {
        assert_eq!(url_secrets(ambiguous), [ambiguous]);
    }
}

/// The gateway lane's L11: an upstream URL shows only as its origin, with
/// a canary in its path, query, fragment and userinfo. A URL that reads
/// more than one way shows as `<redacted>`: a userinfo with a `/` in it
/// (base64), a backslash, a host or port of other characters.
#[test]
fn a_url_shows_only_its_origin() {
    use hennery_proto::frames::url_origin;
    let canary = "canary-0123456789";
    for (url, origin) in [
        (format!("https://h.example/mcp/{canary}"), "https://h.example"),
        (format!("https://h.example:8443/?k={canary}"), "https://h.example:8443"),
        (format!("http://u:{canary}@h.example#{canary}"), "http://h.example"),
        (format!("not a url {canary}"), "<redacted>"),
        (format!("{canary}://h.example/p"), "<redacted>"),
        ("HTTPS://h.example/p".to_string(), "HTTPS://h.example"),
        (format!("https://[::1]:8443/{canary}"), "https://[::1]:8443"),
        (format!("https://u:{canary}/x@h.example/p"), "<redacted>"),
        // A userinfo with a `/` in it, whose first part reads as a host.
        (format!("https://{canary}/rest@h.example/p"), "<redacted>"),
        (format!("https://{canary}\\@h.example/p"), "<redacted>"),
        (format!("https://h.example\\{canary}"), "<redacted>"),
        (format!("https://h.example:{canary}/p"), "<redacted>"),
        (format!("https://h.example{canary}%/p"), "<redacted>"),
        (format!("https://[{canary}]/p"), "<redacted>"),
    ] {
        assert_eq!(url_origin(&url), origin);
        let server = McpServer::Http {
            name: "n".into(),
            url,
            headers: vec![],
        };
        assert!(!format!("{server:?}").contains(canary));
    }
}

fn hello(extra: serde_json::Value) -> HostFrame {
    let mut v = json!({
        "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
        "proof": "p", "attached_sessions": []
    });
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

/// What a host can isolate is read leniently, like its capabilities: a
/// mechanism this build does not know counts as none (fail closed), and an
/// agent left out is not isolated.
#[test]
fn a_hello_announces_per_agent_isolation_read_leniently() {
    let HostFrame::Hello {
        capabilities,
        mcp_isolation,
        ..
    } = hello(json!({
        "capabilities": ["mcp_servers", "park"],
        "mcp_isolation": {"claude": "claude_strict", "codex": "codex_home_from_the_future", "fake": "none"}
    }))
    else {
        panic!("expected hello");
    };
    assert!(capabilities.has(Capability::McpServers));
    assert_eq!(mcp_isolation.get("claude"), McpIsolation::ClaudeStrict);
    assert_eq!(mcp_isolation.get("codex"), McpIsolation::None);
    assert_eq!(mcp_isolation.get("fake"), McpIsolation::None);
    assert_eq!(mcp_isolation.get("absent"), McpIsolation::None);
    assert!(mcp_isolation.isolates("claude"));
    assert!(!mcp_isolation.isolates("codex") && !mcp_isolation.isolates("absent"));
    // An older host sends no map: nothing is isolated.
    let HostFrame::Hello { mcp_isolation, .. } = hello(json!({})) else {
        panic!("expected hello");
    };
    assert_eq!(mcp_isolation, AgentIsolation::default());
    // Not a map at all: nothing isolated, and the `hello` still reads.
    let HostFrame::Hello { mcp_isolation, .. } = hello(json!({"mcp_isolation": ["claude"]})) else {
        panic!("expected hello");
    };
    assert_eq!(mcp_isolation, AgentIsolation::default());
    let sent = HostFrame::Hello {
        protocol_version: "1.0".into(),
        host_version: "0".into(),
        host_id: "h".into(),
        proof: "p".into(),
        capabilities: Capabilities(vec![Capability::McpServers]),
        mcp_isolation: AgentIsolation(
            [
                ("claude".to_string(), McpIsolation::ClaudeStrict),
                ("codex".to_string(), McpIsolation::None),
            ]
            .into_iter()
            .collect(),
        ),
        workspace_roots: vec![],
        attached_sessions: vec![],
    };
    let value = serde_json::to_value(&sent).unwrap();
    assert_eq!(value["capabilities"], json!(["mcp_servers"]));
    assert_eq!(
        value["mcp_isolation"],
        json!({"claude": "claude_strict", "codex": "none"})
    );
}

#[test]
fn only_starts_and_resumes_carry_a_delivery() {
    let prompt = CollectorFrame::Prompt {
        request_id: "r".into(),
        session_id: "s".into(),
        turn_id: "t".into(),
        content: vec![],
    };
    assert_eq!(prompt.mcp_delivery(), None);
    let frame = start(
        "",
        McpDelivery {
            mcp_servers: vec![http()],
            isolation_waived: false,
        },
    );
    assert_eq!(frame.mcp_delivery().unwrap().mcp_servers, [http()]);
    assert_eq!(frame.agent(), Some("claude"));
}
```

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
        .register("h", tx, Capabilities::default())
```

with:

```rust
        .register("h", tx, Capabilities::default(), Default::default())
```

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
    let first = hub.register("h", tx, Capabilities(vec![Capability::Park])).unwrap();
    assert!(hub.has_capability("h", Capability::Park));
    assert!(!hub.has_capability("h", Capability::Images));
    hub.unregister("h", first.conn_id);
    assert!(!hub.has_capability("h", Capability::Park), "a gone host has none");
    let (tx, _rx) = mpsc::unbounded_channel();
    hub.register("h", tx, Capabilities::default()).unwrap();
```

with:

```rust
    let first = hub
        .register("h", tx, Capabilities(vec![Capability::Park]), Default::default())
        .unwrap();
    assert!(hub.has_capability("h", Capability::Park));
    assert!(!hub.has_capability("h", Capability::Images));
    hub.unregister("h", first.conn_id);
    assert!(!hub.has_capability("h", Capability::Park), "a gone host has none");
    let (tx, _rx) = mpsc::unbounded_channel();
    hub.register("h", tx, Capabilities::default(), Default::default())
        .unwrap();
```

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
        .register("h", tx, Capabilities(vec![Capability::Projects]))
```

with:

```rust
        .register("h", tx, Capabilities(vec![Capability::Projects]), Default::default())
```

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
    let registration = hub.register("h", tx, Capabilities::default()).unwrap();
```

with:

```rust
    let registration = hub
        .register("h", tx, Capabilities::default(), Default::default())
        .unwrap();
```

Create `crates/hennery-sessions/tests/hub_mcp.rs`:

```rust
//! Plan 8c (the lane's L3): MCP servers go out only on a connection that
//! announced `mcp_servers`, and only for an agent it isolates unless the
//! collector waived that. Checked by the hub against the connection the
//! frame would go out on; nothing is sent otherwise.

use hennery_proto::frames::{
    AgentIsolation, Capabilities, Capability, CollectorFrame, McpDelivery, McpIsolation, McpServer, NameValue,
    SessionBody,
};
use hennery_sessions::hub::{Hub, RequestError};
use std::time::Duration;
use tokio::sync::mpsc;

fn start(agent: &str, servers: bool, waived: bool) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: "r1".into(),
        session_id: "s1".into(),
        committed_seq: 0,
        agent: agent.into(),
        cwd: "/tmp".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: McpDelivery {
            mcp_servers: if servers {
                vec![McpServer::Http {
                    name: "hennery-notes".into(),
                    url: "https://hennery.example/mcp/notes".into(),
                    headers: vec![NameValue::new("Authorization", "Bearer hst_0123456789abcdef")],
                }]
            } else {
                vec![]
            },
            isolation_waived: waived,
        },
    }
}

fn connect(hub: &Hub, capabilities: Capabilities) -> mpsc::UnboundedReceiver<CollectorFrame> {
    let (tx, rx) = mpsc::unbounded_channel();
    let isolation = AgentIsolation(
        [
            ("claude".to_string(), McpIsolation::ClaudeStrict),
            ("codex".to_string(), McpIsolation::None),
        ]
        .into_iter()
        .collect(),
    );
    let registration = hub.register("h", tx, capabilities, isolation).unwrap();
    hub.mark_ready("h", registration.conn_id);
    rx
}

/// Send `frame`; `Ok` once it went out (resolved by the test), or the error.
async fn send(
    hub: &Hub,
    rx: &mut mpsc::UnboundedReceiver<CollectorFrame>,
    frame: CollectorFrame,
) -> Result<(), RequestError> {
    let request = hub.request("h", "r1", frame, Duration::from_secs(5));
    tokio::pin!(request);
    tokio::select! {
        result = &mut request => result.map(|_| ()),
        Some(_) = rx.recv() => {
            hub.resolve("r1", SessionBody::session_started("r1", "a1"));
            request.await.map(|_| ())
        }
    }
}

#[tokio::test]
async fn servers_go_only_where_they_are_announced_and_isolated_or_waived() {
    let with = Capabilities(vec![Capability::McpServers]);
    let cases = [
        // (capabilities, agent, servers, waived, delivered)
        (with.clone(), "claude", true, false, true),
        (with.clone(), "codex", true, false, false),
        (with.clone(), "unknown", true, false, false),
        (with.clone(), "codex", true, true, true),
        (Capabilities::default(), "claude", true, false, false),
        (Capabilities::default(), "claude", true, true, false),
        // No servers: every host takes the frame, as before plan 8c.
        (Capabilities::default(), "codex", false, false, true),
    ];
    for (capabilities, agent, servers, waived, delivered) in cases {
        let hub = Hub::new();
        let mut rx = connect(&hub, capabilities.clone());
        let result = send(&hub, &mut rx, start(agent, servers, waived)).await;
        let case = format!("{capabilities:?} {agent} servers={servers} waived={waived}");
        if delivered {
            assert_eq!(result, Ok(()), "{case}");
        } else {
            assert_eq!(result, Err(RequestError::McpUndeliverable), "{case}");
            assert!(rx.try_recv().is_err(), "sent anyway: {case}");
        }
    }
}

#[tokio::test]
async fn the_hub_reports_each_agents_isolation_from_the_live_connection() {
    let hub = Hub::new();
    assert_eq!(hub.mcp_isolation("h", "claude"), None);
    let _rx = connect(&hub, Capabilities(vec![Capability::McpServers]));
    assert_eq!(
        hub.mcp_isolation("h", "claude"),
        Some((true, McpIsolation::ClaudeStrict))
    );
    assert_eq!(hub.mcp_isolation("h", "codex"), Some((true, McpIsolation::None)));
    assert_eq!(hub.mcp_isolation("h", "absent"), Some((true, McpIsolation::None)));
}

/// A host that reconnects on an older build (no `mcp_servers`) loses what
/// its last connection announced at once: nothing with servers goes out on
/// the new one, and the hub reports no isolation for it.
#[tokio::test]
async fn a_reconnect_without_the_capability_takes_no_servers() {
    let hub = Hub::new();
    let old = connect(&hub, Capabilities(vec![Capability::McpServers]));
    drop(old); // the old socket's writer is gone
    let mut rx = connect(&hub, Capabilities::default());
    let result = send(&hub, &mut rx, start("claude", true, false)).await;
    assert_eq!(result, Err(RequestError::McpUndeliverable));
    assert!(rx.try_recv().is_err(), "sent anyway");
    assert_eq!(hub.mcp_isolation("h", "claude"), Some((false, McpIsolation::None)));
}

/// A frame nobody waits for (`Hub::notify`) is checked too.
#[test]
fn notify_sends_no_servers_a_connection_cannot_take() {
    let hub = Hub::new();
    let mut rx = connect(&hub, Capabilities::default());
    assert!(!hub.notify("h", start("claude", true, false)));
    assert!(rx.try_recv().is_err(), "sent anyway");
    assert!(hub.notify("h", start("claude", false, false)));
}

/// `start(agent, true, waived)` as a resume.
fn resume(agent: &str, waived: bool) -> CollectorFrame {
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        committed_seq,
        agent,
        cwd,
        config,
        hat_id,
        mcp,
    } = start(agent, true, waived)
    else {
        unreachable!()
    };
    CollectorFrame::ResumeSession {
        request_id,
        session_id,
        committed_seq,
        agent,
        cwd,
        agent_session_id: "a1".into(),
        config,
        hat_id,
        mcp,
    }
}

/// A resume carries servers as a start does (an agent keeps none across
/// `session/load`), and the hub checks it the same way: by the agent it
/// names, on the connection it would go out on.
#[tokio::test]
async fn a_resume_with_servers_is_checked_as_a_start_is() {
    let with = Capabilities(vec![Capability::McpServers]);
    for (capabilities, agent, waived, delivered) in [
        // (capabilities, agent, waived, delivered)
        (with.clone(), "claude", false, true),
        (with.clone(), "codex", false, false),
        (with, "codex", true, true),
        (Capabilities::default(), "claude", true, false),
    ] {
        let hub = Hub::new();
        let mut rx = connect(&hub, capabilities.clone());
        let result = send(&hub, &mut rx, resume(agent, waived)).await;
        let case = format!("{capabilities:?} {agent} waived={waived}");
        if delivered {
            assert_eq!(result, Ok(()), "{case}");
        } else {
            assert_eq!(result, Err(RequestError::McpUndeliverable), "{case}");
            assert!(rx.try_recv().is_err(), "sent anyway: {case}");
        }
    }
}

/// A frame that carries no delivery (a prompt) is not the guard's to
/// refuse: it goes out on any connection, one that announced nothing too.
#[tokio::test]
async fn a_frame_without_a_delivery_goes_out_on_any_connection() {
    let hub = Hub::new();
    let mut rx = connect(&hub, Capabilities::default());
    let prompt = CollectorFrame::Prompt {
        request_id: "r1".into(),
        session_id: "s1".into(),
        turn_id: "t1".into(),
        content: vec![],
    };
    assert_eq!(send(&hub, &mut rx, prompt).await, Ok(()));
}
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
            attached_sessions: vec![],
```

with:

```rust
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        .register("host-1", tx, Capabilities(vec![Capability::ResolvePath]))
```

with:

```rust
        .register(
            "host-1",
            tx,
            Capabilities(vec![Capability::ResolvePath]),
            Default::default(),
        )
```

In `crates/hennery-testkit/tests/forget_host.rs`, replace:

```rust
            config: Default::default(),
        }
```

with:

```rust
            config: Default::default(),
            hat_id: String::new(),
            mcp: Default::default(),
        }
```

In `crates/hennery-testkit/tests/forget_host.rs`, replace:

```rust
            config: Default::default(),
        })
```

with:

```rust
            config: Default::default(),
            hat_id: String::new(),
            mcp: Default::default(),
        })
```

In `crates/hennery-testkit/tests/forget_host.rs`, replace:

```rust
        config: Default::default(),
```

with:

```rust
        config: Default::default(),
        hat_id: String::new(),
        mcp: Default::default(),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        cwd: canonical_temp_dir(),
        config: Default::default(),
    }
```

with:

```rust
        cwd: canonical_temp_dir(),
        config: Default::default(),
        hat_id: String::new(),
        mcp: Default::default(),
    }
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        config: Default::default(),
    }
```

with:

```rust
        config: Default::default(),
        hat_id: String::new(),
        mcp: Default::default(),
    }
```

In `crates/hennery-testkit/tests/images.rs`, replace:

```rust
            attached_sessions: vec![],
```

with:

```rust
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust
            capabilities,
            workspace_roots,
            attached_sessions: vec![],
        })
        .await;
```

with:

```rust
            capabilities,
            workspace_roots,
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
        })
        .await;
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust
            attached_sessions: vec![],
        })
```

with:

```rust
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
        })
```

In `crates/hennery-testkit/tests/purge.rs`, replace:

```rust
            attached_sessions: attached,
```

with:

```rust
            attached_sessions: attached,
            mcp_isolation: Default::default(),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            attached_sessions: attached,
```

with:

```rust
            attached_sessions: attached,
            mcp_isolation: Default::default(),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            attached_sessions: vec![],
```

with:

```rust
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        config: Default::default(),
```

with:

```rust
        config: Default::default(),
        hat_id: String::new(),
        mcp: Default::default(),
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    async fn connect_with(collector: &Collector, capabilities: Capabilities) -> Self {
```

with:

```rust
    async fn connect_with(collector: &Collector, capabilities: Capabilities) -> Self {
        Self::connect_announcing(collector, capabilities, Default::default()).await
    }

    /// `connect_with`, announcing `mcp_isolation` too (plan 8c).
    async fn connect_announcing(
        collector: &Collector,
        capabilities: Capabilities,
        mcp_isolation: hennery_proto::frames::AgentIsolation,
    ) -> Self {
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
            attached_sessions: vec![],
```

with:

```rust
            attached_sessions: vec![],
            mcp_isolation,
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    assert_eq!(call.await.unwrap().0, 202);
}

```

with:

```rust
    assert_eq!(call.await.unwrap().0, 202);
}

/// Plan 8c: a start and a resume carry the session's hat, and no servers
/// yet (minting is plan 8e's); the per-agent isolation a `hello` announces
/// reaches the hub, from the live connection.
#[tokio::test]
async fn starts_and_resumes_carry_the_sessions_hat_and_no_servers() {
    use hennery_proto::frames::{AgentIsolation, McpDelivery, McpIsolation, SessionBody};
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    rule(&collector, "/home/me/acme", &acme, true);
    let isolation = AgentIsolation([("fake".to_string(), McpIsolation::ClaudeStrict)].into_iter().collect());
    let capabilities = Capabilities(vec![Capability::ResolvePath, Capability::McpServers]);
    let mut host = ScriptedHost::connect_announcing(&collector, capabilities, isolation).await;
    assert_eq!(
        collector.state.hub.mcp_isolation(HOST, "fake"),
        Some((true, McpIsolation::ClaudeStrict))
    );

    let call = start(&collector, "/home/me/acme");
    host.answer("/home/me/acme", true).await;
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        hat_id,
        mcp,
        ..
    } = host.next().await
    else {
        panic!("expected a start");
    };
    assert_eq!((hat_id.as_str(), &mcp), (acme.as_str(), &McpDelivery::default()));
    host.emit(&session_id, SessionBody::session_started(request_id, "agent-1"))
        .await;
    assert_eq!(call.await.unwrap().0, 202);
    host.parked(&session_id).await;
    wait_for("parked", || async {
        let row = collector.state.store.find_session(&session_id).unwrap().unwrap();
        (row.lifecycle == "parked").then_some(())
    })
    .await;

    let call = send(
        &collector,
        "POST",
        &format!("/api/sessions/{session_id}/resume"),
        json!({}),
    );
    host.answer("/home/me/acme", true).await;
    let CollectorFrame::ResumeSession {
        request_id,
        hat_id,
        mcp,
        ..
    } = host.next().await
    else {
        panic!("expected a resume");
    };
    assert_eq!((hat_id.as_str(), &mcp), (acme.as_str(), &McpDelivery::default()));
    host.emit(&session_id, SessionBody::session_started(request_id, "agent-1"))
        .await;
    assert_eq!(call.await.unwrap().0, 202);
}

```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
            capabilities: Default::default(),
            workspace_roots: vec![],
            attached_sessions: vec![],
        })
        .unwrap(),
```

with:

```rust
            capabilities: Default::default(),
            workspace_roots: vec![],
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
        })
        .unwrap(),
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
            attached_sessions: vec![],
        })
```

with:

```rust
            attached_sessions: vec![],
            mcp_isolation: Default::default(),
        })
```


- [ ] **Step 2: Run them to see them fail, and commit them**

Run: `cargo test --locked -p hennery-proto --test mcp`
Expected: FAIL to compile: `unresolved imports hennery_proto::frames::AgentIsolation, …McpDelivery, …McpIsolation, …McpServer, …NameValue`; `variant Hello does not have a field named mcp_isolation`.

Run: `cargo test --locked -p hennery-sessions --test hub_mcp`
Expected: FAIL to compile, the same imports, and `this method takes 3 arguments but 4 arguments were supplied`.

```bash
git add crates
git commit -m "test(gateway): MCP servers and the hat on start and resume, and the hub's delivery guard"
```

- [ ] **Step 3: The wire, the hub's guard and the hat on the frames**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    AttachedSession, Capabilities, Capability, CollectorFrame, ForgetReason, HostFrame, SessionConfig,
```

with:

```rust
    AgentIsolation, AttachedSession, Capabilities, Capability, CollectorFrame, ForgetReason, HostFrame, SessionConfig,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            ]),
```

with:

```rust
            ]),
            // Nothing isolated yet: the host takes no servers until it
            // announces `mcp_servers` (plan 8c, Task 2).
            mcp_isolation: AgentIsolation::default(),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            cwd,
            config,
        } => attach(
```

with:

```rust
            cwd,
            config,
            hat_id: _,
            mcp: _,
        } => attach(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            config,
        } => attach(
```

with:

```rust
            config,
            hat_id: _,
            mcp: _,
        } => attach(
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        frames::Capabilities,
```

with:

```rust
        frames::Capabilities,
        frames::NameValue,
        frames::McpServer,
        frames::McpDelivery,
        frames::McpIsolation,
        frames::AgentIsolation,
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    ForgetSession,
```

with:

```rust
    ForgetSession,
    /// A session's MCP servers (plan 8c): the host passes a start's or
    /// resume's `mcp_servers` into `session/new` / `session/load` with the
    /// agent's isolation, as `hello.mcp_isolation` reports it, and refuses
    /// servers it cannot isolate unless the collector waived that
    /// (`mcp_isolation_unavailable`). A host without it gets no servers.
    McpServers,
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
            raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
```

with:

```rust
            raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        ))
    }
}

/// A name and a value: an HTTP header of an MCP server, or an environment
/// variable of a stdio one. The value can be a secret (a gateway session
/// token, a stdio server's key), so `Debug` never shows it (ACP core §8).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct NameValue {
    pub name: String,
    pub value: String,
}

impl NameValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl std::fmt::Debug for NameValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NameValue")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// One MCP server for a session (gateway spec §3.2, §3.4): an entry of ACP
/// `session/new` / `session/load` `mcpServers`, tagged by `type` here (ACP
/// itself leaves stdio entries untagged; the host builds those). A new
/// server type or a new required field needs a new capability: a host that
/// cannot decode a start drops it unanswered, and the collector's timeout
/// then drops the connection.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServer {
    /// A streamable-HTTP server: the gateway's `/mcp/<slug>`, with the
    /// session's token in `Authorization`.
    Http {
        name: String,
        url: String,
        #[serde(default)]
        headers: Vec<NameValue>,
    },
    /// A local server the agent runs as its own child.
    Stdio {
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<NameValue>,
    },
}

impl McpServer {
    /// Every part that may be a secret, as `Debug` hides them: the
    /// headers' and the env's values, an HTTP server's URL past its origin
    /// and its userinfo (`url_secrets`), a stdio server's arguments.
    pub fn secret_values(&self) -> Vec<&str> {
        match self {
            Self::Http { url, headers, .. } => headers
                .iter()
                .map(|pair| pair.value.as_str())
                .chain(url_secrets(url))
                .collect(),
            Self::Stdio { args, env, .. } => env
                .iter()
                .map(|pair| pair.value.as_str())
                .chain(args.iter().map(String::as_str))
                .collect(),
        }
    }
}

/// `url` cut at its authority: `(scheme, userinfo, host[:port], rest)`, or
/// `None` when it does not read one way only. Fails closed: a scheme other
/// than `http` or `https` (an MCP server's only ones), a backslash, an `@` past the authority (`u:ab/cd@h`, a
/// userinfo with a `/` in it), or a host or port of other characters than
/// theirs, and the whole URL counts as a secret. Not a URL parser.
fn url_parts(url: &str) -> Option<(&str, Option<&str>, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) || url.contains('\\') {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(end);
    if after.contains('@') {
        return None;
    }
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let port = if let Some(v6) = host_port.strip_prefix('[') {
        // An IPv6 literal, `[…]`, with an optional port.
        let (inside, tail) = v6.split_once(']')?;
        if inside.is_empty() || !inside.chars().all(|c| c.is_ascii_hexdigit() || ":.".contains(c)) {
            return None;
        }
        match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':')?),
        }
    } else {
        let (host, port) = match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        };
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
            return None;
        }
        port
    };
    if port.is_some_and(|port| port.is_empty() || !port.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some((scheme, userinfo, host_port, after))
}

/// `url` as `scheme://host[:port]`: the one form of an upstream URL any
/// log, error body or `Debug` shows (the gateway lane's rule L11). Its
/// path, query, fragment and userinfo can carry a secret. A URL that does
/// not read one way only (`url_parts`) shows as `<redacted>`.
pub fn url_origin(url: &str) -> String {
    match url_parts(url) {
        Some((scheme, _, host_port, _)) => format!("{scheme}://{host_port}"),
        None => "<redacted>".into(),
    }
}

/// What `url_origin` leaves out: the userinfo, and everything after the
/// authority (path, query, fragment). Empty parts are left out; all of the
/// URL when it does not read one way only.
pub fn url_secrets(url: &str) -> Vec<&str> {
    let Some((_, userinfo, _, after)) = url_parts(url) else {
        return vec![url];
    };
    userinfo
        .into_iter()
        .chain(Some(after))
        .filter(|part| !part.is_empty())
        .collect()
}

/// Shows names, the URL's origin (`url_origin`) and the command; never a
/// header's or an env variable's value, the URL's path, nor a stdio
/// server's arguments (which may carry a key).
impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http { name, url, headers } => f
                .debug_struct("Http")
                .field("name", name)
                .field("url", &url_origin(url))
                .field("headers", headers)
                .finish(),
            Self::Stdio {
                name,
                command,
                args,
                env,
            } => f
                .debug_struct("Stdio")
                .field("name", name)
                .field("command", command)
                .field("args", &format_args!("<{} redacted>", args.len()))
                .field("env", env)
                .finish(),
        }
    }
}

/// The MCP part of a `start_session` / `resume_session` (ACP core §3.3,
/// §4.3; plan 8c). Flattened into the frame; empty fields are left out, so
/// a frame without servers is what an older host expects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpDelivery {
    /// Passed in `session/new` / `session/load`. Only to a host that
    /// announced `mcp_servers`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// The collector knowingly delivers to an agent the host cannot isolate
    /// (the mixed-host fallback's default hat, or a single-hat host,
    /// umbrella §8.5). Absent, the host refuses servers for such an agent
    /// (`mcp_isolation_unavailable`): isolation is never lost by omission.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolation_waived: bool,
}

/// How a host keeps an agent's sessions to the servers hennery passes (ACP
/// core §6). Lenient: a mechanism this build does not know reads as `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpIsolation {
    /// `--strict-mcp-config` through `_meta` on every `session/new` and
    /// `session/load` (the pinned Claude adapter, its own CLI).
    ClaudeStrict,
    /// None: the agent also loads the user's own MCP configuration.
    None,
}

/// `hello.mcp_isolation`: per agent id, how the host isolates its MCP
/// servers. An agent left out is not isolated. Deserialized leniently, like
/// `Capabilities`: an unknown mechanism counts as `none`, never as isolated
/// and never a reason to refuse the `hello`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema, TS)]
pub struct AgentIsolation(pub BTreeMap<String, McpIsolation>);

impl AgentIsolation {
    pub fn get(&self, agent: &str) -> McpIsolation {
        self.0.get(agent).copied().unwrap_or(McpIsolation::None)
    }

    /// The host keeps `agent`'s sessions to the servers it is given.
    pub fn isolates(&self, agent: &str) -> bool {
        self.get(agent) != McpIsolation::None
    }
}

impl<'de> Deserialize<'de> for AgentIsolation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Not a map at all reads as empty too: never a reason to refuse
        // the `hello`.
        let Value::Object(raw) = Value::deserialize(deserializer)? else {
            return Ok(Self::default());
        };
        Ok(Self(
            raw.into_iter()
                .map(|(agent, v)| (agent, serde_json::from_value(v).unwrap_or(McpIsolation::None)))
                .collect(),
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        capabilities: Capabilities,
```

with:

```rust
        capabilities: Capabilities,
        /// Per agent, how this host isolates its MCP servers (plan 8c).
        /// Absent means none is isolated (an older host).
        #[serde(default)]
        mcp_isolation: AgentIsolation,
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
            | Self::ForgetHat { .. } => Err(NotAProbe),
        }
    }
}

```

with:

```rust
            | Self::ForgetHat { .. } => Err(NotAProbe),
        }
    }

    /// A start's or resume's MCP part; `None` for every other frame. The
    /// hub's guard reads it (plan 8c), so it is exhaustive on purpose: a
    /// new frame must say whether it carries servers, or this does not
    /// compile.
    pub fn mcp_delivery(&self) -> Option<&McpDelivery> {
        match self {
            Self::StartSession { mcp, .. } | Self::ResumeSession { mcp, .. } => Some(mcp),
            Self::HelloAck { .. }
            | Self::HelloError { .. }
            | Self::Prompt { .. }
            | Self::CancelTurn { .. }
            | Self::SetConfig { .. }
            | Self::AnswerPermission { .. }
            | Self::AnswerElicitation { .. }
            | Self::Ack { .. }
            | Self::ResolvePath { .. }
            | Self::ParkSession { .. }
            | Self::CloseSession { .. }
            | Self::ListProjects { .. }
            | Self::BrowseDirectory { .. }
            | Self::ForgetHat { .. }
            | Self::ForgetSession { .. } => None,
        }
    }

    /// The agent a start or resume names.
    pub fn agent(&self) -> Option<&str> {
        match self {
            Self::StartSession { agent, .. } | Self::ResumeSession { agent, .. } => Some(agent),
            _ => None,
        }
    }
}

```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        #[serde(flatten)]
        config: SessionConfig,
    },
    /// Attach a parked, closed or failed session again: spawn the adapter and
```

with:

```rust
        #[serde(flatten)]
        config: SessionConfig,
        /// The session's hat (`sessions.hat_id`); empty for a session from
        /// before hats. Carried, not yet used by the host (plan 8c).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hat_id: String,
        #[serde(flatten)]
        mcp: McpDelivery,
    },
    /// Attach a parked, closed or failed session again: spawn the adapter and
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        config: SessionConfig,
    },
```

with:

```rust
        config: SessionConfig,
        /// As on `start_session`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        hat_id: String,
        /// Sent again on every resume: an agent keeps no servers across
        /// `session/load` (the spike).
        #[serde(flatten)]
        mcp: McpDelivery,
    },
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        RequestError::Busy => error(StatusCode::SERVICE_UNAVAILABLE, "busy", "the host is busy; try again"),
```

with:

```rust
        RequestError::Busy => error(StatusCode::SERVICE_UNAVAILABLE, "busy", "the host is busy; try again"),
        // Unreachable until plan 8e sends servers; 8e also fails the
        // session it left `starting`.
        RequestError::McpUndeliverable => error(
            StatusCode::CONFLICT,
            "mcp_isolation_unavailable",
            "the host cannot keep this agent's session to its MCP servers",
        ),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        config: req.config,
```

with:

```rust
        config: req.config,
        // The hat just stored. No servers yet: minting and the delivery
        // decision are plan 8e's.
        hat_id: hat.hat_id.clone(),
        mcp: Default::default(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`).
        Err(err) => request_failed(err),
```

with:

```rust
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`); not for `McpUndeliverable`, which the hub
        // refused before sending: that leaves the session `starting`, and
        // plan 8e fails it (unreachable in 8c, which sends no servers).
        Err(err) => request_failed(err),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        config,
```

with:

```rust
        config,
        // The hat the resume just re-resolved, equal to the stored one.
        hat_id: hat.hat_id.clone(),
        mcp: Default::default(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        // code (`Undo::Start`).
```

with:

```rust
        // code (`Undo::Start`); not for `McpUndeliverable`, which the hub
        // refused before sending: that leaves the session `starting`, and
        // plan 8e fails it (unreachable in 8c, which sends no servers).
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        assert_eq!(sent.len(), 2);
    }
}
```

with:

```rust
        assert_eq!(sent.len(), 2);
    }

    /// Plan 8c: the hub's refusal to send servers answers 409
    /// `mcp_isolation_unavailable`, on a start and on a resume alike.
    #[tokio::test]
    async fn an_undeliverable_mcp_delivery_answers_409_mcp_isolation_unavailable() {
        for response in [
            request_failed(RequestError::McpUndeliverable),
            resume_failed(RequestError::McpUndeliverable),
        ] {
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["code"], "mcp_isolation_unavailable", "{body}");
        }
    }
}
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
            .register(HOST, tx.clone(), Capabilities::default())
```

with:

```rust
            .register(HOST, tx.clone(), Capabilities::default(), Default::default())
```

In `crates/hennery-sessions/src/forget.rs`, replace:

```rust
        Err(RequestError::Unsupported) => (pending(RemovalPending::HostNeedsUpdate), false, false),
```

with:

```rust
        // A probe carries no servers: `McpUndeliverable` cannot happen.
        Err(RequestError::Unsupported | RequestError::McpUndeliverable) => {
            (pending(RemovalPending::HostNeedsUpdate), false, false)
        }
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{AgentIsolation, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    Unsupported,
```

with:

```rust
    Unsupported,
    /// The host's connection cannot take the MCP servers a start or resume
    /// carries (`HostConn::takes`, plan 8c): it did not announce
    /// `mcp_servers`, or does not isolate the agent and isolation was not
    /// waived. Nothing was sent.
    McpUndeliverable,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    capabilities: Capabilities,
```

with:

```rust
    capabilities: Capabilities,
    /// From the same `hello`: per agent, how it isolates MCP servers.
    mcp_isolation: AgentIsolation,
}

impl HostConn {
    /// Whether `frame` may go out on this connection (plan 8c, the lane's
    /// L3): MCP servers only to a host that announced `mcp_servers`, which
    /// would otherwise ignore them unseen, and only for an agent it
    /// isolates, unless the collector waived that. Checked under the hosts
    /// lock, against the very connection the frame goes out on, so a host
    /// that reconnects on an older build cannot slip in between.
    fn takes(&self, frame: &CollectorFrame) -> bool {
        let Some(mcp) = frame.mcp_delivery() else {
            return true;
        };
        mcp.mcp_servers.is_empty()
            || (self.capabilities.has(Capability::McpServers)
                && (mcp.isolation_waived || frame.agent().is_some_and(|agent| self.mcp_isolation.isolates(agent))))
    }
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    /// Register a host connection (not yet ready) with the capabilities its
    /// `hello` announced. A second live connection for the same host id is
    /// refused, never allowed to supersede the first silently.
    pub fn register(
        &self,
        host_id: &str,
        tx: mpsc::UnboundedSender<CollectorFrame>,
        capabilities: Capabilities,
```

with:

```rust
    /// Register a host connection (not yet ready) with the capabilities and
    /// the MCP isolation its `hello` announced. A second live connection for
    /// the same host id is refused, never allowed to supersede the first
    /// silently.
    pub fn register(
        &self,
        host_id: &str,
        tx: mpsc::UnboundedSender<CollectorFrame>,
        capabilities: Capabilities,
        mcp_isolation: AgentIsolation,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
                capabilities,
```

with:

```rust
                capabilities,
                mcp_isolation,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust

    /// Send a frame nobody waits for (an answer: its verdict arrives as a
```

with:

```rust

    /// How the host's current connection isolates `agent`'s MCP servers
    /// (`hello.mcp_isolation`), with whether it announced `mcp_servers` at
    /// all; `None` if it is not connected. For plan 8e's delivery decision.
    /// A connection without `mcp_servers` isolates nothing, whatever its
    /// `hello` said: it would take no servers.
    pub fn mcp_isolation(&self, host_id: &str, agent: &str) -> Option<(bool, hennery_proto::frames::McpIsolation)> {
        self.hosts.lock().expect("hosts lock").get(host_id).map(|h| {
            let announced = h.capabilities.has(Capability::McpServers);
            let isolation = if announced {
                h.mcp_isolation.get(agent)
            } else {
                hennery_proto::frames::McpIsolation::None
            };
            (announced, isolation)
        })
    }

    /// Send a frame nobody waits for (an answer: its verdict arrives as a
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
            .get(host_id)
            .filter(|h| h.routable())
            .is_some_and(|h| h.tx.send(frame).is_ok())
```

with:

```rust
            .get(host_id)
            // Nothing that carries servers goes out unchecked, here either.
            .filter(|h| h.routable() && h.takes(&frame))
            .is_some_and(|h| h.tx.send(frame).is_ok())
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
            };
            self.waiters.lock().expect("waiters lock").insert(
```

with:

```rust
            };
            if !host.takes(&frame) {
                return Err(RequestError::McpUndeliverable);
            }
            self.waiters.lock().expect("waiters lock").insert(
```

In `crates/hennery-sessions/src/projects.rs`, replace:

```rust
        RequestError::Unsupported => error(
```

with:

```rust
        // A probe carries no servers: `McpUndeliverable` cannot happen.
        RequestError::Unsupported | RequestError::McpUndeliverable => error(
```

In `crates/hennery-sessions/src/resolve.rs`, replace:

```rust
        Err(RequestError::Unsupported) => Err(NotResolved::Unsupported),
```

with:

```rust
        // A probe carries no servers: `McpUndeliverable` cannot happen.
        Err(RequestError::Unsupported | RequestError::McpUndeliverable) => Err(NotResolved::Unsupported),
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        capabilities,
```

with:

```rust
        capabilities,
        mcp_isolation,
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    let Some(registration) = state.hub.register(&host_id, tx.clone(), capabilities.clone()) else {
```

with:

```rust
    let Some(registration) = state
        .hub
        .register(&host_id, tx.clone(), capabilities.clone(), mcp_isolation)
    else {
```

Run: `cargo run -p hennery-proto --bin gen`


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test --locked -p hennery-proto --test mcp && cargo test --locked -p hennery-sessions --test hub_mcp && cargo test --locked -p hennery-testkit --test resolve starts_and_resumes`
Expected: PASS, 7, 4 and 1 tests.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `Hub::wait`, remove the `if !host.takes(&frame)` refusal. `servers_go_only_where_they_are_announced_and_isolated_or_waived` and `a_reconnect_without_the_capability_takes_no_servers` fail.
- In `Hub::notify`, drop `&& h.takes(&frame)`. `notify_sends_no_servers_a_connection_cannot_take` fails.
- In `AgentIsolation`'s `Deserialize`, refuse a value that is not a map. `a_hello_announces_per_agent_isolation_read_leniently` fails.
- In `McpServer`'s `Debug`, show the whole `url`. `a_url_shows_only_its_origin` and `debug_never_shows_header_or_env_values_or_arguments` fail.
- In `McpServer::secret_values`, drop `.chain(url_secrets(url))`. `debug_never_shows_header_or_env_values_or_arguments` fails.
- In `url_parts`, drop each refusal in turn: a scheme other than `http`/`https`, an `@` past the authority, a backslash, the host's characters, the IPv6 literal's, the port's digits. `debug_never_shows_header_or_env_values_or_arguments` or `a_url_shows_only_its_origin` fails on each.
- In `Hub::mcp_isolation`, report the `hello`'s isolation without the capability. `a_reconnect_without_the_capability_takes_no_servers` fails.
- In `ws.rs`, register the connection with `Default::default()` in place of the `hello`'s `mcp_isolation`. `starts_and_resumes_carry_the_sessions_hat_and_no_servers` fails on the hub's isolation.
- In `api.rs`, send `hat_id: String::new()` on the start, then on the resume. That test fails on each.
- Every outcome on its own (P46–P63): in `mcp_delivery()`, give a resume `None`; in `agent()`, drop the resume; `a_resume_with_servers_is_checked_as_a_start_is` fails on each. In `request_failed`, answer `McpUndeliverable` with another code, then another status; `an_undeliverable_mcp_delivery_answers_409_mcp_isolation_unavailable` fails on each. In `HostConn::takes`, drop each of its conditions in turn (no servers pass; servers need the capability; a waiver delivers; an isolated agent delivers; a frame without a delivery passes); `servers_go_only_where_they_are_announced_and_isolated_or_waived`, `a_reconnect_without_the_capability_takes_no_servers` or `a_frame_without_a_delivery_goes_out_on_any_connection` fails on each. Read an unknown isolation value, then an absent agent, as `claude_strict`; require `mcp_isolation` in a `hello`; `a_hello_announces_per_agent_isolation_read_leniently` fails on each. Show a parsed URL as `<redacted>`; give an unparsed URL no secret value; the URL tests fail.

- [ ] **Step 6: The full checks**

Expected: all pass; **1393 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates schema web
git commit -m "feat(gateway): MCP servers and the hat on start and resume, gated in the hub"
```

### Task 2: The host passes the servers with Claude's strict flag, and refuses what it cannot isolate

**Files:**
- Create: `crates/hennery-host/src/profile.rs`
- Modify: `crates/hennery-host/src/{session,connection,adapter,lib}.rs`, `crates/hennery-host/src/runtime/agents.rs`, `crates/hennery/src/{main,runtime}.rs`; `crates/hennery-testkit/src/{lib.rs,bin/hennery-fake-acp.rs}`, `crates/hennery-testkit/Cargo.toml`, `Cargo.lock`
- Test: `crates/hennery-testkit/tests/session_mcp.rs` (new), `crates/hennery-testkit/tests/{host_connection,host_session}.rs`, `crates/hennery-host/tests/runtime.rs`; `profile.rs`'s unit tests

**Interfaces:**
- Produces, in `hennery_host::profile`: `Profile {Claude, ClaudeOwnCli, Generic}` with `of_installed(name, own_cli)`, `mcp_isolation()`, `session_meta()`; `mcp_refusal(agent, profile, servers, waived) -> Option<String>`; `acp_servers(&[McpServer])`; `MIN_SECRET_LEN`; `Secrets::of(&[McpServer])`, `redact(&str)`, `redact_body(SessionBody)`.
- Produces: `HostConfig.profiles`, `HostConfig::profile(agent)`, `HostConfig::mcp_isolation()`; `Launch.profile` and `Launch.mcp_servers`; `Agents.profiles`; `hennery_host::adapter::REDACTED` public; `runtime::default_agents` returns the profiles too; the host's `hello` announces `mcp_servers` and `mcp_isolation`; the `mcp_isolation_unavailable` refusal.
- Produces, in the testkit: `FakeScript.session_log`, `FakeScript.new_session_error_echoes`, `FakeScript.prompt_error`, `FakeScript.config_error`.
- Consumes: Task 1's wire; plan 7b's `--use-cli` (`own_cli`).

- [ ] **Step 1: Write the failing tests**

In `Cargo.lock`, replace:

```toml
 "tokio-tungstenite",
 "webauthn-authenticator-rs",
```

with:

```toml
 "tokio-tungstenite",
 "tracing",
 "tracing-subscriber",
 "webauthn-authenticator-rs",
```

In `crates/hennery-host/tests/runtime.rs`, replace:

```rust
    assert!(prepared.in_use.is_some());
```

with:

```rust
    assert!(prepared.in_use.is_some());
    // The pinned Claude is isolated; Codex is not, until plan 8h.
    use hennery_host::profile::Profile;
    assert_eq!(prepared.agents.profiles["claude"], Profile::Claude);
    assert_eq!(prepared.agents.profiles["codex"], Profile::Generic);
```

In `crates/hennery-host/tests/runtime.rs`, replace:

```rust
    assert!(prepared.agents.agents["codex"].env.is_empty());
```

with:

```rust
    assert!(prepared.agents.agents["codex"].env.is_empty());
    // An override is unverified isolation (ACP core §6).
    assert_eq!(
        prepared.agents.profiles["claude"],
        hennery_host::profile::Profile::ClaudeOwnCli
    );
```

In `crates/hennery-testkit/Cargo.toml`, replace:

```toml
tokio-tungstenite.workspace = true
```

with:

```toml
tokio-tungstenite.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                async move |_req: NewSessionRequest, responder, cx| {
                    let id = script.session_id.clone().unwrap_or_else(|| "fake-session-1".into());
                    match script.new_session_error {
                        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
```

with:

```rust
                async move |req: NewSessionRequest, responder, cx| {
                    log_session(&script, "session/new", &req);
                    for line in &script.stdout_lines {
                        write_stdout_line(line);
                    }
                    let id = script.session_id.clone().unwrap_or_else(|| "fake-session-1".into());
                    match script.new_session_error {
                        Some(code) => {
                            let message = if script.new_session_error_echoes {
                                format!("scripted: {}", serde_json::to_string(&req.mcp_servers).unwrap())
                            } else {
                                "scripted".into()
                            };
                            responder.respond_with_error(agent_client_protocol::Error::new(code, message))
                        }
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                async move |req: LoadSessionRequest, responder, cx| {
```

with:

```rust
                async move |req: LoadSessionRequest, responder, cx| {
                    log_session(&script, "session/load", &req);
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                    log_switch(&script, &req);
```

with:

```rust
                    log_switch(&script, &req);
                    if let Some(message) = &script.config_error {
                        return responder
                            .respond_with_error(agent_client_protocol::Error::new(-32603, message.clone()));
                    }
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                async move |req: PromptRequest, responder, cx| {
```

with:

```rust
                async move |req: PromptRequest, responder, cx| {
                    if let Some(message) = &script.prompt_error {
                        return responder
                            .respond_with_error(agent_client_protocol::Error::new(-32603, message.clone()));
                    }
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust

/// Record a switch in the script's `config_log`, if it has one.
```

with:

```rust

/// `line` with a trailing newline, in one `write_all` through the same
/// `std::io::stdout()` the ACP crate's transport writes through (and
/// flushes after every line it sends). Called before the caller's own
/// answer exists, so no JSON-RPC line of the transport's is still being
/// written when this one lands (plan 8c).
fn write_stdout_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(format!("{line}\n").as_bytes());
    let _ = out.flush();
}

/// One `session_log` line: the request as parsed, so an entry the schema
/// could not take is missing from it, as from a real adapter's view.
fn log_session(script: &FakeScript, method: &str, params: &impl serde::Serialize) {
    let Some(path) = &script.session_log else {
        return;
    };
    let line = serde_json::json!({ "method": method, "params": params });
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the session log");
    writeln!(log, "{line}").expect("write the session log");
}

/// Record a switch in the script's `config_log`, if it has one.
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    pub stderr_lines: Vec<String>,
```

with:

```rust
    pub stderr_lines: Vec<String>,
    /// Lines written raw (not JSON-RPC) to stdout right before answering
    /// `session/new`: written while that answer does not exist yet, so the
    /// ACP crate has no JSON-RPC line of its own still being written to the
    /// same stdout (e.g. a fake token, to test that the host's log never
    /// shows what an adapter prints to its own stdout, plan 8c).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stdout_lines: Vec<String>,
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    pub new_session_error: Option<i32>,
```

with:

```rust
    pub new_session_error: Option<i32>,
    /// `new_session_error`'s message quotes the request's `mcpServers` (an
    /// adapter that echoes the config it refuses, plan 8c).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_session_error_echoes: bool,
    /// Answer every `session/prompt` with a JSON-RPC error carrying this
    /// message (an adapter whose error quotes its config, plan 8c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_error: Option<String>,
    /// Answer every `session/set_config_option` with a JSON-RPC error
    /// carrying this message, as `prompt_error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    pub config_log: Option<String>,
```

with:

```rust
    pub config_log: Option<String>,
    /// Append one JSON line per `session/new` and `session/load` to this
    /// file: `{"method", "params"}`, the request as the fake parsed it
    /// (plan 8c: its `mcpServers` and `_meta`). A file, never stderr, which
    /// would reach `adapter_exited`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_log: Option<String>,
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            grandchild_pid_file: None,
            replay: Vec::new(),
            load_error: None,
            new_session_error: None,
```

with:

```rust
            stdout_lines: Vec::new(),
            grandchild_pid_file: None,
            replay: Vec::new(),
            load_error: None,
            new_session_error: None,
            new_session_error_echoes: false,
            prompt_error: None,
            config_error: None,
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            config_log: None,
```

with:

```rust
            config_log: None,
            session_log: None,
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
/// `initialize` offers none (decision 2).
```

with:

```rust
/// `initialize` offers none (decision 2). Plan 8c adds `mcp_servers`.
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
            Capability::ForgetSession,
```

with:

```rust
            Capability::ForgetSession,
            Capability::McpServers,
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
    read_until(&mut stream, body_is("s1", "session_started")).await;
}

```

with:

```rust
    read_until(&mut stream, body_is("s1", "session_started")).await;
}

// Plan 8c: a session's MCP servers (the lane's L3).

/// `frame` carrying one gateway server for its session, the isolation
/// waived or not, for `agent`.
fn with_servers(mut frame: CollectorFrame, agent: &str, waived: bool) -> CollectorFrame {
    match &mut frame {
        CollectorFrame::StartSession { mcp, agent: a, .. } | CollectorFrame::ResumeSession { mcp, agent: a, .. } => {
            *a = agent.into();
            mcp.mcp_servers = vec![hennery_proto::frames::McpServer::Http {
                name: "hennery-notes".into(),
                url: "https://hennery.example/mcp/notes".into(),
                headers: vec![hennery_proto::frames::NameValue::new(
                    "Authorization",
                    "Bearer hst_0123456789abcdef",
                )],
            }];
            mcp.isolation_waived = waived;
        }
        other => panic!("{other:?}"),
    }
    frame
}

#[tokio::test]
async fn hello_announces_mcp_servers_and_how_each_agent_is_isolated() {
    use hennery_host::profile::Profile;
    use hennery_proto::frames::{Capability, McpIsolation};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut cfg = host_with_fake(addr, "mcp-hello", slow_fake());
    cfg.agents.insert("claude".into(), slow_fake());
    cfg.agents.insert("own-cli".into(), slow_fake());
    cfg.profiles.insert("claude".into(), Profile::Claude);
    cfg.profiles.insert("own-cli".into(), Profile::ClaudeOwnCli);
    tokio::spawn(run(cfg));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let ws = accept(tcp).await.unwrap();
    let (_sink, mut stream) = ws.split();
    let HostFrame::Hello {
        capabilities,
        mcp_isolation,
        ..
    } = read_host_frame(&mut stream).await
    else {
        panic!("expected hello");
    };
    assert!(capabilities.has(Capability::McpServers), "{capabilities:?}");
    assert_eq!(
        mcp_isolation.0,
        [
            ("claude".to_string(), McpIsolation::ClaudeStrict),
            ("fake".to_string(), McpIsolation::None),
            ("own-cli".to_string(), McpIsolation::None),
        ]
        .into_iter()
        .collect()
    );
}

/// Servers for an agent the host cannot keep to them are refused before
/// anything is spawned, on a start and on a resume: never dropped, never
/// passed. Waived, they pass; for an isolated agent, they pass unwaived.
#[tokio::test]
async fn servers_an_agent_cannot_be_kept_to_are_refused_before_any_spawn() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    let mut cfg = host_with_fake(addr, "mcp-refuse", counting_fake(&spawns));
    cfg.agents.insert("claude".into(), counting_fake(&spawns));
    cfg.profiles
        .insert("claude".into(), hennery_host::profile::Profile::Claude);
    tokio::spawn(run(cfg));
    let (mut sink, mut stream, _) = accept_host(&listener).await;

    for (request_id, frame) in [
        ("r1", with_servers(start("r1", "s1"), "fake", false)),
        (
            "r2",
            with_servers(resume("r2", "s1", 0, "fake-session-1"), "fake", false),
        ),
    ] {
        send_frame(&mut sink, &frame).await;
        match read_until(&mut stream, error_for(request_id)).await {
            HostFrame::Error { code, message, .. } => {
                assert_eq!(code, "mcp_isolation_unavailable", "{message}");
                assert!(!message.contains("hst_0123456789abcdef"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(!spawns.exists(), "an adapter was spawned");

    send_frame(&mut sink, &with_servers(start("r3", "s2"), "fake", true)).await;
    read_until(&mut stream, body_is("s2", "session_started")).await;
    send_frame(&mut sink, &with_servers(start("r4", "s3"), "claude", false)).await;
    read_until(&mut stream, body_is("s3", "session_started")).await;
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 2);
}

/// The fake adapter appending each `session/new` and `session/load` it
/// parses to `log`.
fn logging_fake(log: &std::path::Path) -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        session_log: Some(log.to_string_lossy().into_owned()),
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

/// A delivered start or resume reaches the adapter through the connection
/// with its servers, and with its agent's profile's `_meta`: strict for
/// Claude, on `session/new` and on `session/load` alike, none for a
/// generic agent.
#[tokio::test]
async fn a_delivered_start_reaches_the_adapter_with_its_servers_and_profile() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let mut cfg = host_with_fake(addr, "mcp-deliver", logging_fake(&log));
    cfg.agents.insert("claude".into(), logging_fake(&log));
    cfg.profiles
        .insert("claude".into(), hennery_host::profile::Profile::Claude);
    tokio::spawn(run(cfg));
    let (mut sink, mut stream, _) = accept_host(&listener).await;

    send_frame(&mut sink, &with_servers(start("r1", "s1"), "claude", false)).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(&mut sink, &with_servers(start("r2", "s2"), "fake", true)).await;
    read_until(&mut stream, body_is("s2", "session_started")).await;
    // A fresh session id: a resume of a live one restarts it, no load.
    send_frame(
        &mut sink,
        &with_servers(resume("r3", "s3", 0, "agent-3"), "claude", false),
    )
    .await;
    read_until(&mut stream, body_is("s3", "session_started")).await;

    // Each line is written before its request is answered, so before the
    // `session_started` read above.
    let logged: Vec<(String, serde_json::Value)> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| {
            let line: serde_json::Value = serde_json::from_str(line).unwrap();
            (line["method"].as_str().unwrap().to_string(), line["params"].clone())
        })
        .collect();
    let methods: Vec<&str> = logged.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(methods, ["session/new", "session/new", "session/load"], "{logged:?}");
    let servers = serde_json::json!([{
        "type": "http",
        "name": "hennery-notes",
        "url": "https://hennery.example/mcp/notes",
        "headers": [{"name": "Authorization", "value": "Bearer hst_0123456789abcdef"}],
    }]);
    let strict = serde_json::json!({"claudeCode": {"options": {"extraArgs": {"strict-mcp-config": ""}}}});
    for (method, params) in &logged {
        assert_eq!(params["mcpServers"], servers, "{method}: {params}");
    }
    assert_eq!(logged[0].1["_meta"], strict, "{}", logged[0].1);
    assert!(
        logged[1].1.get("_meta").is_none_or(serde_json::Value::is_null),
        "{}",
        logged[1].1
    );
    assert_eq!(logged[2].1["_meta"], strict, "{}", logged[2].1);
}

```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
        cwd: std::env::temp_dir(),
```

with:

```rust
        cwd: std::env::temp_dir(),
        profile: Default::default(),
        mcp_servers: Vec::new(),
```

Create `crates/hennery-testkit/tests/session_mcp.rs`:

```rust
//! Plan 8c: a session's MCP servers and its profile's `_meta` reach the
//! adapter on `session/new` and on every `session/load` (the spike: Claude
//! keeps neither across a load), and the session's secret values never
//! reach what the session reports (ACP core §8).

use hennery_host::outbox::Outbox;
use hennery_host::profile::Profile;
use hennery_host::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{ConfigValue, HostFrame, McpServer, NameValue, SessionBody, SessionConfig};
use hennery_testkit::{FakeScript, SCRIPT_ENV};
use serde_json::{Value, json};
use std::time::Duration;

const TOKEN: &str = "hst_live_token_0123456789";
const ENV_KEY: &str = "files-key-0123456789";
/// A key in a stdio server's argument.
const ARG_KEY: &str = "arg-secret-0123456789";
/// A key in an HTTP server's URL path: only its origin may show (the
/// gateway lane's rule L11).
const URL_KEY: &str = "url-secret-0123456789";
/// A value `scrub` cuts in part (`ghp_…` up to the comma, which `Secrets`
/// does not cut at): redacted whole only if it is redacted before `scrub`.
const SCRUBBED: &str = "ghp_0123456789,tail-secret-0123";

fn servers() -> Vec<McpServer> {
    vec![
        McpServer::Http {
            name: "hennery-notes".into(),
            url: format!("https://hennery.example/mcp/notes/{URL_KEY}"),
            headers: vec![NameValue::new("Authorization", format!("Bearer {TOKEN}"))],
        },
        McpServer::Stdio {
            name: "hennery-files".into(),
            command: "/usr/local/bin/files-mcp".into(),
            args: vec!["--root".into(), "/srv".into(), format!("--key={ARG_KEY}")],
            env: vec![NameValue::new("FILES_KEY", ENV_KEY), NameValue::new("GH", SCRUBBED)],
        },
    ]
}

/// The ACP form of `servers()`: an `http` entry, and an untagged stdio one.
fn acp_servers() -> Value {
    json!([
        {"type": "http", "name": "hennery-notes", "url": format!("https://hennery.example/mcp/notes/{URL_KEY}"),
         "headers": [{"name": "Authorization", "value": format!("Bearer {TOKEN}")}]},
        {"name": "hennery-files", "command": "/usr/local/bin/files-mcp",
         "args": ["--root", "/srv", format!("--key={ARG_KEY}")],
         "env": [{"name": "FILES_KEY", "value": ENV_KEY}, {"name": "GH", "value": SCRUBBED}]}
    ])
}

fn strict() -> Value {
    json!({"claudeCode": {"options": {"extraArgs": {"strict-mcp-config": ""}}}})
}

fn fake(script: &FakeScript) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    fake
}

/// Launch one session actor and wait for its first `session_started` or
/// `start_failed`.
async fn attach(script: &FakeScript, attach: Attach, profile: Profile, mcp_servers: Vec<McpServer>) -> Uplink {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach,
        config: Default::default(),
        agent: fake(script),
        cwd: std::env::temp_dir(),
        profile,
        mcp_servers,
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    wait_for(&uplink, |body| {
        matches!(
            body,
            SessionBody::SessionStarted { .. } | SessionBody::StartFailed { .. }
        )
    })
    .await;
    // Closed so the adapter is gone before the next launch reads the log
    // (a failed start has ended already).
    let _ = handle.send(SessionCmd::Close {
        request_id: "r-close".into(),
    });
    handle.finished().await;
    uplink
}

async fn wait_for(uplink: &Uplink, pred: impl Fn(&SessionBody) -> bool) -> Vec<HostFrame> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = uplink.pending().unwrap();
        if frames
            .iter()
            .any(|f| matches!(f, HostFrame::Session { body, .. } if pred(body)))
        {
            return frames;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out: {frames:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The session log's lines, as `(method, params)`.
fn logged(path: &std::path::Path) -> Vec<(String, Value)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let line: Value = serde_json::from_str(line).unwrap();
            (line["method"].as_str().unwrap().to_string(), line["params"].clone())
        })
        .collect()
}

fn script_logging_to(path: &std::path::Path) -> FakeScript {
    FakeScript {
        session_log: Some(path.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

#[tokio::test]
async fn claudes_servers_and_strict_flag_go_on_new_and_on_every_load() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log);
    attach(&script, Attach::New, Profile::Claude, servers()).await;
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    attach(&script, load.clone(), Profile::Claude, servers()).await;
    attach(&script, load, Profile::Claude, servers()).await;
    let logged = logged(&log);
    let methods: Vec<&str> = logged.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(methods, ["session/new", "session/load", "session/load"]);
    for (method, params) in &logged {
        assert_eq!(params["mcpServers"], acp_servers(), "{method}: {params}");
        assert_eq!(params["_meta"], strict(), "{method}: {params}");
    }
}

/// The strict flag is sent with no servers too (umbrella §8.5): without it
/// the user's own MCP servers and claude.ai connectors would load.
#[tokio::test]
async fn claude_is_strict_without_servers_too_and_its_own_cli_as_well() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log);
    attach(&script, Attach::New, Profile::Claude, vec![]).await;
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    attach(&script, load, Profile::ClaudeOwnCli, vec![]).await;
    for (method, params) in logged(&log) {
        assert_eq!(params["mcpServers"], json!([]), "{method}: {params}");
        assert_eq!(params["_meta"], strict(), "{method}: {params}");
    }
}

#[tokio::test]
async fn a_generic_agent_gets_its_servers_and_no_meta() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("sessions.jsonl");
    let script = script_logging_to(&log);
    attach(&script, Attach::New, Profile::Generic, servers()).await;
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    attach(&script, load, Profile::Generic, vec![]).await;
    let logged = logged(&log);
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[0].1["mcpServers"], acp_servers());
    assert_eq!(logged[1].1["mcpServers"], json!([]));
    for (method, params) in &logged {
        assert!(params.get("_meta").is_none_or(Value::is_null), "{method}: {params}");
    }
}

/// An adapter that echoes its MCP config (on stderr, then exiting): the
/// bare token and the env value are redacted from `adapter_exited`, as
/// from a `start_failed` or a `host_note`.
#[tokio::test]
async fn the_sessions_secret_values_never_reach_what_it_reports() {
    let script = FakeScript {
        stderr_lines: vec![
            format!("mcp config: token={TOKEN}"),
            format!("files: FILES_KEY={ENV_KEY}"),
            format!("argv: files-mcp --key={ARG_KEY}"),
            format!("upstream: https://hennery.example/mcp/notes/{URL_KEY}"),
            format!("gh: {SCRUBBED}"),
        ],
        exit_after_chunks: Some(0),
        ..Default::default()
    };
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach: Attach::New,
        config: Default::default(),
        agent: fake(&script),
        cwd: std::env::temp_dir(),
        profile: Profile::Claude,
        mcp_servers: servers(),
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    wait_for(&uplink, |body| matches!(body, SessionBody::SessionStarted { .. })).await;
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "r1".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type": "text", "text": "hi"})],
    }));
    let frames = wait_for(&uplink, |body| matches!(body, SessionBody::AdapterExited { .. })).await;
    let tail = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::AdapterExited { stderr_tail, .. },
                ..
            } => Some(stderr_tail.clone()),
            _ => None,
        })
        .unwrap();
    assert!(tail.contains("mcp config: token=[redacted]"), "{tail}");
    assert!(tail.contains("FILES_KEY=[redacted]"), "{tail}");
    let everything = serde_json::to_string(&frames).unwrap();
    for secret in [TOKEN, ENV_KEY, ARG_KEY, URL_KEY, "tail-secret"] {
        assert!(!everything.contains(secret), "{everything}");
    }
}

/// An adapter whose errors quote the session's secrets: in the note about
/// a start's switch that failed (with a value `scrub` would cut in part,
/// so it must be redacted first), in a refused `set_config`'s answer, and
/// in a failed turn's error. None reaches what the host reports.
#[tokio::test]
async fn an_adapters_errors_quoting_its_secrets_are_redacted_in_every_answer_and_fact() {
    let script = FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        config_error: Some(format!("refused {SCRUBBED} for {TOKEN}")),
        prompt_error: Some(format!("prompt failed: {TOKEN} {ENV_KEY} {URL_KEY}")),
        ..Default::default()
    };
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach: Attach::New,
        config: SessionConfig {
            model: Some("large".into()),
            ..Default::default()
        },
        agent: fake(&script),
        cwd: std::env::temp_dir(),
        profile: Profile::Claude,
        mcp_servers: servers(),
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    wait_for(&uplink, |body| matches!(body, SessionBody::HostNote { .. })).await;
    assert!(handle.send(SessionCmd::SetConfig {
        request_id: "r1".into(),
        config_id: "effort".into(),
        value: ConfigValue::Id("high".into()),
    }));
    let answer = tokio::time::timeout(Duration::from_secs(10), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "r2".into(),
        turn_id: "t1".into(),
        content: vec![json!({"type": "text", "text": "hi"})],
    }));
    let frames = wait_for(&uplink, |body| matches!(body, SessionBody::TurnEnded { .. })).await;
    let _ = handle.send(SessionCmd::Close {
        request_id: "r-close".into(),
    });
    handle.finished().await;
    let (answer, facts) = (
        serde_json::to_string(&answer).unwrap(),
        serde_json::to_string(&frames).unwrap(),
    );
    // The adapter did quote them, and they were redacted.
    assert!(
        answer.contains("config_failed") && answer.contains("[redacted]"),
        "{answer}"
    );
    for kind in ["host_note", "turn_ended"] {
        assert!(facts.contains(kind), "{facts}");
    }
    assert!(facts.contains("prompt failed: [redacted]"), "{facts}");
    for secret in [TOKEN, ENV_KEY, URL_KEY, "tail-secret"] {
        assert!(!answer.contains(secret), "{answer}");
        assert!(!facts.contains(secret), "{facts}");
    }
}

/// A resume whose replay names an unknown update kind that quotes a
/// secret `scrub` would cut in part: the note about it is redacted before
/// it is scrubbed.
#[tokio::test]
async fn a_replay_note_quoting_a_secret_is_redacted_before_scrub() {
    let script = FakeScript {
        replay: vec![json!({"sessionUpdate": format!("unknown {SCRUBBED}")})],
        ..Default::default()
    };
    let load = Attach::Load {
        agent_session_id: "fake-session-1".into(),
    };
    let uplink = attach(&script, load, Profile::Claude, servers()).await;
    let frames = uplink.pending().unwrap();
    let note = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::HostNote { note, text },
                ..
            } if note == "replay_unknown_dropped" => Some(text.clone()),
            _ => None,
        })
        .expect("a replay note");
    assert!(note.contains("[redacted]") && !note.contains("tail-secret"), "{note}");
}

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An adapter whose `session/new` error quotes its MCP config: the
/// `start_failed` it becomes, and the host's log line about it, show
/// neither the token nor the env value. The actor runs on the test's own
/// thread (a current-thread runtime), so the thread's subscriber sees it.
/// Only the host's own lines: the ACP crate logs the adapter's answer at
/// `debug`, echo included, which the process's output caps
/// (`hennery_host::logging`, `session_mcp_log.rs`).
#[tokio::test]
async fn a_start_failure_quoting_the_servers_is_redacted_in_the_fact_and_the_log() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("hennery_host=debug")
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let script = FakeScript {
        new_session_error: Some(-32603),
        new_session_error_echoes: true,
        ..Default::default()
    };
    let uplink = attach(&script, Attach::New, Profile::Claude, servers()).await;
    let frames = uplink.pending().unwrap();
    let message = frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::StartFailed { message, .. },
                ..
            } => Some(message.clone()),
            _ => None,
        })
        .unwrap();
    // The adapter did quote them, and they were redacted.
    assert!(
        message.contains("hennery-notes") && message.contains("[redacted]"),
        "{message}"
    );
    let log = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    assert!(log.contains("session start failed"), "{log}");
    for secret in [TOKEN, ENV_KEY, ARG_KEY, URL_KEY, "tail-secret"] {
        assert!(!message.contains(secret), "{message}");
        assert!(!log.contains(secret), "{log}");
    }
}
```


- [ ] **Step 2: Run them to see them fail, and commit them**

Run: `cargo test --locked -p hennery-testkit --test session_mcp`
Expected: FAIL to compile: `unresolved import hennery_host::profile`; `struct Launch has no field named profile` (and `mcp_servers`).

```bash
git add Cargo.lock crates
git commit -m "test(host): per-session MCP servers, Claude's strict flag and the refusal"
```

- [ ] **Step 3: Profiles, the servers and the strict flag on new and load, the refusal, redaction**

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
    group_killed: bool,
```

with:

```rust
    group_killed: bool,
    /// Its session's secret values (`redact_with`), redacted from the
    /// stderr tail before `scrub`: a value `scrub` cuts in part would no
    /// longer match whole.
    secrets: crate::profile::Secrets,
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
                group_killed: false,
```

with:

```rust
                group_killed: false,
                secrets: Default::default(),
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
    /// The last `STDERR_TAIL_BYTES` of stderr, scrubbed of token-like
    /// strings. Waits briefly for the pipe to drain if the adapter exited.
```

with:

```rust
    /// Redact `secrets` from the stderr tail.
    pub fn redact_with(&mut self, secrets: crate::profile::Secrets) {
        self.secrets = secrets;
    }

    /// The last `STDERR_TAIL_BYTES` of stderr, its session's secret values
    /// redacted (`redact_with`), then scrubbed of token-like strings.
    /// Waits briefly for the pipe to drain if the adapter exited.
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
        if !truncated {
            return scrub(&String::from_utf8_lossy(&bytes));
        }
        match bytes.iter().position(|&b| b == b'\n') {
            Some(idx) => scrub(&String::from_utf8_lossy(&bytes[idx + 1..])),
```

with:

```rust
        let clean = |bytes: &[u8]| scrub(&self.secrets.redact(&String::from_utf8_lossy(bytes)));
        if !truncated {
            return clean(&bytes);
        }
        match bytes.iter().position(|&b| b == b'\n') {
            Some(idx) => clean(&bytes[idx + 1..]),
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
const REDACTED: &str = "[redacted]";
```

with:

```rust
/// What a redacted secret reads as.
pub const REDACTED: &str = "[redacted]";
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
use crate::outbox::Outbox;
```

with:

```rust
use crate::outbox::Outbox;
use crate::profile::Profile;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    AgentIsolation, AttachedSession, Capabilities, Capability, CollectorFrame, ForgetReason, HostFrame, SessionConfig,
```

with:

```rust
    AgentIsolation, AttachedSession, Capabilities, Capability, CollectorFrame, ForgetReason, HostFrame, McpDelivery,
    SessionConfig,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    pub agents: HashMap<String, AgentCommand>,
```

with:

```rust
    pub agents: HashMap<String, AgentCommand>,
    /// Each agent's profile (ACP core §6), from where it came: the
    /// installed set's `claude` is `Claude` (`ClaudeOwnCli` with
    /// `--use-cli`); an agent not listed, a `--agent` command, is `Generic`.
    pub profiles: HashMap<String, Profile>,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            agents: HashMap::new(),
```

with:

```rust
            agents: HashMap::new(),
            profiles: HashMap::new(),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            .collect()
```

with:

```rust
            .collect()
    }

    /// The profile of `agent` (`Generic` unless listed).
    pub fn profile(&self, agent: &str) -> Profile {
        self.profiles.get(agent).copied().unwrap_or_default()
    }

    /// `hello.mcp_isolation`: every configured agent, isolated or not.
    pub fn mcp_isolation(&self) -> AgentIsolation {
        AgentIsolation(
            self.agents
                .keys()
                .map(|agent| (agent.clone(), self.profile(agent).mcp_isolation()))
                .collect(),
        )
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        Vec::new(),
```

with:

```rust
        Announce::default(),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
async fn handshake(
    collector_url: &str,
    host_id: &str,
    key: &HostKey,
    workspace_roots: Vec<String>,
```

with:

```rust
/// What a `hello` reports of the host's configuration: nothing for a
/// probe.
#[derive(Default)]
struct Announce {
    workspace_roots: Vec<String>,
    mcp_isolation: AgentIsolation,
}

async fn handshake(
    collector_url: &str,
    host_id: &str,
    key: &HostKey,
    announce: Announce,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            capabilities: Capabilities(vec![
                Capability::Park,
                Capability::Images,
                Capability::Projects,
                Capability::ResolvePath,
                Capability::ForgetSession,
            ]),
            // Nothing isolated yet: the host takes no servers until it
            // announces `mcp_servers` (plan 8c, Task 2).
            mcp_isolation: AgentIsolation::default(),
            workspace_roots,
```

with:

```rust
            // `mcp_servers`: it passes a session's servers, isolated as
            // `mcp_isolation` says, and refuses those it cannot isolate
            // (plan 8c).
            capabilities: Capabilities(vec![
                Capability::Park,
                Capability::Images,
                Capability::Projects,
                Capability::ResolvePath,
                Capability::ForgetSession,
                Capability::McpServers,
            ]),
            mcp_isolation: announce.mcp_isolation,
            workspace_roots: announce.workspace_roots,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        cfg.reported_roots(),
```

with:

```rust
        Announce {
            workspace_roots: cfg.reported_roots(),
            mcp_isolation: cfg.mcp_isolation(),
        },
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
                        Err(err) => tracing::warn!(error = %err, "ignoring unknown or invalid frame"),
```

with:

```rust
                        // By the error's kind and place only: its text can
                        // quote a string of the frame, a header value say.
                        Err(err) => tracing::warn!(
                            kind = ?err.classify(),
                            line = err.line(),
                            column = err.column(),
                            "ignoring unknown or invalid frame"
                        ),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    config: SessionConfig,
```

with:

```rust
    config: SessionConfig,
    mcp: McpDelivery,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    }
    // Before anything can be enqueued for this session: continue from the
```

with:

```rust
    }
    // Servers this host cannot keep the agent to, unless the collector
    // waived that, are refused before anything is spawned: never dropped,
    // never passed (plan 8c, the lane's L3).
    let profile = cfg.profile(&req.agent);
    if let Some(problem) =
        crate::profile::mcp_refusal(&req.agent, profile, &req.mcp.mcp_servers, req.mcp.isolation_waived)
    {
        uplink.reply(HostFrame::Error {
            request_id: req.request_id,
            code: "mcp_isolation_unavailable".into(),
            message: problem,
        });
        return Ok(());
    }
    // Before anything can be enqueued for this session: continue from the
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    spawn_or_restart(uplink, sessions, req, command, options);
```

with:

```rust
    spawn_or_restart(uplink, sessions, req, command, profile, options);
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    command: AgentCommand,
```

with:

```rust
    command: AgentCommand,
    profile: Profile,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            spawn_or_restart(&uplink, &sessions, req, command, options);
```

with:

```rust
            spawn_or_restart(&uplink, &sessions, req, command, profile, options);
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        cwd: PathBuf::from(req.cwd),
```

with:

```rust
        cwd: PathBuf::from(req.cwd),
        profile,
        mcp_servers: req.mcp.mcp_servers,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            cwd,
            config,
            hat_id: _,
            mcp: _,
        } => attach(
```

with:

```rust
            cwd,
            config,
            // Carried for plan 8h's composed `CODEX_HOME`; unused here.
            hat_id: _,
            mcp,
        } => attach(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
                attach: Attach::New,
                config,
            },
```

with:

```rust
                attach: Attach::New,
                config,
                mcp,
            },
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            mcp: _,
```

with:

```rust
            mcp,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
                config,
            },
```

with:

```rust
                config,
                mcp,
            },
```

In `crates/hennery-host/src/lib.rs`, replace:

```rust
pub mod paths;
```

with:

```rust
pub mod paths;
pub mod profile;
```

Create `crates/hennery-host/src/profile.rs`:

```rust
//! Adapter profiles (ACP core §6), as far as plan 8c needs them: what a
//! session's MCP servers get from the agent's adapter, decided by where the
//! agent came from, never by its name alone.

use hennery_proto::frames::{McpIsolation, McpServer, NameValue, SessionBody};
use serde_json::{Map, Value};

/// How the host treats an agent's sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// The pinned `claude-agent-acp` from the installed set with its bundled
    /// CLI: strict MCP config on every `session/new` and `session/load`.
    Claude,
    /// The pinned `claude-agent-acp` with the operator's own CLI
    /// (`--use-cli`, `CLAUDE_CODE_EXECUTABLE`): strict MCP config is still
    /// sent, but an override is unverified (ACP core §6), so the host
    /// reports no isolation.
    ClaudeOwnCli,
    /// Anything else: a `--agent` command, and Codex until its composed
    /// `CODEX_HOME` (plan 8h). No `_meta`, no isolation.
    #[default]
    Generic,
}

impl Profile {
    /// The profile of an installed set's adapter `name`; `own_cli` when
    /// `host.toml` gives it the operator's CLI.
    pub fn of_installed(name: &str, own_cli: bool) -> Self {
        match (name, own_cli) {
            ("claude", false) => Self::Claude,
            ("claude", true) => Self::ClaudeOwnCli,
            _ => Self::Generic,
        }
    }

    /// What `hello.mcp_isolation` reports for this agent.
    pub fn mcp_isolation(self) -> McpIsolation {
        match self {
            Self::Claude => McpIsolation::ClaudeStrict,
            Self::ClaudeOwnCli | Self::Generic => McpIsolation::None,
        }
    }

    /// `_meta` for `session/new` and `session/load`. Claude's strict flag
    /// goes on every one, with servers or without (umbrella §8.5, the
    /// spike's conclusion 2): the adapter does not keep it across a load,
    /// and without it the user's own MCP servers and claude.ai connectors
    /// load beside hennery's. The host sends no other `_meta` today; any
    /// later key joins this object.
    pub fn session_meta(self) -> Option<Map<String, Value>> {
        match self {
            Self::Claude | Self::ClaudeOwnCli => {
                let mut meta = Map::new();
                meta.insert(
                    "claudeCode".into(),
                    serde_json::json!({ "options": { "extraArgs": { "strict-mcp-config": "" } } }),
                );
                Some(meta)
            }
            Self::Generic => None,
        }
    }
}

/// Why a start or resume is refused before anything is spawned (plan 8c,
/// the lane's L3): it carries servers for an agent this host cannot keep to
/// them, and the collector did not waive isolation. Never dropped, never
/// passed on.
pub fn mcp_refusal(agent: &str, profile: Profile, servers: &[McpServer], waived: bool) -> Option<String> {
    (!servers.is_empty() && !waived && profile.mcp_isolation() == McpIsolation::None).then(|| {
        format!("this host cannot keep agent {agent}'s sessions to the MCP servers it is given, and isolation was not waived")
    })
}

/// The ACP `mcpServers` entries for `servers`.
pub fn acp_servers(servers: &[McpServer]) -> Vec<agent_client_protocol::schema::v1::McpServer> {
    use agent_client_protocol::schema::v1::{EnvVariable, HttpHeader, McpServer as Acp, McpServerHttp, McpServerStdio};
    servers
        .iter()
        .map(|server| match server {
            McpServer::Http { name, url, headers } => Acp::Http(
                McpServerHttp::new(name, url).headers(
                    headers
                        .iter()
                        .map(|NameValue { name, value }| HttpHeader::new(name, value))
                        .collect(),
                ),
            ),
            McpServer::Stdio {
                name,
                command,
                args,
                env,
            } => Acp::Stdio(
                McpServerStdio::new(name, command).args(args.clone()).env(
                    env.iter()
                        .map(|NameValue { name, value }| EnvVariable::new(name, value))
                        .collect(),
                ),
            ),
        })
        .collect()
}

/// The shortest value redacted from what a session reports (`Secrets`):
/// below it a value is too likely to be an ordinary word (`1`, `true`).
pub const MIN_SECRET_LEN: usize = 8;

/// Where `Secrets` cuts a value into parts, besides whitespace: a URL's
/// delimiters, and its userinfo's (so `user:password`'s password is caught
/// alone). It errs on the safe side: an ordinary word of 8 bytes or more in
/// a stdio server's argument (`Projects` in a path) is redacted too, which
/// costs a little diagnostic text, never a secret.
const SEPARATORS: &str = "/?#&=:@";

/// The secret values of a session's servers (ACP core §8): every part
/// `McpServer::secret_values` names, each of its parts between whitespace
/// or URL delimiters (`SEPARATORS`: so a bare token echoed without its
/// `Bearer `, or one segment of a URL's path, is caught too), and the
/// JSON-escaped form of each (an adapter may echo its config as JSON), at
/// least `MIN_SECRET_LEN` bytes long. Longest first, so a value is redacted
/// before a part of it. `Debug` shows how many, never one.
#[derive(Clone, Default)]
pub struct Secrets(Vec<String>);

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secrets(<{} redacted>)", self.0.len())
    }
}

impl Secrets {
    pub fn of(servers: &[McpServer]) -> Self {
        let mut values: Vec<String> = servers
            .iter()
            .flat_map(McpServer::secret_values)
            .flat_map(|value| {
                std::iter::once(value).chain(value.split(|c: char| c.is_whitespace() || SEPARATORS.contains(c)))
            })
            .filter(|value| value.len() >= MIN_SECRET_LEN)
            .flat_map(|value| {
                let json = serde_json::to_string(value).expect("a string serializes");
                let escaped = json[1..json.len() - 1].to_string();
                [Some(value.to_string()), (escaped != value).then_some(escaped)]
            })
            .flatten()
            .collect();
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Self(values)
    }

    /// `text` with every secret value replaced by `[redacted]`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.0 {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), crate::adapter::REDACTED);
            }
        }
        out
    }

    /// `body` with the secret values redacted from the text hennery itself
    /// writes in it: a start failure's message, a turn's error, a note, a
    /// stderr tail. Exhaustive on purpose: a new body must say whether it
    /// carries such text, or this does not compile. ACP payloads are the
    /// adapter's, verbatim (ACP core §2.3); whether a token an agent prints
    /// into one is redacted is plan 8e's open question.
    pub fn redact_body(&self, body: SessionBody) -> SessionBody {
        match body {
            SessionBody::StartFailed {
                request_id,
                code,
                message,
            } => SessionBody::StartFailed {
                request_id,
                code,
                message: self.redact(&message),
            },
            SessionBody::TurnEnded {
                turn_id,
                outcome,
                stop_reason,
                error,
            } => SessionBody::TurnEnded {
                turn_id,
                outcome,
                stop_reason,
                error: error.map(|error| self.redact(&error)),
            },
            SessionBody::HostNote { note, text } => SessionBody::HostNote {
                note,
                text: self.redact(&text),
            },
            SessionBody::AdapterExited {
                code,
                signal,
                stderr_tail,
            } => SessionBody::AdapterExited {
                code,
                signal,
                stderr_tail: self.redact(&stderr_tail),
            },
            // Ids, enums, extracts and the adapter's own payloads: no text
            // hennery writes.
            body @ (SessionBody::SessionStarted { .. }
            | SessionBody::TurnStarted { .. }
            | SessionBody::AcpUpdate { .. }
            | SessionBody::SessionParked { .. }
            | SessionBody::SessionClosed
            | SessionBody::ConfigApplied { .. }
            | SessionBody::PendingOpened { .. }
            | SessionBody::PendingResolved { .. }
            | SessionBody::AnswerResult { .. }
            | SessionBody::GitState { .. }) => body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::TurnOutcome;

    fn servers() -> Vec<McpServer> {
        vec![
            McpServer::Http {
                name: "hennery-notes".into(),
                url: "https://hennery.example/mcp/notes".into(),
                headers: vec![
                    NameValue::new("Authorization", "Bearer hst_token_0123456789"),
                    NameValue::new("X-Short", "abc"),
                ],
            },
            McpServer::Stdio {
                name: "hennery-files".into(),
                command: "/bin/files".into(),
                args: vec![],
                env: vec![
                    NameValue::new("KEY", "files-key-0123456789"),
                    NameValue::new("ON", "1"),
                    NameValue::new("QUOTED", "a \"quoted\" key"),
                ],
            },
        ]
    }

    #[test]
    fn only_the_pinned_claude_with_its_own_cli_is_isolated() {
        assert_eq!(Profile::of_installed("claude", false), Profile::Claude);
        assert_eq!(Profile::of_installed("claude", true), Profile::ClaudeOwnCli);
        assert_eq!(Profile::of_installed("codex", false), Profile::Generic);
        assert_eq!(Profile::Claude.mcp_isolation(), McpIsolation::ClaudeStrict);
        assert_eq!(Profile::ClaudeOwnCli.mcp_isolation(), McpIsolation::None);
        assert_eq!(Profile::Generic.mcp_isolation(), McpIsolation::None);
        // The strict flag goes to both Claude profiles, exactly as the
        // adapter reads it; nothing to any other agent.
        let strict = serde_json::json!({"claudeCode": {"options": {"extraArgs": {"strict-mcp-config": ""}}}});
        for profile in [Profile::Claude, Profile::ClaudeOwnCli] {
            assert_eq!(Value::Object(profile.session_meta().unwrap()), strict);
        }
        assert_eq!(Profile::Generic.session_meta(), None);
    }

    #[test]
    fn servers_are_refused_only_for_an_agent_not_isolated_unless_waived() {
        let servers = servers();
        assert_eq!(mcp_refusal("claude", Profile::Claude, &servers, false), None);
        assert!(mcp_refusal("fake", Profile::Generic, &servers, false).is_some());
        assert!(mcp_refusal("claude", Profile::ClaudeOwnCli, &servers, false).is_some());
        assert_eq!(mcp_refusal("fake", Profile::Generic, &servers, true), None);
        assert_eq!(mcp_refusal("fake", Profile::Generic, &[], false), None);
    }

    #[test]
    fn servers_become_acp_entries_with_stdio_untagged() {
        let acp = serde_json::to_value(acp_servers(&servers())).unwrap();
        assert_eq!(
            acp,
            serde_json::json!([
                {"type": "http", "name": "hennery-notes", "url": "https://hennery.example/mcp/notes",
                 "headers": [{"name": "Authorization", "value": "Bearer hst_token_0123456789"},
                             {"name": "X-Short", "value": "abc"}]},
                {"name": "hennery-files", "command": "/bin/files", "args": [],
                 "env": [{"name": "KEY", "value": "files-key-0123456789"}, {"name": "ON", "value": "1"},
                         {"name": "QUOTED", "value": "a \"quoted\" key"}]}
            ])
        );
    }

    #[test]
    fn secrets_are_redacted_whole_and_by_part_but_not_short_values() {
        let secrets = Secrets::of(&servers());
        let text = "sent Bearer hst_token_0123456789; echoed hst_token_0123456789 and files-key-0123456789, abc 1";
        assert_eq!(
            secrets.redact(text),
            "sent [redacted]; echoed [redacted] and [redacted], abc 1"
        );
        assert_eq!(Secrets::default().redact(text), text);
        // Echoed as JSON, the escaped form is caught too.
        let echoed = serde_json::to_string("a \"quoted\" key").unwrap();
        assert_eq!(secrets.redact(&format!("config: {echoed}")), "config: \"[redacted]\"");

        // A part of a URL's path, or its userinfo's password, echoed alone.
        let url = Secrets::of(&[McpServer::Http {
            name: "n".into(),
            url: "https://user:pw-secret-0123@h.example/mcp/url-key-0123456789?k=query-key-0123".into(),
            headers: vec![],
        }]);
        assert_eq!(
            url.redact("saw url-key-0123456789, query-key-0123 and pw-secret-0123 at /mcp"),
            "saw [redacted], [redacted] and [redacted] at /mcp"
        );
        // Its `Debug` shows none of them.
        let debug = format!("{secrets:?}");
        assert!(
            debug.starts_with("Secrets(<") && !debug.contains("0123456789"),
            "{debug}"
        );
    }

    #[test]
    fn the_text_hennery_writes_is_redacted_in_every_body_that_has_some() {
        let secrets = Secrets::of(&servers());
        let echo = "echoed hst_token_0123456789".to_string();
        let redacted = |body| serde_json::to_string(&secrets.redact_body(body)).unwrap();
        for body in [
            SessionBody::StartFailed {
                request_id: "r".into(),
                code: "start_failed".into(),
                message: echo.clone(),
            },
            SessionBody::HostNote {
                note: "config_failed".into(),
                text: echo.clone(),
            },
            SessionBody::AdapterExited {
                code: Some(3),
                signal: None,
                stderr_tail: echo.clone(),
            },
            SessionBody::TurnEnded {
                turn_id: "t".into(),
                outcome: TurnOutcome::Failed,
                stop_reason: None,
                error: Some(echo.clone()),
            },
        ] {
            let text = redacted(body);
            assert!(
                text.contains("echoed [redacted]") && !text.contains("hst_token"),
                "{text}"
            );
        }
        // An ACP payload is the adapter's, verbatim (ACP core §2.3).
        let update = SessionBody::AcpUpdate {
            indexed: Default::default(),
            payload: serde_json::json!({ "text": echo }),
        };
        assert!(redacted(update).contains("hst_token_0123456789"));
    }
}
```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

```rust
use crate::identity::{CONFIG_FILE, read_table, write_private};
```

with:

```rust
use crate::identity::{CONFIG_FILE, read_table, write_private};
use crate::profile::Profile;
```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

```rust
    pub agents: HashMap<String, AgentCommand>,
```

with:

```rust
    pub agents: HashMap<String, AgentCommand>,
    /// Each launched agent's profile (`Profile::of_installed`): an agent
    /// with its `--use-cli` CLI is not the pinned one any more.
    pub profiles: HashMap<String, Profile>,
```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

```rust
        match (overrides.get(name), cli_var(name)) {
            (Some(path), Some(var)) => match check_cli(path) {
                Ok(path) => {
```

with:

```rust
        let mut own_cli = false;
        match (overrides.get(name), cli_var(name)) {
            (Some(path), Some(var)) => match check_cli(path) {
                Ok(path) => {
                    own_cli = true;
```

In `crates/hennery-host/src/runtime/agents.rs`, replace:

```rust
        }
        out.agents.insert(name.clone(), command);
```

with:

```rust
        }
        out.profiles.insert(name.clone(), Profile::of_installed(name, own_cli));
        out.agents.insert(name.clone(), command);
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
pub use crate::adapter::{AgentCommand, NESTING_VARS};
```

with:

```rust
pub use crate::adapter::{AgentCommand, NESTING_VARS};
use crate::profile::{Profile, Secrets};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    ConfigValue, ElicitationAction, HostFrame, Indexed, ParkReason, PendingExtract, PendingKind, PendingReason,
    PendingResolution, SessionBody, SessionConfig, TurnOutcome,
```

with:

```rust
    ConfigValue, ElicitationAction, HostFrame, Indexed, McpServer, ParkReason, PendingExtract, PendingKind,
    PendingReason, PendingResolution, SessionBody, SessionConfig, TurnOutcome,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    pub cwd: PathBuf,
```

with:

```rust
    pub cwd: PathBuf,
    /// The agent's profile: Claude's strict MCP flag goes on every
    /// `session/new` and `session/load` (ACP core §6).
    pub profile: Profile,
    /// Passed in `session/new` / `session/load` (plan 8c); their secret
    /// values are redacted from what the session reports.
    pub mcp_servers: Vec<McpServer>,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        attach: Attach::New,
        config: SessionConfig::default(),
        agent,
        cwd,
    };
    self::launch(uplink, launch, options)
}
```

with:

```rust
        attach: Attach::New,
        config: SessionConfig::default(),
        agent,
        cwd,
        profile: Profile::default(),
        mcp_servers: Vec::new(),
    };
    self::launch(uplink, launch, options)
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        cwd,
    };
```

with:

```rust
        cwd,
        profile: Profile::default(),
        mcp_servers: Vec::new(),
    };
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        ending: ending.clone(),
```

with:

```rust
        ending: ending.clone(),
        secrets: Secrets::of(&launch.mcp_servers),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The `host_note` for dropped unknown kinds, if any were dropped.
    fn note(&self) -> Option<SessionBody> {
```

with:

```rust
    /// The `host_note` for dropped unknown kinds, if any were dropped, the
    /// session's `secrets` redacted before `scrub`.
    fn note(&self, secrets: &Secrets) -> Option<SessionBody> {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            text: scrub(&format!(
                "dropped {total} update(s) of unknown kind during session/load: {}",
                kinds.join(", ")
            )),
```

with:

```rust
            text: scrub(&secrets.redact(&format!(
                "dropped {total} update(s) of unknown kind during session/load: {}",
                kinds.join(", ")
            ))),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// Shared with the handle (`SessionHandle::agent_session_id`).
    agent_session_id: Arc<Mutex<Option<String>>>,
}
```

with:

```rust
    /// Shared with the handle (`SessionHandle::agent_session_id`).
    agent_session_id: Arc<Mutex<Option<String>>>,
    /// The secret values of the session's MCP servers (ACP core §8).
    secrets: Secrets,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    fn emit(&self, body: SessionBody) {
        if let Err(err) = self.uplink.emit(&self.session_id, body) {
            tracing::error!(session_id = %self.session_id, error = %err, "failed to persist a session frame");
```

with:

```rust
    /// `err`'s text with the session's secret values redacted: an
    /// adapter's error can quote its MCP config (ACP core §8).
    fn redacted(&self, err: &dyn std::fmt::Display) -> String {
        self.secrets.redact(&err.to_string())
    }

    fn emit(&self, body: SessionBody) {
        // ACP core §8: an adapter can echo its MCP config in an error or on
        // stderr.
        let body = self.secrets.redact_body(body);
        if let Err(err) = self.uplink.emit(&self.session_id, body) {
            tracing::error!(session_id = %self.session_id, error = %self.redacted(&err), "failed to persist a session frame");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        tracing::warn!(session_id = %self.session_id, code = error.code, message = %error.message, "session start failed");
        self.emit(SessionBody::StartFailed {
            request_id,
            code: error.code.into(),
            message: error.message,
```

with:

```rust
        // Redacted before it is logged too, not only when emitted.
        let message = self.secrets.redact(&error.message);
        tracing::warn!(session_id = %self.session_id, code = error.code, %message, "session start failed");
        self.emit(SessionBody::StartFailed {
            request_id,
            code: error.code.into(),
            message,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            ..
        } = launch;
```

with:

```rust
            profile,
            mcp_servers,
            ..
        } = launch;
        let mcp = SessionMcp {
            servers: crate::profile::acp_servers(&mcp_servers),
            meta: profile.session_meta(),
        };
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        };
        let (updates_tx, mut updates) = mpsc::unbounded_channel::<Inbound>();
```

with:

```rust
        };
        adapter.redact_with(self.secrets.clone());
        let (updates_tx, mut updates) = mpsc::unbounded_channel::<Inbound>();
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let session_id = self.session_id.clone();
```

with:

```rust
        let session_id = self.session_id.clone();
        let secrets = self.secrets.clone();
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                tracing::debug!(%session_id, error = %err, "ACP connection ended");
```

with:

```rust
                let error = secrets.redact(&err.to_string());
                tracing::debug!(%session_id, %error, "ACP connection ended");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    match tokio::time::timeout_at(deadline, negotiate(&conn, cwd, &attach, &mut updates, &mut replay)).await {
```

with:

```rust
                    match tokio::time::timeout_at(deadline, negotiate(&conn, cwd, &attach, &mcp, &mut updates, &mut replay)).await {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        if let Some(note) = replay.note() {
```

with:

```rust
        if let Some(note) = replay.note(&self.secrets) {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                text: scrub(&format!("{what}: {}", applied.failures.join("; "))),
```

with:

```rust
                // Redacted before `scrub`, which would cut a secret in part.
                text: scrub(&self.secrets.redact(&format!("{what}: {}", applied.failures.join("; ")))),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                                    tracing::warn!(session_id = %self.session_id, error = %err, "session/cancel not sent");
```

with:

```rust
                                    tracing::warn!(session_id = %self.session_id, error = %self.redacted(&err), "session/cancel not sent");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        self.uplink.reply(HostFrame::Error {
            request_id,
            code: code.into(),
            message,
```

with:

```rust
        // The message can quote the adapter's error (ACP core §8).
        self.uplink.reply(HostFrame::Error {
            request_id,
            code: code.into(),
            message: self.secrets.redact(&message),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                tracing::warn!(session_id = %self.session_id, error = %err, "set_config not sent");
```

with:

```rust
                tracing::warn!(session_id = %self.session_id, error = %self.redacted(&err), "set_config not sent");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            tracing::debug!(session_id = %self.session_id, error = %err, "cancellation not sent to the adapter");
```

with:

```rust
            tracing::debug!(session_id = %self.session_id, error = %self.redacted(&err), "cancellation not sent to the adapter");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            tracing::debug!(session_id = %self.session_id, error = %err, "withdrawal not acknowledged to the adapter");
```

with:

```rust
            tracing::debug!(session_id = %self.session_id, error = %self.redacted(&err), "withdrawal not acknowledged to the adapter");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                tracing::warn!(session_id = %self.session_id, error = %err, "answer not sent to the adapter");
```

with:

```rust
                tracing::warn!(session_id = %self.session_id, error = %self.redacted(&err), "answer not sent to the adapter");
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    attach: &Attach,
```

with:

```rust
    attach: &Attach,
    mcp: &SessionMcp,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                .send_request(NewSessionRequest::new(cwd))
```

with:

```rust
                .send_request(
                    NewSessionRequest::new(cwd)
                        .mcp_servers(mcp.servers.clone())
                        .meta(mcp.meta.clone()),
                )
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    let load = conn.send_request(LoadSessionRequest::new(id.clone(), cwd)).block_task();
```

with:

```rust
    // The same servers and `_meta` as on `session/new`: the adapter keeps
    // neither across a load (the spike).
    let load = conn
        .send_request(
            LoadSessionRequest::new(id.clone(), cwd)
                .mcp_servers(mcp.servers.clone())
                .meta(mcp.meta.clone()),
        )
        .block_task();
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            }
        }
    }
}

/// The config options a new or loaded session starts with, and whether they
```

with:

```rust
            }
        }
    }
}

/// What `session/new` and `session/load` carry besides the cwd (plan 8c):
/// the session's MCP servers as ACP entries, and the profile's `_meta`.
/// Never `Debug`: the entries hold the session's secrets.
struct SessionMcp {
    servers: Vec<agent_client_protocol::schema::v1::McpServer>,
    meta: Option<serde_json::Map<String, Value>>,
}

/// The config options a new or loaded session starts with, and whether they
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let (agents, _set_in_use) = if args.agents.is_empty() {
        runtime::default_agents(&args.data_dir, &args.mirrors).await
    } else {
        (args.agents.into_iter().collect(), None)
    };
    let collector_url = args.collector_url.unwrap_or(paired.collector_url);
    let mut cfg = HostConfig::new(collector_url, paired.host_id, paired.key, args.data_dir);
    cfg.workspace_roots = workspace_roots;
    cfg.home = home;
    cfg.agents = agents;
```

with:

```rust
    // A `--agent` command is a generic agent (ACP core §6): no profile.
    let (agents, profiles, _set_in_use) = if args.agents.is_empty() {
        runtime::default_agents(&args.data_dir, &args.mirrors).await
    } else {
        (args.agents.into_iter().collect(), Default::default(), None)
    };
    let collector_url = args.collector_url.unwrap_or(paired.collector_url);
    let mut cfg = HostConfig::new(collector_url, paired.host_id, paired.key, args.data_dir);
    cfg.workspace_roots = workspace_roots;
    cfg.home = home;
    cfg.agents = agents;
    cfg.profiles = profiles;
```

In `crates/hennery/src/runtime.rs`, replace:

```rust
/// installing the pinned one if it is not current. The file returned holds
/// that set in use for as long as it is kept.
pub async fn default_agents(
    data_dir: &Path,
    mirrors: &MirrorArgs,
) -> (HashMap<String, AgentCommand>, Option<std::fs::File>) {
```

with:

```rust
/// installing the pinned one if it is not current, with their profiles.
/// The file returned holds that set in use for as long as it is kept.
pub async fn default_agents(
    data_dir: &Path,
    mirrors: &MirrorArgs,
) -> (
    HashMap<String, AgentCommand>,
    HashMap<String, hennery_host::profile::Profile>,
    Option<std::fs::File>,
) {
```

In `crates/hennery/src/runtime.rs`, replace:

```rust
    (prepared.agents.agents, prepared.in_use)
```

with:

```rust
    (prepared.agents.agents, prepared.agents.profiles, prepared.in_use)
```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test --locked -p hennery-testkit --test session_mcp --test host_connection && cargo test --locked -p hennery-host --lib profile && cargo test --locked -p hennery-host --test runtime`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `negotiate`, drop `.meta(mcp.meta.clone())` from `session/new`, then from `session/load`; then `.mcp_servers(mcp.servers.clone())` from each. `claudes_servers_and_strict_flag_go_on_new_and_on_every_load` fails on each of the four.
- In `connection.rs`'s `attach`, make the `mcp_refusal` check never refuse. `servers_an_agent_cannot_be_kept_to_are_refused_before_any_spawn` fails.
- Drop `Capability::McpServers` from the `hello`. `hello_announces_mcp_servers_and_how_each_agent_is_isolated` fails.
- In `Actor::emit`, drop `self.secrets.redact_body(body)`. `an_adapters_errors_quoting_its_secrets_are_redacted_in_every_answer_and_fact` fails (the turn's error).
- In `Secrets::redact_body`, leave the `TurnEnded` error as it is; in the start's config note, scrub without redacting first; in `reject`, send the message as it is. `an_adapters_errors_quoting_its_secrets_are_redacted_in_every_answer_and_fact` fails on each.
- In `Replay::note`, scrub without redacting first. `a_replay_note_quoting_a_secret_is_redacted_before_scrub` fails.
- In `Secrets::of`, cut values at whitespace only; then leave `:@` out of `SEPARATORS`. `secrets_are_redacted_whole_and_by_part_but_not_short_values` fails on each.
- Every outcome on its own (P50, P51, P64–P78): in `spawn_or_restart`, pass no servers, then the default profile; `a_delivered_start_reaches_the_adapter_with_its_servers_and_profile` fails on each. In `Profile::of_installed`, `mcp_isolation`, `session_meta` and `mcp_refusal`, change each arm in turn (claude, claude with its own CLI, any other; strict, none; no servers, waived, isolated); the profile tests, `claude_is_strict_without_servers_too_and_its_own_cli_as_well` or `a_generic_agent_gets_its_servers_and_no_meta` fail on each. Keep short values in `Secrets`; send the host's refusal with another code; give an unlisted agent another profile; report each agent's isolation from one fixed profile; their tests fail on each.
- In `Actor::start`, drop `adapter.redact_with(self.secrets.clone())`; then, in `Adapter::stderr_tail`, `scrub` before redacting. `the_sessions_secret_values_never_reach_what_it_reports` fails on each.
- In `McpServer::secret_values`, drop `.chain(args.iter().map(String::as_str))`, then `.chain(url_secrets(url))`. `the_sessions_secret_values_never_reach_what_it_reports` fails on each (the stdio key; the URL's key, L11), and `a_start_failure_quoting_the_servers_is_redacted_in_the_fact_and_the_log` on the second.
- In `Secrets::of`, leave out the JSON-escaped form; then give `Secrets` a `Debug` that shows its values. `secrets_are_redacted_whole_and_by_part_but_not_short_values` fails on each.
- In `Actor::start_failed`, log `error.message` unredacted. `a_start_failure_quoting_the_servers_is_redacted_in_the_fact_and_the_log` fails on the log.
- In `Secrets::redact_body`, leave the `StartFailed`, then the `HostNote`, then the `AdapterExited` text as it is. `the_text_hennery_writes_is_redacted_in_every_body_that_has_some` fails on each.
- In `from_set`, drop `own_cli = true`. `a_set_without_its_cli_launches_that_agent_only_with_an_override` fails on `ClaudeOwnCli`.
- In `run_host`, drop `cfg.profiles = profiles`: the compiler refuses it (`profiles` unused, `-D warnings`).

- [ ] **Step 6: The full checks**

Expected: all pass; **1408 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(host): pass a session's MCP servers with Claude's strict flag, refuse what cannot be isolated"
```

### Task 3: The process's log never shows a token

**Files:**
- Create: `crates/hennery-host/src/logging.rs`
- Modify: `crates/hennery-host/src/lib.rs`, `crates/hennery-host/Cargo.toml`, `Cargo.lock`, `crates/hennery/src/log.rs`
- Test: `crates/hennery-testkit/tests/session_mcp_log.rs` (new, a binary of its own: it installs the `log` bridge); `log.rs`'s `every_subscriber_is_installed_capped`

**Interfaces:**
- Produces: `hennery_host::logging::{MESSAGE_TRACING_TARGETS, secret_cap, capped}`; `hennery`'s `log::install`, through which `log::init` installs every subscriber.
- Consumes: plan 7c-iii's `log::init`.

- [ ] **Step 1: Write the failing test**

Create `crates/hennery-testkit/tests/session_mcp_log.rs`:

```rust
//! Plan 8c, ACP core §8: a session's gateway token never reaches the log,
//! even at `RUST_LOG=trace`, nor its server's URL past the origin (the
//! gateway lane's rule L11). A real host gets a `start_session` with the
//! token in an MCP header from a fake collector, and passes it to the fake
//! adapter's `session/new`: tungstenite traces the frame and the ACP crate
//! the JSON-RPC line, both with the token. The same for a `session/load`,
//! an adapter error that quotes the servers, and a frame that does not
//! decode, which the host logs by its error's kind and place only. The fake
//! adapter also prints a stray (non-JSON-RPC) stdout line quoting the token
//! and the URL key, which the ACP crate cannot parse and so quotes whole in
//! a `warn` event of its own (`agent-client-protocol` 2.2.0) — the reason
//! that target is held at `error`, not `info`.
//! Run twice: once under a plain
//! subscriber, which proves they do, and once under the one the process
//! installs (`logging::capped`), which must not show it. Each run's
//! subscriber is the thread's own (the runtime is current-thread, so the
//! host's tasks run here); the `log` bridge tungstenite needs is installed
//! once, globally, so this test has a binary of its own.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_host::{HostConfig, run};
use hennery_proto::frames::{CollectorFrame, HostFrame, McpDelivery, McpServer, NameValue};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tracing_subscriber::util::SubscriberInitExt;

const TOKEN: &str = "hst_logged_token_0123456789";
/// A key in the server's URL path: never logged (the gateway lane's L11).
const URL_KEY: &str = "url-logged-0123456789";

/// A `tracing` writer into a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
}

fn servers() -> McpDelivery {
    McpDelivery {
        mcp_servers: vec![McpServer::Http {
            name: "hennery-notes".into(),
            url: format!("https://hennery.example/mcp/notes/{URL_KEY}"),
            headers: vec![NameValue::new("Authorization", format!("Bearer {TOKEN}"))],
        }],
        isolation_waived: false,
    }
}

/// A host with the fake adapter as `claude` (and as `echo`, whose
/// `session/new` error quotes its servers), driven by a fake collector: a
/// frame that does not decode, with the token in it; a start and a resume
/// with the token in an MCP header; a start that fails quoting it. Returns
/// once each has been answered.
async fn run_sessions_with_the_token() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        HostKey::from_seed([1; 32]),
        data.path().to_path_buf(),
    );
    let mut claude = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    // A stray (non-JSON-RPC) line on its stdout, quoting the token and the
    // URL key, right before it answers `session/new`: the ACP crate cannot
    // parse it and quotes it whole in a `warn` event of its own.
    let claude_script = hennery_testkit::FakeScript {
        stdout_lines: vec![format!("debug: servers [{TOKEN}] {URL_KEY}")],
        ..Default::default()
    };
    claude.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&claude_script).unwrap(),
    ));
    cfg.agents.insert("claude".into(), claude);
    let mut echo = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        new_session_error: Some(-32603),
        new_session_error_echoes: true,
        ..Default::default()
    };
    echo.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    cfg.agents.insert("echo".into(), echo);
    for agent in ["claude", "echo"] {
        cfg.profiles
            .insert(agent.into(), hennery_host::profile::Profile::Claude);
    }
    let host = tokio::spawn(run(cfg));

    let (tcp, _) = listener.accept().await.unwrap();
    let ws = tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let next = async |stream: &mut futures::stream::SplitStream<_>| -> HostFrame {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => return serde_json::from_str(&text).unwrap(),
                Ok(Some(Ok(_))) => {}
                other => panic!("expected a host frame: {other:?}"),
            }
        }
    };
    assert!(matches!(next(&mut stream).await, HostFrame::Hello { .. }));
    let send = |frame: &CollectorFrame| Message::text(serde_json::to_string(frame).unwrap());
    sink.send(send(&CollectorFrame::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        collector_version: "test".into(),
        committed: Default::default(),
    }))
    .await
    .unwrap();
    // Headers as a string: serde's error quotes it.
    let malformed = serde_json::json!({
        "type": "start_session", "request_id": "r0", "session_id": "s0", "committed_seq": 0,
        "agent": "claude", "cwd": "/",
        "mcp_servers": [{"type": "http", "name": "n", "url": "https://h.example", "headers": format!("Bearer {TOKEN}")}]
    });
    sink.send(Message::text(malformed.to_string())).await.unwrap();
    let cwd = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let start = |request_id: &str, session_id: &str, agent: &str| CollectorFrame::StartSession {
        request_id: request_id.into(),
        session_id: session_id.into(),
        committed_seq: 0,
        agent: agent.into(),
        cwd: cwd.clone(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: servers(),
    };
    let resume = CollectorFrame::ResumeSession {
        request_id: "r3".into(),
        session_id: "s2".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: cwd.clone(),
        agent_session_id: "fake-session-1".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: servers(),
    };
    for (frame, session, kind) in [
        (start("r1", "s1", "claude"), "s1", "session_started"),
        (resume, "s2", "session_started"),
        (start("r4", "s3", "echo"), "s3", "start_failed"),
    ] {
        sink.send(send(&frame)).await.unwrap();
        loop {
            if let HostFrame::Session { session_id, body, .. } = next(&mut stream).await
                && session_id == session
                && serde_json::to_value(&body).unwrap()["kind"] == kind
            {
                break;
            }
        }
    }
    host.abort();
}

fn plain(writer: Captured, filter: &str) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish()
}

#[tokio::test]
async fn a_sessions_token_is_never_logged_even_at_trace() {
    // The `log` bridge, for tungstenite; each run below sets its own
    // subscriber over this empty global one.
    tracing_subscriber::registry().try_init().unwrap();
    let everything = "trace,agent_client_protocol=trace,tungstenite=trace";

    // Both libraries really trace the token...
    let control = Captured::default();
    {
        let _guard = tracing::subscriber::set_default(plain(control.clone(), everything));
        run_sessions_with_the_token().await;
    }
    let control = control.text();
    for target in ["tungstenite", "agent_client_protocol"] {
        for secret in [TOKEN, URL_KEY] {
            assert!(
                control
                    .lines()
                    .any(|line| line.contains(target) && line.contains(secret)),
                "no {target} line with {secret} in the plain log"
            );
        }
    }
    // The ACP crate's own `warn`, quoting the adapter's stray stdout whole:
    // the probe for capping that target at `error`.
    assert!(
        control.lines().any(|line| {
            line.contains("agent_client_protocol") && line.contains("Invalid transport input") && line.contains(TOKEN)
        }),
        "no agent_client_protocol warn line quoting the stray stdout's token in the plain log: {control}"
    );

    // ...and the process's own subscriber, which logs, never shows it.
    let capped = Captured::default();
    {
        let subscriber = hennery_host::logging::capped(plain(capped.clone(), everything));
        let _guard = tracing::subscriber::set_default(subscriber);
        run_sessions_with_the_token().await;
    }
    let capped = capped.text();
    assert!(capped.contains("connected to collector"), "{capped}");
    assert!(capped.contains("ignoring unknown or invalid frame"), "{capped}");
    assert!(capped.contains("session start failed"), "{capped}");
    assert!(!capped.contains(TOKEN), "{capped}");
    assert!(!capped.contains(URL_KEY), "{capped}");
}
```


- [ ] **Step 2: Run it to see it fail, and commit it**

Run: `cargo test --locked -p hennery-testkit --test session_mcp_log`
Expected: FAIL to compile: `cannot find logging in hennery_host`.

```bash
git add crates
git commit -m "test(host): a session's token never reaches the log"
```

- [ ] **Step 3: The cap, and every subscriber installed through it**

In `Cargo.lock`, replace:

```toml
 "toml",
 "tracing",
 "url",
```

with:

```toml
 "toml",
 "tracing",
 "tracing-subscriber",
 "url",
```

In `crates/hennery-host/Cargo.toml`, replace:

```toml
tracing.workspace = true
```

with:

```toml
tracing.workspace = true
# The process's log output, with the message-tracing targets capped (plan 8c).
tracing-subscriber.workspace = true
```

In `crates/hennery-host/src/lib.rs`, replace:

```rust
pub mod identity;
```

with:

```rust
pub mod identity;
pub mod logging;
```

Create `crates/hennery-host/src/logging.rs`:

```rust
//! What the process's log never shows (plan 8c, ACP core §8). Some
//! libraries trace whole messages: at `trace` the ACP crate logs every
//! JSON-RPC line it sends (`session/new`, with a session's gateway token in
//! its MCP headers) and at `debug` the adapter's answers (an error that
//! quotes its config), and tungstenite logs every WebSocket message (the
//! `start_session` frame that carries the token, on both ends). The ACP
//! crate also quotes a misbehaving adapter's raw stdout whole in its own
//! `warn` (`agent-client-protocol` 2.2.0: a line it cannot parse as
//! JSON-RPC becomes a parse-error `Error` whose `data` is that line, logged
//! at `warn`), so it cannot stop at `info` like the others — it is held at
//! `error`. `tungstenite` and `tokio_tungstenite` only ever trace whole
//! messages at `debug` and `trace`, so `info` is enough for them. Each
//! target is held to its own level, whatever `RUST_LOG` says.

use tracing::Subscriber;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::{Layered, SubscriberExt};

/// The targets whose events can carry a session's secrets, and the level
/// each is held to at most: `agent_client_protocol` quotes a misbehaving
/// adapter's stray stdout in its own `warn`, so it is held at `error`;
/// `tungstenite` and `tokio_tungstenite` only trace whole messages at
/// `debug`/`trace`, so `info` is enough.
pub const MESSAGE_TRACING_TARGETS: &[(&str, LevelFilter)] = &[
    ("agent_client_protocol", LevelFilter::ERROR),
    ("tungstenite", LevelFilter::INFO),
    ("tokio_tungstenite", LevelFilter::INFO),
];

/// Every target at any level, but each of `MESSAGE_TRACING_TARGETS` at its
/// own level at most.
pub fn secret_cap() -> Targets {
    MESSAGE_TRACING_TARGETS.iter().fold(
        Targets::new().with_default(LevelFilter::TRACE),
        |targets, (target, level)| targets.with_target(*target, *level),
    )
}

/// `subscriber` with `secret_cap` over it, as a global filter: an event it
/// refuses reaches no layer, so no `RUST_LOG` (`agent_client_protocol=trace`
/// included) lifts it. Every subscriber the process installs is wrapped in
/// this (`hennery`'s `log::init`).
pub fn capped<S>(subscriber: S) -> Layered<Targets, S>
where
    S: Subscriber,
{
    subscriber.with(secret_cap())
}
```

In `crates/hennery/src/log.rs`, replace:

```rust
/// filters, `info` by default.
```

with:

```rust
/// filters, `info` by default; the targets that trace whole messages stay
/// capped at their own level whatever it says, since they would show a
/// session's gateway token (`hennery_host::logging::capped`, plan 8c).
```

In `crates/hennery/src/log.rs`, replace:

```rust
            tracing_subscriber::fmt().with_env_filter(filter("info")).init();
```

with:

```rust
            install(tracing_subscriber::fmt().with_env_filter(filter("info")).finish());
```

In `crates/hennery/src/log.rs`, replace:

```rust
                    tracing_subscriber::fmt()
                        .with_env_filter(filter("info"))
                        .with_ansi(false)
                        // A failed write is said once by the writer, not
                        // once per event on standard error.
                        .log_internal_errors(false)
                        .with_writer(log)
                        .init();
```

with:

```rust
                    install(
                        tracing_subscriber::fmt()
                            .with_env_filter(filter("info"))
                            .with_ansi(false)
                            // A failed write is said once by the writer, not
                            // once per event on standard error.
                            .log_internal_errors(false)
                            .with_writer(log)
                            .finish(),
                    );
```

In `crates/hennery/src/log.rs`, replace:

```rust
    tracing_subscriber::fmt()
        .with_env_filter(filter("warn"))
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();
    Ok(())
```

with:

```rust
    install(
        tracing_subscriber::fmt()
            .with_env_filter(filter("warn"))
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
            .with_writer(std::io::stderr)
            .finish(),
    );
    Ok(())
}

/// Install `subscriber` as the process's, `log` records bridged in, with
/// the targets that trace whole messages capped (plan 8c): the one place
/// this file installs one (`every_subscriber_is_installed_capped`).
fn install<S>(subscriber: S)
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    use tracing_subscriber::util::SubscriberInitExt;
    hennery_host::logging::capped(subscriber).init();
```

In `crates/hennery/src/log.rs`, replace:

```rust
mod tests {
```

with:

```rust
mod tests {
    /// Plan 8c: every subscriber is installed through `install`, which
    /// caps the targets that trace whole messages. Read from the source:
    /// an installed subscriber cannot be inspected.
    #[test]
    fn every_subscriber_is_installed_capped() {
        let source = include_str!("log.rs");
        let init = concat!(".", "init()");
        assert_eq!(
            source.matches(init).count(),
            1,
            "a subscriber installed outside `install`"
        );
        assert!(source.contains(concat!("hennery_host::logging::capped(subscriber)", ".", "init();")));
    }

```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test --locked -p hennery-testkit --test session_mcp_log && cargo test --locked -p hennery --bin hennery every_subscriber`
Expected: PASS. The test's plain run shows the token on a `tungstenite` line, on an `agent_client_protocol` line and on the ACP crate's `Invalid transport input` warning (the fake's `stdout_lines`), so the probe is real.

- [ ] **Step 5: Revert-probes**

- In `capped`, put a `Targets` that lets everything through in place of `secret_cap()`. `a_sessions_token_is_never_logged_even_at_trace` fails.
- In `connect_once`, log an undecodable frame with `error = %err` again. `a_sessions_token_is_never_logged_even_at_trace` fails.
- In `MESSAGE_TRACING_TARGETS`, hold `agent_client_protocol` at `INFO`. `a_sessions_token_is_never_logged_even_at_trace` fails (the ACP crate's warning quotes the stray stdout line).
- Hold `tungstenite` at `TRACE`: the same test fails. `tokio_tungstenite` at `TRACE` is caught by nothing: `tokio-tungstenite` 0.29 traces no message at that target, so its cap is defence in depth, unproven.
- In `log::install`, install the subscriber uncapped; then, at one of `log::init`'s three sites, install one with `.init()` directly. `every_subscriber_is_installed_capped` fails on each.

- [ ] **Step 6: The full checks**

Expected: all pass; **1410 tests**.

- [ ] **Step 7: Commit**

```bash
git add Cargo.lock crates
git commit -m "feat(host): cap the targets that trace whole messages"
```

### Task 4: Writing the spec back

**Files:**
- Modify: `docs/specs/2026-09-26-acp-core-design.md` (§3.3, §4.3, §6, §8), `docs/specs/2026-09-26-mcp-gateway-design.md` (§3.2)

- [ ] **Step 1: The write-back**

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

```markdown
| `start_session` | session_id (collector-minted), committed_seq, agent, cwd (canonical), model?, mode?, axes{}, first_prompt?{turn_id, content[]}, mcp_servers[], hat | `session_started` \| `start_failed` \| `error{unknown_agent}` |
| `resume_session` | session_id, committed_seq, agent, cwd, agent_session_id, model?, mode?, axes{}, mcp_servers[], hat | `session_started` \| `start_failed` \| `error` (as a start) |
```

with:

```markdown
| `start_session` | session_id (collector-minted), committed_seq, agent, cwd (canonical), model?, mode?, axes{}, first_prompt?{turn_id, content[]}, mcp_servers[], isolation_waived?, hat_id | `session_started` \| `start_failed` \| `error{unknown_agent \| mcp_isolation_unavailable}` |
| `resume_session` | session_id, committed_seq, agent, cwd, agent_session_id, model?, mode?, axes{}, mcp_servers[], isolation_waived?, hat_id | `session_started` \| `start_failed` \| `error` (as a start) |
```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

```markdown
- Answers to a session with no live actor are refused `not_attached`. The
  collector logs that refusal; it is no verdict (§4.6).
- A command queued behind an ending actor is answered `not_attached` at once,
  never left to the collector's timeout (§2.2).

**Not on the wire yet** (they arrive with their subsystems): `first_prompt`,
`mcp_servers[]` and `hat` on start/resume (gateway, hats); `hello_ack.server_time`;
```

with:

```markdown
- `start_session`, `resume_session`: `mcp_isolation_unavailable` — the frame
  carries `mcp_servers` for an agent the host cannot keep to them (its
  `hello.mcp_isolation` is `none`), and `isolation_waived` is not set;
  nothing is spawned. Never dropped, never passed (plan 8c).
- `mcp_servers[]` are ACP `mcpServers` entries tagged by `type`: `http`
  `{name, url, headers[{name, value}]}` and `stdio` `{name, command, args[],
  env[{name, value}]}`. Absent or empty when there are none, as are
  `isolation_waived` (the collector knowingly delivers to an agent the host
  cannot isolate: the fallback's default hat, or a single-hat host, umbrella
  §8.5) and `hat_id` (`sessions.hat_id`, carried for the composed
  `CODEX_HOME`). The collector sends servers only to a host whose live
  connection announced `mcp_servers`, and only for an agent it isolates
  unless it waives that, checked by the hub on the connection the frame goes
  out on (plan 8c). A resume re-sends the servers: an agent keeps none
  across `session/load` (the spike).
- Answers to a session with no live actor are refused `not_attached`. The
  collector logs that refusal; it is no verdict (§4.6).
- A command queued behind an ending actor is answered `not_attached` at once,
  never left to the collector's timeout (§2.2).

**Not on the wire yet** (they arrive with their subsystems): `first_prompt`;
`hello_ack.server_time`;
```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

```markdown
  spec §8). The hennery host announces `park` and `images`: `images` says the
  host carries image blocks, and each session still refuses them when its
  agent takes none (`images_unsupported`, above).
```

with:

```markdown
  spec §8). The hennery host announces `park`, `images`, `projects`,
  `resolve_path` and `mcp_servers`: `images` says the host carries image
  blocks, and each session still refuses them when its agent takes none
  (`images_unsupported`, above). `mcp_servers` (plan 8c):
  the host passes a start's or resume's servers into `session/new` /
  `session/load` with each agent's isolation, and refuses those it cannot
  isolate unless waived (`mcp_isolation_unavailable`). `mcp_servers[]`,
  `isolation_waived` and `hat_id` have been on the wire since plan 8c; the
  collector sends no servers until the gateway mints them (plan 8e).
- `mcp_isolation` (plan 8c): per agent id, how the host keeps its sessions to
  the servers it is given: `claude_strict` (the strict flag, §6) or `none`.
  An agent left out is `none`. Deserialized leniently like `capabilities`:
  an unknown value reads as `none`, never as isolated. The collector keeps
  it with the live connection (the hub), not in the host registry.
```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

```markdown
**Built so far:** hat resolution, `mcp_servers`, `_meta` and the first prompt
are not built yet; start and resume carry agent, cwd and the config.
```

with:

```markdown
**Built so far:** the first prompt is not built yet; start and resume carry
agent, cwd, the config, the hat and (plan 8c) the MCP servers with Claude's
`_meta`. The collector sends no servers until plan 8e.
```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

```markdown
**Built so far:** no profiles; an agent is a command line from the host's
config, and every agent gets the same `initialize` (fs and terminal not
advertised, boolean config options and form elicitation advertised, §2.5).
```

with:

```markdown
**Built so far:** every agent gets the same `initialize` (fs and terminal not
advertised, boolean config options and form elicitation advertised, §2.5).
Plan 8c builds the profiles' MCP column only, chosen by where the agent came
from, never by its name alone:
- the installed set's `claude` (the pinned `claude-agent-acp`): the strict
  flag in `_meta` on every `session/new` and `session/load`, with servers or
  without; `hello.mcp_isolation` reports `claude_strict`. **Every Claude
  session hennery runs therefore loses the user's own MCP servers and
  claude.ai connectors**: the gateway is the one place MCP is managed
  (umbrella §8.5; the spike's conclusion 2);
- the same with the operator's CLI (`--use-cli`, `CLAUDE_CODE_EXECUTABLE`):
  the strict flag is still sent, but the override is unverified, so it
  reports `none`;
- everything else (Codex until its composed `CODEX_HOME`, plan 8h; any
  `--agent` command): no `_meta`, `none`.

Servers for an agent reported `none` are refused (`mcp_isolation_unavailable`)
unless the collector waived isolation. The operator's opt-in to unverified
isolation for an override is not built.
```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

```markdown
  to the events table and never sent over SSE.
```

with:

```markdown
  to the events table and never sent over SSE. Nor are they logged (plan
  8c): their `Debug` shows names, and an HTTP server's URL only as
  `scheme://host[:port]`, never a header's or env variable's value, the
  URL's path, query or userinfo, nor a stdio server's arguments; the
  process's log output holds tungstenite, which traces whole messages, at
  `info` and the ACP crate, which also quotes an adapter's stray stdout in
  its warnings, at `error`, whatever `RUST_LOG` says; a frame the host
  cannot decode is logged by its error's kind and place, never its text;
  and the host redacts a session's secret values (each of those parts, its
  parts between whitespace or URL delimiters, and its JSON-escaped form)
  from the text it writes itself, before `scrub`: `start_failed`,
  `turn_ended.error`, `host_note`, the stderr tail of `adapter_exited`, its
  `error` answers and its own log lines. Whether ACP payloads are redacted
  too (a token an agent prints into a tool's output, against §2.3's
  verbatim payloads) is open, for the maintainer before plan 8e.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
   or applies the fallback (umbrella §8.5).
```

with:

```markdown
   or applies the fallback (umbrella §8.5). The collector decides the
   fallback; a frame that delivers to an agent the host cannot isolate says
   so (`isolation_waived`), and without it the host refuses the servers
   (`mcp_isolation_unavailable`, plan 8c).
```


- [ ] **Step 2: Commit**

```bash
git add docs/specs
git commit -m "docs(spec): write back plan 8c's per-session MCP servers on the host"
```

## After this plan

**Release blocker:** do not cut a release with 8c but without 8e: hennery Claude sessions would have no MCP servers at all (decision 4, Q1).

**What plan 8e inherits:**
- **The delivery decision** (the lane's L2): deliver, waive or none, from mixedness and `Hub::mcp_isolation(host, agent)`, read on the live connection. Set `isolation_waived` only for the fallback's default-hat delivery and a single-hat host, never otherwise.
- **An agent's token in an ACP payload** (decision 8, left open): keep payloads verbatim (ACP core §2.3) or redact the session's known secret values from them too (§8). The maintainer's call, before 8e mints the first token.
- **The hub's refusal on a start or resume:** `request_with_undo` answers `RequestError::McpUndeliverable` without sending, and the session is left `starting`. `api.rs`'s start and resume must then fail it (`mark_failed` / `mark_failed_if_starting`, code `mcp_isolation_unavailable`); `request_failed` already answers it 409 with that code. Unreachable in 8c, which sends no servers.
- **A start or resume that reaches a live actor** (a duplicate, a retry) goes to `Restart`, and the actor keeps the servers it started with. If 8e mints a fresh token on every resume (ACP core §4.3: "Every resume mints a fresh gateway token, superseding the previous one"), that adapter holds a superseded one. 8e decides: reuse the token on a re-delivery, or restart the actor.
- **The operator's opt-in to unverified isolation** for an override (ACP core §6), in `host.toml`, reported in `mcp_isolation` (decision 7).
- **Server names** `hennery-<slug>` (ACP core §6) are the gateway's to set. The host passes names as given.
- **The host registry** may record `mcp_isolation` for display (decision 2), with a kernel migration.
- **One status for `mcp_isolation_unavailable`:** the hub's refusal answers 409; a host's refusal reaches the collector as `Rejected` and answers 502. Settle one before 8e makes either reachable.
- **Test that `isolation_waived` is never set** for a session in a hat other than the host's default on a mixed host (the security review's answer D).
- **An agent's HTTP MCP support:** the host passes servers without reading the agent's `initialize` answer (`mcpCapabilities.http`). The pinned Claude takes them; before a waived delivery to another agent (8e) or Codex (8h), refuse or record what happens to an agent that did not announce it.

**What plan 8h inherits:**
- `hat_id` arrives on every start and resume (empty for a session from before hats).
- A Codex profile: `Profile::of_installed("codex", false)` gives `Generic` today. 8h adds the composed `CODEX_HOME`, a new `McpIsolation` value (an older collector reads it as `none`), and that value in `hello.mcp_isolation`.

**Operator items** (live gates, the lane's L10: never run by a lane):
- On every adapter pin bump (ACP core §12), and for every set a host can run (a held rollback set too, which still reports `claude_strict`): while a Claude session runs with a gateway server, search the process list for its token, and record the result per pin; and with the strict flag, a global MCP probe server receives nothing (the spike's harness, automated).
- Plugin-provided MCP servers under strict mode are unmeasured (ACP core §6).
- **Release note** (for the release that carries 8c and 8e): Claude sessions started through hennery no longer see the user's own MCP servers or claude.ai connectors; MCP servers are managed in hennery's gateway (decision 4).

**Spec amendments** (Task 4 writes them back):
- decisions 1, 2, 5: ACP core §3.3: the frames' fields, `mcp_servers` and `mcp_isolation` in `hello`, `mcp_isolation_unavailable`; gateway spec §3.2 step 2: `isolation_waived`.
- decisions 3, 4, 7: ACP core §6's "Built so far".
- decision 8: ACP core §8.
- The lane's L3 is amended by decision 5; ACP core §1's `SessionMcp::servers_for` gains the decision as an input in 8e (the lane's L2).

Then, in order: **8d** (the proxy and session tokens), then **8e**.

---

_Generated with Claude AI — please review before distribution._
