import type { WorkflowRuntimeEvent, WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { hostClient } from '../../../host/hostClient'
import { requestWorkflows } from '../workflowClient'

const FLOW_EVENT_KINDS = new Set(['sent', 'delivered', 'waiting_user'])

/** Notifications are durable facts, never a Renderer-side scheduler or inferred transmission. */
export function useWorkflowExecution(instanceId: string) {
  const scope = useMemo(() => ({ instanceId }), [instanceId])
  const [snapshot, setSnapshot] = useState<{
    scope: typeof scope
    value: WorkflowRuntimeSnapshot
  } | null>(null)
  const [transmissions, setTransmissions] = useState<{
    scope: typeof scope
    events: WorkflowRuntimeEvent[]
  } | null>(null)
  const acceptRef = useRef<((value: WorkflowRuntimeSnapshot) => void) | null>(null)

  useEffect(() => {
    let disposed = false
    let cursor: number | null = null
    let pending = false
    let dirty = false
    const timers = new Set<ReturnType<typeof setTimeout>>()
    const accept = (value: WorkflowRuntimeSnapshot) => {
      if (
        disposed ||
        value.instanceId !== instanceId ||
        (cursor !== null && value.sequence <= cursor)
      )
        return
      const previousCursor = cursor
      cursor = value.sequence
      setSnapshot({ scope, value })
      // The initial snapshot is a baseline, so opening a diagram never replays its history.
      const fresh =
        previousCursor === null
          ? []
          : value.events.filter(
              (event) =>
                event.sequence > previousCursor &&
                FLOW_EVENT_KINDS.has(event.kind) &&
                event.flowIds.length > 0 &&
                Date.now() - event.createdAt < 10_000
            )
      if (!fresh.length) return
      setTransmissions((current) => ({
        scope,
        events: [...(current?.scope === scope ? current.events : []), ...fresh].slice(-64)
      }))
      const timer = setTimeout(() => {
        timers.delete(timer)
        const ids = new Set(fresh.map((event) => event.sequence))
        setTransmissions((current) =>
          current?.scope === scope
            ? { scope, events: current.events.filter((event) => !ids.has(event.sequence)) }
            : current
        )
      }, 2_500)
      timers.add(timer)
    }
    acceptRef.current = accept
    const refresh = async () => {
      if (disposed || document.visibilityState === 'hidden') return
      if (pending) {
        dirty = true
        return
      }
      pending = true
      try {
        const response = await requestWorkflows({
          operation: 'runtimeSnapshot',
          instanceId,
          ...(cursor === null ? {} : { afterSequence: cursor })
        })
        if (response.runtime) accept(response.runtime)
      } catch {
        /* Preserve the last known state; focus/polling retries only reads. */
      } finally {
        pending = false
        if (dirty && !disposed) {
          dirty = false
          void refresh()
        }
      }
    }
    const unsubscribe = hostClient.agent.onWorkflowRuntimeChanged?.(accept)
    const recover = () => {
      void refresh()
    }
    const interval = setInterval(recover, 5_000)
    window.addEventListener('focus', recover)
    document.addEventListener('visibilitychange', recover)
    recover()
    return () => {
      disposed = true
      acceptRef.current = null
      unsubscribe?.()
      clearInterval(interval)
      timers.forEach(clearTimeout)
      window.removeEventListener('focus', recover)
      document.removeEventListener('visibilitychange', recover)
    }
  }, [instanceId, scope])

  const completeUserInput = useCallback(
    async (inputId: string) => {
      const response = await requestWorkflows({
        operation: 'completeUserInput',
        instanceId,
        inputId
      })
      if (!response.runtime) throw new Error('Workflow completion did not return its saved state')
      acceptRef.current?.(response.runtime)
    },
    [instanceId]
  )
  const discardFailedInput = useCallback(
    async (inputId: string) => {
      const response = await requestWorkflows({
        operation: 'discardFailedInput',
        instanceId,
        inputId
      })
      if (!response.runtime) throw new Error('Workflow recovery did not return its saved state')
      acceptRef.current?.(response.runtime)
    },
    [instanceId]
  )

  return {
    snapshot: snapshot?.scope === scope ? snapshot.value : null,
    transmissions: transmissions?.scope === scope ? transmissions.events : [],
    completeUserInput,
    discardFailedInput
  }
}
