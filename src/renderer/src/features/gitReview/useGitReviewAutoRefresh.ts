import { useEffect, useLayoutEffect, useRef } from 'react'
import type { GitReviewTarget } from '@mycopilot/protocol'
import { onGitReviewAgentEvent } from './gitReviewClient'

const REFRESH_DELAY_MS = 200

export function useGitReviewAutoRefresh(
  projectId: string,
  isActive: boolean,
  target: GitReviewTarget,
  sourceKey: string,
  isLoading: boolean,
  refresh: () => Promise<void>
): void {
  const loadingRef = useRef(isLoading)
  useLayoutEffect(() => {
    loadingRef.current = isLoading
  }, [isLoading])
  const conversationId = target.kind === 'lastTurn' ? target.conversationId : undefined

  useEffect(() => {
    if (!isActive || !projectId || target.kind === 'commit') return undefined
    const relevantRuns = new Set<string>()
    let timer: ReturnType<typeof setTimeout> | undefined
    let dirty = false
    let refreshing = false
    let disposed = false
    const rememberRun = (runId: string): void => {
      relevantRuns.add(runId)
      // Bound bookkeeping even when a transport loses a terminal event.
      if (relevantRuns.size > 128) {
        const oldest = relevantRuns.values().next().value
        if (oldest !== undefined) relevantRuns.delete(oldest)
      }
    }
    const schedule = (): void => {
      if (disposed || timer !== undefined) return
      timer = setTimeout(() => {
        timer = undefined
        if (!dirty || disposed) return
        // Wait for the current snapshot instead of invalidating an in-flight request on every edit.
        if (loadingRef.current || refreshing) {
          schedule()
          return
        }
        dirty = false
        refreshing = true
        void refresh()
          .catch(() => undefined)
          .finally(() => {
            refreshing = false
            if (dirty) schedule()
          })
      }, REFRESH_DELAY_MS)
    }
    const unsubscribe = onGitReviewAgentEvent((event) => {
      if (event.type === 'done') {
        if (!relevantRuns.delete(event.runId)) return
      } else if (event.type === 'file_change_updated') {
        if (
          event.fileChange.projectId !== projectId ||
          (conversationId && event.fileChange.conversationId !== conversationId)
        )
          return
        rememberRun(event.runId)
        if (!['applied', 'already_applied', 'outcome_unknown'].includes(event.fileChange.status))
          return
      } else if (event.type === 'command_exited' || event.type === 'command_interrupted') {
        if (
          event.projectId !== projectId ||
          (conversationId && event.conversationId !== conversationId)
        )
          return
        rememberRun(event.runId)
      } else {
        return
      }
      dirty = true
      schedule()
    })
    return () => {
      disposed = true
      if (timer !== undefined) clearTimeout(timer)
      unsubscribe()
    }
  }, [conversationId, isActive, projectId, refresh, sourceKey, target.kind])
}
