// A test host for the browser checks: the built `hennery` binary paired
// with `host join` (the exact command the Hosts screen shows, with the
// options a hermetic run needs), then run with one stand-in agent, so no
// adapter is ever downloaded. Its directory is fresh, and every process is
// stopped by its own id.
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { isAbsolute, join } from 'node:path'
import { BIN, scratchEnv, spawnHennery, type ChildProcess } from './spawn'

/** Where `host join` and `host run`, given no data directory, keep a
 *  host's files in `scratchEnv(home)`: the platform's default (distribution
 *  spec §8), under the scratch home, never the runner's. */
export function defaultHostDir(home: string): string {
  return process.platform === 'darwin'
    ? join(home, 'Library', 'Application Support', 'hennery')
    : join(scratchEnv(home).XDG_DATA_HOME as string, 'hennery')
}

/** SIGTERM, then SIGKILL if it has not gone within 5 s. One that never
 *  started (no pid) has nothing to stop. */
async function end(child: ChildProcess): Promise<void> {
  if (child.pid === undefined || child.exitCode !== null || child.signalCode !== null) return
  const gone = new Promise((done) => child.once('exit', done))
  child.kill('SIGTERM')
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000)
  await gone
  clearTimeout(timer)
}

export interface TestHost {
  dir: string
  /** Runs `command` (as the page shows it) with `extra` options; resolves
   *  with its exit code. */
  join(command: string, extra: string[]): Promise<number>
  /** `host run` in the background, until `stop`. */
  run(): void
  stop(): Promise<void>
}

export function testHost(): TestHost {
  const dir = mkdtempSync(join(tmpdir(), 'hennery-e2e-host-'))
  // A relative HOME is not taken: the binary would use the account's own.
  if (!isAbsolute(dir)) throw new Error(`the scratch home is not absolute: ${dir}`)
  // Its home is the fresh directory too: the host reads $HOME (for `~`),
  // and its data directory is the default under that home, as for anyone
  // who runs the command as the page shows it.
  const env = { RUST_LOG: 'warn' }
  const children: ChildProcess[] = []
  return {
    dir,
    join(command, extra) {
      const words = command.trim().split(/\s+/)
      if (words[0] !== 'hennery') throw new Error(`not a hennery command: ${command}`)
      const child = spawnHennery([...words.slice(1), ...extra], dir, env)
      children.push(child)
      let stderr = ''
      child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
      return new Promise((done, fail) => {
        child.once('error', fail)
        child.once('exit', (code) => {
          if (code !== 0) console.error(`host join exited ${code}: ${stderr}`)
          // A pairing anywhere but the scratch default is a failure, even
          // with exit 0: it would be in a directory the test does not own.
          const pairing = join(defaultHostDir(dir), 'host.toml')
          if (code === 0 && !existsSync(pairing)) {
            fail(new Error(`host join exited 0 but left no ${pairing}: ${stderr}`))
            return
          }
          done(code ?? -1)
        })
      })
    },
    run() {
      // The stand-in agent is never started (no session runs here); any
      // path that exists will do, and the binary's own does everywhere.
      const runner = spawnHennery(['host', 'run', '--agent', `stand-in=${BIN}`], dir, env)
      let stderr = ''
      runner.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
      runner.once('error', (err) => console.error(`host run failed to start: ${err}`))
      runner.once('exit', (code) => {
        if (code !== 0 && code !== null) console.error(`host run exited ${code}: ${stderr}`)
      })
      children.push(runner)
    },
    async stop() {
      // Every process this host started, the join too, however the test
      // ended; then its directory, which holds the host's key.
      try {
        await Promise.all(children.map(end))
      } finally {
        rmSync(dir, { recursive: true, force: true })
      }
    },
  }
}
