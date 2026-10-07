import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { WorkflowActivity, WorkflowInstance } from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'
import { isAssistantMessageGenerating } from '../../chat/assistantGeneration'
import type { ChatConversation } from '../../chat/chatTypes'
import { requestWorkflows } from '../workflowClient'

const REFRESH_DELAY_MS = 100
const PROGRESS_REFRESH_DELAY_MS = 1_000
const RECOVERY_INTERVAL_MS = 5_000

interface ActivitySnapshot {
  running: boolean
  activity: WorkflowActivity | null
}

function latestActivity(
  instance: WorkflowInstance,
  snapshot: ActivitySnapshot | undefined
): WorkflowActivity | null {
  const incoming = instance.activity
  if (!snapshot) return incoming ?? null
  const cached = snapshot.activity
  if (!cached) return null
  // A parent reload may carry a newer interval without changing the configuration revision.
  if (incoming && incoming.startedAt > cached.startedAt) return incoming
  return cached
}

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
  conversations: readonly ChatConversation[],
  foreground = true
): {
  runningInstanceIds: ReadonlySet<string>
  activityByInstanceId: ReadonlyMap<string, WorkflowActivity | null>
} {
  const latestInstances = useRef(instances)
  const foregroundRef = useRef(foreground)
  const previousForeground = useRef(foreground)
  useLayoutEffect(() => {
    latestInstances.current = instances
    foregroundRef.current = foreground
  }, [instances, foreground])
  const [activity, setActivity] = useState<ReadonlyMap<string, ActivitySnapshot>>(new Map())
  const subscriptionKey = JSON.stringify(instances.map(instanceKey).sort())
  const intervalKey = JSON.stringify(
    instances.map((instance) => [
      instance.id,
      instance.activity?.startedAt,
      instance.activity?.completedAt
    ])
  )
  const rootsKey = JSON.stringify(
    [
      ...new Set(
        instances
          .filter(
            (instance) =>
              instance.enabled ||
              latestActivity(instance, activity.get(instanceKey(instance)))?.completedAt === null
          )
          .flatMap((instance) => instance.bindings.map((binding) => binding.conversationId))
      )
    ].sort()
  )
  const openActivityKey = JSON.stringify(
    instances
      .filter(
        (instance) =>
          latestActivity(instance, activity.get(instanceKey(instance)))?.completedAt === null
      )
      .map((instance) => instance.id)
      .sort()
  )
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
    // Disabling an organization does not stop its already active turns.
    const roots = new Set<string>(JSON.parse(rootsKey))
    // Admission freezes activity ownership, so an active interval can outlive every binding.
    if (roots.size === 0 && openActivityKey === '[]') return

    let disposed = false
    let pending = false
    let refreshAgain = false
    let timer: ReturnType<typeof setTimeout> | null = null
    let scheduledAt = 0
    const visible = () => foregroundRef.current && document.visibilityState !== 'hidden'

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
          .map(
            (instance) =>
              [
                instanceKey(instance),
                { running: instance.running, activity: instance.activity ?? null }
              ] as const
          )
          .filter(([key]) => requested.has(key) && current.has(key))
        setActivity((previous) => {
          const next = new Map([...previous].filter(([key]) => current.has(key)))
          for (const [key, snapshot] of received) next.set(key, snapshot)
          if (
            next.size === previous.size &&
            [...next].every(([key, value]) => {
              const before = previous.get(key)
              return (
                before?.running === value.running &&
                before?.activity?.startedAt === value.activity?.startedAt &&
                before?.activity?.completedAt === value.activity?.completedAt
              )
            })
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
    schedule()
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
  }, [subscriptionKey, rootsKey, openActivityKey])

  useEffect(() => {
    // Root messages update immediately; a completion also reconciles a previously running snapshot.
    invalidate.current?.()
  }, [localActivityKey, intervalKey])
  useEffect(() => {
    if (foreground && !previousForeground.current) invalidate.current?.()
    previousForeground.current = foreground
  }, [foreground])

  return useMemo(
    () => ({
      runningInstanceIds: new Set(
        instances
          .filter(
            (instance) =>
              instance.enabled &&
              (instance.bindings.some((binding) => localRunningRoots.has(binding.conversationId)) ||
                (activity.get(instanceKey(instance))?.running ?? instance.running))
          )
          .map((instance) => instance.id)
      ),
      activityByInstanceId: new Map(
        instances.map((instance) => [
          instance.id,
          latestActivity(instance, activity.get(instanceKey(instance)))
        ])
      )
    }),
    [instances, localRunningRoots, activity]
  )
}
