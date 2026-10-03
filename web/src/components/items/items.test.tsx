import { fireEvent, render, screen, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { Item, MarkerKind, ToolContent } from '../../generated/view'
import { ItemView } from '../Transcript'
import { MARKER_LABEL } from './Marker'
import type { ItemEnv, ItemOf } from './types'

const TS = '2026-10-02T10:00:00.000Z'
const env: ItemEnv = { sessionId: 's/1', agent: 'Codex' }

function item<K extends Item['kind']>(kind: K, body: Omit<ItemOf<K>, 'id' | 'version' | 'ts' | 'kind'>): ItemOf<K> {
  return { id: `i-${kind}`, version: 1, ts: TS, turn_id: 't1', kind, ...body } as unknown as ItemOf<K>
}

function show(value: Item, e: ItemEnv = env) {
  return render(<ItemView item={value} env={e} />)
}

describe('user_turn', () => {
  it('shows the text exactly as sent: no Markdown, line breaks kept', () => {
    const text = '  indented\n# not a heading\n**not bold** <b>x</b>\nsecond line\n'
    const { container } = show(item('user_turn', { content: [{ type: 'text', text }] }))
    const p = container.querySelector('.user-text') as HTMLElement
    expect(p.textContent).toBe(text)
    expect(container.querySelector('h1, strong, b')).toBeNull()
    expect(screen.getByText('You')).toBeInTheDocument()
  })

  const HASH = '57cda64cead0869cd5f90dfebb024f4bd9a922aaea91d513de6fa3e949d88921'

  it('shows an image of an allowed type from the attachment store, by its hash', () => {
    const { container } = show(item('user_turn', { content: [{ type: 'image', mimeType: 'image/png', sha256: HASH, size: 3 }] }))
    expect(container.querySelector('img')?.getAttribute('src')).toBe(`/api/attachments/${HASH}`)
  })

  it.each([
    ['a path', 'ab/c?d'],
    ['a traversal', `../${HASH.slice(3)}`],
    ['upper case', HASH.toUpperCase()],
    ['too short', HASH.slice(1)],
    ['too long', `${HASH}0`],
    ['a trailing newline', `${HASH}\n`],
    ['empty', ''],
    ['not text', 7],
  ])('loads nothing for a hash that is %s, and says so', (_name, sha256) => {
    const { container } = show(
      item('user_turn', { content: [{ type: 'image', mimeType: 'image/png', sha256: sha256 as string, size: 3 }] }),
    )
    expect(container.querySelector('img')).toBeNull()
    expect(container.querySelector('.item-note')?.textContent).toContain('not shown')
  })

  it.each(['image/jpeg', 'image/gif', 'image/webp'])('shows %s', (mimeType) => {
    const { container } = show(item('user_turn', { content: [{ type: 'image', mimeType, sha256: HASH, size: 1 }] }))
    expect(container.querySelector('img')).not.toBeNull()
  })

  it('names an image of another type instead of loading it', () => {
    const { container } = show(
      item('user_turn', { content: [{ type: 'image', mimeType: 'image/svg+xml', sha256: HASH, size: 1 }] }),
    )
    expect(container.querySelector('img')).toBeNull()
    expect(container.textContent).toContain('image/svg+xml')
  })

  it('says when the prompt was cut, linking the raw events', () => {
    show(item('user_turn', { content: [{ type: 'text', text: 'x' }], truncated: true }))
    expect(screen.getByText('This prompt was cut.')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Raw events' })).toHaveAttribute('href', '/api/sessions/s%2F1/events')
  })
})

describe('message', () => {
  it('renders Markdown under the agent’s label', () => {
    const { container } = show(item('message', { text: 'some **bold** text' }))
    expect(container.querySelector('.bubble strong')?.textContent).toBe('bold')
    expect(screen.getByText('Codex')).toBeInTheDocument()
  })

  it('says when it was cut', () => {
    show(item('message', { text: 'x', truncated: true }))
    expect(screen.getByText(/This message was cut/)).toBeInTheDocument()
  })
})

describe('thinking', () => {
  it('is collapsed until opened, then Markdown', () => {
    const { container } = show(item('thinking', { text: 'first *thought*' }))
    expect(screen.getByText('Thinking')).toBeInTheDocument()
    expect(container.querySelector('.think-body')).toBeNull()
    fireEvent.click(screen.getByRole('button', { expanded: false }))
    expect(container.querySelector('.think-body em')?.textContent).toBe('thought')
  })

  it('says when it was cut', () => {
    show(item('thinking', { text: 'x', truncated: true }))
    expect(screen.getByText(/This reasoning was cut/)).toBeInTheDocument()
  })
})

describe('tool_call', () => {
  const base = { tool_call_id: 'tc1', title: 'Run a command', tool_kind: 'execute', status: 'completed' }

  it('is one line: name, the command, the status in words; the rest hidden', () => {
    const { container } = show(item('tool_call', { ...base, input: { command: 'echo hi' }, output: 'hi\n' }))
    expect(screen.getByText('Run a command')).toBeInTheDocument()
    expect(screen.getByText('echo hi')).toBeInTheDocument()
    expect(screen.getByText('Done')).toBeInTheDocument()
    expect(container.querySelectorAll('.tool-io')).toHaveLength(0)
  })

  it('falls back to the tool kind for a name', () => {
    show(item('tool_call', { tool_call_id: 'x', tool_kind: 'read' }))
    expect(screen.getByText('read')).toBeInTheDocument()
  })

  it('expands to the input as indented JSON and the output as text', () => {
    const { container } = show(item('tool_call', { ...base, input: { command: 'echo hi' }, output: 'hi <b>x</b>' }))
    fireEvent.click(screen.getByRole('button'))
    const input = container.querySelector('.tool-input') as HTMLElement
    expect(input.textContent).toContain('"command": "echo hi"')
    expect(input.querySelector('.tk-key')?.textContent).toBe('"command"')
    expect(container.querySelector('.tool-output')?.textContent).toBe('outputhi <b>x</b>')
    expect(container.querySelector('.tool-output b')).toBeNull()
  })

  it('never marks the tool’s own input as a redaction', () => {
    const { container } = show(item('tool_call', { ...base, input: { note: 'this was redacted' } }))
    fireEvent.click(screen.getByRole('button'))
    expect(container.querySelector('.tk-redact')).toBeNull()
    expect(container.querySelector('.tool-input .tk-str')?.textContent).toBe('"this was redacted"')
  })

  it('shows a cut input (a string) as it is', () => {
    const { container } = show(item('tool_call', { ...base, input: '{"command": "ech', truncated: true }))
    fireEvent.click(screen.getByRole('button'))
    expect(container.querySelector('.tool-input')?.textContent).toBe('input{"command": "ech')
    expect(screen.getByText(/Part of this tool call was cut/)).toBeInTheDocument()
  })

  it('is not expandable with nothing to show', () => {
    const { container } = show(item('tool_call', { tool_call_id: 'x', status: 'pending' }))
    fireEvent.click(screen.getByRole('button'))
    expect(container.querySelector('.tool-detail')).toBeNull()
    expect(screen.getByText('Pending')).toBeInTheDocument()
  })

  it('shows a status it does not know as sent', () => {
    show(item('tool_call', { tool_call_id: 'x', status: 'paused' }))
    expect(screen.getByText('paused')).toBeInTheDocument()
  })

  function expanded(content: ToolContent[]) {
    const r = show(item('tool_call', { ...base, content }))
    fireEvent.click(screen.getByRole('button'))
    return r.container
  }

  it('shows a text block as text', () => {
    const c = expanded([{ type: 'text', text: 'plain <i>x</i>' }])
    expect(c.textContent).toContain('plain <i>x</i>')
    expect(c.querySelector('i')).toBeNull()
  })

  it('shows a diff: its path, before and after', () => {
    const c = expanded([{ type: 'diff', path: '/srv/work/a.txt', old_text: 'old', new_text: 'new' }])
    expect(c.textContent).toContain('/srv/work/a.txt')
    expect(c.querySelector('.diff-old')?.textContent).toBe('old')
    expect(c.querySelector('.diff-new')?.textContent).toBe('new')
  })

  it('shows a new file’s diff without a before', () => {
    const c = expanded([{ type: 'diff', path: '/srv/work/b.txt', new_text: 'yes' }])
    expect(c.querySelector('.diff-old')).toBeNull()
    expect(c.textContent).toContain('new file')
  })

  it('shows an image of an allowed type as a data URL', () => {
    const c = expanded([{ type: 'image', mime_type: 'image/png', data: 'iVBORw0KGgo=' }])
    expect(c.querySelector('img')?.getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=')
  })

  it('names an image of a refused type instead of showing it', () => {
    const c = expanded([{ type: 'image', mime_type: 'image/svg+xml', data: 'PHN2Zz4=' }])
    expect(c.querySelector('img')).toBeNull()
    expect(c.textContent).toContain('image/svg+xml')
  })

  it('says an image was too large when it has no data', () => {
    const c = expanded([{ type: 'image', mime_type: 'image/png' }])
    expect(c.querySelector('img')).toBeNull()
    expect(c.textContent).toContain('too large')
  })

  it('shows a terminal by its id', () => {
    const c = expanded([{ type: 'terminal', terminal_id: 'term-7' }])
    expect(c.textContent).toContain('term-7')
  })

  it('shows an other block’s raw JSON as text', () => {
    const c = expanded([{ type: 'other', raw: '{"type":"audio"}' }])
    expect(c.textContent).toContain('{"type":"audio"}')
  })

  it('shows a block of an unknown type as its JSON', () => {
    const c = expanded([{ type: 'hologram', x: 1 } as unknown as ToolContent])
    expect(c.textContent).toContain('"hologram"')
  })

  it('lists the locations', () => {
    const c = expanded([])
    expect(c.querySelector('.tool-locations')).toBeNull()
    const { container } = show(item('tool_call', { ...base, locations: [{ path: '/srv/work/x.rs', line: 4 }, { path: '/srv/work/y.rs' }] }))
    fireEvent.click(within(container).getByRole('button'))
    expect(container.querySelector('.tool-locations')?.textContent).toBe('/srv/work/x.rs:4/srv/work/y.rs')
  })

  it('warns loudly of a possible fabrication, quoting it as text, without expanding', () => {
    const { container } = show(item('tool_call', { ...base, output: 'x', fabricated: '<tool_use><b>x</b></tool_use>' }))
    const alert = screen.getByRole('alert')
    expect(alert).toHaveTextContent('Possible fabricated tool call')
    expect(alert).toHaveTextContent('<tool_use><b>x</b></tool_use>')
    expect(container.querySelector('.fab-warn b')).toBeNull()
  })

  it.each(['__proto__', 'constructor', 'toString'])('shows a status named %s as sent, the fabrication banner too', (status) => {
    const { container } = show(item('tool_call', { ...base, status, fabricated: 'made up' }))
    expect(screen.getByRole('alert')).toHaveTextContent('Possible fabricated tool call')
    expect(container.querySelector('.tool-status')?.textContent).toBe(status)
    expect((container.querySelector('.tool-status') as HTMLElement).style.color).toBe('var(--fg-quiet)')
  })

  it('has no warning when nothing was fabricated', () => {
    show(item('tool_call', { ...base }))
    expect(screen.queryByRole('alert')).toBeNull()
  })
})

describe('plan', () => {
  it('shows its step list', () => {
    const { container } = show(item('plan', { entries: [{ content: 'step one', status: 'in_progress' }] }))
    expect(within(container.querySelector('ol') as HTMLElement).getByText('step one')).toBeInTheDocument()
  })
})

describe('question', () => {
  type Q = Omit<ItemOf<'question'>, 'id' | 'version' | 'ts' | 'kind'>
  const permission: Q = {
    pending_id: 'p1',
    question_kind: 'permission',
    request: {
      type: 'permission',
      title: 'Run rm -rf build?',
      options: [
        { option_id: 'a', name: 'Yes, always', option_kind: 'allow_always' },
        { option_id: 'b', name: 'Allow', option_kind: 'allow_once' },
        { option_id: 'c', name: 'Allow (really reject)', option_kind: 'reject_once' },
        { option_id: 'd', name: 'Never', option_kind: 'reject_always' },
      ],
    },
    answerable: true,
    state: 'open',
    answered: false,
  }

  it('shows the request, its options in order, styled by kind and never by name, and no buttons', () => {
    const { container } = show(item('question', permission))
    expect(screen.getByText('Run rm -rf build?')).toBeInTheDocument()
    expect(screen.getByText('Codex asks for permission')).toBeInTheDocument()
    const options = Array.from(container.querySelectorAll('.q-opt'))
    expect(options.map((o) => o.textContent)).toEqual(['Yes, always', 'Allow', 'Allow (really reject)', 'Never'])
    expect(options.map((o) => o.className)).toEqual([
      'q-opt q-opt-always',
      'q-opt q-opt-allow',
      'q-opt q-opt-reject',
      'q-opt q-opt-reject',
    ])
    expect(screen.queryByRole('button')).toBeNull()
    expect(screen.getByText('Needs your answer')).toBeInTheDocument()
  })

  it('says a permission with no options cannot be answered here', () => {
    show(item('question', { ...permission, request: { type: 'permission', options: [] } }))
    expect(screen.getByText(/cannot be answered here: stop, park or close the session/)).toBeInTheDocument()
  })

  it.each([
    [{ answered: true, answerable: false }, 'Sent'],
    [{ answered: true, delivered: true, answerable: false }, 'Answered'],
    [{ answered: true, delivered: false, answerable: false }, 'Sent, but the agent was no longer waiting'],
    [{ state: 'delivered' as const, answerable: false }, 'Answered'],
    [{ state: 'cancelled' as const, reason: 'agent_withdrew' as const, answerable: false }, 'The agent stopped waiting (the agent withdrew the question)'],
    [{ state: 'cancelled' as const, reason: 'host_revoked' as const, answerable: false }, 'The agent stopped waiting (the host was revoked)'],
    [{ state: 'cancelled' as const, answered: true, delivered: true, answerable: false }, 'Answered'],
    [{ answerable: false }, 'Open'],
  ])('%o reads “%s”', (patch, text) => {
    show(item('question', { ...permission, ...patch }))
    expect(screen.getByText(text)).toBeInTheDocument()
  })

  it('shows a reason named like a prototype key as sent', () => {
    show(item('question', { ...permission, state: 'cancelled', reason: 'constructor' as never, answerable: false }))
    expect(screen.getByText('The agent stopped waiting (constructor)')).toBeInTheDocument()
  })

  it('shows an elicitation’s message, fields, hints and options', () => {
    show(
      item('question', {
        pending_id: 'p2',
        question_kind: 'elicitation',
        request: {
          type: 'elicitation',
          message: 'Tabs or spaces?',
          form_supported: false,
          fields: [
            { key: 'indent', label: 'Indent', hint: 'Pick one', field_kind: 'single', options: [{ value: 'tabs', label: 'Tabs', description: 'one char' }, { value: 'spaces' }] },
            { key: 'other', label: 'Other', field_kind: 'unsupported' },
          ],
        },
        answerable: false,
        state: 'open',
        answered: false,
      }),
    )
    expect(screen.getByText('Tabs or spaces?')).toBeInTheDocument()
    expect(screen.getByText('Codex asks')).toBeInTheDocument()
    expect(screen.getByText('Pick one')).toBeInTheDocument()
    expect(screen.getByText('Tabs')).toBeInTheDocument()
    expect(screen.getByText('one char')).toBeInTheDocument()
    expect(screen.getByText('spaces')).toBeInTheDocument()
    expect(screen.getByText(/This field cannot be filled in here/)).toBeInTheDocument()
    expect(screen.getByText(/can only be declined or cancelled/)).toBeInTheDocument()
  })
})

describe('marker', () => {
  // Written out, not read from the table under test: a changed label fails.
  const WORDS: [MarkerKind, string][] = [
    ['parked', 'Parked'],
    ['resumed', 'Resumed'],
    ['closed', 'Closed'],
    ['host_restarted', 'The host restarted'],
    ['host_offline', 'Host offline'],
    ['host_back', 'Host back'],
    ['turn_interrupted', 'The turn was interrupted'],
    ['turn_failed', 'The turn failed'],
    ['turn_cancelled', 'The turn was stopped'],
    ['turn_not_delivered', 'The turn was not delivered'],
    ['start_not_delivered', 'The start was not delivered'],
    ['start_failed', 'The session failed to start'],
    ['adapter_exited', 'The agent’s process exited'],
    ['transcript_gap', 'Part of the transcript is missing'],
    ['host_note', 'A note from the host'],
    ['conflict', 'The host sent a conflicting update'],
    ['hat_reassigned', 'Moved to another hat'],
    ['elided', 'This turn holds more than is shown here'],
  ]

  it('covers all 18 kinds, each once', () => {
    expect(WORDS).toHaveLength(18)
    expect(new Set(WORDS.map(([k]) => k))).toEqual(new Set(Object.keys(MARKER_LABEL)))
  })

  it.each(WORDS)('%s reads “%s”', (kind, words) => {
    const { container } = show(item('marker', { marker: kind }))
    const label = (container.querySelector('.divider > span')?.firstChild as Text | null)?.textContent
    expect(label).toBe(words)
    expect(container.textContent).not.toContain(`(${kind})`)
  })

  it.each([
    ['parked', 'idle', 'it was idle'],
    ['parked', 'operator', 'on request'],
    ['host_offline', 'host_revoked', 'the host was revoked'],
    ['start_failed', 'agent_has_no_record', 'the agent has no record of this session'],
    ['parked', 'adapter_exited', 'the agent’s process exited'],
    ['host_offline', 'host_offline', 'the host went offline'],
    ['start_failed', 'agent_not_logged_in', 'the agent is not logged in on the host'],
    ['start_failed', 'load_unsupported', 'the agent cannot open an earlier session'],
    ['start_failed', 'start_failed', 'the agent could not start'],
    ['start_failed', 'start_not_delivered', 'the start never reached the host'],
    ['start_failed', 'host_offline', 'the host went offline before it started'],
    ['start_failed', 'host_revoked', 'the host was revoked'],
    ['start_failed', 'unknown_agent', 'the host does not know this agent'],
    ['host_note', 'config_failed', 'a setting did not take'],
    ['host_note', 'reapply_failed', 'a setting could not be applied again'],
    ['host_note', 'cancel_unanswered', 'a question was left unanswered when the turn stopped'],
    ['host_note', 'replay_unknown_dropped', 'history of an unknown kind was left out'],
    ['host_note', 'agent_home_moved', 'the agent’s data directory moved'],
    ['elided', 'items', 'too many items'],
    ['elided', 'questions', 'too many questions'],
    ['host_note', 'some_note', 'some_note'],
    ['adapter_exited', 'code 1', 'code 1'],
    ['transcript_gap', '4..9', 'host events 4..9'],
    ['conflict', '12', 'at host event 12'],
    ['elided', 'bytes', 'too much text'],
    ['start_failed', 'some_new_code', 'some_new_code'],
  ] as [MarkerKind, string, string][])('%s with reason %s reads “%s”', (kind, reason, words) => {
    const { container } = show(item('marker', { marker: kind, reason }))
    expect(container.querySelector('.divider')?.textContent).toContain(`(${words})`)
  })

  it('keeps its text behind a disclosure, as text', () => {
    const { container } = show(item('marker', { marker: 'adapter_exited', text: 'panic: <script>x</script>' }))
    const details = container.querySelector('details.marker-text') as HTMLDetailsElement
    expect(details.open).toBe(false)
    expect(details.querySelector('pre')?.textContent).toBe('panic: <script>x</script>')
    expect(container.querySelector('script')).toBeNull()
  })

  it('names the hats of a reassignment when they are known', () => {
    const names: Record<string, string> = { h1: 'Work', h2: 'Home' }
    const { container } = show(item('marker', { marker: 'hat_reassigned', from: 'h1', to: 'h2' }), { ...env, hatName: (id) => names[id] })
    expect(container.querySelector('.divider')?.textContent).toContain('from Work to Home')
  })

  it('shows the hats’ ids when their names are not known', () => {
    const { container } = show(item('marker', { marker: 'hat_reassigned', from: 'h1', to: 'h2' }))
    expect(container.querySelector('.divider')?.textContent).toContain('from h1 to h2')
  })

  it('offers “Send again” for an undelivered turn only through its seam', () => {
    const m = item('marker', { marker: 'turn_not_delivered', about_turn: 't9' })
    const { unmount } = show(m)
    expect(screen.getByText('The turn was not delivered')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Send again' })).toBeNull()
    unmount()
    const onSendAgain = vi.fn()
    show(m, { ...env, onSendAgain })
    fireEvent.click(screen.getByRole('button', { name: 'Send again' }))
    expect(onSendAgain).toHaveBeenCalledWith(m)
  })

  it('points an elided turn to the raw events', () => {
    show(item('marker', { marker: 'elided', reason: 'items' }))
    expect(screen.getByText(/^This turn holds more than is shown here/)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Raw events' })).toBeInTheDocument()
  })

  it('shows a reason as sent for a kind named like a prototype key', () => {
    const { container } = show(item('marker', { marker: '__proto__' as MarkerKind, reason: 'toString' }))
    expect(container.querySelector('.divider')?.textContent).toContain('(toString)')
  })

  it('names a kind it does not know rather than failing', () => {
    const { container } = show(item('marker', { marker: 'from_the_future' as MarkerKind }))
    expect(container.textContent).toContain('from_the_future')
  })
})

describe('unrecognised', () => {
  it('is collapsed, naming its kind, with its raw JSON as text', () => {
    const { container } = show(item('unrecognised', { update_kind: 'acp_update/new_thing', raw: '{"a":"<b>x</b>"}', truncated: true }))
    const details = container.querySelector('details') as HTMLDetailsElement
    expect(details.open).toBe(false)
    expect(details.querySelector('summary')?.textContent).toBe('Unsupported update (acp_update/new_thing)')
    expect(details.querySelector('pre')?.textContent).toBe('{"a":"<b>x</b>"}')
    expect(container.querySelector('pre b')).toBeNull()
    expect(screen.getByText('This update was cut.')).toBeInTheDocument()
  })
})

describe('an item of a kind this client does not know', () => {
  it('is named, not dropped', () => {
    const { container } = show({ id: 'x', version: 1, ts: TS, kind: 'hologram' } as unknown as Item)
    expect(container.textContent).toContain('Unsupported item (hologram)')
  })
})
