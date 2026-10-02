// A test host for the browser checks: the built `hennery` binary paired
// with `host join` (the exact command the Hosts screen shows, with the
// options a hermetic run needs), then run with one stand-in agent, so no
// adapter is ever downloaded. Its directory is fresh, and every process is
// stopped by its own id.
import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BIN, scratchEnv } from './collector'

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
  // Its home is the fresh directory too: the host reads $HOME (for `~`).
  const env = { ...scratchEnv(dir), HENNERY_HOST_DATA_DIR: dir, RUST_LOG: 'warn' }
  const children: ChildProcess[] = []
  return {
    dir,
    join(command, extra) {
      const words = command.trim().split(/\s+/)
      if (words[0] !== 'hennery') throw new Error(`not a hennery command: ${command}`)
      const child = spawn(BIN, [...words.slice(1), ...extra], { stdio: ['ignore', 'ignore', 'pipe'], env })
      children.push(child)
      let stderr = ''
      child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
      return new Promise((done, fail) => {
        child.once('error', fail)
        child.once('exit', (code) => {
          if (code !== 0) console.error(`host join exited ${code}: ${stderr}`)
          done(code ?? -1)
        })
      })
    },
    run() {
      // The stand-in agent is never started (no session runs here); any
      // path that exists will do, and the binary's own does everywhere.
      const runner = spawn(BIN, ['host', 'run', '--data-dir', dir, '--agent', `stand-in=${BIN}`], {
        stdio: ['ignore', 'ignore', 'pipe'],
        env,
      })
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
