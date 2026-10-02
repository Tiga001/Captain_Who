import type { WorkflowRuntimeEvent, WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { hostClient } from '../../../host/hostClient'
import { requestWorkflows } from '../workflowClient'

const FLOW_EVENT_KINDS = new Set(['sent', 'recalled'])

/** Notifications are durable facts, never a Renderer-side scheduler or inferred transmission. */
export function useWorkflowExecution(instanceId: string, foreground = true) {
  const scope = useMemo(() => ({ instanceId }), [instanceId])
  const [snapshot, setSnapshot] = useState<{
    scope: typeof scope
    value: WorkflowRuntimeSnapshot
  } | null>(null)
  const [transmissions, setTransmissions] = useState<{
    scope: typeof scope
    events: WorkflowRuntimeEvent[]
  } | null>(null)
  const foregroundRef = useRef(foreground)
  const baselineRequiredRef = useRef(false)
  const refreshRef = useRef<(() => void) | null>(null)
  useLayoutEffect(() => {
    if (foregroundRef.current !== foreground) baselineRequiredRef.current = true
    foregroundRef.current = foreground
  }, [foreground])
  useEffect(() => {
    if (!foreground) setTransmissions(null)
    else refreshRef.current?.()
  }, [foreground])

  useEffect(() => {
    let disposed = false
    let cursor: number | null = null
    let pending = false
    let dirty = false
    const timers = new Set<ReturnType<typeof setTimeout>>()
    const accept = (value: WorkflowRuntimeSnapshot, baseline = false) => {
      if (
        disposed ||
        value.instanceId !== instanceId ||
        (cursor !== null && value.sequence < cursor)
      )
        return
      const suppressAnimation = baselineRequiredRef.current
      if (baseline && foregroundRef.current && document.visibilityState !== 'hidden')
        baselineRequiredRef.current = false
      if (value.sequence === cursor) return
      const previousCursor = cursor
      cursor = value.sequence
      setSnapshot({ scope, value })
      // The initial snapshot is a baseline, so opening a diagram never replays its history.
      const fresh =
        previousCursor === null ||
        suppressAnimation ||
        !foregroundRef.current ||
        document.visibilityState === 'hidden'
          ? []
          : value.events.filter(
              (event) =>
                event.sequence > previousCursor &&
                FLOW_EVENT_KINDS.has(event.kind) &&
                event.sourceNodeId !== null &&
                event.targetNodeId !== null &&
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
        if (response.runtime) accept(response.runtime, true)
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
    refreshRef.current = recover
    const visibilityChanged = () => {
      if (document.visibilityState === 'hidden') {
        baselineRequiredRef.current = true
        setTransmissions(null)
        timers.forEach(clearTimeout)
        timers.clear()
      } else recover()
    }
    const interval = setInterval(recover, 5_000)
    window.addEventListener('focus', recover)
    document.addEventListener('visibilitychange', visibilityChanged)
    recover()
    return () => {
      disposed = true
      refreshRef.current = null
      unsubscribe?.()
      clearInterval(interval)
      timers.forEach(clearTimeout)
      window.removeEventListener('focus', recover)
      document.removeEventListener('visibilitychange', visibilityChanged)
    }
  }, [instanceId, scope])

  return {
    snapshot: snapshot?.scope === scope ? snapshot.value : null,
    transmissions: foreground && transmissions?.scope === scope ? transmissions.events : []
  }
}
