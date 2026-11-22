"""Isolated authenticated ACP v2 smoke experiment; raw evidence stays in /tmp."""
import glob,json,os,subprocess,tempfile,threading,time,queue

root=tempfile.mkdtemp(prefix='hennery-v2-live-')
print('Evidence:',root,flush=True)
for agent,version in [('codex-acp',os.environ.get('ACP_CODEX_VERSION','2.7.0')),('claude-agent-acp',os.environ.get('ACP_CLAUDE_VERSION','0.107.3'))]:
    if os.environ.get('ACP_AGENT') and os.environ['ACP_AGENT'] != agent:continue
    folder=os.path.join(root,agent);os.makedirs(folder)
    home=os.path.join(folder,'home');os.makedirs(home)
    base=os.environ.get('ACP_PACKAGES',os.path.expanduser('~/.local/share/hennery/adapters/sets/*/*'))
    packages=glob.glob(base+'/node_modules/@agentclientprotocol/'+agent+'/package.json')
    pkg=next(p for p in packages if json.load(open(p))['version']==version)
    env=os.environ.copy()
    for key in ('CLAUDECODE','CLAUDE_CODE_ENTRYPOINT','CLAUDE_CODE_SSE_PORT','CODEX_CONFIG','CODEX_PATH'):
        env.pop(key,None)
    if agent=='codex-acp':
        os.symlink(os.path.expanduser('~/.codex/auth.json'),home+'/auth.json')
        env['CODEX_HOME']=home
    else:
        # Preserve login access; isolate project settings and session output.
        env['CLAUDE_CONFIG_DIR']=home
        for name in ('.credentials.json',):
            source=os.path.expanduser('~/.claude/'+name)
            if os.path.isfile(source):os.symlink(source,home+'/'+name)
    q=queue.Queue();events=[]
    err=open(folder+'/stderr.log','w')
    p=subprocess.Popen(['node',os.path.dirname(pkg)+'/dist/index.js'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=err,text=True,env=env,cwd=folder,start_new_session=True)
    def send(m):p.stdin.write(json.dumps(m)+'\n');p.stdin.flush()
    def reader(proc=p,output=q,log=events):
        for line in proc.stdout:
            try:m=json.loads(line)
            except ValueError:continue
            log.append(m);output.put(m)
    threading.Thread(target=reader,daemon=True).start()
    seq=0
    def call(method,params,timeout=60):
        global seq
        seq+=1;ident=seq;send(dict(jsonrpc='2.0',id=ident,method=method,params=params))
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            try:m=q.get(timeout=min(1,max(.01,end-time.monotonic())))
            except queue.Empty:continue
            if m.get('id')==ident and 'method' not in m:return m
            if 'id' in m and 'method' in m:
                send(dict(jsonrpc='2.0',id=m['id'],error=dict(code=-32601,message='unsupported by experiment')))
        return {'error':{'message':'experiment timeout','method':method}}
    try:
        init=call('initialize',dict(protocolVersion=2,info=dict(name='hennery-spike',version='0'),capabilities={}))
        new=call('session/new',dict(cwd=folder,mcpServers=[]))
        result={'agent':agent,'initialize':init,'new':new}
        sid=new.get('result',{}).get('sessionId')
        if sid:
            prompt=os.environ.get('ACP_PROMPT','In this temporary directory only, start two background shell jobs: sleep 8 then exit 0, and sleep 12 then exit 7. Use native background execution. Reply briefly immediately after starting both; do not wait or poll. Do not modify files, access credentials, network, or other directories.')
            result['prompt']=call('session/prompt',dict(sessionId=sid,prompt=[dict(type='text',text=prompt)]))
            end=time.monotonic()+35
            cancelled=False
            while time.monotonic()<end:
                try:m=q.get(timeout=1)
                except queue.Empty:continue
                if 'id' in m and 'method' in m:send(dict(jsonrpc='2.0',id=m['id'],error=dict(code=-32601,message='unsupported by experiment')))
                update=m.get('params',{}).get('update',{})
                bg=update.get('background') or []
                if os.environ.get('ACP_CANCEL') and bg and not cancelled:
                    cancelled=True
                    send(dict(jsonrpc='2.0',method='session/cancel',params=dict(sessionId=sid)))
                    result['foreground_cancel']='notification sent; observe subsequent states'
                    terminal=next((b.get('terminalId') for b in bg if b.get('type')=='terminal'),None)
                    if terminal:result['terminal_cancel']=call('terminal/cancel',dict(sessionId=sid,terminalId=terminal))
            result['status']=call('session/status',dict(sessionId=sid))
            result['close']=call('session/close',dict(sessionId=sid))
            result['resume']=call('session/resume',dict(sessionId=sid,cwd=folder,mcpServers=[]))
            if 'result' in result['resume']:
                result['close_after_resume']=call('session/close',dict(sessionId=sid))
        result['updates']=[m.get('params',{}).get('update') for m in events if m.get('method')=='session/update']
        json.dump(result,open(folder+'/result.json','w'),indent=2)
        print(agent,json.dumps({'created':bool(sid),'error':new.get('error'),'updates':len(result['updates'])}),flush=True)
    finally:
        import signal
        try:os.killpg(p.pid,signal.SIGTERM)
        except ProcessLookupError:pass
        try:p.wait(timeout=5)
        except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait()
        err.close()
