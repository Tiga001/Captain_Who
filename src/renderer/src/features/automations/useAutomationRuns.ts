import { useCallback, useEffect, useRef, useState } from 'react'
import type { AutomationRun } from './automationTypes'
import type { AutomationErrorDetails, AutomationLoadStatus } from './automationTypes'
import { getAutomationErrorDetails, listAutomationRuns } from './automationClient'
import { subscribeAutomationRealtime } from './automationRealtime'

export interface UseAutomationRunsResult {
  runs: AutomationRun[]
  status: AutomationLoadStatus
  error: AutomationErrorDetails | null
  nextCursor: string | null
  isRefreshing: boolean
  isLoadingMore: boolean
  refresh(): Promise<void>
  loadMore(): Promise<void>
}

function mergeRuns(
  current: readonly AutomationRun[],
  incoming: readonly AutomationRun[]
): AutomationRun[] {
  const byId = new Map(current.map((run) => [run.runId, run]))
  for (const run of incoming) {
    const previous = byId.get(run.runId)
    if (!previous || run.updatedAt >= previous.updatedAt) byId.set(run.runId, run)
  }
  return [...byId.values()].sort(
    (left, right) => right.createdAt - left.createdAt || right.runId.localeCompare(left.runId)
  )
}

export function useAutomationRuns(
  automationId?: string | null,
  enabled = true,
  limit = 30
): UseAutomationRunsResult {
  const [runs, setRuns] = useState<AutomationRun[]>([])
  const [status, setStatus] = useState<AutomationLoadStatus>('idle')
  const [error, setError] = useState<AutomationErrorDetails | null>(null)
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [isRefreshing, setIsRefreshing] = useState(false)
  const [isLoadingMore, setIsLoadingMore] = useState(false)
  const mountedRef = useRef(false)
  const requestRef = useRef(0)
  const cursorRef = useRef<string | null>(null)
  const runsRef = useRef<AutomationRun[]>([])
  const refreshRef = useRef<() => Promise<void>>(async () => undefined)

  useEffect(() => {
    cursorRef.current = nextCursor
  }, [nextCursor])

  useEffect(() => {
    runsRef.current = runs
  }, [runs])

  const refresh = useCallback(async (): Promise<void> => {
    if (!enabled || !automationId) return
    const request = ++requestRef.current
    setError(null)
    setIsRefreshing(runsRef.current.length > 0)
    if (runsRef.current.length === 0) setStatus('loading')
    try {
      const output = await listAutomationRuns(automationId, { limit })
      if (!mountedRef.current || request !== requestRef.current) return
      setRuns((current) => mergeRuns(current, output.runs))
      setNextCursor(output.nextCursor)
      setStatus('ready')
      setIsRefreshing(false)
      setIsLoadingMore(false)
    } catch (caught) {
      if (!mountedRef.current || request !== requestRef.current) return
      setError(getAutomationErrorDetails(caught))
      setStatus(runsRef.current.length > 0 ? 'ready' : 'error')
      setIsRefreshing(false)
      setIsLoadingMore(false)
    }
  }, [automationId, enabled, limit])

  useEffect(() => {
    refreshRef.current = refresh
  }, [refresh])

  useEffect(() => {
    mountedRef.current = true
    requestRef.current += 1
    setRuns([])
    runsRef.current = []
    setNextCursor(null)
    setError(null)
    setStatus(enabled && automationId ? 'loading' : 'idle')
    if (enabled && automationId) void refresh()
    const unsubscribe = enabled
      ? subscribeAutomationRealtime((signal) => {
          if (signal.type === 'resync' || signal.event.automationId === automationId) {
            void refreshRef.current()
          }
        })
      : () => undefined
    return () => {
      mountedRef.current = false
      requestRef.current += 1
      unsubscribe()
    }
  }, [automationId, enabled, refresh])

  const loadMore = useCallback(async (): Promise<void> => {
    const cursor = cursorRef.current
    if (!enabled || !automationId || !cursor || isLoadingMore) return
    const request = ++requestRef.current
    setIsLoadingMore(true)
    try {
      const output = await listAutomationRuns(automationId, { cursor, limit })
      if (!mountedRef.current || request !== requestRef.current || cursorRef.current !== cursor) {
        return
      }
      setRuns((current) => mergeRuns(current, output.runs))
      setNextCursor(output.nextCursor)
      setError(null)
      setIsLoadingMore(false)
    } catch (caught) {
      if (!mountedRef.current || request !== requestRef.current) return
      setError(getAutomationErrorDetails(caught))
      setIsLoadingMore(false)
    }
  }, [automationId, enabled, isLoadingMore, limit])

  return {
    runs,
    status,
    error,
    nextCursor,
    isRefreshing,
    isLoadingMore,
    refresh,
    loadMore
  }
}
