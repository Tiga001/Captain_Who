import { useCallback, useEffect, useRef, useState } from 'react'
import type { AutomationAttention } from './automationTypes'
import type { AutomationErrorDetails, AutomationLoadStatus } from './automationTypes'
import {
  acknowledgeAutomationAttention,
  getAutomationAttention,
  getAutomationErrorDetails,
  hasAutomationHostApi
} from './automationClient'
import { subscribeAutomationRealtime, synchronizeAutomationSequence } from './automationRealtime'

export interface UseAutomationAttentionResult {
  items: AutomationAttention[]
  unreadCount: number
  status: AutomationLoadStatus
  error: AutomationErrorDetails | null
  nextCursor: string | null
  isLoadingMore: boolean
  refresh(): Promise<void>
  loadMore(): Promise<void>
  acknowledge(attentionId: string): Promise<AutomationAttention>
}

function mergeAttention(
  current: readonly AutomationAttention[],
  incoming: readonly AutomationAttention[]
): AutomationAttention[] {
  const byId = new Map(current.map((item) => [item.attentionId, item]))
  for (const item of incoming) {
    const previous = byId.get(item.attentionId)
    if (!previous || (item.readAt ?? 0) >= (previous.readAt ?? 0)) byId.set(item.attentionId, item)
  }
  return [...byId.values()].sort(
    (left, right) =>
      right.createdAt - left.createdAt || right.attentionId.localeCompare(left.attentionId)
  )
}

export function useAutomationAttention(enabled = true, limit = 50): UseAutomationAttentionResult {
  const apiAvailable = hasAutomationHostApi()
  const effectiveEnabled = enabled && apiAvailable
  const [items, setItems] = useState<AutomationAttention[]>([])
  const [unreadCount, setUnreadCount] = useState(0)
  const [status, setStatus] = useState<AutomationLoadStatus>(effectiveEnabled ? 'loading' : 'ready')
  const [error, setError] = useState<AutomationErrorDetails | null>(null)
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [isLoadingMore, setIsLoadingMore] = useState(false)
  const mountedRef = useRef(false)
  const requestRef = useRef(0)
  const cursorRef = useRef<string | null>(null)
  const itemsRef = useRef<AutomationAttention[]>([])
  const refreshRef = useRef<() => Promise<void>>(async () => undefined)
  const acknowledgeInFlightRef = useRef(new Map<string, Promise<AutomationAttention>>())

  useEffect(() => {
    cursorRef.current = nextCursor
  }, [nextCursor])

  useEffect(() => {
    itemsRef.current = items
  }, [items])

  const refresh = useCallback(async (): Promise<void> => {
    if (!effectiveEnabled) return
    const request = ++requestRef.current
    if (itemsRef.current.length === 0) setStatus('loading')
    try {
      const output = await getAutomationAttention({ limit })
      if (!mountedRef.current || request !== requestRef.current) return
      synchronizeAutomationSequence(output.lastSequence)
      setItems(output.items)
      setUnreadCount(output.unreadCount)
      setNextCursor(output.nextCursor)
      setError(null)
      setStatus('ready')
      setIsLoadingMore(false)
    } catch (caught) {
      if (!mountedRef.current || request !== requestRef.current) return
      setError(getAutomationErrorDetails(caught))
      setStatus(itemsRef.current.length > 0 ? 'ready' : 'error')
      setIsLoadingMore(false)
    }
  }, [effectiveEnabled, limit])

  useEffect(() => {
    refreshRef.current = refresh
  }, [refresh])

  useEffect(() => {
    mountedRef.current = true
    if (!effectiveEnabled) {
      setItems([])
      itemsRef.current = []
      setUnreadCount(0)
      setStatus('ready')
      setError(null)
      return () => {
        mountedRef.current = false
      }
    }
    void refresh()
    const unsubscribe = subscribeAutomationRealtime(() => void refreshRef.current())
    return () => {
      mountedRef.current = false
      requestRef.current += 1
      unsubscribe()
    }
  }, [effectiveEnabled, refresh])

  const loadMore = useCallback(async (): Promise<void> => {
    const cursor = cursorRef.current
    if (!effectiveEnabled || !cursor || isLoadingMore) return
    const request = ++requestRef.current
    setIsLoadingMore(true)
    try {
      const output = await getAutomationAttention({ cursor, limit })
      if (!mountedRef.current || request !== requestRef.current || cursorRef.current !== cursor) {
        return
      }
      synchronizeAutomationSequence(output.lastSequence)
      setItems((current) => mergeAttention(current, output.items))
      setUnreadCount(output.unreadCount)
      setNextCursor(output.nextCursor)
      setError(null)
      setIsLoadingMore(false)
    } catch (caught) {
      if (!mountedRef.current || request !== requestRef.current) return
      setError(getAutomationErrorDetails(caught))
      setIsLoadingMore(false)
    }
  }, [effectiveEnabled, isLoadingMore, limit])

  const acknowledge = useCallback((attentionId: string): Promise<AutomationAttention> => {
    const existing = acknowledgeInFlightRef.current.get(attentionId)
    if (existing) return existing
    const operation = acknowledgeAutomationAttention(attentionId)
      .then((output) => {
        if (mountedRef.current) {
          const changedFromUnread = itemsRef.current.some(
            (item) =>
              item.attentionId === attentionId &&
              item.readAt === null &&
              output.attention.readAt !== null
          )
          setItems((current) =>
            current.map((item) => (item.attentionId === attentionId ? output.attention : item))
          )
          if (changedFromUnread) setUnreadCount((current) => Math.max(0, current - 1))
          void refreshRef.current()
        }
        return output.attention
      })
      .finally(() => {
        acknowledgeInFlightRef.current.delete(attentionId)
      })
    acknowledgeInFlightRef.current.set(attentionId, operation)
    return operation
  }, [])

  return {
    items,
    unreadCount,
    status,
    error,
    nextCursor,
    isLoadingMore,
    refresh,
    loadMore,
    acknowledge
  }
}
