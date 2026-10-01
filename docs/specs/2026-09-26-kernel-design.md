# hennery — kernel (subsystem spec)

- **Date:** 2026-09-26
- **Status:** Draft. Amended 2026-10-01 to match what plans 3a (host
  pairing), 3b-i, 3b-ii and 3b-iii (operator auth, `owner_id`) and 3c
  (passkeys) built, and the decisions confirmed with them. Where a section
  still describes something not built yet, it says so.
- **Refines:** [architecture spec](2026-09-25-hennery-architecture-design.md) §4
  (data model), §7 (auth and pairing), §8 (hats), §9 (kernel), §12.4
  (backups). Consumed by the [ACP core](2026-09-26-acp-core-design.md),
  [MCP gateway](2026-09-26-mcp-gateway-design.md) and
  [frontend](2026-09-26-frontend-design.md) specs.

The kernel is the `hennery-kernel` crate: what every mode of the collector needs
regardless of whether sessions or the gateway are enabled — operator auth,
host pairing and identity, hats, push delivery, outbound HTTP policy,
settings, storage and configuration.

---

## 1. Storage

- One SQLite database, `<data>/hennery.db`, WAL mode, `rusqlite` with the
  bundled SQLite (no system library; static builds).
- **One writer thread** owns the write connection and receives work over a
  channel; reads use a small pool. This serialises writes by construction,
  which idempotent ingest (ACP core §8) relies on.
  *Built so far:* no writer thread. The operator, the host registry and the
  sessions store each hold their own connection to `hennery.db`, and every
  transaction is `IMMEDIATE`: it takes the write lock as it begins and waits
  out a 5 s busy timeout. (A deferred transaction that reads first would fail
  its first write with `SQLITE_BUSY_SNAPSHOT`, which the timeout does not
  cover.)
- Migrations are embedded SQL lists, one per component sharing the file: the
  sessions store keeps its version in `user_version`, the kernel's tables in a
  `schema_versions(component, version)` row. Each step reads the version in
  its own `IMMEDIATE` transaction, so two connections migrating one file apply
  it once. They run at startup before anything listens. A database (or a
  component) newer than the binary refuses to start with a clear message
  (downgrades are not supported).
- Every table has `owner_id` (umbrella §4); `owners` names its owner in `id`,
  and `schema_versions` (bookkeeping) has none. In v1 it is always the single
  owner's id and every query filters by it. Each component binds to the
  database's owner when it opens (the oldest row of `owners`, by `created_at`
  then `id`, the one query that does not filter); teams would take it from the
  request instead, and the statements already name it.
- **Data directory:** `$HENNERY_DATA_DIR`, else `$XDG_DATA_HOME/hennery`
  (Linux) / `~/Library/Application Support/hennery` (macOS); a host on the same
  machine keeps its own files there too (distribution §8; under `hennery up`
  in `<data>/collector` and `<data>/host`). Collector contents:
  `hennery.db`, `master.key`, `vapid.key`, `config.toml`, `admin.sock`,
  `setup-url` (until setup), `attachments/` (ACP core §7).

### 1.1 Kernel tables

```sql
owners(id TEXT PK, contact NULL, created_at, set_up_at NULL)  -- made at first start (§3.1)
password_credentials(owner_id PK, phc TEXT, updated_at)
passkeys(id TEXT PK, owner_id, credential_id UNIQUE, credential JSON, sign_count,
  label, created_at, last_used_at NULL)                       -- §3.2
auth_sessions(id_hash PK, owner_id, user_agent, created_at, last_seen_at,
  last_step_up_at NULL, expires_at)
hosts(id TEXT PK, owner_id, name, public_key, default_hat_id, platform,
  host_version, capabilities JSON, agents JSON, workspace_roots JSON,
  last_doctor JSON, last_seen_at, created_at, revoked_at NULL)
pairing_codes(code_hash PK, owner_id, created_at, expires_at, used_at NULL)
settings(owner_id, key, value, PRIMARY KEY(owner_id, key))   -- public_url, contact, push defaults
project_recents(owner_id, host_id, hat_id, path, last_used_at,
  PRIMARY KEY(host_id, hat_id, path))
push_subscriptions(id TEXT PK, owner_id, endpoint, p256dh, auth, device_label,
  created_at, last_success_at, last_error)
purged_hats(hat_id PK, owner_id, purged_at)                  -- §5.5
```

`hats` and `hat_path_rules` are in §5.1.

Times are integer Unix seconds; the REST API shows RFC 3339.
`hosts.public_key` (lowercase hex) is unique, across owners too.
`passkeys.credential_id` is unique across owners, so one credential never signs
in to two accounts; `sign_count` is the counter last accepted (§3.2) and
`credential` is `webauthn-rs`'s `Passkey`.

*Built so far:* `hosts` lacks `default_hat_id` (hats), `agents`,
`workspace_roots` (`probe_agents`) and `last_doctor`; `project_recents`,
`push_subscriptions` and `purged_hats` do not exist yet.

## 2. Configuration

