// Offline ACP double: no SDK, CLI, account credentials or model provider.
import assert from 'node:assert/strict'
import { appendFileSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { createInterface } from 'node:readline'

const scenario = JSON.parse(readFileSync(new URL('../scenario.json', import.meta.url)))
const settings = JSON.parse(readFileSync(join(process.env.CLAUDE_CONFIG_DIR, 'settings.json')))
const mode = settings.permissions?.defaultMode ?? 'auto'
const trace = value => appendFileSync(new URL('../trace.jsonl', import.meta.url), JSON.stringify(value) + '\n')
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n')
let session, permission
createInterface({ input: process.stdin }).on('line', async line => {
  const message = JSON.parse(line)
  if (!message.method) { permission(message.result); return }
  const { id, method, params } = message
  trace({ method, mode })
  try {
    let result = {}
    if (method === 'session/new' || method === 'session/load') {
      session = params
      result = { sessionId: params.sessionId ?? 'offline-session',
        modes: { currentModeId: scenario.wrongModeOn === method ? 'auto' : mode } }
    }
    if (method === 'session/prompt') {
      assert.equal(mode, 'default', 'Auto mode cannot check synthetic-provider actions')
      const answer = new Promise(resolve => { permission = resolve })
      send({ id: 'permission', method: 'session/request_permission', params: {
        options: [{ optionId: 'deny', kind: 'reject_once' },
          { optionId: 'always', kind: 'allow_always' }, { optionId: 'once', kind: 'allow_once' }],
      } })
      assert.deepEqual(await answer, { outcome: { outcome: 'selected', optionId: 'once' } })
      trace({ permission: 'allow_once' })
      const nonce = params.prompt[0].text.match(/nonce ([^.]+)\./)[1]
      const definitions = new Map()
      if (!Object.hasOwn(session._meta.claudeCode.options.extraArgs ?? {}, 'strict-mcp-config')) {
        for (const file of [join(process.env.CLAUDE_CONFIG_DIR, '.claude.json'), join(session.cwd, '.mcp.json')]) {
          for (const [name, definition] of Object.entries(JSON.parse(readFileSync(file)).mcpServers)) {
            definitions.set(name, definition)
          }
        }
      }
      for (const definition of session.mcpServers) {
        definitions.set(definition.name, { ...definition,
          headers: Object.fromEntries(definition.headers.map(header => [header.name, header.value])) })
      }
      for (const definition of definitions.values()) {
        await fetch(definition.url, { method: 'POST',
          headers: { 'content-type': 'application/json', ...definition.headers },
          body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { arguments: { nonce } } }),
        })
      }
      result = { stopReason: 'end_turn' }
    }
    send({ id, result })
  } catch (error) {
    send({ id, error: { code: -32603, message: error.message } })
  }
})
