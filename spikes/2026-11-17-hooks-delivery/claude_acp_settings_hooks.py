import json, subprocess, os, time, glob
entry = glob.glob(os.path.expanduser("~/.local/share/hennery/adapters/sets/*/claude/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"))[0]
node = glob.glob(os.path.expanduser("~/.local/share/hennery/adapters/sets/*/claude/node_modules/.bin/claude-agent-acp"))
env = {k:v for k,v in os.environ.items() if k not in ("CLAUDECODE","CLAUDE_CODE_ENTRYPOINT","CLAUDE_CODE_SSE_PORT")}
p = subprocess.Popen(["node", entry], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open("/tmp/spike-claude-hooks/out/stderr.txt","w"), env=env, text=True, cwd="/tmp/spike-claude-hooks/proj")
def send(o): p.stdin.write(json.dumps(o)+"\n"); p.stdin.flush()
def pump(pred, limit=240):
    t=time.time()
    while time.time()-t<limit:
        line=p.stdout.readline()
        if not line: return None
        try: m=json.loads(line)
        except: continue
        if m.get("method")=="session/request_permission":
            opts=m["params"]["options"]; pick=next((o for o in opts if o["kind"].startswith("allow")), opts[0])
            send({"jsonrpc":"2.0","id":m["id"],"result":{"outcome":{"outcome":"selected","optionId":pick["optionId"]}}})
            print("permission ->", pick["optionId"]); continue
        if m.get("method")=="session/update":
            u=m["params"]["update"]
            if u.get("sessionUpdate")=="agent_message_chunk": print("agent:", u["content"].get("text","")[:200])
            continue
        if pred(m): return m
settings={"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"/tmp/spike-claude-hooks/hook.sh SessionStart","timeout":10}]}],
                   "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/tmp/spike-claude-hooks/hook.sh PreToolUse","timeout":10}]}]}}
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":False,"writeTextFile":False}}}})
r=pump(lambda m:m.get("id")==1); print("init:", json.dumps(r)[:200])
send({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp/spike-claude-hooks/proj","mcpServers":[],"_meta":{"claudeCode":{"options":{"settings":settings,"extraArgs":{"strict-mcp-config":""}}}}}})
r=pump(lambda m:m.get("id")==2); print("session/new:", json.dumps(r)[:300])
sid=r["result"]["sessionId"]
send({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":"Reply with the exact token after MARKER_HOOK_CONTEXT from your context, or NONE. Then run the shell command: echo hi"}]}})
r=pump(lambda m:m.get("id")==3); print("prompt done:", json.dumps(r)[:200])
p.terminate()
