// Opt-in exact-pin acceptance: actual ACP adapter/CLI and MCP calls against a
// synthetic local model provider. No account credentials or operator config.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, realpathSync, rmSync } from 'node:fs'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { resolve, join } from 'node:path'
import { createInterface } from 'node:readline'

assert(process.argv[2], 'supply node_modules installed from adapters/claude/package-lock.json')
const modules = resolve(process.argv[2])
const output = resolve(process.argv[3] ?? '.superpowers/mcp-source-acceptance')
mkdirSync(output, { recursive: true })
for (const [name, version] of [
  ['@agentclientprotocol/claude-agent-acp', '0.93.0'],
  ['@anthropic-ai/claude-agent-sdk', '0.3.300'],
]) {
  assert.equal(JSON.parse(readFileSync(join(modules, name, 'package.json'), 'utf8')).version, version)
}
const home = realpathSync(mkdtempSync(join(tmpdir(), 'hennery-mcp-source-proof-')))
const project = join(home, 'project'), config = join(home, 'claude')
mkdirSync(project)
mkdirSync(config)
let calls = [], apiCalls = 0, currentNonce = 'source-policy-proof'

// MCP endpoints record which definition actually received the request, including
// credentials that must never be merged across a colliding server definition.
const server = createServer(async (req, res) => {
  try {
    let raw = ''
    for await (const part of req) raw += part
    const body = raw ? JSON.parse(raw) : {}
    if (req.url.startsWith('/mcp/')) {
      const name = req.url.slice(5)
      calls.push({ server: name, method: body.method, nonce: body.params?.arguments?.nonce,
        auth: req.headers.authorization, alien: req.headers['x-native-only'] })
      if (body.id === undefined) { res.writeHead(202); res.end(); return }
      let result = {}
      if (body.method === 'initialize') result = {
        protocolVersion: body.params.protocolVersion, capabilities: { tools: {} },
        serverInfo: { name, version: '1' },
      }
      if (body.method === 'tools/list') result = { tools: [{
        name: 'marker', description: `Return ${name} marker`, annotations: { readOnlyHint: true },
        inputSchema: { type: 'object', properties: { nonce: { type: 'string' } }, required: ['nonce'] },
      }] }
      if (body.method === 'tools/call') result = { content: [{ type: 'text',
        text: JSON.stringify({ server: name, nonce: body.params.arguments.nonce }),
      }] }
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ jsonrpc: '2.0', id: body.id, result }))
      return
    }
    if (req.url.includes('count_tokens')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end('{"input_tokens":1}')
      return
    }
    if (req.url.includes('/messages')) {
      apiCalls++
      assert(apiCalls < 40, 'bounded synthetic provider calls')
      const done = body.messages?.some(m => Array.isArray(m.content)
        && m.content.some(c => c.type === 'tool_result' && JSON.stringify(c).includes(currentNonce)))
      const names = (body.tools ?? []).filter(t => t.name.startsWith('mcp__')).map(t => t.name)
      const content = done || !names.length
        ? [{ type: 'text', text: 'Fixture complete.' }]
        : names.map((name, i) => ({ type: 'tool_use', id: `proof_${apiCalls}_${i}`, name,
          input: { nonce: currentNonce } }))
      const stop = content[0].type === 'tool_use' ? 'tool_use' : 'end_turn'
      const message = { id: `msg_${apiCalls}`, type: 'message', role: 'assistant', model: body.model,
        content, stop_reason: stop, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 10 } }
      if (!body.stream) {
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end(JSON.stringify(message))
        return
      }
      res.writeHead(200, { 'content-type': 'text/event-stream' })
      const event = (type, data) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`)
      event('message_start', { message: { ...message, content: [], stop_reason: null,
        usage: { input_tokens: 10, output_tokens: 0 } } })
      content.forEach((c, index) => {
        event('content_block_start', { index,
          content_block: c.type === 'tool_use' ? { ...c, input: {} } : { type: 'text', text: '' } })
        event('content_block_delta', { index, delta: c.type === 'tool_use'
          ? { type: 'input_json_delta', partial_json: JSON.stringify(c.input) }
          : { type: 'text_delta', text: c.text } })
        event('content_block_stop', { index })
      })
      event('message_delta', { delta: { stop_reason: stop, stop_sequence: null }, usage: { output_tokens: 10 } })
      event('message_stop', {})
      res.end()
      return
    }
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end('{}')
  } catch (error) { console.error(error); res.writeHead(500); res.end() }
})
await new Promise(done => server.listen(0, '127.0.0.1', done))
const base = `http://127.0.0.1:${server.address().port}`
const native = { mcpServers: {
  'hennery-probe': { type: 'http', url: base + '/mcp/collision', headers: { 'X-Native-Only': 'native-secret' } },
  native: { type: 'http', url: base + '/mcp/native' },
} }
writeFileSync(join(home, '.claude.json'), JSON.stringify(native))
writeFileSync(join(config, '.claude.json'), JSON.stringify(native))
writeFileSync(join(config, 'settings.json'), JSON.stringify({ enableAllProjectMcpServers: true }))
writeFileSync(join(project, '.mcp.json'), JSON.stringify({ mcpServers: {
  'hennery-probe': { type: 'http', url: base + '/mcp/project-collision',
    headers: { 'X-Native-Only': 'project-secret' } },
  'project-native': { type: 'http', url: base + '/mcp/project-native' },
} }))

