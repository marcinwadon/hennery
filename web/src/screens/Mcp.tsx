// MCP (frontend spec §8, gateway spec §1, §9): the gateway's connections,
// each with its state, its token (write-only) and the hosts it is mounted
// on; adding, editing and deleting one. Creating, deleting, setting a token
// and the changes that move a token's reach need a fresh step-up, which the
// client asks for when the server refuses.
import { useEffect, useRef, useState } from 'react'
import { messageOf } from '../api/errors'
import { hats as listHats, hosts as listHosts } from '../api/manage'
import { connections as listConnections, createConnection, deleteConnection, updateConnection } from '../api/mcp'
import { useClient } from '../app-client'
import ConfirmDialog from '../components/ConfirmDialog'
import ConnectionForm from '../components/ConnectionForm'
import Mounts from '../components/Mounts'
import TokenForm from '../components/TokenForm'
import type { HatItem, HostItem, McpConnectionItem, UpdateMcpConnectionRequest } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import {
  NEW_FORM,
  capped,
  changeOf,
  createRequest,
  dropsToken,
  formOf,
  kindLabel,
  serverName,
  statusLabel,
  type ConnectionForm as Form,
} from '../lib/mcp'
import { Text } from '../lib/text'
import '../manage.css'

type Pending =
  | { kind: 'delete'; connection: McpConnectionItem }
  | { kind: 'drop_token'; connection: McpConnectionItem; change: UpdateMcpConnectionRequest }

const titleId = (id: string) => `mcp-${id}`

/** The hat a new connection starts in: the default for new hosts, else the
 *  first one not being purged. */
function firstHat(hats: HatItem[]): string {
  const open = hats.filter((h) => !h.purging)
  return (open.find((h) => h.default_for_new_hosts) ?? open[0])?.id ?? ''
}

