// MCP (frontend spec §8, gateway spec §1, §9): the gateway's connections,
// each with its state, its token (write-only) and the hosts it is mounted
// on; adding, editing and deleting one. Creating, deleting, setting a token
// and the changes that move a token's reach need a fresh step-up, which the
// client asks for when the server refuses.
import { useEffect, useRef, useState } from 'react'
import { messageOf } from '../api/errors'
import { hats as listHats, hosts as listHosts } from '../api/manage'
import { connections as listConnections, createConnection, deleteConnection, updateConnection, oauthRedirect } from '../api/mcp'
import { useClient } from '../app-client'
import ConfirmDialog from '../components/ConfirmDialog'
import ConnectionForm from '../components/ConnectionForm'
import Mounts from '../components/Mounts'
import TokenForm from '../components/TokenForm'
import StdioServers from '../components/StdioServers'
import OauthClientForm from '../components/OauthClientForm'
import OauthConnect from '../components/OauthConnect'
import McpStatus from '../components/McpStatus'
import { useMcpScope } from '../components/McpScope'
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

export default function Mcp({ stdio = false, oauth = false }: { stdio?: boolean; oauth?: boolean }) {
  const client = useClient()
  const scope = useMcpScope()
  const local = useResource(() => scope ? Promise.resolve([]) : listConnections(client), [!!scope])
  const list = scope ?? local
  const hats = useResource(() => listHats(client))
  const hosts = useResource(() => listHosts(client))
  const redirect = useResource(() => oauth ? oauthRedirect(client) : Promise.resolve(null), [oauth])
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
  const refresh = scope?.refresh ?? (async () => { const rows = await listConnections(client); local.set(rows); return rows })

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
    if ((connection.has_credential || connection.oauth?.client_id) && dropsToken(connection, change)) setPending({ kind: 'drop_token', connection, change })
    else void update(connection, change)
  }

  // The 204 is the server saying the token is stored (gateway spec §9): only
  // `has_credential` changes, and nothing else is read back, so a mount
  // saved meanwhile is never undone by an older copy of the list.
  const tokenSaved = (id: string) =>
    list.set((prev) => (prev ?? []).map((c) => (c.id === id ? { ...c, has_credential: true } : c)))

  const hatName = (id: string) => hats.data?.find((h) => h.id === id)?.name

  return (
    <div className="manage mcp-screen">
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
      <p className="manage-subtitle">Servers your sessions receive, by hat and host.</p>
      <section className="card notice" aria-label="How sessions get these">
        <p>
          A connection reaches the agent sessions in its hat on the hosts ticked below, as an MCP server named{' '}
          <code>hennery-&lt;slug&gt;</code>. Changes apply to new and resumed sessions.
        </p>
        {hosts.data?.some(h => !h.mcp_delivery) && <p>Codex may also load your own ~/.codex MCP servers.</p>}
      </section>
      {[list.error, hats.error, hosts.error, redirect.error].map(
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
            oauth={oauth} redirectUri={redirect.data?.redirect_uri}
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
      <h2 className="section-heading">Gateway connections</h2>
      <div className="mcp-inventory-head" aria-hidden="true"><span>Connection</span><span>Auth</span><span>Status</span><span>Hat</span><span>Hosts whose sessions get it</span><span /></div>
      <ul className="cards mcp-inventory" aria-label="Connections">
        {(list.data ?? []).map((connection) => (
          <ConnectionCard
            oauth={oauth}
            refresh={refresh}
            waiting={scope?.waiting}
            key={connection.id}
            connection={connection}
            hatName={hatName(connection.hat_id)}
            hat={hats.data?.find(h => h.id === connection.hat_id)}
            hosts={hosts.data}
            editing={editing?.id === connection.id ? editing : null}
            editBusy={editing?.id === connection.id && editBusy}
            locked={editing?.id !== connection.id && editBusy}
            editError={editing?.id === connection.id ? editError : null}
            onEdit={() => {
              // A pending save still owns this form, its error and its focus.
              if (updating.current) return
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
      <StdioServers enabled={stdio} hosts={hosts.data} hats={hats.data} compact />
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
            <Text>{pending.connection.label}</Text> goes, with {pending.connection.oauth ? 'its OAuth grant, client settings and mounts' : 'its token and its mounts'}. Sessions using{' '}
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
          {pending.connection.oauth && <p>This also deletes the OAuth grant and vendor client settings. Connect and enter client settings again afterwards.</p>}
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
  oauth: boolean
  refresh: () => Promise<McpConnectionItem[]>
  waiting?: (id: string, active: boolean) => void
  connection: McpConnectionItem
  hat?: HatItem
  hatName?: string
  hosts: HostItem[] | null
  /** The connection as its form opened, while it is edited. */
  editing: McpConnectionItem | null
  editBusy: boolean
  locked: boolean
  editError: string | null
  onEdit: () => void
  onSave: (form: Form, base: McpConnectionItem) => void
  onCancelEdit: () => void
  onDelete: () => void
  onChanged: (connection: McpConnectionItem) => void
  onTokenSaved: () => void
}

function ConnectionCard(props: CardProps) {
  const [clientRevision, setClientRevision] = useState(0)
  const [expanded, setExpanded] = useState(false)
  const { connection: c, hatName, hosts, editing } = props
  const id = titleId(c.id)
  const tools =
    c.tool_allowlist === null ? 'Every tool' : c.tool_allowlist.length === 0 ? 'None' : c.tool_allowlist.join(', ')
  return (
    <li className="card connection" aria-labelledby={id}>
      <div className="connection-summary">
      <div className="connection-identity">
        <h2 className="card-title" id={id} tabIndex={-1}>
          <Text>{c.label}</Text>
        </h2>
        <Text className="mono connection-url">{c.url}</Text>
      </div>
        <span className="connection-auth">{c.cred_kind === 'none' ? 'No auth' : kindLabel(c.cred_kind)}</span>
        <span className={`state mcp-${c.status}`}>{props.oauth && c.oauth && c.status === 'needs_auth' ? 'May need sign-in' : props.oauth && c.status === 'not_connected' && c.oauth ? 'Not connected' : statusLabel(c.status)}</span>
        <span className="connection-hat"><Text>{hatName ?? c.hat_id}</Text></span>
        <Mounts hat={props.hat} connection={c} hosts={hosts} onChanged={props.onChanged} compact />
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
      <details className="connection-settings" open={expanded || editing !== null} onToggle={event => setExpanded(event.currentTarget.open)}>
      <summary>Connection settings</summary>
      <dl className="facts">
        <dt>Agents see</dt>
        <dd>
          <Text className="mono">{serverName(c.slug)}</Text>
        </dd>
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
          oauth={props.oauth} redirectUri={c.oauth?.redirect_uri}
          initial={formOf(editing)}
          submit="Save"
          busy={props.editBusy}
          error={props.editError}
          onSubmit={(form) => props.onSave(form, editing)}
          onCancel={props.onCancelEdit}
        />
      ) : (
        <div className="card-actions">
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            id={`${id}-edit`}
            onClick={props.onEdit}
            aria-disabled={props.locked}
          >
            Edit
          </button>
          <span className="spacer" />
          <button type="button" className="btn btn-danger btn-sm" onClick={props.onDelete}>
            Delete
          </button>
        </div>
      )}
      {c.cred_kind === 'static' && <TokenForm connection={c} hatName={hatName} onSaved={props.onTokenSaved} />}
      {props.oauth && c.cred_kind === 'oauth_client' && <OauthClientForm connection={c} onChanged={item => { setClientRevision(n => n + 1); props.onChanged(item) }} />}
      {props.oauth && (c.cred_kind === 'oauth_client' || c.cred_kind === 'oauth_dcr') && <OauthConnect key={clientRevision} connection={c} refresh={props.refresh} waiting={props.waiting} />}
      {props.oauth && <McpStatus connection={c} onChanged={props.onChanged} />}
      </details>
    </li>
  )
}
