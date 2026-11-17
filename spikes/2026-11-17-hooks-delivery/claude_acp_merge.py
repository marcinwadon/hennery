"""Measure whether Claude's programmatic `settings.hooks` (passed in
`_meta.claudeCode.options.settings`) run alongside hooks from a settings
file, or replace them. Uses a project `.claude/settings.json` so the
operator's ~/.claude is untouched. Throwaway spike code."""
import glob
import json
import os
import subprocess
import time

ROOT = "/tmp/spike-claude-merge"
PROJ = f"{ROOT}/proj"
LOG = f"{ROOT}/hooks.log"
HOOK = f"{ROOT}/hook.sh"

subprocess.run(["rm", "-rf", ROOT], check=True)
os.makedirs(f"{PROJ}/.claude")
with open(HOOK, "w") as f:
    f.write("#!/bin/sh\nIN=$(cat)\n" f'echo "$(date +%T) $1 $IN" >> {LOG}\n' "exit 0\n")
os.chmod(HOOK, 0o755)
subprocess.run(["git", "init", "-q", PROJ], check=True)
file_hooks = {"hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": f"{HOOK} FILE-SessionStart", "timeout": 10}]}],
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": f"{HOOK} FILE-PreToolUse", "timeout": 10}]}],
}}
with open(f"{PROJ}/.claude/settings.json", "w") as f:
    json.dump(file_hooks, f)
prog_hooks = {"hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": f"{HOOK} PROG-SessionStart", "timeout": 10}]}],
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": f"{HOOK} PROG-PreToolUse", "timeout": 10}]}],
}}

entry = glob.glob(os.path.expanduser(
    "~/.local/share/hennery/adapters/sets/*/claude/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"))[0]
env = {k: v for k, v in os.environ.items() if k not in ("CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT")}
p = subprocess.Popen(["node", entry], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=open(f"{ROOT}/stderr.txt", "w"), env=env, text=True, cwd=PROJ)


def send(o):
    p.stdin.write(json.dumps(o) + "\n")
    p.stdin.flush()


def pump(pred, limit=240):
    t = time.time()
    while time.time() - t < limit:
        line = p.stdout.readline()
        if not line:
            return None
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("method") == "session/request_permission":
            opts = m["params"]["options"]
            pick = next((o for o in opts if o["kind"].startswith("allow")), opts[0])
            send({"jsonrpc": "2.0", "id": m["id"], "result": {"outcome": {"outcome": "selected", "optionId": pick["optionId"]}}})
            continue
        if m.get("method") == "session/update":
            continue
        if pred(m):
            return m


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": False, "writeTextFile": False}}}})
pump(lambda m: m.get("id") == 1)
send({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {"cwd": PROJ, "mcpServers": [], "_meta": {"claudeCode": {"options": {"settings": prog_hooks, "extraArgs": {"strict-mcp-config": ""}}}}}})
r = pump(lambda m: m.get("id") == 2)
sid = r["result"]["sessionId"]
send({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "Run the shell command: echo hi. Then reply with just: done"}]}})
r = pump(lambda m: m.get("id") == 3)
print("prompt:", json.dumps(r)[:120])
p.terminate()
print("--- hook log (expect FILE-* and PROG-* for both events)")
if os.path.exists(LOG):
    for line in open(LOG):
        print(line[:40].rstrip())
else:
    print("no hook log")
