# External session MCP: scoped read foundation

This first implementation exposes external **discovery and session metadata**.
It does not execute agents. Create/prompt, durable operation recovery, transcript
reads/waits, targeted cancellation and lifecycle/ordinary-answer tools remain
subsequent slices. Existing operator REST, delegated agent tokens and plug-in
activity credentials do not authenticate this endpoint.

## Desktop connection decision (2026-10-29)

Claude Desktop is the first real acceptance client. A CLI or protocol fixture
is not Desktop acceptance. The initial dependable path is a local stdio bridge
using the same collector HTTP endpoint as other clients.

Official documentation says remote connectors originate from Anthropic's cloud
and require a publicly reachable server. Request-header authentication is beta
and limited to some organizations; OAuth is the other authenticated remote
path. We therefore do not assume a particular account has custom headers.
Incoming OAuth is not implemented here. Where header auth is available, direct
remote connection may use the same scoped bearer credential and endpoint.

Sources checked on 2026-10-29:
- [Remote connector network requirements](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp).
- [Authentication choices and request-header beta](https://claude.com/docs/connectors/custom/add-unlisted).
- [Official Desktop local-server setup](https://modelcontextprotocol.io/docs/develop/connect-local-servers).

No real Desktop account, configuration or production service was changed during
implementation. Follow the manual acceptance below in a deliberately provisioned
scratch setup before claiming Desktop support verified.

## Identity and provisioning

The operator issues a separate external client through:

- `GET /api/external-clients`: list grants, never credentials or credential hashes.
- `POST /api/external-clients`: operator session and fresh step-up required.
- `DELETE /api/external-clients/{id}`: operator session and fresh step-up required;
  revocation is permanent for this credential and does not reopen old work.

For example, after signing in and completing an existing password/passkey check,
the operator can submit this JSON through the authenticated same-origin API:

```json
{
  "label": "Desktop scratch",
  "hat_id": "HAT_ID",
  "roots": [{ "host_id": "HOST_ID", "root": "/workspace/demo" }],
  "read_metadata": true,
  "existing_sessions": false,
  "expires_in_days": 7
}
```

The response contains `client` and a token shown once. Save the token to a
same-user regular file with mode `0600`; do not put it into URLs, command-line
arguments, shared Desktop settings or logs. The store keeps only its hash.
Expiry is explicit, from one to 90 days. Issuance checks hat ownership/purge,
paired active hosts, canonical root syntax and server path-rule resolution.
Actual host realpath/workspace/source-policy checks are required again before
execution tools can be introduced; this slice reads metadata, not files.

A client is bound to one hat and explicit host/root pairs. Discovery is always
available in those pairs. Metadata permission is separate, and existing-session
access is an additional explicit grant. The default created-by-client selection
returns no existing sessions; this first slice has no create/adopt operation.
Filters and cursors can only narrow scope. Root checks use path segments, so
`/workspace/demo-other` is outside `/workspace/demo`. Credentials cannot assert
an owner, bypass source policy or impersonate an operator or delegated parent.

Every request rechecks expiration/revocation and hat purge. Metadata queries
also check host revocation and the current stored session hat. Cached principals
are revalidated at the database gate and before publishing collected results.
Token rotation, grant editing, quotas for execution and durable write admission
are future work; revoke and issue a separately reviewed grant in this slice.

## HTTP and stdio

The endpoint is `POST /mcp/external/hennery`, with:

```text
Authorization: Bearer EXTERNAL_CLIENT_TOKEN
Content-Type: application/json
Accept: application/json, text/event-stream
```

It implements stateless MCP profiles `2025-11-25`, `2025-06-18` and `2025-03-26`,
with JSON responses, no required MCP session ID and no persistent GET stream.
An unsupported offered version receives the supported version during initialize;
the client must accept that negotiated profile. Present Origin must match the
collector's public origin. Cookies alone never authenticate this endpoint.
Responses are private/no-store. Bodies, response sizes and concurrent requests
are bounded. No incoming OAuth or claim of the latest protocol is made.

Tools in this slice:

| Tool | Result |
| --- | --- |
| `hennery_capabilities` | Client scope/revision, expiry, limits and explicit execution=false. |
| `host_list` | Granted host IDs/names, connectivity and currently connected runtime IDs. |
| `workspace_list(host_id)` | Explicitly granted roots only; no arbitrary browsing or recents. |
| `session_list(after?, limit?)` | Scoped metadata, at most 50 rows and an ID continuation cursor. |
| `session_get(session_id)` | One authorized metadata record; out-of-scope and missing both report not_found. |

Session tools require metadata permission. Reads do not acknowledge unseen or
needs-you state. No transcript, prompt content, permission options, credential
or connection administration is exposed. Session pagination is an eventual
metadata listing, not a transcript snapshot/delta cursor.

For a Desktop local MCP entry, merge an entry such as this into the existing
configuration using Desktop's Developer / Edit Config flow. Use absolute paths;
do not replace other entries. The example is a configuration template, not a
command executed by this work:

```json
{
  "mcpServers": {
    "hennery": {
      "command": "/absolute/path/to/hennery",
      "args": [
        "external-mcp",
        "--url", "https://collector.example/mcp/external/hennery",
        "--token-file", "/absolute/private/path/hennery-external.token"
      ]
    }
  }
}
```

The bridge uses TLS and refuses redirects, URL credentials and arbitrary API
paths. Plain HTTP is allowed only for loopback scratch tests. It emits only
JSON-RPC lines on stdout; notifications do not produce fake responses. It reads
no collector database and borrows no cookie. Other stdio clients can run the
same command. Packaging as a Desktop extension can follow independently.

## Acceptance and next slices

Automated scope/protocol/bridge checks are separate from these **pending manual
Desktop checks**:

1. Use an isolated collector and a deliberately issued scratch credential.
   Start the bridge through Desktop; confirm its connection and tool schemas.
2. Ask Desktop to list the authorized host/root and report the client's scope.
   Check that it explains execution is unavailable in this first slice.
3. With the default grant, confirm existing sessions are absent. With a separate
   explicit existing-session grant, read an allowed metadata record and verify
   another hat/root/session remains inaccessible.
4. Revoke the credential while Desktop remains connected. The next call must
   fail; reconnecting or reinitializing must not restore access.
5. Record Desktop version, offered/negotiated MCP version, observed tool calls
   and result. Do not copy credentials or private transcript data into evidence.

The first execution acceptance remains pending until later slices provide:
create -> prompt -> lost response/disconnect -> operation_get -> wait/read of
that same operation, without duplicate agent execution. Before exposing writes,
add server-written ExternalClient provenance, durable client request admission
and retained receipt/tombstone semantics, grant checks at dispatch/auto-resume,
and server-resolved per-(hat, actual ACP) source policy with truthful applied
attach evidence. Reconcile the accepted prompt-status foundation onto current
main when this resolver is needed; do not add a second prompt dedupe queue.

Then add exact-turn cancellation, explicit park/resume/close and ordinary
elicitation answers. Permission requests remain operator UI actions, and reads
never grant approval. Attachments, post-start configuration, incoming OAuth and
additional protocol profiles are outside this first slice.
