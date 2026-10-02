import { describe, expect, it } from 'vitest'
import { json, routed } from '../test-stream'
import { cancel, prompt, resume, setConfig } from './turns'

describe('turn routes', () => {
  it('posts the prompt, cancel, config and resume of a session, its id encoded', async () => {
    const t = routed(() => json({ turn_id: 't1' }, 202))
    await prompt(t.client, 'a/b?c', [{ type: 'text', text: 'hi' }])
    await cancel(t.client, 'a/b?c')
    await setConfig(t.client, 'a/b?c', 'model', 'fast')
    await resume(t.client, 'a/b?c')
    const bodies = t.fetch.mock.calls.map((call) => (call[1]?.body === undefined ? undefined : JSON.parse(String(call[1].body))))
    expect(t.calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      'POST /api/sessions/a%2Fb%3Fc/prompt',
      'POST /api/sessions/a%2Fb%3Fc/cancel',
      'POST /api/sessions/a%2Fb%3Fc/config',
      'POST /api/sessions/a%2Fb%3Fc/resume',
    ])
    expect(bodies).toEqual([{ content: [{ type: 'text', text: 'hi' }] }, undefined, { config_id: 'model', value: 'fast' }, undefined])
  })
})
