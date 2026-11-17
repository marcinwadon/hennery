"""Measure the codex-acp hook path end to end: CODEX_CONFIG with hooks,
`_codex/hooks/list`, `_codex/hooks/trust`, then a session whose hooks must
fire. Uses a synthetic CODEX_HOME (only auth.json symlinked) so nothing in
the operator's ~/.codex changes. Throwaway spike code."""
import glob
import json
import os
import subprocess
import sys
import time

ROOT = "/tmp/spike-codex-acp"
HOME = f"{ROOT}/home"
PROJ = f"{ROOT}/proj"
LOG = f"{ROOT}/hooks.log"
HOOK = f"{ROOT}/hook.sh"

subprocess.run(["rm", "-rf", ROOT], check=True)
os.makedirs(HOME)
os.makedirs(PROJ)
os.symlink(os.path.expanduser("~/.codex/auth.json"), f"{HOME}/auth.json")
with open(HOOK, "w") as f:
    f.write(
        "#!/bin/sh\nIN=$(cat)\n"
        f'echo "$(date +%T) $1 $IN" >> {LOG}\n'
        'if [ "$1" = "SessionStart" ]; then printf \'{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"MARKER_HOOK_CONTEXT_9152"}}\'; fi\n'
        "exit 0\n"
    )
os.chmod(HOOK, 0o755)
subprocess.run(["git", "init", "-q", PROJ], check=True)

hooks = {
    "SessionStart": [{"hooks": [{"type": "command", "command": f"{HOOK} SessionStart", "timeout": 10}]}],
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": f"{HOOK} PreToolUse", "timeout": 10}]}],
}
entry = glob.glob(os.path.expanduser(
    "~/.local/share/hennery/adapters/sets/*/codex/node_modules/@agentclientprotocol/codex-acp/dist/index.js"))[0]
env = {k: v for k, v in os.environ.items() if k not in ("CODEX_CONFIG", "CODEX_PATH")}
env["CODEX_HOME"] = HOME
env["CODEX_CONFIG"] = json.dumps({"hooks": hooks})
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
            print("permission ->", pick["optionId"])
            continue
        if m.get("method") == "session/update":
            u = m["params"]["update"]
            if u.get("sessionUpdate") == "agent_message_chunk":
                print("agent:", u["content"].get("text", "")[:120].replace("\n", " "))
            continue
        if pred(m):
            return m


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": False, "writeTextFile": False}}}})
r = pump(lambda m: m.get("id") == 1)
print("init ok:", r is not None and "result" in r)
send({"jsonrpc": "2.0", "id": 2, "method": "_codex/hooks/list", "params": {"cwd": PROJ}})
r = pump(lambda m: m.get("id") == 2)
print("hooks/list:", json.dumps(r)[:1500])
listed = r["result"]["hooks"] if r and "result" in r else []
if listed:
    send({"jsonrpc": "2.0", "id": 3, "method": "_codex/hooks/trust", "params": {"cwd": PROJ, "hooks": [{"key": h["key"], "currentHash": h["currentHash"]} for h in listed]}})
    r = pump(lambda m: m.get("id") == 3)
    print("hooks/trust:", json.dumps(r)[:500])
send({"jsonrpc": "2.0", "id": 4, "method": "session/new", "params": {"cwd": PROJ, "mcpServers": []}})
r = pump(lambda m: m.get("id") == 4)
print("session/new ok:", r is not None and "result" in r, json.dumps(r)[:200] if r and "error" in r else "")
sid = r["result"]["sessionId"]
send({"jsonrpc": "2.0", "id": 5, "method": "session/prompt", "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "Reply with the exact token after MARKER_HOOK_CONTEXT from your context, or NONE. Then run the shell command: echo hi"}]}})
r = pump(lambda m: m.get("id") == 5)
print("prompt:", json.dumps(r)[:200])
p.terminate()
print("--- hook log")
if os.path.exists(LOG):
    for line in open(LOG):
        print(line[:70].rstrip())
else:
    print("no hook log")
print("--- config.toml")
cfg = f"{HOME}/config.toml"
print(open(cfg).read() if os.path.exists(cfg) else "no config.toml")
