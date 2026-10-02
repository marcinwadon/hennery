// One tool call (frontend spec §6.4): a line with its name, a summary and its
// status, expanding to the input, the output, the content blocks and the
// locations. Every one of them is text. A possible fabrication (F-13) is a
// banner that shows whether or not the call is expanded.
import { useState } from 'react'
import type { Location, ToolContent } from '../../generated/view'
import { tokJSON } from '../../lib/highlight'
import { Icon } from '../../lib/ui'
import { CutNote, Speaker } from './parts'
import { SHOWN_IMAGE_TYPES } from './UserTurn'
import type { ItemEnv, ItemOf } from './types'

// Read with `Object.hasOwn` only: a status is the agent's text, and
// `__proto__` or `constructor` must not reach the prototype.
const STATUS_WORDS: Record<string, string> = {
  pending: 'Pending',
  in_progress: 'Running',
  completed: 'Done',
  failed: 'Failed',
}

const STATUS_COLOUR: Record<string, string> = {
  pending: 'var(--fg-quiet)',
  in_progress: 'var(--st-wait)',
  completed: 'var(--st-run)',
  failed: 'var(--st-attn)',
}

type Tool = ItemOf<'tool_call'>

/** The line's name: the agent's title, else its kind of tool. */
export function toolName(tool: Tool): string {
  return tool.title || tool.tool_kind || 'Tool'
}

/** The line's summary: a shell command, else the first place it touched. */
export function toolSummary(tool: Tool): string {
  const input = tool.input
  if (input && typeof input === 'object' && !Array.isArray(input)) {
    const command = (input as Record<string, unknown>).command
    if (typeof command === 'string') return command
    if (Array.isArray(command) && command.every((part) => typeof part === 'string')) return command.join(' ')
  }
  const first = tool.locations?.[0]
  return first ? locationText(first) : ''
}

function locationText(location: Location): string {
  return location.line === undefined ? location.path : `${location.path}:${location.line}`
}

/** The input as it is shown: a string (a cut one is the start of its JSON)
 *  as it is, anything else as indented JSON. */
export function inputText(input: unknown): string {
  if (typeof input === 'string') return input
  try {
    return JSON.stringify(input, null, 2) ?? String(input)
  } catch {
    return String(input)
  }
}

/** JSON, one line per element, in the token colours. A tool's input is the
 *  agent's words: it gets no redaction mark (tokJSON's `tk-redact`), which
 *  only the server's own redactions may wear. */
function JsonLines({ text }: { text: string }) {
  return (
    <>
      {text.split('\n').map((line, i) => (
        <span key={i} className="io-json-ln">
          {tokJSON(line).map((seg, j) => (
            <span key={j} className={seg.c === 'tk-redact' ? 'tk-str' : seg.c}>
              {seg.t}
            </span>
          ))}
        </span>
      ))}
    </>
  )
}

function ContentBlock({ block }: { block: ToolContent }) {
  switch (block.type) {
    case 'text':
      return <div className="tool-io">{block.text}</div>
    case 'diff':
      return (
        <div className="tool-io tool-diff">
          <span className="io-label">
            {block.old_text === undefined ? 'new file' : 'diff'} <bdi>{block.path}</bdi>
          </span>
          {block.old_text !== undefined && (
            <>
              <span className="io-label">before</span>
              <pre className="diff-old">{block.old_text}</pre>
            </>
          )}
          <span className="io-label">after</span>
          <pre className="diff-new">{block.new_text}</pre>
        </div>
      )
    case 'image':
      if (!SHOWN_IMAGE_TYPES.has(block.mime_type)) {
        return (
          <p className="item-note">
            An image of type <bdi>{block.mime_type}</bdi>, not shown.
          </p>
        )
      }
      if (!block.data) return <p className="item-note">An image too large to show here.</p>
      return <img className="tool-image" src={`data:${block.mime_type};base64,${block.data}`} alt="Tool output" />
    case 'terminal':
      return (
        <div className="tool-io">
          <span className="io-label">terminal</span>
          <bdi>{block.terminal_id}</bdi>
        </div>
      )
    case 'other':
      return (
        <div className="tool-io">
          <span className="io-label">other</span>
          {block.raw}
        </div>
      )
    default: {
      // A block type this view does not know: its JSON, as text.
      return <div className="tool-io">{inputText(block)}</div>
    }
  }
}

export default function ToolCall({ item, env }: { item: Tool; env: ItemEnv }) {
  const [open, setOpen] = useState(false)
  const hasInput = item.input !== undefined && item.input !== null
  const content = item.content ?? []
  const locations = item.locations ?? []
  const expandable = hasInput || !!item.output || content.length > 0 || locations.length > 0
  const summary = toolSummary(item)
  const status = item.status
  return (
    <Speaker who="agent" label={env.agent} ts={item.ts}>
      {item.fabricated !== undefined && (
        <div className="fab-warn" role="alert">
          <span className="fab-warn-tag">Possible fabricated tool call</span>
          <span>{item.fabricated}</span>
        </div>
      )}
      <button
        type="button"
        className="tool-line"
        aria-expanded={expandable ? open : undefined}
        onClick={() => expandable && setOpen((o) => !o)}
      >
        <Icon.Wrench size={16} />
        <span className="tool-name">{toolName(item)}</span>
        {summary && <span className="tool-summary">{summary}</span>}
        {status && (
          <span className="tool-status" style={{ color: Object.hasOwn(STATUS_COLOUR, status) ? STATUS_COLOUR[status] : 'var(--fg-quiet)' }}>
            {Object.hasOwn(STATUS_WORDS, status) ? STATUS_WORDS[status] : status}
          </span>
        )}
      </button>
      {open && (
        <div className="tool-detail">
          {hasInput && (
            <div className="tool-io tool-input">
              <span className="io-label">input</span>
              <JsonLines text={inputText(item.input)} />
            </div>
          )}
          {item.output && (
            <div className="tool-io tool-output">
              <span className="io-label">output</span>
              {item.output}
            </div>
          )}
          {content.map((block, i) => (
            <ContentBlock key={i} block={block} />
          ))}
          {locations.length > 0 && (
            <ul className="tool-locations">
              {locations.map((location, i) => (
                <li key={i}>
                  <bdi>{locationText(location)}</bdi>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {item.truncated && <CutNote sessionId={env.sessionId}>Part of this tool call was cut.</CutNote>}
    </Speaker>
  )
}
