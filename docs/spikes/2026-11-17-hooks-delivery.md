# Spike: delivering hooks and skills to Claude Code and Codex

- **Date:** 2026-11-17
- **Answers:** whether hennery can deliver skills and hooks to a host so that
  both Claude Code and Codex sessions use them, without an interactive step on
  the host. Feeds the skills/hooks catalogue design.
- **Method:** measured against a synthetic `CODEX_HOME` and the pinned
  adapters in `~/.local/share/hennery/adapters`; Codex source read from
  `openai/codex` (`codex-rs/hooks`, `codex-rs/config`). Harness in
  [`spikes/2026-11-17-hooks-delivery/`](../../spikes/2026-11-17-hooks-delivery/).
- **Versions:** `claude-agent-acp` 0.100.0 (Claude Code 2.1.325),
  `codex-acp` 2.5.0, codex-cli 0.167.0, Linux x86_64.

## Questions

1. Do both agents read the same skill directory format, and from where?
2. Does a hook placed in the user-level Codex config run in a non-interactive
   session, given Codex's hook trust model?
3. Can hennery record that trust itself, and from the ACP side?
4. Can hennery hand Claude Code hooks per session without editing the
   operator's files?
5. Do hooks hennery passes per session run alongside the operator's own
   file-configured hooks, or replace them?

Harness files: `hooks.json` and `hook.sh` (the probe), `codex_hooks_list.py`
(app-server `hooks/list`), `codex_app_server_turn.py` (a turn through
app-server), `codex_acp_hooks.py` (the full codex-acp path),
`claude_acp_settings_hooks.py` (Claude per-session hooks) and
`claude_acp_merge.py` (file plus programmatic hooks). Each creates its own
directory under `/tmp` and symlinks only Codex's `auth.json`.

## Skills

Both agents implement the Agent Skills format (`<name>/SKILL.md` with `name`
and `description`). Fields beyond that are ignored by the other agent.

| | Claude Code | Codex |
|---|---|---|
| User directory | `~/.claude/skills` or `$CLAUDE_CONFIG_DIR/skills` | `$CODEX_HOME/skills` and `~/.agents/skills` |
| Project directory | `.claude/skills`, cwd up to repo root | `.agents/skills`, cwd up to repo root |
| Extra directories | `--add-dir` also loads that directory's `.claude/skills` | none |
| Symlinked skill folders | followed | followed |
| Product metadata | ~20 frontmatter extensions, unknown fields ignored | `agents/openai.yaml` (`interface`, `policy.allow_implicit_invocation`) |

Claude Code 2.1.325 does **not** scan `~/.agents/skills`; in its binary the
path appears only in the Cursor import table. Tools such as `npx skills`
therefore symlink `~/.agents/skills/<name>` into `~/.claude/skills/<name>`.
A per-host delivery is one hennery-owned skill store plus a symlink per agent
directory.

## Hooks

### Codex: untrusted hooks are skipped silently

A `hooks.json` with a `SessionStart` and a `PreToolUse` (`Bash`) command hook
was placed in a synthetic `CODEX_HOME` (only `auth.json` symlinked).

| Run | Hook fired | Notes |
|---|---|---|
| `codex exec`, no trust recorded | no | no warning in exec output |
| `codex exec --dangerously-bypass-hook-trust` | yes, both | `permission_mode: bypassPermissions` in hook input |
| `codex exec` with `hooks.state` trust in `config.toml` | yes, both | `additionalContext` from SessionStart reached the model |
| `codex app-server` `thread/start` + `turn/start`, same trust | yes, both | app-server emits `hook/started` / `hook/completed` notifications |

### Codex: trust is a config.toml entry hennery can write

`hooks/list` on the app-server returns, per hook, a `key` and a
`currentHash`:

```
key:         /tmp/spike-codex-hooks/home/hooks.json:pre_tool_use:0:0
currentHash: sha256:91076c10e02983ac56fb5ce54def42cd0da301bd825c03c6c377ab6a1c24fc0c
```

The key is `<absolute source path>:<event snake_case>:<group index>:<handler
index>`. The hash is `version_for_toml` over the normalised handler
(`codex-rs/hooks/src/engine/discovery.rs`, `hook_hash`). Trust is recorded as:

