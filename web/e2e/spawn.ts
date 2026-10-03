// The one place the browser checks start a process (a guard in
// `src/security.test.ts` holds every other file of `e2e/` to that): the
// built `hennery` binary, always in `scratchEnv`, so nothing of the
// runner's own hennery setup, home or XDG directories reaches it.
import { spawn, type ChildProcess, type StdioOptions } from 'node:child_process'
import { join, resolve } from 'node:path'

export type { ChildProcess }

export const BIN = process.env.HENNERY_BIN ?? resolve(process.cwd(), '../target/debug/hennery')

/** What a child may take from the runner's environment: where programs
 *  are, the locale and time zone, and the temporary directory. */
export const INHERITED = ['PATH', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TZ', 'TMPDIR'] as const

/** A child's environment, built from an allowlist: `INHERITED` from the
 *  runner, and its home and XDG directories under `dir`. Nothing else of the
 *  runner's reaches it: no `HENNERY_*` variable (a data or log directory, a
 *  service flag), no `CODEX_HOME`, no token. A test adds what it needs
 *  through `extra`. */
export function scratchEnv(dir: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {}
  for (const key of INHERITED) if (process.env[key] !== undefined) env[key] = process.env[key]
  return {
    ...env,
    HOME: dir,
    XDG_DATA_HOME: join(dir, 'xdg-data'),
    XDG_CONFIG_HOME: join(dir, 'xdg-config'),
    XDG_CACHE_HOME: join(dir, 'xdg-cache'),
    XDG_STATE_HOME: join(dir, 'xdg-state'),
  }
}

/** `hennery <args>` in `scratchEnv(dir)`, plus `extra` (which may not
 *  name a `HENNERY_*` variable the test did not choose itself). */
export function spawnHennery(
  args: string[],
  dir: string,
  extra: NodeJS.ProcessEnv,
  stdio: StdioOptions = ['ignore', 'ignore', 'pipe'],
): ChildProcess {
  return spawn(BIN, args, { stdio, env: { ...scratchEnv(dir), ...extra } })
}
