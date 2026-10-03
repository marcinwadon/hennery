# hennery

hennery is a self-hosted cockpit for coding agents. It lets one developer drive
[Agent Client Protocol](https://agentclientprotocol.com) (ACP) sessions — Claude
Code, Codex, or any other ACP adapter — running on any of their machines, from a
single browser tab or a phone. It also includes an MCP gateway: authorize an
integration once, then choose which hosts and projects are allowed to use it.

hennery never proxies agent licences. You bring your own subscriptions and log in
to each agent CLI on your own machines; hennery only drives the sessions.

## Status

**Early testing.** hennery is ready for early testers who build it from
source. There are no releases, installers or images yet, and stored data may
not survive an upgrade: expect things to change.

What works:

- **Sessions from the browser.** The collector serves its web UI from the
  binary. From it you start Claude Code and Codex sessions on any paired
  machine, prompt them, answer their permission requests, and park and
  resume them.
- **Hats.** Path rules put each session in a hat by its project directory,
  and each hat keeps its sessions and notification settings apart.
- **Push.** The browser can get Web Push notices when a session asks a
  question, finishes or fails.
- **The MCP gateway.** Connect an MCP server once, and choose the hosts
  whose sessions may use it.

Known limitations:

- One owner per collector, and a password to sign in; passkeys have an API
  but no screen yet.
- Codex sessions also load the user's own ~/.codex MCP servers until per-hat isolation lands.
- The designs are in [`docs/specs/`](docs/specs/) and the plans in
  [`docs/plans/`](docs/plans/); [`docs/README.md`](docs/README.md) lists
  them.

## Quickstart

You need [Nix](https://nixos.org/download/) with flakes enabled, and on
every machine that runs agents, the agents' own CLIs logged in (Claude Code
and Codex, each with your own subscription). hennery runs on macOS and
Linux.

### 1. Build

Get the source:

<!-- check: clone -->
```sh
git clone https://github.com/marcinwadon/hennery.git
cd hennery
```

Build the web UI, then the binary, which embeds it. The dev shell brings
Rust, Node and pnpm; with direnv, `direnv allow` once and leave out the
`nix develop -c` prefixes. `HENNERY_WEB_REQUIRE=1` makes the build fail if
the web UI was not built first, instead of embedding a placeholder page.

<!-- check: build -->
```sh
nix develop -c pnpm --dir web install --frozen-lockfile
nix develop -c pnpm --dir web build
HENNERY_WEB_REQUIRE=1 nix develop -c cargo build --release --locked -p hennery
export PATH="$PWD/target/release:$PATH"
```

Or let Nix build the whole package:

<!-- check: nix -->
```sh
nix build
./result/bin/hennery --version
```

The binary is then `result/bin/hennery`; put that directory on your `PATH`
instead.

### 2. Start

<!-- check: up -->
```sh
hennery up
```

`hennery up` runs the collector and a host for this machine, and pairs the
two. It listens on `http://127.0.0.1:7117` and keeps its data in
`~/Library/Application Support/hennery` (macOS) or `~/.local/share/hennery`
(Linux). The first start downloads the agents' adapters, about 272 MB.

On its first start it prints a one-time setup link, valid for an hour (when
its output is not a terminal, the link is in `collector/setup-url` in the
data directory). Open it, choose a password, and you are signed in. Back up
`collector/master.key` together with `collector/hennery.db`: the gateway's
credentials cannot be read without it.

### 3. Pair another machine

A host on another machine reaches the collector over HTTPS only. Put the
collector behind a reverse proxy that serves it at an `https://` address
and passes WebSocket upgrades on, and start it with that address before
you set it up:

<!-- check: up-public -->
```sh
hennery up --public-url https://hennery.example
```

In the web UI, **Hosts → Add host** shows a pairing code, valid for ten
minutes, and the command that uses it. Build hennery on the other machine,
run that command there, then start the host:

<!-- check: join -->
```sh
hennery host join https://hennery.example ABCD-EFGH
```

<!-- check: host-run -->
```sh
hennery host run
```

### 4. Start a session

**New session**: pick a host, an agent and a project directory, then
**Start session**, and type your first prompt.

MCP servers are connected, and given to hosts, on the **MCP** screen.

## Recovery

Run these on the collector's machine while hennery runs, as the user that
runs it; each asks for confirmation on the terminal. For a collector not in
the default place, add `--data-dir`.

- **A forgotten password:** set a new one. It signs out every session and
  removes every passkey.

  <!-- check: reset-password -->
  ```sh
  hennery admin reset-password
  ```

- **The collector moved** (another address, or behind a proxy): every
  sign-in and change is refused with `origin_mismatch` until hennery knows
  the address the browser now uses. Give it that origin: `https://`, or
  `http://` to a loopback address, with no path. It signs out every
  session; passkeys stop working if the host name changes.

  <!-- check: reset-public-url -->
  ```sh
  hennery admin reset-public-url https://hennery.example
  ```

## Licence

hennery is licensed under the [GNU Affero General Public License v3.0](LICENSE).

