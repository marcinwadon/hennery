// A real collector for the browser checks: the built `hennery` binary
// (`HENNERY_BIN`, else the workspace's debug build), on a fresh data
// directory and a port of its own choosing, read from the setup link it
// writes. Stopped by its own process id.
import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

export interface Collector {
  /** `http://localhost:<port>`. */
  origin: string
  /** The one-time setup link, `<origin>/setup#<token>`. */
  setupLink: string
  stop(): Promise<void>
}

const BIN = process.env.HENNERY_BIN ?? resolve(process.cwd(), '../target/debug/hennery')

export async function startCollector(): Promise<Collector> {
  const dir = mkdtempSync(join(tmpdir(), 'hennery-e2e-'))
  const child: ChildProcess = spawn(BIN, ['collector', '--data-dir', dir, '--listen', '127.0.0.1:0'], {
    stdio: ['ignore', 'ignore', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
  })
  let stderr = ''
  child.stderr?.on('data', (chunk: Buffer) => (stderr += chunk.toString()))
  const setupLink = await waitFor(() => {
    if (child.exitCode !== null) throw new Error(`the collector exited (${child.exitCode}): ${stderr}`)
    try {
      return readFileSync(join(dir, 'setup-url'), 'utf8').trim() || null
    } catch {
      return null
    }
  })
  return {
    origin: new URL(setupLink).origin,
    setupLink,
    async stop() {
      if (child.exitCode === null) {
        child.kill('SIGTERM')
        await new Promise((done) => child.once('exit', done))
      }
      rmSync(dir, { recursive: true, force: true })
    },
  }
}

async function waitFor<T>(probe: () => T | null): Promise<T> {
  for (let i = 0; i < 300; i++) {
    const value = probe()
    if (value !== null) return value
    await new Promise((r) => setTimeout(r, 100))
  }
  throw new Error('the collector wrote no setup link within 30 s')
}