export default function Mcp() {
  const client = useClient()
  const list = useResource(() => listConnections(client))
  const hats = useResource(() => listHats(client))
  const hosts = useResource(() => listHosts(client))
  const [adding, setAdding] = useState(false)
  const [addBusy, setAddBusy] = useState(false)
  const [addError, setAddError] = useState<string | null>(null)
  // The connection being edited, as it was when its form opened: the form
  // starts from it, and a change is what the form changed of it, so an
  // answer folded in meanwhile (a mount) cannot make a stale field look
  // changed.
  const [editing, setEditing] = useState<McpConnectionItem | null>(null)
  // Refs, not state: two presses in one frame both see the first.
  const creating = useRef(false)
  const updating = useRef(false)
  const [editBusy, setEditBusy] = useState(false)
  const [editError, setEditError] = useState<string | null>(null)
  const [pending, setPending] = useState<Pending | null>(null)
  // The element to focus once the list has re-rendered, by id.
  const [focus, setFocus] = useState<string | null>(null)

  useEffect(() => {
    if (focus === null) return
    document.getElementById(focus)?.focus()
    setFocus(null)
  }, [focus, list.data, editing, adding])

  // As functions of the list held, so two answers landing together both
  // stay.
  const replace = (item: McpConnectionItem) =>
    list.set((prev) => (prev ?? []).map((c) => (c.id === item.id ? item : c)))

  const create = async (form: Form, slug: string, hatId: string) => {
    if (creating.current) return
    creating.current = true
    setAddBusy(true)
    setAddError(null)
    try {
      const item = await createConnection(client, createRequest(form, slug, hatId))
      list.set((prev) => [...(prev ?? []), item])
      setAdding(false)
      setFocus(titleId(item.id))
    } catch (err) {
      setAddError(messageOf(err))
    } finally {
      creating.current = false
      setAddBusy(false)
    }
  }

  const update = async (connection: McpConnectionItem, change: UpdateMcpConnectionRequest) => {
    if (updating.current) return
    updating.current = true
    setEditBusy(true)
    setEditError(null)
    try {
      replace(await updateConnection(client, connection.id, change))
      setEditing(null)
      setFocus(`${titleId(connection.id)}-edit`)
    } catch (err) {
      setEditError(messageOf(err))
    } finally {
      updating.current = false
      setEditBusy(false)
    }
  }

  const save = (connection: McpConnectionItem, base: McpConnectionItem, form: Form) => {
    const change = changeOf(base, form)
    if (Object.keys(change).length === 0) {
      setEditing(null)
      setFocus(`${titleId(connection.id)}-edit`)
      return
    }
    // The server deletes the token with another kind or origin: say so
    // first, when there is one to lose.
    if (connection.has_credential && dropsToken(connection, change)) setPending({ kind: 'drop_token', connection, change })
    else void update(connection, change)
  }

  // The 204 is the server saying the token is stored (gateway spec §9): only
  // `has_credential` changes, and nothing else is read back, so a mount
  // saved meanwhile is never undone by an older copy of the list.
  const tokenSaved = (id: string) =>
    list.set((prev) => (prev ?? []).map((c) => (c.id === id ? { ...c, has_credential: true } : c)))

  const hatName = (id: string) => hats.data?.find((h) => h.id === id)?.name

  return (
    <div className="manage">
      <header className="manage-head">
        <h1 id="mcp-heading" tabIndex={-1}>
          MCP
        </h1>
        <button
          type="button"
          id="mcp-add"
          className="btn btn-primary btn-sm"
          onClick={() => {
            setAddError(null)
            setAdding(true)
          }}
          disabled={adding || hats.data === null}
        >
          Add connection
        </button>
      </header>
      <section className="card notice" aria-label="How sessions get these">
        <p>
          A connection reaches the agent sessions in its hat on the hosts ticked below, as an MCP server named{' '}
          <code>hennery-&lt;slug&gt;</code>. Changes apply to new and resumed sessions.
        </p>
        <p>
          Codex, Claude run with your own CLI, and any other agent command (this may change): a session gets a
          connection only when both are in its host’s default hat, and it also loads your own MCP servers (such as <code>~/.codex</code>), so it is not
          isolated. Sessions in other hats on that host get none.
        </p>
      </section>
      {[list.error, hats.error, hosts.error].map(
        (shown, i) =>
          shown && (
            <p key={i} className="form-error" role="alert">
              <Text>{shown}</Text>
            </p>
          ),
      )}
      {adding && hats.data && (
        <section className="card" aria-label="Add a connection">
          <h2 className="card-title">Add a connection</h2>
          <ConnectionForm
            initial={NEW_FORM}
            create={{ hats: hats.data.filter((h) => !h.purging), hatId: firstHat(hats.data) }}
            submit="Create"
            busy={addBusy}
            error={addError}
            onSubmit={create}
            onCancel={() => {
              setAdding(false)
              setFocus('mcp-add')
            }}
          />
        </section>
      )}
      {list.data && list.data.length === 0 && !adding && <p className="empty">No connection yet.</p>}
      <ul className="cards" aria-label="Connections">
        {(list.data ?? []).map((connection) => (
          <ConnectionCard
            key={connection.id}
            connection={connection}
            hatName={hatName(connection.hat_id)}
            hosts={hosts.data}
            editing={editing?.id === connection.id ? editing : null}
            editBusy={editBusy}
            editError={editing?.id === connection.id ? editError : null}
            onEdit={() => {
              setEditError(null)
              setEditing(connection)
            }}
            onSave={(form, base) => save(connection, base, form)}
            onCancelEdit={() => {
              setEditing(null)
              setFocus(`${titleId(connection.id)}-edit`)
            }}
            onDelete={() => setPending({ kind: 'delete', connection })}
            onChanged={replace}
            onTokenSaved={() => tokenSaved(connection.id)}
          />
        ))}
      </ul>
      {pending?.kind === 'delete' && (
        <ConfirmDialog
          title="Delete this connection?"
          confirm="Delete"
          danger
          action={async () => {
            await deleteConnection(client, pending.connection.id)
            list.set((prev) => (prev ?? []).filter((c) => c.id !== pending.connection.id))
          }}
          onClose={() => setPending(null)}
          returnFocus={() => document.getElementById('mcp-heading')}
        >
          <p>
            <Text>{pending.connection.label}</Text> goes, with its token and its mounts. Sessions using{' '}
            <code>
              <Text>{serverName(pending.connection.slug)}</Text>
            </code>{' '}
            are refused from now on. A new connection needs its token set again.
          </p>
        </ConfirmDialog>
      )}
      {pending?.kind === 'drop_token' && (
        <ConfirmDialog
          title="Delete the stored token?"
          confirm="Save and delete the token"
          danger
          action={async () => {
            replace(await updateConnection(client, pending.connection.id, pending.change))
            setEditing(null)
          }}
          onClose={() => setPending(null)}
          returnFocus={() => document.getElementById(`${titleId(pending.connection.id)}-edit`)}
        >
          <p>
            Another server address or another kind of sign-in deletes the token stored for{' '}
            <Text>{pending.connection.label}</Text>. Set one again afterwards.
          </p>
        </ConfirmDialog>
      )}
    </div>
  )
}

