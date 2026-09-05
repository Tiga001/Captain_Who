import { useCallback, useEffect, useRef, useState } from 'react'
import type { AgentManualContextCompactionOperation } from '@mycopilot/protocol'
import {
  getManualContextCompactionStatus,
  onManualContextCompaction,
  startManualContextCompaction
} from '../agent/agentClient'

export function mergeManualCompactionOperations(
  current: readonly AgentManualContextCompactionOperation[],
  incoming: readonly AgentManualContextCompactionOperation[]
): AgentManualContextCompactionOperation[] {
  const byId = new Map(current.map((operation) => [operation.operationId, operation]))
  for (const operation of incoming) {
    const previous = byId.get(operation.operationId)
    if (
      previous &&
      (previous.updatedAt > operation.updatedAt ||
        (previous.status !== 'running' && operation.status === 'running') ||
        (previous.status !== 'running' && previous.isBusy === false && operation.isBusy === true))
    )
      continue
    byId.set(operation.operationId, operation)
  }
  return [...byId.values()]
    .sort((a, b) => a.startedAt - b.startedAt || a.operationId.localeCompare(b.operationId))
    .slice(-50)
}

export function useManualContextCompaction(conversationId?: string) {
  const [byConversation, setByConversation] = useState<
    Record<string, AgentManualContextCompactionOperation[]>
  >({})
  const [loaded, setLoaded] = useState<Record<string, boolean>>({})
  const [starting, setStarting] = useState<Record<string, boolean>>({})
  const pendingRequests = useRef(new Map<string, string>())
  const inFlight = useRef(new Set<string>())
  const merge = useCallback((id: string, incoming: AgentManualContextCompactionOperation[]) => {
    setByConversation((current) => ({
      ...current,
      [id]: mergeManualCompactionOperations(current[id] ?? [], incoming)
    }))
  }, [])
  const load = useCallback(
    async (id: string) => {
      const result = await getManualContextCompactionStatus({ conversationId: id })
      merge(id, result.operations)
      setLoaded((current) => ({ ...current, [id]: true }))
    },
    [merge]
  )
  useEffect(
    () => onManualContextCompaction((operation) => merge(operation.conversationId, [operation])),
    [merge]
  )
  useEffect(() => {
    if (!conversationId) return
    void load(conversationId).catch(() => {
      /* Retry through polling; don't claim idle before hydration. */
    })
    const timer = window.setInterval(() => {
      void load(conversationId).catch(() => {})
    }, 3000)
    return () => window.clearInterval(timer)
  }, [conversationId, load])
  const start = useCallback(
    async (id: string) => {
      if (inFlight.current.has(id)) return
      inFlight.current.add(id)
      setStarting((current) => ({ ...current, [id]: true }))
      const requestId = pendingRequests.current.get(id) ?? crypto.randomUUID()
      pendingRequests.current.set(id, requestId)
      try {
        const operation = await startManualContextCompaction({ conversationId: id, requestId })
        merge(id, [operation])
        pendingRequests.current.delete(id)
      } finally {
        inFlight.current.delete(id)
        setStarting((current) => ({ ...current, [id]: false }))
      }
    },
    [merge]
  )
  const operations = conversationId ? (byConversation[conversationId] ?? []) : []
  return {
    operations,
    start,
    ready: !conversationId || Boolean(loaded[conversationId]),
    isRunning:
      Boolean(conversationId && starting[conversationId]) ||
      operations.some((operation) => operation.status === 'running' || operation.isBusy)
  }
}