```toml
[hooks.state."/tmp/spike-codex-hooks/home/hooks.json:pre_tool_use:0:0"]
trusted_hash = "sha256:9107…fc0c"
```

Writing that block by hand and starting a fresh `codex exec` made the hook
run. Codex's own `/hooks` review does the same through `config/batchWrite`
with `key_path = "hooks.state"`. A changed handler changes the hash and is
skipped again until re-trusted.

### Codex: codex-acp can inject hooks per adapter process and trust them

`codex-acp` reads `CODEX_CONFIG` (JSON) at startup. If it contains `hooks`,
the adapter starts `codex app-server -c hooks=<toml>`; those hooks have
`source = "sessionFlags"`. The adapter exposes two ACP extension methods:

- `_codex/hooks/list` `{cwd}`: lists only the session-flag hooks with
  `key`, `currentHash`, `trustStatus`.
- `_codex/hooks/trust` `{cwd, hooks: [{key, currentHash}]}`: writes
  `hooks.state.<key>.trusted_hash` through `config/batchWrite` into the
  user `config.toml` and reports `{trusted: bool}`.

hennery currently strips `CODEX_CONFIG` from the adapter environment
(`crates/hennery-host/src/adapter.rs`); it would set it deliberately instead.

Measured end to end (`codex_acp_hooks.py`): the pinned `codex-acp` started
with `CODEX_CONFIG={"hooks": …}` and a synthetic `CODEX_HOME`; over ACP,
`initialize`, then `_codex/hooks/list`, `_codex/hooks/trust`, `session/new`,
`session/prompt`.

| Step | Result |
|---|---|
| `_codex/hooks/list` | both hooks listed, `source: "sessionFlags"`, keys `/<session-flags>/config.toml:pre_tool_use:0:0` and `…:session_start:0:0`, `trustStatus: "untrusted"` |
| `_codex/hooks/trust` with `{key, currentHash}` | `{"trusted": true}`; two `[hooks.state."/<session-flags>/config.toml:…"]` tables with `trusted_hash` appeared in the synthetic `config.toml` |
| session | both hooks fired; the `SessionStart` `additionalContext` reached the model |

The session-flag key carries no file path, so it is stable across sessions
and hosts for the same hook position; trust entries accumulate only when a
handler's hash changes.

### Claude Code: hooks through programmatic settings work per session

`session/new` with
`_meta.claudeCode.options.settings = {"hooks": {...}}` (same JSON shape as
`settings.json`) against the pinned adapter: both hooks fired, the
`SessionStart` `additionalContext` reached the model, and nothing was written
under `~/.claude`. The adapter also loads `settingSources`
`["user", "project", "local"]`, so the operator's own `settings.json` hooks
already run in hennery sessions.

Programmatic hooks add to file hooks, they do not replace them
(`claude_acp_merge.py`): with a `SessionStart` and a `PreToolUse` hook in
the project's `.claude/settings.json` and a different pair in the
programmatic `settings`, all four fired in one session.

### Shared contract

The two hook systems share the JSON config shape (event → matcher group →
handlers), the stdin JSON (`hook_event_name`, `tool_name`, `tool_input`,
`cwd`, `session_id`), exit code 2 as block, and
`hookSpecificOutput.{permissionDecision, additionalContext}`. Common events:
`SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`,
`PermissionRequest`, `PreCompact`, `PostCompact`, `SubagentStart`,
`SubagentStop`. Codex ignores `UserPromptSubmit` and `Stop`, fires
`PreToolUse` for `Bash` only, and has no skill-frontmatter hooks.

## Consequences for the design

- One catalogue entry can serve both agents: skills as a directory, hooks as
  one JSON hook set rendered to each agent.
- Per-host delivery of skills is file placement plus symlinks. Per-host
  delivery of hooks for Codex is `hooks.json` (or `CODEX_CONFIG` per adapter
  process) plus a `hooks.state` trust entry the host writes from the
  `hooks/list` hash; for Claude it is a per-session `settings` object.
- Hooks with `permissionDecision: allow` bypass hennery's operator
  permission flow. The catalogue should treat hook sets as a stronger
  capability than skills.
