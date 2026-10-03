// A test host for the browser checks: the built `hennery` binary paired
// with `host join` (the exact command the Hosts screen shows, with the
// options a hermetic run needs), then run with one agent given by
// `--agent`, so no adapter is ever downloaded: a stand-in never started, or
// the scripted `hennery-fake-acp`. Its directory is fresh, and every
// process is stopped by its own id.
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join } from 'node:path'
import { BIN, scratchEnv, spawnHennery, type ChildProcess } from './spawn'

/** Where `host join` and `host run`, given no data directory, keep a
 *  host's files in `scratchEnv(home)`: the platform's default (distribution
 *  spec §8), under the scratch home, never the runner's. */
export function defaultHostDir(home: string): string {
  return process.platform === 'darwin'
    ? join(home, 'Library', 'Application Support', 'hennery')
    : join(scratchEnv(home).XDG_DATA_HOME as string, 'hennery')
}

/** The scripted stand-in agent (crates/hennery-testkit), built beside the
 *  binary: `cargo build -p hennery -p hennery-testkit --bins`. */
export const FAKE_ACP = join(dirname(BIN), 'hennery-fake-acp')

/** The one `HENNERY_*` variable a test may hand a host's agents: the fake
 *  agent's script. Any other (a data or log directory, a secret) is the
 *  host's own to set, or nobody's. */
export const AGENT_VARS: readonly string[] = ['HENNERY_FAKE_ACP_SCRIPT']

export interface HostOptions {
  /** `--agent <name>=<command>`; else a stand-in that is never started. */
  agent?: string
  /** More of the host's environment, which its agents inherit. */
  agentEnv?: NodeJS.ProcessEnv
}

/** `hennery-fake-acp` as the agent `name`, following `script` (its
 *  `FakeScript`, as JSON). */
export function fakeAgent(name: string, script: object): HostOptions {
  return { agent: `${name}=${FAKE_ACP}`, agentEnv: { HENNERY_FAKE_ACP_SCRIPT: JSON.stringify(script) } }
}

/** The directories `scratchEnv` sets: a test that set one would move the
 *  host's data directory (`spawnHennery` spreads `extra` over them). */
const SCRATCH_DIRS = /^(HOME|XDG_(DATA|CONFIG|CACHE|STATE)_HOME)$/i

/** What a host's processes get on top of `scratchEnv(dir)`: the agents'
 *  environment, which may name no `HENNERY_*` variable but `AGENT_VARS`,
 *  and none of the home and XDG directories `scratchEnv` sets. Its data
 *  directory is the platform's default under the scratch home
 *  (`defaultHostDir`), which nothing the test adds can move. */
export function hostEnv(agentEnv: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  for (const key of Object.keys(agentEnv)) {
    if ((/^HENNERY_/i.test(key) && !AGENT_VARS.includes(key)) || SCRATCH_DIRS.test(key)) {
      throw new Error(`a test host's agents may not be given ${key}`)
    }
  }
  return { ...agentEnv, RUST_LOG: 'warn' }
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
  /** `host run` in the background, until `stop` (or `crash`). */
  run(): void
  /** SIGKILLs the running `host run` by its own id, as a crash would, and
   *  resolves once it is gone; its agents' guard ends their group. */
  crash(): Promise<void>
  stop(): Promise<void>
}

export function testHost(options: HostOptions = {}): TestHost {
  const dir = mkdtempSync(join(tmpdir(), 'hennery-e2e-host-'))
  // A relative HOME is not taken: the binary would use the account's own.
  if (!isAbsolute(dir)) throw new Error(`the scratch home is not absolute: ${dir}`)
  // Its home is the fresh directory too: the host reads $HOME (for `~`),
  // and its data directory is the default under that home, as for anyone
  // who runs the command as the page shows it.
  const env = hostEnv(options.agentEnv)
  const agent = options.agent ?? `stand-in=${BIN}`
  const children: ChildProcess[] = []
  let runner: ChildProcess | undefined
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
      // The default stand-in agent is never started (no session runs);
      // any path that exists will do, and the binary's own does everywhere.
      const child = spawnHennery(['host', 'run', '--agent', agent], dir, env)
      let stderr = ''
      child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
      child.once('error', (err) => console.error(`host run failed to start: ${err}`))
      child.once('exit', (code) => {
        if (code !== 0 && code !== null) console.error(`host run exited ${code}: ${stderr}`)
      })
      children.push(child)
      runner = child
    },
    async crash() {
      const child = runner
      if (child?.pid === undefined || child.exitCode !== null || child.signalCode !== null) {
        throw new Error('no host run to crash')
      }
      const gone = new Promise((done) => child.once('exit', done))
      child.kill('SIGKILL')
      await gone
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
