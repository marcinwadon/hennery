// What a test host's processes are handed (e2e/host.ts): `spawnHennery`
// runs each in `scratchEnv(dir)` plus `hostEnv(…)` (the composition
// `e2e-spawn.test.ts` pins), and the agents the host starts inherit it. So
// with the runner's own `HENNERY_*` variables set, the home and XDG
// directories must still be the host's fresh directory (its data directory
// is the platform's default under it), and the only `HENNERY_*` variable the
// fake agent's script.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { AGENT_VARS, fakeAgent, hostEnv } from '../e2e/host'
import { scratchEnv } from '../e2e/spawn'

const DIR = '/srv/work/e2e-host'
const RUNNER = {
  HENNERY_DATA_DIR: '/srv/work/runner-data',
  HENNERY_HOST_DATA_DIR: '/srv/work/runner-host',
  HENNERY_LOG_DIR: '/srv/work/runner-logs',
  HENNERY_DEV_TOKEN: 'runner-token',
}

/** The environment `spawnHennery(args, dir, extra)` gives its process. */
function handed(extra: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  return { ...scratchEnv(DIR), ...extra }
}

const henneryKeys = (env: NodeJS.ProcessEnv) => Object.keys(env).filter((k) => /^HENNERY_/i.test(k)).sort()

describe('a test host for the browser checks', () => {
  const saved: Record<string, string | undefined> = {}
  beforeEach(() => {
    for (const [key, value] of Object.entries(RUNNER)) {
      saved[key] = process.env[key]
      process.env[key] = value
    }
  })
  afterEach(() => {
    for (const key of Object.keys(RUNNER)) {
      if (saved[key] === undefined) delete process.env[key]
      else process.env[key] = saved[key]
    }
  })

  it('runs with the fake agent in its own home, and only the script beside it', () => {
    const agent = fakeAgent('claude', { chunks: ['hi'] })
    const env = handed(hostEnv(agent.agentEnv))
    expect(env.HOME).toBe(DIR)
    for (const key of ['XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME']) {
      expect(env[key]?.startsWith(`${DIR}/`)).toBe(true)
    }
    expect(henneryKeys(env)).toEqual(['HENNERY_FAKE_ACP_SCRIPT'])
    expect(JSON.parse(env.HENNERY_FAKE_ACP_SCRIPT!)).toEqual({ chunks: ['hi'] })
    expect(agent.agent).toMatch(/^claude=\S+\/hennery-fake-acp$/)
  })

  it('without an agent, hands it no HENNERY_* variable at all', () => {
    expect(henneryKeys(handed(hostEnv()))).toEqual([])
  })

  it('refuses any other HENNERY_* variable for its agents, whatever its case', () => {
    expect(AGENT_VARS).toEqual(['HENNERY_FAKE_ACP_SCRIPT'])
    for (const key of [...Object.keys(RUNNER), 'HENNERY_MASTER_KEY', 'HENNERY_SERVICE', 'hennery_log_dir']) {
      expect(() => hostEnv({ [key]: '/srv/work/elsewhere' })).toThrow(key)
    }
  })

  it('refuses the home and every XDG directory scratchEnv sets: none may move the host’s data directory', () => {
    const scratch = ['HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME']
    for (const key of scratch) expect(Object.keys(scratchEnv(DIR))).toContain(key)
    for (const key of [...scratch, 'home', 'xdg_data_home']) {
      expect(() => hostEnv({ [key]: '/srv/work/elsewhere' })).toThrow(key)
    }
    // Any other variable an agent needs is its own.
    expect(hostEnv({ LANG: 'C.UTF-8' }).LANG).toBe('C.UTF-8')
  })

  it('starts every process of the host in that environment', () => {
    const source = readFileSync(join(process.cwd(), 'e2e', 'host.ts'), 'utf8')
    expect(source).toContain('const env = hostEnv(options.agentEnv)')
    const calls = source.match(/spawnHennery\([^\n]*\)/g) ?? []
    expect(calls).toHaveLength(2)
    for (const call of calls) expect(call).toMatch(/, dir, env\)$/)
  })
})
