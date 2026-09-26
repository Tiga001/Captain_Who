import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { WorkflowInstance } from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'
import { isAssistantMessageGenerating } from '../../chat/assistantGeneration'
import type { ChatConversation } from '../../chat/chatTypes'
import { requestWorkflows } from '../workflowClient'

const REFRESH_DELAY_MS = 100
const PROGRESS_REFRESH_DELAY_MS = 1_000
const RECOVERY_INTERVAL_MS = 5_000

function instanceKey(instance: WorkflowInstance): string {
  return JSON.stringify([
    instance.id,
    instance.revision,
    instance.enabled,
    instance.bindings.map((binding) => [binding.nodeId, binding.conversationId])
  ])
}

/** Live presentation only: activity never reloads cards or changes a staged configuration. */
export function useWorkflowActivity(
  instances: readonly WorkflowInstance[],
  conversations: readonly ChatConversation[]
): ReadonlySet<string> {
  const latestInstances = useRef(instances)
  useLayoutEffect(() => {
    latestInstances.current = instances
  }, [instances])
  const [activity, setActivity] = useState<ReadonlyMap<string, boolean>>(new Map())
  const subscriptionKey = JSON.stringify(instances.map(instanceKey).sort())
  const localRunningRoots = useMemo(
    () =>
      new Set(
        conversations
          .filter((conversation) => conversation.messages.some(isAssistantMessageGenerating))
          .map((conversation) => conversation.id)
      ),
    [conversations]
  )
  const localActivityKey = JSON.stringify([...localRunningRoots].sort())
  const invalidate = useRef<(() => void) | null>(null)

  useEffect(() => {
    const enabled = latestInstances.current.filter((instance) => instance.enabled)
    const roots = new Set(
      enabled.flatMap((instance) => instance.bindings.map((b) => b.conversationId))
    )
    if (roots.size === 0) return

    let disposed = false
    let pending = false
    let refreshAgain = false
    let timer: ReturnType<typeof setTimeout> | null = null
    let scheduledAt = 0
    const visible = () => document.visibilityState !== 'hidden'

    const refresh = async () => {
      if (disposed || !visible()) return
      if (pending) {
        refreshAgain = true
        return
      }
      pending = true
      const requested = new Set(latestInstances.current.map(instanceKey))
      try {
        // The host includes descendants and conversations whose history is not loaded in the UI.
        const response = await requestWorkflows({ operation: 'listInstances' })
        if (disposed || !response.instances) return
        const current = new Set(latestInstances.current.map(instanceKey))
        const received = response.instances
          .map((instance) => [instanceKey(instance), instance.running] as const)
          .filter(([key]) => requested.has(key) && current.has(key))
        setActivity((previous) => {
          const next = new Map([...previous].filter(([key]) => current.has(key)))
          for (const [key, running] of received) next.set(key, running)
          if (
            next.size === previous.size &&
            [...next].every(([key, value]) => previous.get(key) === value)
          )
            return previous
          return next
        })
      } catch {
        // A failed background read keeps the last known state; recovery never flashes the card.
      } finally {
        pending = false
        if (refreshAgain && !disposed) {
          refreshAgain = false
          schedule()
        }
      }
    }
    const schedule = (delay = REFRESH_DELAY_MS) => {
      if (disposed || !visible()) return
      const deadline = Date.now() + delay
      if (timer !== null) {
        if (deadline >= scheduledAt) return
        clearTimeout(timer)
      }
      scheduledAt = deadline
      timer = setTimeout(() => {
        timer = null
        void refresh()
      }, delay)
    }
    invalidate.current = schedule
    const unsubscribeAgent = hostClient.agent.onEvent((event) => {
      // Streaming tokens and tool progress must not cause storage reads.
      if (
        event.type === 'started' ||
        event.type === 'done' ||
        (event.type === 'error' && !event.recoverable)
      )
        schedule()
    })
    const unsubscribeCollaboration = hostClient.agent.onCollaborationEvent((event) => {
      if (!roots.has(event.rootConversationId)) return
      if (event.kind === 'turn_started') schedule()
      else if (event.kind === 'turn_updated') {
        // Durable trace checkpoints can also report progress. Bound their reads independently
        // of token frequency while prioritizing explicit completion notifications.
        const settled =
          event.transmission?.kind === 'completion' ||
          event.activities.some((activity) =>
            ['completed', 'failed', 'interrupted'].includes(activity.semantic)
          )
        schedule(settled ? REFRESH_DELAY_MS : PROGRESS_REFRESH_DELAY_MS)
      }
    })
    const recover = () => schedule()
    const unsubscribeResync = hostClient.agent.onCollaborationResync(recover)
    const interval = setInterval(recover, RECOVERY_INTERVAL_MS)
    window.addEventListener('focus', recover)
    document.addEventListener('visibilitychange', recover)
    return () => {
      disposed = true
      invalidate.current = null
      if (timer !== null) clearTimeout(timer)
      clearInterval(interval)
      unsubscribeAgent()
      unsubscribeCollaboration()
      unsubscribeResync()
      window.removeEventListener('focus', recover)
      document.removeEventListener('visibilitychange', recover)
    }
  }, [subscriptionKey])

  useEffect(() => {
    // Root messages update immediately; a completion also reconciles a previously running snapshot.
    invalidate.current?.()
  }, [localActivityKey])

  return useMemo(
    () =>
      new Set(
        instances
          .filter(
            (instance) =>
              instance.enabled &&
              (instance.bindings.some((binding) => localRunningRoots.has(binding.conversationId)) ||
                (activity.get(instanceKey(instance)) ?? instance.running))
          )
          .map((instance) => instance.id)
      ),
    [instances, localRunningRoots, activity]
  )
}