Precedence: CLI flags > environment (`HENNERY_*`) > `<data>/config.toml` >
defaults. Settings that the operator edits in the UI (`public_url`, push
policy) live in the database, not in the file. Secrets are never accepted as
CLI flags (they would show in process listings); they come from files, the
environment, or systemd credentials. (A pairing code, single use and short
lived, may be `host join`'s argument; left out, it is read from stdin, §4.1.)

`config.toml` is read, never created. It has two keys: `listen` (§7) and
`public_url`, which also takes `--public-url` / `HENNERY_PUBLIC_URL`. An
unknown key, an empty `listen`, an empty `HENNERY_LISTEN` or a file its group
or others can write stops the start, naming the source. Before setup the
configured `public_url` is the setup link's base (§3.1); once set up, the
stored one stays in effect, and a configured one that differs only produces a
warning naming `hennery admin reset-public-url` (§4.2).

## 3. Operator authentication

### 3.1 Setup

- **The first start creates the owner** (`owner-` and 16 random hex digits)
  with no login method, so what is written before setup (`up`'s pairing code
  and host, §4.2) already has an owner. Setup completes it and sets
  `owners.set_up_at`; of two racing setups one wins and the other is told
  setup is done. "Set up" means `set_up_at` is set: before it no login opens a
  session and the admin resets (§4.2) refuse.
- Until set up: generate a 256-bit setup token and write
  `<public_url or http://localhost:PORT>/setup#<token>` to `<data>/setup-url`
  (mode 0600; the base is the configured `public_url`, §2, else the first
  listener's port). The token is in the fragment, so it never reaches a
  request line, a log or `Referer`; it leaves the browser only in the
  `POST /api/setup` body. The full link is printed to the terminal **only when
  stdout is a TTY**; otherwise (service, container) only the path of the
  `setup-url` file is logged, so the token never lands in a log collector.
  (`hennery admin setup-url`, §4.2, prints it on request, terminal or not.)
  Valid for 1 hour, single use; a restart before setup issues a new one and
  invalidates the old.
- Only the token's hash is kept, in memory. The file is written once the
  listener is bound, created 0600 under a temporary name and renamed into
  place, so a link at `setup-url` is replaced, never written through. It is
  removed when setup succeeds, and at any start once set up.
- The setup form sets the owner password, `public_url` (pre-filled from the
  request's origin, must be `https://` or a loopback `http://` origin), and the
  name of the default hat; it then offers passkey registration.
- `POST /api/setup {token, password, public_url}`: `Origin` must equal the
  submitted `public_url`'s origin (there is no stored one yet); a missing one
  is refused. Checks in order: the URL's shape (400 `invalid`), `Origin` (403
  `origin_mismatch`), an owner already set up (409 `already_set_up`), the
  token (401 `invalid_setup_token`: unknown, used or expired), the password
  (400 `invalid`). The token is used up only when the owner's row commits.
  201 `{public_url}` with the session cookie; the session counts as stepped up
  (§3.4), so setup's passkey offer needs no second check.
- **`public_url`** is stored as its origin as a browser serialises `Origin`
  (lowercase scheme and host, no default port, no trailing slash, IDNA
  punycode) and compared as a plain string. No path, query, fragment or
  credentials. Loopback means `localhost`, `127.0.0.0/8` or `[::1]`. A stored
  value that no longer parses stops the start.
- `/setup`, `/setup.js` and `/api/setup` answer with `Referrer-Policy:
  no-referrer` and `Cache-Control: no-store`.

*Built so far:* the setup page is a static page of the kernel's (until the
frontend), and `SetupRequest` has no default hat name; hats add it.

### 3.2 Login

- **Password:** Argon2id (PHC string, `password-auth`), verification on a
  blocking thread; at least 12 characters, at most 1024 bytes; at most two
  hashes or verifies at once. `POST /api/auth/login {password}` → 204 with
  the cookie, or 401 `invalid_password`. Login attempts are rate limited per
  client address (5 wrong passwords per minute, then a 60 s lockout doubling
  up to 1 h: 429 `rate_limited` with `Retry-After`); only a success clears the
  address. Constant-time failure path: an attempt that is not rate limited
  runs exactly one Argon2 verify (against a dummy hash before setup); a
  limited one runs none, so the right password gets 429 too.
- **Rate limits** (here and §4.1) key on the canonical client address, IPv6
  per /64 and all of `127.0.0.0/8` as one; every attempt counts the moment it
  starts, so a parallel burst gets no more in than the budget. State is in
  memory, bounded, and a restart clears it; past capacity a live entry is
  never evicted (new addresses share one overflow budget), and loopback always
  keeps an entry of its own. `X-Forwarded-For` is not trusted: behind a
  reverse proxy every client shares the proxy's budget.
- **Passkeys:** `webauthn-rs` with RP id and origin derived from `public_url`;
  subdomains and arbitrary ports not allowed. The RP id is `public_url`'s host
  exactly (not a registrable parent) and its origin the only one accepted; a
  `public_url` at an IP address has no RP id, so every passkey start answers
  409 `passkeys_unavailable` and the password works as before. No attestation
  is asked for; user verification is required.
  - **Ceremonies** are kept in memory, keyed by a random 256-bit ceremony id:
    single use (a finish takes it, whatever it then finds), 5 minutes, bound
    to the session that began it (registration, step-up), at most one per
    session and kind. Login ceremonies have a pool of their own, so a flood of
    login starts never pushes out a registration or a step-up. A restart
    drops them. An unknown, used or expired ceremony answers 400
    `invalid_ceremony`.
  - Several passkeys per owner, each with a label (1–64 characters, no control
    characters) and last-used time, listed in registration order. In v1
    passkeys add to the password and never replace it, so every passkey is
    removable.
  - **Login** is username-first: `login/start` offers every passkey of the
    owner's (`allowCredentials`), or answers 409 `no_passkeys`; discoverable
    login is not offered. Passkey login starts have their own per-address
    budget (30 a minute, then the same lockout), so a password lockout does
    not stop passkey login. Passkey step-up is bound to its session and has
    no limiter. A refused assertion answers 401 `passkey_refused`.
  - **Counter:** a login or step-up must send a signature counter above the
    one stored, unless both are 0 (synced passkeys); it is compared and set in
    the finish's transaction. A counter that does not move on is refused,
    writes nothing and logs a warning naming the passkey; the passkey is not
    disabled (that would hand a clone's holder a lever).
- **Sessions:** server-side (`auth_sessions`: random 256-bit id hashed at
  rest). Cookie `hennery_session`: `HttpOnly`, `Secure` (from `public_url`'s
  scheme, never from the peer address), `SameSite=Strict`, `Path=/`,
  `Max-Age` 30 days. 30-day sliding expiry; use slides it at most once a
  minute, and a slid response re-sends the cookie. Up to four
  `hennery_session` cookies in a request are tried in order (a sibling
  subdomain can plant a same-named one). Expired rows are pruned when a
  session opens; `User-Agent` is stored without control characters, at most
  256 characters. A login, password or passkey, opens a stepped-up session
  (§3.4), and only while the credential it checked is still current (the same
  statement), so a login checked just before a reset opens no session after
  it.
  - Settings lists sessions and can revoke them: `GET /api/auth/sessions`
    lists the live ones, most recently used first, with `current`;
    `DELETE /api/auth/sessions/{id}` (step-up) answers 204, or 404 for an
    unknown or expired one; revoking the request's own session clears its
    cookie. `POST /api/auth/logout` needs no session: 204, cookie cleared.
  - **Ending a session** (revoke, logout, expiry, a reset) ends every stream it
    holds open. Expiry is re-checked against the wall clock at least hourly,
    since the monotonic clock skips a suspend.
- **Changing `public_url`** changes the OAuth redirect URI: OAuth
  registrations must be redone. It changes the passkey RP id only when the
  host name changes; then it removes every passkey in its transaction, since
  authenticators scope them by RP id. A move to another port or scheme keeps
  them; they sign in at the new origin only. Every ceremony under way ends
  either way, and every finish re-reads the relying party under the database
  lock, so a change lands wholly before or after it. Settings says so before
  saving, and the change requires step-up (§3.4).

*Deferred:* identity from a fronting proxy. A header trusted by peer address
is forgeable by any local process when the proxy runs on loopback; a later
version may accept signed assertions only (umbrella §7.1, §15).

### 3.3 Origin rules and CSRF

State-changing endpoints accept only `application/json`. `Origin` is checked
per route class:

| Route | Authentication | `Origin` |
|---|---|---|
| Browser routes, state-changing methods (every method but `GET` and `HEAD`) | Session cookie | Must match `public_url`; a missing `Origin` is rejected. Same rule on every listener (§7) |
| Browser routes, `GET` (JSON, SSE streams, attachments, logos) | Session cookie (`SameSite=Strict`) | Browsers do not send `Origin` on same-origin `GET`s, so these check `Sec-Fetch-Site` instead: `same-origin` or `none` accepted, anything else rejected; a present `Origin` must match |
| `GET /api/hosts/ws` | Unauthenticated until a valid `hello` proof (ACP core §3.5) | Exempt |
| `POST /api/setup` | Setup token (§3.1) | Must equal the submitted `public_url`'s origin; a missing `Origin` is rejected |
| `POST /api/hosts/enroll` | Pairing code | Exempt |
| `/mcp/*` (gateway proxy) | Bearer token | Exempt |
| `GET /api/mcp/oauth/callback` | `state` plus the flow cookie (gateway §4.3) | Exempt |
| `/healthz`, `/readyz` | None (no data) | Exempt |
| `GET /setup`, `GET /setup.js` (static, no data) | None | Exempt |

- State-changing means every method but `GET` and `HEAD`: `OPTIONS` is refused
  like `POST`, and hennery serves no CORS. Before setup every state-changing
  browser request is refused, 403 `setup_required`.
- A wrong or missing `Origin` answers 403 `origin_mismatch`; a non-ASCII one
  matches nothing. A `Content-Type` other than `application/json` (parameters
  allowed), or a body without one, answers 415 `unsupported_media_type`; a
  body-less request needs none.
- On `GET`/`HEAD` a missing `Sec-Fetch-Site` is accepted (curl, older
  browsers), which relies on **no `GET` or `HEAD` route changing state**; any
  value but `same-origin`/`none` answers 403 `cross_site`.
- These rules run before the cookie check, so a cross-origin request gets 403
  before 401 `unauthenticated`.
- `/api/setup` and `/api/auth/*` read at most 16 KiB of body; more is 413
  `body_too_large`, before any parse or password check.
- A JSON body these routes or enrollment refuse keeps its status (400, 413,
  415, 422) with a fixed `ApiError` (`invalid_body`, `body_too_large`,
  `unsupported_media_type`), never the parser's message, which would quote the
  request back. *Built so far:* the session API still answers with the
  parser's text.

### 3.4 Step-up authentication

Some actions require a password or passkey check within the last **5
minutes** (`auth_sessions.last_step_up_at`), even inside a valid session:

- minting pairing codes;
- creating or editing gateway connection URLs, credentials, pre-registered
  clients or the "internal network" flag;
- local stdio server configuration;
- registering or revoking passkeys; revoking hosts or auth sessions;
- deleting sessions and purging hats;
- changing `public_url`.

Without a fresh check the endpoint answers 403 `step_up_required`; the
frontend prompts and retries (frontend spec §3).

Fresh means strictly less than 300 s since `last_step_up_at`; setup and every
login stamp it. `POST /api/auth/step-up/password {password}` → 204, or 401
`invalid_password`; its limiter is its own, with the login policy, so a login
flood does not stop a signed-in owner stepping up and wrong step-ups do not
lock login. A passkey registration is checked at its start, at its finish and
again in its write: the `INSERT` requires the registering session to be live
and stepped up, so a session revoked or a password reset mid-ceremony stores
nothing.

*Built so far:* step-up guards minting pairing codes, revoking hosts and auth
sessions, and registering and removing passkeys; the other actions get it with
their endpoints.

## 4. Host identity and pairing

### 4.1 Pairing

1. `POST /api/hosts/pairing-codes` (operator, step-up) → code: 8 characters
   from a base32 alphabet without ambiguous characters, displayed as
   `XXXX-XXXX`; TTL 10 minutes; single use; stored hashed.
2. `hennery host join <public_url> [<code>]` on the machine (without the code,
   one line of at most 256 bytes is read from stdin, which keeps it out of
   argv and history; a longer one is refused and never echoed): the
   host generates an Ed25519 keypair (`host.key` in its data dir, 0600;
   distribution §8) and calls `POST /api/hosts/enroll {code, public_key, name,
   host_version, platform}`.
3. The collector creates the `hosts` row with the installation's default hat
   as its default hat and returns `{host_id}`. Transport security is TLS.
4. The host connects (`/api/hosts/ws`) and proves possession in `hello`
   (ACP core §3.5).

- **Codes** are Crockford base32 (40 bits): case, dashes and spaces are
  ignored, `O` reads as `0` and `I`/`L` as `1`. A code is dead exactly 600 s
  after minting, and is spent in the transaction that creates the host.
- **The table stays bounded:** every mint first deletes the spent and expired
  codes, and at most 16 codes are live at once; a mint past that is 409
  `too_many_codes` (the admin socket's mint fails with the same message).
  `hennery up`'s own mint at start (§4.2) is not capped, so live operator
  codes can never stop the all-in-one from pairing its host.
- **Enroll refusals:** a key paired already gets 409 `already_paired`; a
  malformed key, or a name, version or platform outside 1–64 printable
  characters (no control or Unicode format characters), gets 400 `invalid`;
  neither spends the code. A wrong code is 401 `invalid_code`. Host ids are
  collector-minted: `host-` and 16 hex digits.
- **The URL** is `https://`, or `http://` to loopback only, with no path; the
  WebSocket URL is derived from it (`wss`/`ws`, `/api/hosts/ws`). Enrollment
  uses no proxy. *Built so far:* `join` refuses `https://` before it spends a
  code, since the host WebSocket has no TLS yet.

**Idempotent:** if a pairing exists, `host join` first sends a probe `hello`
(nothing attached and no `resend_complete`, so the host never becomes ready):
- `hello_ack` or `already_connected`: nothing changes and no code is spent;
- `revoked`: a new key and host id replace the pairing;
- `bad_proof` is ambiguous (a reset or restored collector): `join` refuses and
  names `host.key` and `host.toml` to remove. A pairing with another collector
  is refused the same way; any two loopback URLs (any port, `localhost`) are
  the same collector.

Whenever a new identity is written beside an outbox, that outbox is moved
aside as `outbox.db.orphaned-<old host id>` (or `-unpaired`), never deleted.
The new key is written to `host.key.pending` before enrolling, so a directory
that cannot hold it costs no code; half a pairing is an error naming the file
to remove. Once enrollment answers, `host.toml.pending` is written before
anything is renamed, then the key and `host.toml` are renamed into place, and
nothing after the enrollment deletes the key it enrolled. `host.toml` names the
enrolled public key: loading a pairing rolls a staged `host.toml.pending`
forward only onto the key it names (a crash between the renames), and refuses
a staged or stored `host.toml` whose key is another. A `host.toml` from before
it named its key still loads.

Enrollment is rate limited **per client address** (5 wrong codes per 10
minutes, then a 60 s lockout doubling up to 1 h; §3.2 for how addresses are
keyed); wrong codes never invalidate other outstanding codes. Every attempt
counts the moment it starts, and a success clears the address. A locked-out
address gets 429 `rate_limited` with `Retry-After`, and its code is not
checked, so not spent.

### 4.2 All-in-one pairing and the admin socket

`hennery up`'s supervisor pairs its own host with no operator step. When the
host has no pairing yet, the supervisor creates one pipe and hands its write
end to the collector child and its read end to the host child, each as an
**inherited file descriptor**; the supervisor never reads the code. The
collector mints one code once it has migrated and is listening, and writes it
there; the host joins over loopback, and sees end-of-file if the collector dies
first. The code is never on a command line or in the environment. Each child
checks its descriptor at start, before anything else: it must be 3 or above,
open, and a pipe; the host reads at most 256 bytes from it. An existing
pairing gets no pipe: `up` never re-pairs a key the collector does not know.
The host child is handed the collector's current loopback URL, which wins over
the stored one, so a new port keeps working. The code and the host carry the
owner the first start made (§3.1), so a host paired before setup is the
owner's once setup completes.

**A revoked all-in-one host is not re-paired automatically, and does not take
`up` down:** `host run` exits 78 (`EX_CONFIG`), and `up` logs which files to
remove and keeps the collector serving, on every later start too until they
are removed.

The collector listens on `<data>/admin.sock` (Unix socket, mode 0600) for
`hennery admin …` recovery commands (print setup URL, reset password, reset
`public_url`, list hosts, mint a pairing code, restart pending) and for
`hennery backup` / `hennery restore`. **State-changing commands** (backup,
restore, password reset, `public_url` reset, pairing-code minting) require a
typed `yes` on a terminal; without a terminal the CLI refuses and sends
nothing. `setup-url` and `hosts` need none. There is no `--yes`: scripts mint
pairing codes over the HTTP API with step-up. A new password is typed twice on
the terminal with echo off, never as an argument (§2). The confirmation is
enforced by the CLI: it stops accidental and non-interactive use, not a process
that speaks the socket protocol directly (§10).

- **Protocol:** one JSON line each way, at most 16 KiB, within 10 s; anything
  else is answered `refused` and changes nothing. Each connection's peer uid
  must equal the collector's effective uid, or it is dropped unanswered; the
  CLI in turn refuses a socket served by another uid before sending anything.
  Only a command's name is logged, never its arguments; state-changing ones at
  `warn`, with the peer's pid.
- **One collector per data directory:** at start the collector connects to an
  existing `admin.sock`. If it answers, the start fails; if the connection is
  refused (left by a killed collector), the socket is replaced; any other error
  stops the start, naming the path. The socket is bound before the database and
  the setup link, and removed when the collector stops. Two simultaneous starts
  can still race (an `flock` would close that; not taken). When the path does
  not fit `sockaddr_un.sun_path` (104 bytes on macOS, 108 on Linux), the
  collector warns and runs without the socket.
- **The CLI side:** before any prompt, `hennery admin` checks the socket: that
  its path fits `sun_path` (a path too long is named, with the limit, rather
  than reported as a failed connection), that it connects, and that it is the
  same uid's. A connection closed without a request, which that check makes,
  is not answered and not warned about. Each command gets 30 s for connect,
  request and answer; past that the CLI says the outcome is unknown. For
  `hennery up`'s data directory the socket is `<data>/collector/admin.sock`.
- **`setup-url`** returns the live link, or issues a fresh one once its hour is
  up (the old token dies); once set up it fails.
- **`reset-password` is the recovery.** It refuses before setup, takes a
  password as login does (§3.2), replaces the credential (changing nothing
  unless exactly one row was replaced), ends every session and stream, clears
  the login limiters, and **removes every passkey** and ends every ceremony: a
  stolen password alone can register a passkey, so recovery must not depend on
  the owner spotting it. The CLI says so before it asks for the password, and
  reports the sessions ended and passkeys removed. A later change-password
  route must not be the recovery: it keeps the passkeys.
- **`reset-public-url <url>`** refuses before setup, replaces the stored value
  with no restart, ends every session, and removes the passkeys only when the
  host name changes (§3.2). The CLI warns about passkeys before it asks, and
  reports how many it removed.

### 4.3 Host lifecycle

- Rename, change default hat, **revoke** (step-up). Revoking closes the host's
  connection, rejects future `hello`s with `revoked`, and calls the lifecycle
  hooks (§5.5): sessions are parked and their gateway tokens revoked. The
  host's adapters keep running until it next connects; it is then told it is
  revoked and stops them.
- **Revoke order:** mark the host revoked; kick its connection and wait, at
  most 10 s, until it is unregistered; then call the hooks; answer 200
  `HostItem`. The socket task re-checks revocation right after registering and
  never reconciles or marks a revoked host ready; when a revoked host's socket
  ends, the hooks run again, so a revoke whose wait timed out converges. A
  failed registry check there is retried (after 200 ms, 1 s and 5 s), then
  logged with the remedy: revoke the host again. A
  repeated revoke re-runs every step. A connected host is kicked, so it learns
  of the revoke when it reconnects, at once.
- `last_seen`, versions, platform, capabilities, workspace roots, agent
  availability and the last doctor report are updated from `hello` and
  `probe_agents`. *Built so far:* an accepted `hello` updates `host_version`
  (only if it has enrollment's shape), `capabilities` and `last_seen_at`, which
  is the last accepted `hello`, not liveness; no rename or default hat yet.
- One live connection per host (ACP core §3.5).

## 5. Hats

### 5.1 Model

```sql
hats(id TEXT PK, owner_id, name, colour, logo_mime NULL, logo_bytes NULL,
  push_policy JSON, created_at)
hat_path_rules(id TEXT PK, owner_id, host_id, prefix, hat_id, verified BOOL)
```

`hosts.default_hat_id` names each host's default hat. Setup creates one hat; it
is the default for new hosts until changed. A hat referenced by a host default
cannot be deleted; otherwise hats are removed by purge (§5.5).

**Logos** are uploaded as SVG or PNG (≤ 64 KiB), **sanitised server-side**
(SVG: scripts, event handlers, `foreignObject`, external references and
non-`data:` URLs removed; PNG: decoded and re-encoded) and served from
`GET /api/hats/{id}/logo` with `nosniff` and a `default-src 'none'`
policy. The frontend renders them only as `<img>`, never inline.

### 5.2 Resolution

Given `(host_id, path)`:

1. The path is **canonical**: absolute, symlinks resolved, no `.`/`..`, no
   trailing slash. Canonicalisation happens **on the host**, which is where the
   filesystem is (`resolve_path` request, §5.4). Rule prefixes are stored
   canonicalised the same way (resolved through the host when the rule is
   saved; a rule for a path that does not exist is stored as typed, normalised
   lexically, and marked unverified).
2. Candidate rules are those of that host whose prefix equals the path or is a
   **path-segment prefix** of it (`/p/acme` matches `/p/acme` and `/p/acme/x`,
   never `/p/acme-infra`).
3. The longest candidate prefix wins; with no candidate, the host's default
   hat.

**At start:** `resolve_path` → rule match → the hat is stored on the session
with the canonical cwd → `start_session` (ACP core §4.3). **At resume** the
hat is re-resolved; a mismatch refuses the resume until the operator
re-assigns the session. Re-assignment is allowed for any session with no
running adapter (parked, closed or failed), with a warning, and writes a `hat_reassigned{from, to}` timeline event
(ACP core §4.9).

### 5.3 Where hats are enforced (kernel side)

- Project recents are stored per (host, hat) and returned only for the hat the
  browsing path resolves to.
- Push notifications follow the session's hat policy (§6).
- The gateway's `MountPolicy` joins mounts with the principal's hat (gateway
  spec §3.1).

### 5.4 Protocol addition

`resolve_path{path}` → `resolved_path{canonical, exists, is_dir}` (or `error`)
is part of the frame catalogue (ACP core §3.3). It is used for typed paths in
New session, for session start and resume, for rule saving, and by the hat
tester.

### 5.5 Lifecycle hooks and purge

The kernel defines a `LifecycleHooks` trait with `on_host_revoked(host_id)`
and `on_hat_purged(hat_id)`; `hennery-sessions` and `hennery-gateway` each
implement it, so the kernel never imports either. Hooks must be idempotent: a
repeated revoke, and a revoked host's socket ending, call them again (§4.3).
*Built so far:* only `on_host_revoked`, implemented by the sessions module.

**Purge a hat** (`POST /api/hats/{id}/purge`, step-up; the default hat of any
host cannot be purged until the hosts are moved to another hat):

- sessions: every session of the hat is deleted as in ACP core §4.10;
- gateway: its connections with their grants, mounts, session tokens,
  standalone clients and stdio servers are deleted (gateway §2);
- kernel: its path rules, project recents and the hat row are deleted, and the
  hat is recorded in `purged_hats`;
- hosts: after every handshake the collector sends `forget_hat{hat_id}` for
  each hat in `purged_hats` (kept 30 days); the host deletes its composed agent
  home for that hat (ACP core §6) once no process of that hat runs. Deletion
  is idempotent.

## 6. Push

- **VAPID keys:** P-256 key pair generated on first run into `<data>/vapid.key`
  (0600). The VAPID `sub` claim is `mailto:<owner contact>` if configured, else
  the `public_url`; never a non-routable placeholder (iOS silently rejects
  those). Setup does not ask for the contact; it is an optional field in
  Settings.
- **Delivery:** `web-push-native` builds RFC 8291 (aes128gcm) requests with
  VAPID (RFC 8292), sent with the shared HTTP client under the egress policy
  (§7.1; push endpoints must be public). TTL 1 hour, urgency `high` for "needs
  your answer", `normal` otherwise. Non-2xx responses are logged with status;
  404/410 delete the subscription.
- **Policy** per hat: `muted`; `details` (include the agent's question title);
  `generic_title` ("Session needs your answer", without the session title).
  Default: not muted, no details, session title shown. Triggers are defined by
  the modules that own them (ACP core §10, gateway §7).

## 7. HTTP server

- `axum` on **one or more listeners** (`listen = ["127.0.0.1:7117"]` in
  `config.toml`; `--listen` repeatable; `HENNERY_LISTEN` comma-separated;
  default `127.0.0.1:7117`). Every listener serves the same router and the
  same authentication and `Origin` rules (§3.3). Start fails if any address
  cannot be bound. At most 8 listeners; they are bound in the order given,
  before the data directory is touched, and one that fails stops the others.
  TLS is provided by the deployment topology (umbrella §7.5); the kernel does
  not terminate TLS in v1.
- **Browser access is bound to `public_url`, not to a listener.** Passkeys
  need a domain as RP id (an IP address cannot be one) and state-changing
  requests must carry the `public_url` origin, so a browser has to reach the
  collector through the `public_url` address. Extra listeners are for
  origin-exempt clients: e.g. loopback for the local host child of
  `hennery up` plus a LAN or Tailscale address for remote hosts and gateway
  clients, without binding `0.0.0.0`. `doctor` warns when `public_url` reaches
  neither a listener nor a reverse proxy (distribution §7, check 16).
- Static assets are embedded (`rust-embed`, deterministic timestamps): hashed
  assets `Cache-Control: immutable`, `index.html` and the service worker
  `no-cache` with an ETag.
- Response compression for JSON and HTML only; **never** on SSE or the gateway
  proxy routes.
- Structured logging (`tracing`), JSON optional. Secrets never logged: tokens,
  cookies, `Authorization` headers and OAuth parameters are redacted by a
  shared layer.

### 7.1 Outbound HTTP (egress policy)

One shared `reqwest` client for every outbound call — gateway proxy, OAuth
discovery/registration/token calls, Web Push:

- redirects are never followed;
- DNS is resolved by hennery and each address is checked before connecting:
  loopback, link-local (including `169.254.169.254`), RFC 1918, unique-local
  IPv6, CGNAT and other non-public ranges are refused, unless the caller passes
  an explicit "internal network" allowance (a gateway connection the operator
  marked so; never Web Push);
- per-caller limits on concurrent requests and idle streams (gateway §5.7).

It lives in the kernel so that push delivery can use it without depending on
the gateway.

### 7.2 Content-Security-Policy

Every HTML response carries:

```
Content-Security-Policy: script-src 'self' 'sha256-<theme bootstrap>';
  img-src 'self' data: blob:; object-src 'none'; frame-ancestors 'none';
  base-uri 'none'
```

The only inline script is the theme bootstrap (frontend spec §8), whose hash is
computed at build time; a page with no inline script (the static setup page,
§3.1) leaves the hash out. One layer over the whole router sets the header on
every `text/html` response, so no page can be added without it. Together with the markdown pipeline (no raw HTML,
frontend spec §6.4) this makes agent output unable to run script in the UI.

## 8. API

| Method & path | Purpose |
|---|---|
| `GET /api/capabilities` | `{mode: full \| gateway, features[]}` |
| `POST /api/setup` | One-time owner setup |
| `GET /setup`, `GET /setup.js` | Static setup page, until the frontend (§3.1) |
| `POST /api/auth/login` / `logout` | Password login / logout |
| `POST /api/auth/passkeys/{register,login}/{start,finish}` | WebAuthn ceremonies (register: step-up) |
| `GET /api/auth/passkeys`, `DELETE /api/auth/passkeys/{id}` | List passkeys (label, created, last used); remove (step-up) |
| `POST /api/auth/step-up/password`, `…/step-up/passkey/{start,finish}` | Step-up (§3.4) |
| `GET/DELETE /api/auth/sessions[/{id}]` | Signed-in devices (revoke: step-up) |
| `GET/PATCH /api/settings` | `public_url` (step-up), contact, push defaults |
| `POST /api/hosts/pairing-codes` | Mint a pairing code (step-up) → 201 `{code, expires_at}`, or 409 `too_many_codes` (§4.1) |
| `POST /api/hosts/enroll` | Host enrollment (code-authenticated, §4.1) → 201 `{host_id}` |
| `GET /api/hosts`, `PATCH/DELETE /api/hosts/{id}` | List, rename/default hat, revoke (step-up) |
| `GET /api/hosts/ws` | Host WebSocket (ACP core) |
| `GET/POST /api/hats`, `PATCH /api/hats/{id}` | Hats |
| `GET/PUT /api/hats/{id}/logo` | Sanitised logo (§5.1) |
| `POST /api/hats/{id}/purge` | Purge a hat (step-up, §5.5) |
| `GET/PUT /api/hosts/{id}/path-rules` | Path rules (full set) |
| `POST /api/hats/resolve` | `{host_id, path}` → `{canonical, hat_id, rule_id?}` |
| `GET /api/push/vapid`, `POST/DELETE /api/push/subscriptions` | Push |
| `GET /healthz`, `GET /readyz` | Process up (200 `ok`) / the database answers within 2 s (200 `ready`, else 503 `not ready`); plain text, on every listener |

`GET /api/hosts` lists every paired host, revoked ones included, oldest first,
as `HostItem {host_id, name, platform, host_version, capabilities, connected,
created_at, last_seen_at?, revoked_at?}`; `connected` means connected,
reconciled and not being kicked. `DELETE /api/hosts/{id}` answers 200
`HostItem`, or 404.

*Built so far:* the auth, passkey, host and health routes, `POST /api/setup`
and the setup page. No `/api/capabilities`, `/api/settings` (`public_url`
changes only through `hennery admin reset-public-url`, §4.2),
`PATCH /api/hosts/{id}`, hats, path rules or push yet.

## 9. Backups

`hennery backup <file>` (via the admin socket, TTY confirmation) writes a
tarball containing a consistent copy of `hennery.db` (SQLite online backup API),
`attachments/`, `master.key` and `vapid.key`, mode 0600, with a warning that it
contains every secret hennery holds. `hennery restore <file>` refuses to overwrite
an existing data directory without `--force`.

## 10. Threat model and deployment

- **In the default `hennery up` install the collector runs as the same OS user
  as every agent.** Any agent can therefore read `master.key`, `hennery.db` (all
  transcripts and every encrypted grant, which `master.key` decrypts) and use
  `admin.sock`. Hats do not change this (umbrella §8.4).
- **Recommendation:** whenever the gateway holds credentials for more than one
  hat, run the collector as a separate OS user or in a container (the Docker
  image), with the hosts paired to it like any remote host.
- **`hennery up` warns** at start, in Settings and in `doctor` when gateway
  credentials exist for more than one hat and the collector shares its OS user
  with the host child.
- The admin socket's TTY confirmation (§4.2) protects against accidents, not
  against a local process of the same user. The socket is operator-equivalent
  for any process of the collector's user: no password or session is needed to
  reset credentials, mint codes or read the setup link.
- Pairing codes are stored as unsalted SHA-256: 40 bits hash quickly, so this
  keeps a code out of a casual look at the database, not from a reader of a
  live `hennery.db`, who runs as the collector's user and holds every other
  secret anyway. The data directory is created 0700, and `hennery.db` with its
  `-wal`/`-shm` is made 0600 on every open, without following symlinks; a
  loose existing directory is warned about.
- The `hello` proof is not bound to the collector's identity: a local relay
  that forwards the upgrade and its nonce between a host and the real
  collector gets a valid `hello` through. TLS with the collector's certificate
  checked (`wss://`) is what closes this (ACP core §3.5).
- **`master.key` stays a file (0600) in v1, not an OS keystore entry**
  (decided 2026-09-27). A keystore would stop same-user agents from reading
  the file but not from asking the running collector through `admin.sock` or
  the database it decrypts, and it breaks headless and container installs. The
  separate OS user or container above is the real protection.

## 11. Testing

- Setup token single use and expiry; restart invalidates it; the link is
  printed only to a TTY, and appears in neither output stream otherwise.
- Login rate limiting; constant-time failure path, by counting the Argon2
  verifies per attempt (wall-clock timing is not asserted). A login checked
  before a password reset opens no session.
- A route table pins every operator route behind the browser rules and the
  cookie, and every exempt one.
- Passkey flows with a software authenticator (`webauthn-authenticator-rs`'s
  `SoftPasskey`, from the `webauthn-rs` project), at the kernel level and over
  HTTP: registration, login, step-up, a counter that goes back, another
  origin, a missing user verification. A browser's JSON and a real
  authenticator's scoping by RP id are not covered.
- Owner filter: every SQL statement of the operator, passkeys, host registry
  and sessions store is prepared under SQLite's authorizer; each table it
  touches must read its owner column, one compared with a parameter, every
  `INSERT` names `owner_id`, and the owner column is never returned or copied.
  A file with SQL outside the list fails unless exempted with a reason. A
  second owner's rows are invisible to, and unchanged by, every component.
- Admin socket: a live socket refused, a stale one replaced, an unknown or
  oversized request refused; confirmations driven through a pty, the
  password's echo off, a non-terminal refused; a `public_url` reset takes
  effect without a restart.
- `Origin` rules per route class (§3.3), including a missing `Origin`, on
  every listener; start fails when one of several addresses is taken.
- Step-up: every listed action refused without a fresh check, accepted within
  5 minutes, refused after.
- CSP header present on every HTML response; the theme bootstrap hash matches
  the built file.
- Egress: redirects not followed; private, link-local and metadata addresses
  refused; internal-network allowance honoured only where granted.
- Pairing: expiry, single use, per-address rate limiting without invalidating
  other codes; idempotent re-join; revoked host rejected in `hello`. Every
  typed spelling of a code; death at exactly 600 s; a parallel burst gets no
  more than five attempts; a forged, replayed or unsolicited `hello` gets
  `bad_proof`; a revoke while connected or mid-handshake never reattaches;
  re-join after a revoke moves the outbox aside; `https://` refused before a
  code is spent.
- Hat resolution table tests: segment match, longest prefix, default fallback,
  symlinked paths (canonical on host), unverified rules.
- Logo sanitisation: scripts, event handlers and external references removed.
- Purge: sessions, gateway rows and rules removed; `forget_hat` sent after
  handshakes.
- Push: 410 removes the subscription; non-2xx logged; generic title honoured.
- Migration from every released schema version (fixtures kept per release).

## 12. Open questions

None open. Resolved by the maintainer on 2026-09-27:

1. **Owner contact for VAPID** — not asked at setup; derived from
   `public_url` unless set in Settings (§6).
2. **Multiple listeners** — supported in v1; browser access stays bound to
   `public_url` (§7).
3. **`master.key` in the OS keystore** — no; a file in v1 (§10).
