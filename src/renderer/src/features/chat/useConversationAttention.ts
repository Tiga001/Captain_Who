import { useEffect, useMemo, useSyncExternalStore } from 'react'
import { hostClient } from '../../host/hostClient'
import { ConversationAttentionStore, EMPTY_ATTENTION } from './conversationAttentionStore'
import type { ChatConversation } from './chatTypes'

export type ConversationAttentionById = Readonly<
  Record<string, { waitingApproval: boolean; waitingAnswer: boolean; unread: boolean }>
>
const emptySubscribe = () => () => {}
const emptySnapshot = () => EMPTY_ATTENTION
const fallbackCache = new WeakMap<
  ChatConversation['messages'],
  { waitingApproval: boolean; waitingAnswer: boolean }
>()
function fallback(messages: ChatConversation['messages']) {
  let value = fallbackCache.get(messages)
  if (!value) {
    value = { waitingApproval: false, waitingAnswer: false }
    for (const message of messages) {
      if (message.role !== 'assistant') continue
      value.waitingApproval ||= message.agentRun?.status === 'waiting_for_approval'
      value.waitingAnswer ||= message.agentRun?.status === 'waiting_for_user_input'
      if (value.waitingApproval && value.waitingAnswer) break
    }
    fallbackCache.set(messages, value)
  }
  return value
}

export function useConversationAttention(
  conversations: readonly ChatConversation[]
): ConversationAttentionById {
  const api = hostClient.humanInteraction
  const store = useMemo(
    () =>
      typeof api?.getAttention === 'function' && typeof api?.onRequestChanged === 'function'
        ? new ConversationAttentionStore(api)
        : null,
    [api]
  )
  const pending = useSyncExternalStore(
    store?.subscribe ?? emptySubscribe,
    store?.getSnapshot ?? emptySnapshot
  )
  const ids = JSON.stringify(
    conversations.filter((item) => !item.archivedAt).map((item) => item.id)
  )
  useEffect(() => store?.connect(hostClient.agent?.onEvent), [store])
  useEffect(() => store?.watch(JSON.parse(ids)), [store, ids])
  const serialized = JSON.stringify(
    Object.fromEntries(
      conversations.map((conversation) => [
        conversation.id,
        {
          waitingApproval:
            !conversation.archivedAt &&
            (pending[conversation.id]?.waitingApproval ??
              fallback(conversation.messages).waitingApproval),
          waitingAnswer:
            !conversation.archivedAt &&
            (pending[conversation.id]?.waitingAnswer ??
              fallback(conversation.messages).waitingAnswer),
          unread: Boolean(conversation.unreadAt)
        }
      ])
    )
  )
  // Token-only updates keep the projection reference stable for sidebar and graph consumers.
  return useMemo(() => JSON.parse(serialized) as ConversationAttentionById, [serialized])
}
