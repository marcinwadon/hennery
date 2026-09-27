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
collector over a WebSocket, with a REST/SSE API behind a development token (see
[the plan](docs/plans/2026-09-26-walking-skeleton.md)). Everything else —
real auth, hats, the MCP gateway, the frontend, distribution — is still design,
in [`docs/specs/`](docs/specs/). Expect everything to change.

Development uses the Nix flake dev shell: `nix develop` (or `direnv allow`),
then `cargo test --workspace`.

## Licence

hennery is licensed under the [GNU Affero General Public License v3.0](LICENSE).

