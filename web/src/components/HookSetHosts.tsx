// Which hosts get a hook set: the generic host checklist, with the hook-set
// routes and status words.
import { hookSetHosts, setHookSetHosts } from '../api/hookSets'
import { useClient } from '../app-client'
import type { HatItem, HookSetHostState, HostItem } from '../generated/protocol'
import { HostChecklist } from './SkillHosts'

function statusWord(state: HookSetHostState | undefined): string | null {
  switch (state?.status) {
    case undefined:
      return null
    case 'assigned':
      return 'assigned'
    case 'unsupported':
      return 'host too old'
    case 'offline':
      return 'host offline'
    default:
      return 'unknown'
  }
}

interface Props {
  hookSetId: string
  hosts: HostItem[] | null
  hats: HatItem[] | null
  onChanged?(): void
}

export default function HookSetHosts({ hookSetId, hosts, hats, onChanged }: Props) {
  const client = useClient()
  return (
    <HostChecklist
      subject={hookSetId}
      load={() => hookSetHosts(client, hookSetId)}
      save={(ids) => setHookSetHosts(client, hookSetId, ids)}
      legend="Hosts that get this hook set"
      statusWord={statusWord}
      hosts={hosts}
      hats={hats}
      onChanged={onChanged}
    />
  )
}
