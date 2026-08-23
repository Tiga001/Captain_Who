import { useCallback, useEffect, useRef, useState } from 'react'
import type { AutomationTask } from './automationTypes'
import type { AutomationErrorDetails, AutomationLoadStatus } from './automationTypes'
import { getAutomation, getAutomationErrorDetails } from './automationClient'
import {
  cacheAutomationTask,
  getCachedAutomationTask,
  subscribeAutomationCache
} from './automationCache'
import { subscribeAutomationRealtime } from './automationRealtime'

export interface UseAutomationDetailResult {
  task: AutomationTask | null
  status: AutomationLoadStatus
  error: AutomationErrorDetails | null
  isRefreshing: boolean
  refresh(): Promise<AutomationTask | null>
}

export function useAutomationDetail(
  automationId?: string | null,
  enabled = true
): UseAutomationDetailResult {
  const cached = automationId ? getCachedAutomationTask(automationId) : null
  const [task, setTask] = useState<AutomationTask | null>(cached)
  const [status, setStatus] = useState<AutomationLoadStatus>(
    enabled && automationId ? (cached ? 'ready' : 'loading') : 'idle'
  )
  const [error, setError] = useState<AutomationErrorDetails | null>(null)
  const [isRefreshing, setIsRefreshing] = useState(false)
  const mountedRef = useRef(false)
  const requestRef = useRef(0)
  const taskRef = useRef<AutomationTask | null>(cached)
  const refreshRef = useRef<() => Promise<AutomationTask | null>>(async () => null)

  const refresh = useCallback(async (): Promise<AutomationTask | null> => {
    if (!enabled || !automationId) return null
    const request = ++requestRef.current
    const cacheAtRequestStart = getCachedAutomationTask(automationId)
    setError(null)
    setIsRefreshing(taskRef.current !== null)
    if (!taskRef.current) setStatus('loading')
    try {
      const next = await getAutomation(automationId)
      if (!mountedRef.current || request !== requestRef.current) return null
      cacheAutomationTask(next)
      const authoritative = getCachedAutomationTask(automationId)
      taskRef.current = authoritative
      setTask(authoritative)
      setStatus('ready')
      setIsRefreshing(false)
      return authoritative
    } catch (caught) {
      if (!mountedRef.current || request !== requestRef.current) return null
      const authoritative = getCachedAutomationTask(automationId)
      // The request can have reached the server before a newer event/list response populated
      // the shared cache. In that case its late error must not erase the newer projection.
      if (authoritative && authoritative !== cacheAtRequestStart) {
        taskRef.current = authoritative
        setTask(authoritative)
        setError(null)
        setIsRefreshing(false)
        setStatus('ready')
        return authoritative
      }
      const details = getAutomationErrorDetails(caught)
      setError(details)
      setIsRefreshing(false)
      if (details.code === 'not_found') {
        taskRef.current = null
        setTask(null)
      }
      setStatus(taskRef.current ? 'ready' : 'error')
      return null
    }
  }, [automationId, enabled])

  useEffect(() => {
    refreshRef.current = refresh
  }, [refresh])

  useEffect(() => {
    mountedRef.current = true
    requestRef.current += 1
    const nextCached = automationId ? getCachedAutomationTask(automationId) : null
    taskRef.current = nextCached
    setTask(nextCached)
    setError(null)
    setStatus(enabled && automationId ? (nextCached ? 'ready' : 'loading') : 'idle')
    setIsRefreshing(false)
    if (enabled && automationId) void refresh()

    const stopCache = subscribeAutomationCache(() => {
      if (!mountedRef.current || !automationId) return
      const next = getCachedAutomationTask(automationId)
      taskRef.current = next
      setTask(next)
      if (next) {
        setStatus('ready')
        setError(null)
      }
    })
    const stopRealtime = enabled
      ? subscribeAutomationRealtime((signal) => {
          if (signal.type === 'resync' || signal.event.automationId === automationId) {
            if (signal.type === 'event' && signal.event.kind === 'deleted') {
              setTask(null)
              taskRef.current = null
              setStatus('ready')
              return
            }
            void refreshRef.current()
          }
        })
      : () => undefined
    return () => {
      mountedRef.current = false
      requestRef.current += 1
      stopCache()
      stopRealtime()
    }
  }, [automationId, enabled, refresh])

  return { task, status, error, isRefreshing, refresh }
}
