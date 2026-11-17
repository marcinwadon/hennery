import json, subprocess, sys, os, time
env = dict(os.environ, CODEX_HOME="/tmp/spike-codex-hooks/home")
p = subprocess.Popen(["codex","app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, text=True, cwd="/tmp/spike-codex-hooks/proj")
def send(o): p.stdin.write(json.dumps(o)+"\n"); p.stdin.flush()
def wait(id_):
    while True:
        line = p.stdout.readline()
        if not line: return None
        try: m = json.loads(line)
        except: continue
        if m.get("id") == id_: return m
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"spike","title":"spike","version":"0.0.1"}}})
r = wait(1); print("init:", json.dumps(r)[:300])
send({"jsonrpc":"2.0","method":"initialized","params":{}})
send({"jsonrpc":"2.0","id":2,"method":"hooks/list","params":{"cwds":["/tmp/spike-codex-hooks/proj"]}})
r = wait(2); print(json.dumps(r, indent=1)[:4000])
p.terminate()
