// The one place the browser checks start a process (a guard in
// `src/security.test.ts` holds every other file of `e2e/` to that): the
// built `hennery` binary, always in `scratchEnv`, so nothing of the
// runner's own hennery setup, home or XDG directories reaches it.
import { spawn, type ChildProcess, type StdioOptions } from 'node:child_process'
import { join, resolve } from 'node:path'

export type { ChildProcess }

export const BIN = process.env.HENNERY_BIN ?? resolve(process.cwd(), '../target/debug/hennery')

/** The runner's environment without any `HENNERY_*` variable (a data or
 *  log directory, a service flag: each would send the binary to the
 *  runner's own files), with its home and XDG directories under `dir`. */
export function scratchEnv(dir: string): NodeJS.ProcessEnv {
  const env = { ...process.env }
  for (const key of Object.keys(env)) if (/^HENNERY_/.test(key)) delete env[key]
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
