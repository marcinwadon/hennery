// The browser checks start processes in one place only, `e2e/spawn.ts`,
// whose one `spawn` runs the binary in `scratchEnv`: a check that started
// its own would hand the binary the runner's home and any `HENNERY_*`
// variable, and with them the operator's own data and log directories.
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { INHERITED, scratchEnv } from '../e2e/spawn'

// Vitest runs from `web/`.
const ROOT = process.cwd()
const HELPER = join('e2e', 'spawn.ts')
// A call of any of these, not as a method of something else, or the module.
const STARTS = /(?<![.\w])(spawn|spawnSync|exec|execSync|execFile|execFileSync|fork)\s*\(|child_process/

function files(dir: string): string[] {
  if (!existsSync(dir)) return []
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) return files(path)
    return /\.(ts|tsx|js|mjs|cjs)$/.test(name) ? [path] : []
  })
}

describe('the browser checks', () => {
  const all = [...files(join(ROOT, 'e2e')), ...files(join(ROOT, 'scripts'))]

  it('are all read', () => {
    expect(all.map((f) => relative(ROOT, f))).toContain(HELPER)
    expect(all.length).toBeGreaterThan(4)
  })

  it('start a process only through spawn.ts', () => {
    const found = all.filter((f) => relative(ROOT, f) !== HELPER && STARTS.test(readFileSync(f, 'utf8')))
    expect(found.map((f) => relative(ROOT, f))).toEqual([])
  })

  it('start it, there, only in scratchEnv', () => {
    const helper = readFileSync(join(ROOT, HELPER), 'utf8')
    expect(helper.match(/(?<![.\w])spawn\s*\(/g)).toHaveLength(1)
    expect(helper).toContain('spawn(BIN, args, { stdio, env: { ...scratchEnv(dir), ...extra } })')
  })
})

describe('a child of the browser checks', () => {
  const set = (key: string, value: string | undefined) => {
    if (value === undefined) delete process.env[key]
    else process.env[key] = value
  }
  const saved = ['HENNERY_DATA_DIR', 'HENNERY_LOG_DIR', 'CODEX_HOME', 'E2E_UNEXPECTED', 'PATH'].map(
    (key) => [key, process.env[key]] as const,
  )
  afterEach(() => saved.forEach(([key, value]) => set(key, value)))

  it('gets only the allowlisted variables of the runner, and its scratch home', () => {
    set('HENNERY_DATA_DIR', '/runner/data')
    set('HENNERY_LOG_DIR', '/runner/logs')
    set('CODEX_HOME', '/runner/codex')
    set('E2E_UNEXPECTED', 'secret')
    set('PATH', '/usr/bin:/bin')
    const env = scratchEnv('/scratch')
    const scratch = ['HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME']
    expect(Object.keys(env).every((key) => (INHERITED as readonly string[]).includes(key) || scratch.includes(key))).toBe(true)
    expect(env.E2E_UNEXPECTED).toBeUndefined()
    expect(env.HENNERY_DATA_DIR).toBeUndefined()
    expect(env.HENNERY_LOG_DIR).toBeUndefined()
    expect(env.CODEX_HOME).toBeUndefined()
    expect(env.PATH).toBe('/usr/bin:/bin')
    expect(env.HOME).toBe('/scratch')
    for (const key of scratch.slice(1)) expect(env[key]?.startsWith('/scratch/')).toBe(true)
  })
})
