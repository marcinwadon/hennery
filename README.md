# hennery

hennery is a self-hosted cockpit for coding agents. It lets one developer drive
[Agent Client Protocol](https://agentclientprotocol.com) (ACP) sessions — Claude
Code, Codex, or any other ACP adapter — running on any of their machines, from a
single browser tab or a phone. It also includes an MCP gateway: authorize an
integration once, then choose which hosts and projects are allowed to use it.

hennery never proxies agent licences. You bring your own subscriptions and log in
to each agent CLI on your own machines; hennery only drives the sessions.

## Status

**Pre-alpha.** Nothing is usable yet. A walking skeleton exists: a Rust
workspace where a host runs an ACP adapter per session and relays it to a
collector over a WebSocket (see
[the plan](docs/plans/2026-09-26-walking-skeleton.md)). Hosts pair with the
collector once and prove their key on every connection. The REST/SSE API is
behind the operator's session: on its first start the collector writes a
one-time setup link (valid for an hour), and the operator sets a password there
and signs in with it afterwards (see
[the operator auth plan](docs/plans/2026-10-02-operator-auth.md)). Everything
else — passkeys, hats, the MCP gateway, the frontend, distribution — is still
design, in [`docs/specs/`](docs/specs/). Expect everything to change.

Development uses the Nix flake dev shell: `nix develop` (or `direnv allow`),
then `cargo test --workspace`.

## Recovery

There is no admin command for these yet; both are done by hand, with the
collector stopped. The database is `<data-dir>/hennery.db` for
`hennery collector`, and `<data-dir>/collector/hennery.db` for `hennery up`.
Open it with `nix shell nixpkgs#sqlite -c sqlite3 <path to hennery.db>`.

- **The collector moved** (another port, or behind a proxy on another origin):
  every state-changing request, login included, is refused with
  `origin_mismatch`, because it must come from the stored `public_url`. Store
  the origin the browser now uses, e.g.
  `UPDATE settings SET value = 'https://hennery.example' WHERE key = 'public_url';`
  It must be `https://`, or `http://` to a loopback address, with no path.
  `http://localhost:7117` and `http://127.0.0.1:7117` are different origins:
  use the one in the browser's address bar.
- **A forgotten password:** remove the owner, in this order (the other tables
  point at `owners`):
  `DELETE FROM auth_sessions; DELETE FROM password_credentials; DELETE FROM settings; DELETE FROM owners;`
  On its next start the collector has no owner and writes a fresh setup link.

## Licence

hennery is licensed under the [GNU Affero General Public License v3.0](LICENSE).

