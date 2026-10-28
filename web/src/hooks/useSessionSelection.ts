import { useState } from 'react'
import { closeTargets, type BulkCloseTarget } from '../lib/bulkClose'

interface Selection { readonly scope: string; readonly generation: number; readonly targets: readonly BulkCloseTarget[] }

/** scopeKey includes raw search, hat, host, status, lifecycle and Hide closed;
 * exclude sort/group/collapse/viewport/pagination. This selection is unrelated
 * to the route's selected session. Rendering a new scope clears synchronously.
 * Call setChecked only for an explicit checkbox event, never for list updates.
 */
export function useSessionSelection(scopeKey: string) {
  const [selection, setSelection] = useState<Selection>({ scope: scopeKey, generation: 0, targets: [] })
  const current = selection.scope === scopeKey ? selection : { scope: scopeKey, generation: selection.generation + 1, targets: [] }
  if (selection.scope !== scopeKey) setSelection(current)
  const currentScope = (before: Selection) => before.scope === scopeKey && before.generation === current.generation
  const clear = () => setSelection(before => currentScope(before) ? { ...before, targets: [] } : before)
  const setChecked = (target: BulkCloseTarget, checked: boolean) => setSelection(before => {
    // An event from an old rendered scope cannot restore its selection.
    if (!currentScope(before)) return before
    const targets = before.targets.filter(held => held.id !== target.id)
    return { ...before, targets: checked && target.lifecycle !== undefined && target.lifecycle !== 'closed' ? closeTargets([...targets, target]) : targets }
  })
  // A missing first-page row is not deleted; prune only explicit closed facts.
  const reconcile = (summaries: ReadonlyMap<string, { lifecycle: string }>) => setSelection(before => {
    if (!currentScope(before)) return before
    const targets = before.targets.filter(target => summaries.get(target.id)?.lifecycle !== 'closed')
    return targets.length === before.targets.length ? before : { ...before, targets }
  })
  return { targets: current.targets, selectedIds: new Set(current.targets.map(target => target.id)), setChecked, clear, reconcile }
}
