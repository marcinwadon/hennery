import json, subprocess, os, time
env = dict(os.environ, CODEX_HOME="/tmp/spike-codex-hooks/home")
p = subprocess.Popen(["codex","app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, text=True, cwd="/tmp/spike-codex-hooks/proj")
def send(o): p.stdin.write(json.dumps(o)+"\n"); p.stdin.flush()
def wait(pred, limit=150):
    t=time.time()
    while time.time()-t<limit:
        line = p.stdout.readline()
        if not line: return None
        try: m = json.loads(line)
        except: continue
        if pred(m): return m
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"spike","title":"spike","version":"0.0.1"}}}); wait(lambda m:m.get("id")==1)
send({"jsonrpc":"2.0","method":"initialized","params":{}})
send({"jsonrpc":"2.0","id":2,"method":"thread/start","params":{"cwd":"/tmp/spike-codex-hooks/proj","approvalPolicy":"never","sandbox":"read-only"}})
r=wait(lambda m:m.get("id")==2); print("thread/start:", json.dumps(r)[:400])
tid=r["result"]["thread"]["id"]
send({"jsonrpc":"2.0","id":3,"method":"turn/start","params":{"threadId":tid,"input":[{"type":"text","text":"Reply with the exact token after MARKER_HOOK_CONTEXT from your context, or NONE. Then run the shell command: echo hi"}]}})
msgs=[]
m=wait(lambda m: (msgs.append(m) or False) or m.get("method")=="turn/completed")
for x in msgs:
    meth=x.get("method","")
    if "hook" in meth.lower() or "item/completed" in meth: print(meth, json.dumps(x.get("params",{}))[:300])
print("completed:", json.dumps(m)[:300] if m else None)
p.terminate()
