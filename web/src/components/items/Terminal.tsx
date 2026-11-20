interface TerminalData {
  terminal_id: string
  command: string | null
  cwd: string | null
  output: string
  exit_code: number | null
  signal: string | null
}

/** Display-only terminal evidence. Never execute output or infer an exit code. */
export default function Terminal({ item }: { item: TerminalData }) {
  const ended = item.exit_code !== null || item.signal !== null
  return <details className="terminal-item" open><summary><span>Terminal</span>{item.command && <code>{item.command}</code>}{ended && <span className={item.exit_code !== null && item.exit_code !== 0 ? 'terminal-failed' : ''}>{item.signal ? `Signal ${item.signal}` : `Exit ${item.exit_code}`}</span>}</summary>
    {item.cwd && <p className="hint"><bdi>{item.cwd}</bdi></p>}
    {item.output && <pre className="terminal-output">{item.output}</pre>}
  </details>
}