interface CardProps {
  connection: McpConnectionItem
  hatName?: string
  hosts: HostItem[] | null
  /** The connection as its form opened, while it is edited. */
  editing: McpConnectionItem | null
  editBusy: boolean
  editError: string | null
  onEdit: () => void
  onSave: (form: Form, base: McpConnectionItem) => void
  onCancelEdit: () => void
  onDelete: () => void
  onChanged: (connection: McpConnectionItem) => void
  onTokenSaved: () => void
}

function ConnectionCard(props: CardProps) {
  const { connection: c, hatName, hosts, editing } = props
  const id = titleId(c.id)
  const tools =
    c.tool_allowlist === null ? 'Every tool' : c.tool_allowlist.length === 0 ? 'None' : c.tool_allowlist.join(', ')
  return (
    <li className="card connection" aria-labelledby={id}>
      <div className="card-head">
        <h2 className="card-title" id={id} tabIndex={-1}>
          <Text>{c.label}</Text>
        </h2>
        <span className={`state mcp-${c.status}`}>{statusLabel(c.status)}</span>
      </div>
      {c.status_note && (
        <p className="hint">
          <Text>{capped(c.status_note)}</Text>
        </p>
      )}
      {c.status === 'needs_auth' && c.cred_kind === 'static' && (
        <p className="hint">The server refused the token: set a new one below.</p>
      )}
      {c.status === 'needs_auth' && c.cred_kind === 'none' && (
        <p className="hint">The server asks for credentials: edit the connection to send a token.</p>
      )}
      <dl className="facts">
        <dt>Agents see</dt>
        <dd>
          <Text className="mono">{serverName(c.slug)}</Text>
        </dd>
        <dt>Server URL</dt>
        <dd>
          <Text className="mono">{c.url}</Text>
        </dd>
        <dt>Hat</dt>
        <dd>{hatName !== undefined ? <Text>{hatName}</Text> : <Text className="mono">{c.hat_id}</Text>}</dd>
        <dt>Authentication</dt>
        <dd>
          {c.cred_kind === 'static' ? (
            <>
              Token in <Text className="mono">{c.static_header}</Text>
              {c.static_prefix !== '' && (
                <>
                  {' '}
                  after <Text className="mono">{JSON.stringify(c.static_prefix)}</Text>
                </>
              )}
            </>
          ) : (
            kindLabel(c.cred_kind)
          )}
        </dd>
        {c.cred_kind === 'static' && (
          <>
            <dt>Token</dt>
            <dd>{c.has_credential ? 'Set' : 'Not set yet'}</dd>
          </>
        )}
        {c.account_label && (
          <>
            <dt>Signed in as</dt>
            <dd>
              <Text>{capped(c.account_label)}</Text>
            </dd>
          </>
        )}
        <dt>Tools</dt>
        <dd>
          <Text>{tools}</Text>
        </dd>
        {c.internal_network && (
          <>
            <dt>Network</dt>
            <dd>Internal network allowed</dd>
          </>
        )}
      </dl>
      {editing ? (
        <ConnectionForm
          initial={formOf(editing)}
          submit="Save"
          busy={props.editBusy}
          error={props.editError}
          onSubmit={(form) => props.onSave(form, editing)}
          onCancel={props.onCancelEdit}
        />
      ) : (
        <div className="card-actions">
          <button type="button" className="btn btn-ghost btn-sm" id={`${id}-edit`} onClick={props.onEdit}>
            Edit
          </button>
          <span className="spacer" />
          <button type="button" className="btn btn-danger btn-sm" onClick={props.onDelete}>
            Delete
          </button>
        </div>
      )}
      {c.cred_kind === 'static' && <TokenForm connection={c} hatName={hatName} onSaved={props.onTokenSaved} />}
      <Mounts connection={c} hosts={hosts} onChanged={props.onChanged} />
    </li>
  )
}
