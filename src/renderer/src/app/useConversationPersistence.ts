import { useCallback, useEffect, useRef, useState } from 'react'
import type { ChatConversation, ChatMessage, ChatMessageUiState } from '../features/chat/chatTypes'
import {
  saveChatMessageUiState,
  saveChatMessageState,
  saveConversationMeta,
  upsertChatMessages
} from '../features/storage/storageClient'
import type { PendingMessageSave } from './AppShellSupport'
import { ChatMessagePersistenceQueue } from './chatMessagePersistence'

type PendingMessageUpsert = {
  messages: ChatMessage[]
  positionOffset: number
}

type PendingMessageUiStateSave = {
  conversationId: string
  messageId: string
  uiState: ChatMessageUiState | undefined
}

export function useConversationPersistence() {
  const [conversationSaveQueues] = useState(() => new Map<string, Promise<void>>())
  const [pendingConversationSaves] = useState(() => new Map<string, ChatConversation>())
  const pendingConversationSavesRef = useRef(pendingConversationSaves)
  const [messageUpsertQueues] = useState(() => new Map<string, Promise<void>>())
  const [pendingMessageUpserts] = useState(() => new Map<string, PendingMessageUpsert[]>())
  const pendingMessageUpsertsRef = useRef(pendingMessageUpserts)
  const [pendingMessageSaves] = useState(() => new Map<string, PendingMessageSave>())
  const pendingMessageSavesRef = useRef(pendingMessageSaves)
  const messageUiStateSaveQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingMessageUiStateSavesRef = useRef<Map<string, PendingMessageUiStateSave>>(new Map())

  const waitForConversationSaves = useCallback(
    async (conversationId: string) => {
      while (true) {
        const pendingSave = conversationSaveQueues.get(conversationId)
        if (!pendingSave) return
        await pendingSave
      }
    },
    [conversationSaveQueues]
  )

  const waitForMessageUpserts = useCallback(
    async (conversationId: string) => {
      while (true) {
        const pendingUpsert = messageUpsertQueues.get(conversationId)
        if (!pendingUpsert) return
        await pendingUpsert
      }
    },
    [messageUpsertQueues]
  )

  // Conversation metadata, message insertion, and message-state updates use independent queues.
  // Waiting at each boundary preserves storage ordering without blocking unrelated conversations.
  const enqueueConversationMetaSave = useCallback(
    (conversation: ChatConversation) => {
      const conversationId = conversation.id
      pendingConversationSaves.set(conversationId, conversation)

      if (conversationSaveQueues.has(conversationId)) {
        return
      }

      const drainSaves = async () => {
        while (true) {
          const payload = pendingConversationSaves.get(conversationId)
          if (!payload) return

          pendingConversationSaves.delete(conversationId)
          try {
            await saveConversationMeta(payload)
          } catch (error) {
            console.error('Failed to save conversation metadata to SQLite', error)
          }
        }
      }

      const nextSave = drainSaves().finally(() => {
        conversationSaveQueues.delete(conversationId)
      })
      conversationSaveQueues.set(conversationId, nextSave)
    },
    [conversationSaveQueues, pendingConversationSaves]
  )

  const enqueueChatMessagesUpsert = useCallback(
    (conversationId: string, messages: ChatMessage[], positionOffset: number) => {
      const pendingUpserts = pendingMessageUpserts.get(conversationId) ?? []
      pendingMessageUpserts.set(conversationId, [...pendingUpserts, { messages, positionOffset }])

      if (messageUpsertQueues.has(conversationId)) {
        return
      }

      const drainUpserts = async () => {
        while (true) {
          const pendingUpserts = pendingMessageUpserts.get(conversationId) ?? []
          const payload = pendingUpserts[0]
          if (!payload) return

          const remainingUpserts = pendingUpserts.slice(1)
          if (remainingUpserts.length > 0) {
            pendingMessageUpserts.set(conversationId, remainingUpserts)
          } else {
            pendingMessageUpserts.delete(conversationId)
          }

          try {
            await waitForConversationSaves(conversationId)
            await upsertChatMessages(conversationId, payload.messages, payload.positionOffset)
          } catch (error) {
            console.error('Failed to upsert chat messages to SQLite', error)
          }
        }
      }

      const nextUpsert = drainUpserts().finally(() => {
        messageUpsertQueues.delete(conversationId)
      })
      messageUpsertQueues.set(conversationId, nextUpsert)
    },
    [messageUpsertQueues, pendingMessageUpserts, waitForConversationSaves]
  )

  const [messagePersistenceQueue] = useState(
    () =>
      new ChatMessagePersistenceQueue(
        {
          save: async ({ conversationId, message }) => {
            await waitForConversationSaves(conversationId)
            await waitForMessageUpserts(conversationId)
            await saveChatMessageState(conversationId, message)
          }
        },
        pendingMessageSaves,
        (error) => console.error('Failed to save chat message state to SQLite', error)
      )
  )

  const enqueueChatMessageStateSave = useCallback(
    (conversationId: string, message: ChatMessage) => {
      messagePersistenceQueue.persistNow(conversationId, message)
    },
    [messagePersistenceQueue]
  )

  const enqueueChatMessageCheckpoint = useCallback(
    (conversationId: string, message: ChatMessage) => {
      messagePersistenceQueue.scheduleCheckpoint(conversationId, message)
    },
    [messagePersistenceQueue]
  )

  const flushChatMessageStateSave = useCallback(
    (conversationId: string, messageId: string) =>
      messagePersistenceQueue.flushMessage(conversationId, messageId),
    [messagePersistenceQueue]
  )

  const flushConversationMessageStateSaves = useCallback(
    (conversationId: string) => messagePersistenceQueue.flushConversation(conversationId),
    [messagePersistenceQueue]
  )

  const sealAndFlushChatMessageStateSaves = useCallback(
    () => messagePersistenceQueue.sealAndFlushAll(),
    [messagePersistenceQueue]
  )

  const waitForMessageStateSaves = useCallback(
    async (conversationId: string) => {
      while (true) {
        await messagePersistenceQueue.flushConversation(conversationId)
        const pendingUiStateSaves = [...messageUiStateSaveQueuesRef.current.entries()]
          .filter(([key]) => key.startsWith(`${conversationId}:`))
          .map(([, pendingSave]) => pendingSave)
        if (pendingUiStateSaves.length === 0) return
        await Promise.allSettled(pendingUiStateSaves)
      }
    },
    [messagePersistenceQueue]
  )

  const enqueueChatMessageUiStateSave = useCallback(
    (conversationId: string, messageId: string, uiState: ChatMessageUiState | undefined) => {
      const key = `${conversationId}:${messageId}`
      pendingMessageUiStateSavesRef.current.set(key, { conversationId, messageId, uiState })

      if (messageUiStateSaveQueuesRef.current.has(key)) {
        return
      }

      const drainSaves = async () => {
        while (true) {
          const payload = pendingMessageUiStateSavesRef.current.get(key)
          if (!payload) return

          pendingMessageUiStateSavesRef.current.delete(key)
          try {
            await waitForConversationSaves(payload.conversationId)
            await waitForMessageUpserts(payload.conversationId)
            await messagePersistenceQueue.flushMessage(payload.conversationId, payload.messageId)
            await saveChatMessageUiState(payload.conversationId, payload.messageId, payload.uiState)
          } catch (error) {
            console.error('Failed to save chat message UI state to SQLite', error)
          }
        }
      }

      const nextSave = drainSaves().finally(() => {
        messageUiStateSaveQueuesRef.current.delete(key)
      })
      messageUiStateSaveQueuesRef.current.set(key, nextSave)
    },
    [messagePersistenceQueue, waitForConversationSaves, waitForMessageUpserts]
  )

  useEffect(() => {
    const flush = () => void messagePersistenceQueue.flushAll()
    const flushWhenHidden = () => {
      if (document.visibilityState === 'hidden') flush()
    }

    window.addEventListener('beforeunload', flush)
    window.addEventListener('pagehide', flush)
    document.addEventListener('visibilitychange', flushWhenHidden)
    return () => {
      window.removeEventListener('beforeunload', flush)
      window.removeEventListener('pagehide', flush)
      document.removeEventListener('visibilitychange', flushWhenHidden)
      flush()
    }
  }, [messagePersistenceQueue])

  return {
    enqueueChatMessagesUpsert,
    enqueueChatMessageCheckpoint,
    enqueueChatMessageStateSave,
    enqueueChatMessageUiStateSave,
    enqueueConversationMetaSave,
    flushChatMessageStateSave,
    flushConversationMessageStateSaves,
    pendingConversationSavesRef,
    pendingMessageSavesRef,
    pendingMessageUpsertsRef,
    sealAndFlushChatMessageStateSaves,
    waitForConversationSaves,
    waitForMessageStateSaves,
    waitForMessageUpserts
  }
}