let seq = 0, pending = new Map(), stderr = '', child, lines
function launch() {
  seq = 0; pending = new Map()
  child = spawn(process.execPath, [join(modules, '@agentclientprotocol/claude-agent-acp/dist/index.js')], {
    cwd: project, detached: true, stdio: ['pipe', 'pipe', 'pipe'], env: {
      PATH: process.env.PATH, HOME: home, CLAUDE_CONFIG_DIR: config,
      XDG_CONFIG_HOME: join(home, 'xdg-config'), XDG_DATA_HOME: join(home, 'xdg-data'),
      XDG_CACHE_HOME: join(home, 'xdg-cache'), XDG_STATE_HOME: join(home, 'xdg-state'),
      ANTHROPIC_API_KEY: 'synthetic-fixture-key', ANTHROPIC_BASE_URL: base,
      ENABLE_TOOL_SEARCH: 'false', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
      DISABLE_TELEMETRY: '1', DISABLE_ERROR_REPORTING: '1',
    },
  })
  child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-16000) })
  lines = createInterface({ input: child.stdout })
  lines.on('line', line => {
    try {
      const msg = JSON.parse(line)
      if (msg.method && msg.id !== undefined) {
        let result = {}
        if (msg.method === 'session/request_permission') result = { outcome: { outcome: 'selected',
          optionId: msg.params.options.find(o => o.kind === 'allow_once')?.optionId ?? msg.params.options[0].optionId } }
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }) + '\n')
      } else if (pending.has(msg.id)) {
        const entry = pending.get(msg.id)
        pending.delete(msg.id)
        msg.error ? entry.reject(Error(JSON.stringify(msg.error))) : entry.resolve(msg.result)
      }
    } catch (error) { console.error(error) }
  })
}
async function stopChild() {
  if (!child || child.exitCode !== null || child.signalCode !== null) { lines?.close(); return }
  const exited = new Promise(done => child.once('exit', done))
  try { process.kill(-child.pid, 'SIGTERM') } catch {}
  const timer = setTimeout(() => { try { process.kill(-child.pid, 'SIGKILL') } catch {} }, 2000)
  await exited
  clearTimeout(timer)
  lines.close()
}
function request(method, params) {
  return new Promise((resolve, reject) => {
    const id = ++seq
    pending.set(id, { resolve, reject })
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
  })
}
const timer = setTimeout(() => {
  for (const entry of pending.values()) entry.reject(Error('runtime bound exceeded'))
  if (child) { try { process.kill(-child.pid, 'SIGKILL') } catch {} }
}, 55_000)
try {
  const servers = [{ type: 'http', name: 'hennery-probe', url: base + '/mcp/managed',
    headers: [{ name: 'Authorization', value: 'Bearer synthetic-managed' }] }]
  let sessionId
  const results = []
  for (const [method, mode, managed] of [
    ['session/new', 'strict', true], ['session/load', 'strict', true],
    ['session/load', 'combined', true], ['session/new', 'combined', true],
    ['session/load', 'strict', true], ['session/new', 'strict', false],
    ['session/new', 'combined', false], ['session/load', 'combined', false],
  ]) {
    launch()
    await request('initialize', { protocolVersion: 1, clientCapabilities: {},
      clientInfo: { name: 'hennery-source-proof', version: '1' } })
    calls = []; currentNonce = `source-policy-${results.length + 1}`
    const meta = { claudeCode: { options: mode === 'strict'
      ? { extraArgs: { 'strict-mcp-config': '' } } : { strictMcpConfig: false } } }
    const response = await request(method, { cwd: project, mcpServers: managed ? servers : [], _meta: meta,
      ...(method === 'session/load' ? { sessionId } : {}) })
    if (method === 'session/new') sessionId = response.sessionId
    const answer = await request('session/prompt', { sessionId,
      prompt: [{ type: 'text', text: `Use every MCP marker tool with nonce ${currentNonce}.` }] })
    const used = calls.filter(c => c.method === 'tools/call')
    assert(calls.filter(c => c.server !== 'managed').every(c => c.auth !== 'Bearer synthetic-managed'))
    if (managed) {
      assert(used.some(c => c.server === 'managed' && c.nonce === currentNonce))
      assert(!calls.some(c => c.server === 'collision' || c.server === 'project-collision'))
      assert(calls.filter(c => c.server === 'managed').every(c => c.auth === 'Bearer synthetic-managed' && !c.alien))
    }
    if (mode === 'strict') assert(calls.every(c => c.server === 'managed'))
    else {
      assert(used.some(c => c.server === 'native' && c.nonce === currentNonce))
      assert(used.some(c => c.server === 'project-native' && c.nonce === currentNonce))
    }
    if (!managed) assert(!calls.some(c => c.server === 'managed' || c.auth === 'Bearer synthetic-managed'))
    assert.equal(answer.stopReason, 'end_turn')
    await request('session/close', { sessionId })
    await stopChild()
    results.push({ method, mode, managed, nonce: currentNonce, calls })
    console.log(JSON.stringify({ case: results.length, method, mode, managed, toolCalls: used.length }))
  }
  writeFileSync(join(output, 'runtime-matrix.json'), JSON.stringify({ results, apiCalls, stderr }, null, 2))
  console.log('8 runtime cases passed; real ACP/CLI MCP calls, synthetic model provider; no account-connector proof')
} finally {
  clearTimeout(timer)
  await stopChild()
  server.closeAllConnections()
  server.close()
  rmSync(home, { recursive: true, force: true })
}
