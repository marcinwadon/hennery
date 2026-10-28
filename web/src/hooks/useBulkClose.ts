import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { useClient } from '../app-client'
import { BulkCloseController } from '../lib/bulkClose'

/** Mount once above the list rows so SSE/filter changes cannot lose results. */
export function useBulkClose() {
  const client = useClient()
  const controller = useMemo(() => new BulkCloseController(client), [client])
  useEffect(() => { controller.activate(); return controller.stop }, [controller])
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot)
  return { ...state, start: controller.start, prepareRetry: controller.prepareRetry, confirmRetry: controller.confirmRetry, checkUnknown: controller.checkUnknown, reconcile: controller.reconcile }
}
