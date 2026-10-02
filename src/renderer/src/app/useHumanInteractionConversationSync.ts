import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from 'react'
import type { ChatConversation, ChatMessage } from '../features/chat/chatTypes'
import { upsertWorkflowDeliveryTimelineItem } from '../features/agentRun/workflowDeliveryTimeline'
import { loadConversation } from '../features/storage/storageClient'
import { hostClient } from '../host/hostClient'
import { removeCoveredWorkflowMessages } from './workflowConversationDelivery'

const terminal = (status: string) => ['completed', 'failed', 'cancelled'].includes(status)
function durableSequence(message: ChatMessage, excludeWorkflow = false): number {
  return Math.max(
    0,
    ...(message.agentRun?.timeline ?? [])
      .filter((item) => !excludeWorkflow || item.type !== 'workflow_delivery')
      .map(
        (item) =>
          item.traceSequence ?? (item.type === 'user_guidance' ? item.sequence : undefined) ?? 0
      )
  )
}
function liveMessageIsAhead(live: ChatMessage, stored: ChatMessage): boolean {
  const current = live.agentRun,
    incoming = stored.agentRun
  if (!current?.runId || current.runId !== incoming?.runId) return false
  if (terminal(current.status) && !terminal(incoming.status)) return true
  const currentSequence = durableSequence(live),
    storedSequence = durableSequence(stored)
  if (currentSequence > storedSequence) return true
  // A newly committed delivery can advance the stored trace while its subsequent narration is
  // still streaming locally. Merge that delivery without replacing the uncommitted live text.
  if (
    current.status === 'running' &&
    incoming.status === 'running' &&
    durableSequence(live, true) >= durableSequence(stored, true)
  )
    return true
  if (currentSequence < storedSequence || terminal(incoming.status)) return false
  // Deltas can lead both the durable trace and the last saved Renderer projection. A same-boundary
  // reload must not erase that live stream even if it arrived just before the read began.
  if (current.status === 'running' && incoming.status === 'running') return true
  return (current.lastResponseAt ?? 0) > (incoming.lastResponseAt ?? 0)
}

/** A newer stream must retain deliveries already committed to the same Run's trace. */
function mergeWorkflowDeliveries(preferred: ChatMessage, other: ChatMessage): ChatMessage {
  const run = preferred.agentRun
  if (!run?.runId || run.runId !== other.agentRun?.runId) return preferred
  let timeline = run.timeline
  for (const item of other.agentRun.timeline) {
    if (item.type === 'workflow_delivery') {
      timeline = upsertWorkflowDeliveryTimelineItem(timeline, item)
    }
  }
  return timeline === run.timeline ? preferred : { ...preferred, agentRun: { ...run, timeline } }
}

/** Only attaches authoritative messages/Run identities; never submits or restarts an agent. */
export function mergeHumanInteractionConversation(
  current: ChatConversation,
  stored: ChatConversation,
  baseline: ChatConversation | undefined
): ChatConversation {
  const before = new Map(baseline?.messages.map((message) => [message.id, message]) ?? [])
  const live = new Map(current.messages.map((message) => [message.id, message]))
  const storedIds = new Set(stored.messages.map((message) => message.id))
  const messages = stored.messages.map((message) => {
    const latest = live.get(message.id)
    const atReadStart = before.get(message.id)
    // Preserve new runtime facts, but a UI-only edit must not mask an authoritative terminal Run.
    if (
      latest &&
      latest !== atReadStart &&
      (latest.agentRun !== atReadStart?.agentRun ||
        latest.content !== atReadStart?.content ||
        latest.status !== atReadStart?.status)
    )
      return mergeWorkflowDeliveries(latest, message)
    if (latest && liveMessageIsAhead(latest, message))
      return mergeWorkflowDeliveries(latest, message)
    return latest
      ? mergeWorkflowDeliveries({ ...message, uiState: latest.uiState ?? message.uiState }, latest)
      : message
  })
  messages.push(...current.messages.filter((message) => !storedIds.has(message.id)))
  return {
    ...current,
    messages: removeCoveredWorkflowMessages(messages),
    messagesLoaded: true,
    updatedAt: Math.max(current.updatedAt, stored.updatedAt)
  }
}

export function useHumanInteractionConversationSync({
  activeConversationId,
  conversationsRef,
  setConversations
}: {
  activeConversationId: string | null
  conversationsRef: MutableRefObject<ChatConversation[]>
  setConversations: Dispatch<SetStateAction<ChatConversation[]>>
}) {
  useEffect(() => {
    const api = hostClient.humanInteraction
    if (!api) return
    let disposed = false
    const pending = new Set<string>()
    const dirty = new Set<string>()
    const timers = new Map<string, number>()
    const refresh = async (conversationId: string, attempt = 0): Promise<void> => {
      if (disposed) return
      if (pending.has(conversationId)) {
        dirty.add(conversationId)
        return
      }
      const baseline = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      if (!baseline || baseline.messagesLoaded === false) return
      pending.add(conversationId)
      try {
        const stored = await loadConversation(conversationId)
        if (!disposed && stored)
          setConversations((current) =>
            current.map((conversation) =>
              conversation.id === conversationId
                ? mergeHumanInteractionConversation(conversation, stored, baseline)
                : conversation
            )
          )
      } catch {
        // A Host reconnection can race its first notification. Retry the read, never the answer.
        if (!disposed && attempt < 3)
          timers.set(
            conversationId,
            window.setTimeout(
              () => {
                timers.delete(conversationId)
                void refresh(conversationId, attempt + 1)
              },
              250 * 2 ** attempt
            )
          )
      } finally {
        pending.delete(conversationId)
        if (dirty.delete(conversationId)) void refresh(conversationId)
      }
    }
    const unsubscribe = api.onRequestChanged((request) => {
      void refresh(request.conversationId)
    })
    const resync = () => {
      for (const conversation of conversationsRef.current) void refresh(conversation.id)
    }
    const unsubscribeResync = api.onResync?.(resync)
    const focus = () => {
      if (activeConversationId) void refresh(activeConversationId)
    }
    window.addEventListener('focus', focus)
    window.addEventListener('online', focus)
    if (activeConversationId) void refresh(activeConversationId)
    return () => {
      disposed = true
      unsubscribe()
      unsubscribeResync?.()
      for (const timer of timers.values()) window.clearTimeout(timer)
      window.removeEventListener('focus', focus)
      window.removeEventListener('online', focus)
    }
  }, [activeConversationId, conversationsRef, setConversations])
}
