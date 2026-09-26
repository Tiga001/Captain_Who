import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { AgentTreeSnapshot, WorkflowInstance } from '@mycopilot/protocol'
import { hostCollaborationDataSource } from '../../agentCollaboration/collaborationClient'
import { isAssistantMessageGenerating } from '../../chat/assistantGeneration'
import type { ChatConversation } from '../../chat/chatTypes'

const ACTIVE_STATUSES = new Set(['running', 'waiting_approval'])
interface MonitorState {
  key: string
  running: ReadonlySet<string>
  waitingApproval: ReadonlySet<string>
}

const sameIds = (left: ReadonlySet<string>, right: ReadonlySet<string>) =>
  left.size === right.size && [...left].every((id) => right.has(id))

/** Read-only live state; disabling a workflow does not stop its conversations. */
export function useWorkflowMonitor(
  instance: WorkflowInstance,
  conversations: readonly ChatConversation[]
): {
  runningConversationIds: ReadonlySet<string>
  waitingApprovalConversationIds: ReadonlySet<string>
} {
  const bindingKey = JSON.stringify(
    instance.bindings.map((binding) => binding.conversationId).sort()
  )
  const key = JSON.stringify([instance.id, bindingKey])
  const roots = useMemo(() => new Set<string>(JSON.parse(bindingKey)), [bindingKey])
  const latestConversations = useRef(conversations)
  useLayoutEffect(() => {
    latestConversations.current = conversations
  }, [conversations])
  const [state, setState] = useState<MonitorState>({
    key,
    running: new Set(),
    waitingApproval: new Set()
  })
  const invalidate = useRef<(() => void) | null>(null)
  const localRunning = useMemo(
    () =>
      new Set(
        conversations
          .filter(
            (conversation) =>
              roots.has(conversation.id) && conversation.messages.some(isAssistantMessageGenerating)
          )
          .map((conversation) => conversation.id)
      ),
    [conversations, roots]
  )
  const localKey = JSON.stringify([...localRunning].sort())

  useEffect(() => {
    if (!roots.size) return
    let disposed = false
    const activeAgents = new Map<string, Set<string>>()
    const waitingApprovalAgents = new Map<string, Set<string>>()
    const sequences = new Map<string, number>()
    const revisions = new Map<string, number>()
    const rootAgentIds = new Map<string, string>()
    const runs = new Map<string, { root: string; agent: string }>()
    const currentRuns = new Map<string, string>()
    const pending = new Set<string>()
    const again = new Set<string>()
    const timers = new Map<string, { deadline: number; handle: ReturnType<typeof setTimeout> }>()
    const visible = () => document.visibilityState !== 'hidden'
    const publish = () => {
      if (disposed) return
      const running = new Set([...activeAgents].filter(([, ids]) => ids.size).map(([id]) => id))
      const waitingApproval = new Set(
        [...waitingApprovalAgents].filter(([, ids]) => ids.size).map(([id]) => id)
      )
      setState((previous) => {
        if (
          previous.key === key &&
          sameIds(previous.running, running) &&
          sameIds(previous.waitingApproval, waitingApproval)
        )
          return previous
        return { key, running, waitingApproval }
      })
    }
    const acceptTree = (root: string, tree: AgentTreeSnapshot | null) => {
      if (tree && tree.rootConversationId !== root) return
      if (tree) rootAgentIds.set(root, tree.rootAgentId)
      if (tree && tree.lastSequence < (sequences.get(root) ?? 0)) {
        schedule(root, 1_000)
        return
      }
      sequences.set(root, tree?.lastSequence ?? 0)
      activeAgents.set(
        root,
        new Set(
          tree?.agents
            .filter(
              (agent) => agent.lifecycle === 'active' && ACTIVE_STATUSES.has(agent.displayStatus)
            )
            .map((agent) => agent.agentId)
        )
      )
      waitingApprovalAgents.set(
        root,
        new Set(
          tree?.agents
            .filter(
              (agent) => agent.lifecycle === 'active' && agent.displayStatus === 'waiting_approval'
            )
            .map((agent) => agent.agentId)
        )
      )
      publish()
    }
    const refresh = async (root: string) => {
      if (disposed || !visible()) return
      if (pending.has(root)) {
        again.add(root)
        return
      }
      pending.add(root)
      const revision = revisions.get(root) ?? 0
      try {
        const tree = await hostCollaborationDataSource.getTree({ rootConversationId: root })
        if (!disposed) {
          if ((revisions.get(root) ?? 0) === revision) acceptTree(root, tree)
          else schedule(root)
        }
      } catch {
        /* Failed background reads retain the last known state. */
      } finally {
        pending.delete(root)
        if (again.delete(root) && !disposed) schedule(root)
      }
    }
    function schedule(root: string, delay = 100) {
      if (disposed || !visible()) return
      const deadline = Date.now() + delay
      const timer = timers.get(root)
      if (timer && timer.deadline <= deadline) return
      if (timer) clearTimeout(timer.handle)
      timers.set(root, {
        deadline,
        handle: setTimeout(() => {
          timers.delete(root)
          void refresh(root)
        }, delay)
      })
    }
    const recover = () => roots.forEach((root) => schedule(root))
    invalidate.current = recover
    const unsubscribe = hostCollaborationDataSource.subscribe((event) => {
      const root = event.rootConversationId
      if (!roots.has(root) || event.sequence <= (sequences.get(root) ?? 0)) return
      revisions.set(root, (revisions.get(root) ?? 0) + 1)
      rootAgentIds.set(root, event.rootAgentId)
      if (event.runId) {
        runs.set(event.runId, { root, agent: event.agentId })
        if (event.kind === 'turn_started' || !currentRuns.has(event.agentId))
          currentRuns.set(event.agentId, event.runId)
        if (runs.size > 1_000) runs.delete(runs.keys().next().value!)
      }
      sequences.set(root, event.sequence)
      const active = new Set(activeAgents.get(root))
      const waiting = new Set(waitingApprovalAgents.get(root))
      // A delayed terminal event from an older run must not clear its replacement run.
      const isCurrentRun = (agent: string) =>
        !currentRuns.has(agent) || currentRuns.get(agent) === event.runId
      if (event.kind === 'turn_started') {
        active.add(event.agentId)
        waiting.delete(event.agentId)
      }
      if (
        event.transmission?.kind === 'completion' &&
        event.transmission.sourceAgentId &&
        isCurrentRun(event.transmission.sourceAgentId)
      ) {
        active.delete(event.transmission.sourceAgentId)
        waiting.delete(event.transmission.sourceAgentId)
      }
      for (const activity of event.activities) {
        if (!isCurrentRun(activity.agentId)) continue
        if (activity.semantic === 'started' || activity.semantic === 'waiting_approval') {
          active.add(activity.agentId)
          if (activity.semantic === 'waiting_approval') waiting.add(activity.agentId)
          else waiting.delete(activity.agentId)
        } else if (['completed', 'failed', 'interrupted'].includes(activity.semantic)) {
          active.delete(activity.agentId)
          waiting.delete(activity.agentId)
        }
      }
      activeAgents.set(root, active)
      waitingApprovalAgents.set(root, waiting)
      publish()
      // Progress checkpoints are coalesced independently of streamed text.
      schedule(
        root,
        event.kind === 'turn_updated' &&
          !event.transmission &&
          !event.activities.some((a) => ['completed', 'failed', 'interrupted'].includes(a.semantic))
          ? 1_000
          : 100
      )
    })
    const unsubscribeAgent = hostCollaborationDataSource.subscribeAgentEvents?.((event) => {
      if (
        event.type !== 'started' &&
        event.type !== 'done' &&
        event.type !== 'error' &&
        event.type !== 'state' &&
        event.type !== 'approval_required'
      )
        return
      if (event.type === 'error' && event.recoverable) return
      const runId = event.runId
      if (!runId) {
        recover()
        return
      }
      let owner = runs.get(runId)
      if (!owner) {
        const conversation = latestConversations.current.find(
          (item) =>
            roots.has(item.id) && item.messages.some((message) => message.agentRun?.runId === runId)
        )
        const agent = conversation && rootAgentIds.get(conversation.id)
        if (conversation && agent) {
          const latestRun = [...conversation.messages]
            .reverse()
            .find((message) => message.agentRun?.runId)?.agentRun?.runId
          if (event.type !== 'started' && latestRun && latestRun !== runId) {
            schedule(conversation.id)
            return
          }
          owner = { root: conversation.id, agent }
        }
      }
      if (owner) {
        const currentRun = currentRuns.get(owner.agent)
        if (event.type !== 'started' && currentRun && currentRun !== runId) {
          schedule(owner.root)
          return
        }
        if (event.type === 'started' || !currentRun) currentRuns.set(owner.agent, runId)
        const status =
          event.type === 'state'
            ? event.state.status
            : event.type === 'done'
              ? event.status
              : event.type === 'approval_required'
                ? 'waiting_for_approval'
                : event.type === 'started'
                  ? 'running'
                  : 'failed'
        const isActive =
          status === 'running' ||
          status === 'waiting_for_approval' ||
          status === 'waiting_for_user_input'
        if (!isActive) runs.delete(runId)
        revisions.set(owner.root, (revisions.get(owner.root) ?? 0) + 1)
        const active = new Set(activeAgents.get(owner.root))
        const waiting = new Set(waitingApprovalAgents.get(owner.root))
        if (isActive) active.add(owner.agent)
        else active.delete(owner.agent)
        if (status === 'waiting_for_approval') waiting.add(owner.agent)
        else waiting.delete(owner.agent)
        activeAgents.set(owner.root, active)
        waitingApprovalAgents.set(owner.root, waiting)
        publish()
        schedule(owner.root)
      } else recover()
    })
    const unsubscribeResync = hostCollaborationDataSource.subscribeResync(recover)
    const interval = setInterval(recover, 5_000)
    window.addEventListener('focus', recover)
    document.addEventListener('visibilitychange', recover)
    roots.forEach((root) => {
      void refresh(root)
    })
    return () => {
      disposed = true
      invalidate.current = null
      timers.forEach(({ handle }) => clearTimeout(handle))
      clearInterval(interval)
      unsubscribe()
      unsubscribeAgent?.()
      unsubscribeResync()
      window.removeEventListener('focus', recover)
      document.removeEventListener('visibilitychange', recover)
    }
  }, [key, roots])

  useEffect(() => {
    invalidate.current?.()
  }, [localKey])
  return useMemo(
    () => ({
      runningConversationIds: new Set([
        ...localRunning,
        ...(state.key === key ? state.running : [])
      ]),
      waitingApprovalConversationIds: state.key === key ? state.waitingApproval : new Set<string>()
    }),
    [key, localRunning, state]
  )
}
