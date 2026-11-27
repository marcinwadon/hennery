# Same-hat peer sessions

The operator requested native agent access to other independent sessions on
2026-11-27. Reading their history is the first priority; sending a message must
not create or require a parent-child relationship.

## Scope and authority

The collector derives the caller's owner, session and hat from its current
session token. A separate per-hat policy has three values: `off`, `read` and
`read_write`. Existing hats default to `off`; delegation remains independent.
The built-in `hennery_peers` MCP server is delivered on the next real attach
when enabled, using the existing rotating session token and source-isolation
decision. Its delivered policy revision is bound to that exact token generation.
Changing policy cuts current reads and invalidates old grants, including an
off/on cycle. Caller-supplied owner, hat or sender fields never grant authority.

Peers can discover retained sessions and read their canonical collector view
within the same owner and hat, including closed/archived sessions and sessions
on other paired hosts. A host need not be online for historical reads. Deleted
sessions, purged hats and revoked hosts are unavailable. A guessed ID from
another hat produces the same refusal as an unknown ID.

## Tools

- `session_list`: bounded, paginated same-hat metadata, with optional search.
- `session_read`: canonical transcript pages and versioned chunks of large
  items, using the same reader as external session MCP. Reads do not acknowledge
  operator attention or answer questions. Provider-private memory is not read.
- `session_send`: one bounded text message with a caller-scoped retry key.
  Return a durable receipt; retrying identical content cannot duplicate it.
  The collector writes the source session identity into the message origin.
- `message_status`: retrieve the caller's durable receipt by retry key. Another
  session cannot inspect this receipt through that key.

Peer input is agent input, not operator approval. It cannot answer permission
requests, change runtime permissions, close sessions or assume family authority.
The original delegation server and its direct-child restrictions remain intact.

Messages use the existing durable prompt queue and steering transport. Automatic
delivery targets the current turn when supported, otherwise a new turn. Closed
or archived sessions are never reopened by a peer. The UI and receiving agent
identify the originating session. Transport never generates automatic replies.
The operator explicitly chose automatic delivery. New messages are limited to
32 per sender between operator turns; retries do not consume that budget.
Policy revision changes, hat moves, deletion and host revocation invalidate
unstarted delivery, including before an automatic resume. Accepted intent may
survive the sender parking, but cannot reopen a closed or archived recipient.

The shared independent-input scheduler retains its historical internal
`operator` queue class. Authority is derived from the separate, server-written
`Peer` input origin and message ledger, never from that queue class. An explicit
operator edit becomes operator input and survives deletion of its original peer
sender. Deleting either endpoint scrubs its peer receipts and pending peer
input; already retained copies in another session's history remain there.
The schema migration appends new fields
and a receipt ledger without rewriting historical turns or delegation grants.

## Verification

Cover independent same-hat sessions on different hosts, closed history, opaque
cursor binding, large-item reconstruction, unknown/foreign/deleted targets,
caller token rotation and policy off/on, source/target hat changes, source
provenance, duplicate/conflicting retry keys and persisted message delivery.
Retain existing external-MCP read and parent-child authorization regressions.
Use scratch stores and fake agents; do not probe real user conversations.
