// Skills: the Agent Skills directories the collector keeps and the hosts
// that link them for Claude Code and Codex. An upload, a replacement and a
// deletion need a fresh step-up, which the client asks for when refused.
import { useRef, useState } from 'react'
import { messageOf } from '../api/errors'
import { hats as listHats, hosts as listHosts } from '../api/manage'
import { createSkill, deleteSkill, listSkills, replaceSkillArchive, skill as readSkill } from '../api/skills'
import { useClient } from '../app-client'
import ConfirmDialog from '../components/ConfirmDialog'
import Markdown from '../components/Markdown'
import SkillHosts from '../components/SkillHosts'
import type { SkillItem } from '../generated/protocol'
import { useResource } from '../hooks/useResource'
import { fileToBase64 } from '../lib/image'
import { Text } from '../lib/text'
import '../manage.css'

export default function Skills() {
  const client = useClient()
  const list = useResource(() => listSkills(client))
  const hats = useResource(() => listHats(client))
  const hosts = useResource(() => listHosts(client))
  const [selected, setSelected] = useState<string | null>(null)
  const detail = useResource(() => (selected ? readSkill(client, selected) : Promise.resolve(null)), [selected])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [deleting, setDeleting] = useState<SkillItem | null>(null)
  // Counts archive replacements, so the host states are read again.
  const [replaced, setReplaced] = useState(0)
  // Refs, not state: two presses in one frame both see the first.
  const working = useRef(false)
  const uploadRef = useRef<HTMLInputElement>(null)
  const replaceRef = useRef<HTMLInputElement>(null)

  const send = async (file: File | undefined, run: (data: string) => Promise<void>) => {
    if (!file || working.current) return
    working.current = true
    setBusy(true)
    setError(null)
    try {
      await run(await fileToBase64(file))
    } catch (err) {
      setError(messageOf(err))
    } finally {
      working.current = false
      setBusy(false)
    }
  }

  const upload = (file: File | undefined) =>
    send(file, async (data) => {
      const item = await createSkill(client, data)
      list.set((prev) => [...(prev ?? []).filter((s) => s.id !== item.id), item])
      setSelected(item.id)
    })

  const replace = (item: SkillItem, file: File | undefined) =>
    send(file, async (data) => {
      const next = await replaceSkillArchive(client, item.id, data)
      list.set((prev) => (prev ?? []).map((s) => (s.id === next.id ? next : s)))
      detail.reload()
      // The hosts' states were for the old archive: read them again.
      setReplaced((n) => n + 1)
    })

  // The list holds the host count: read it again after a host change.
  const hostsChanged = () => {
    void listSkills(client).then(list.set, () => {})
  }

  const current = list.data?.find((s) => s.id === selected) ?? null

  return (
    <div className="manage skills-screen">
      <header className="manage-head">
        <h1 id="skills-heading" tabIndex={-1}>Skills</h1>
        <input
          ref={uploadRef}
          type="file"
          accept=".tar.gz,.tgz"
          hidden
          aria-hidden="true"
          tabIndex={-1}
          data-testid="skill-input"
          onChange={(e) => {
            const file = e.target.files?.[0]
            e.target.value = ''
            void upload(file)
          }}
        />
        <button
          type="button"
          className="btn btn-primary btn-sm"
          onClick={() => uploadRef.current?.click()}
          aria-disabled={busy}
        >
          Upload skill
        </button>
      </header>
      <p className="manage-subtitle">Skill directories your hosts link for Claude Code and Codex.</p>
      {[list.error, hats.error, hosts.error, error].map(
        (shown, i) =>
          shown && (
            <p key={i} className="form-error" role="alert">
              <Text>{shown}</Text>
            </p>
          ),
      )}
      {list.data && list.data.length === 0 && <p className="empty">No skill yet.</p>}
      <ul className="cards" aria-label="Skills" aria-busy={busy}>
        {(list.data ?? []).map((s) => (
          <li key={s.id} className="card" aria-labelledby={`skill-${s.id}`}>
            <h2 className="card-title" id={`skill-${s.id}`}>
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                aria-pressed={selected === s.id}
                onClick={() => setSelected(selected === s.id ? null : s.id)}
              >
                <Text>{s.name}</Text>
              </button>
            </h2>
            <p><Text>{s.description}</Text></p>
            <p className="hint">
              <code>{s.content_hash.slice(0, 12)}</code> · Hosts: {s.host_ids.length}
            </p>
          </li>
        ))}
      </ul>
      {current && (
        <section className="card" aria-label="Skill details">
          <h2 className="section-heading"><Text>{current.name}</Text></h2>
          {detail.error && (
            <p className="form-error" role="alert"><Text>{detail.error}</Text></p>
          )}
          {detail.data && <Markdown>{detail.data.skill_md}</Markdown>}
          <SkillHosts
            key={current.id}
            skillId={current.id}
            hosts={hosts.data}
            hats={hats.data}
            refresh={replaced}
            onChanged={hostsChanged}
          />
          <input
            ref={replaceRef}
            type="file"
            accept=".tar.gz,.tgz"
            hidden
            aria-hidden="true"
            tabIndex={-1}
            data-testid="skill-replace-input"
            onChange={(e) => {
              const file = e.target.files?.[0]
              e.target.value = ''
              void replace(current, file)
            }}
          />
          <div className="card-actions">
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => replaceRef.current?.click()} aria-disabled={busy}>
              Replace archive
            </button>
            <span className="spacer" />
            <button type="button" className="btn btn-danger btn-sm" onClick={() => setDeleting(current)}>
              Delete
            </button>
          </div>
        </section>
      )}
      {deleting && (
        <ConfirmDialog
          title="Delete this skill?"
          confirm="Delete"
          danger
          action={async () => {
            await deleteSkill(client, deleting.id)
            list.set((prev) => (prev ?? []).filter((s) => s.id !== deleting.id))
            setSelected(null)
          }}
          onClose={() => setDeleting(null)}
          returnFocus={() => document.getElementById('skills-heading')}
        >
          <p>
            <Text>{deleting.name}</Text> goes from the catalogue, and hosts unlink it.
          </p>
        </ConfirmDialog>
      )}
    </div>
  )
}
