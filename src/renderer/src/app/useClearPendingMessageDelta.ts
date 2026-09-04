import { useCallback } from 'react'
import type { PendingMessageDelta } from './AppShellSupport'

export function useClearPendingMessageDelta(
  pendingMessageDeltaMap: Map<string, PendingMessageDelta>
) {
  return useCallback(
    (runId: string) => {
      const pendingDelta = pendingMessageDeltaMap.get(runId)
      if (!pendingDelta) return

      window.clearTimeout(pendingDelta.timerId)
      pendingMessageDeltaMap.delete(runId)
    },
    [pendingMessageDeltaMap]
  )
}
