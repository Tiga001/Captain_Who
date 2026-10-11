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
import { PersistenceDrain } from './persistenceDrain'

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
  const [pendingConversationSaves] = useState(() => new Map<string, ChatConversation>())
  const pendingConversationSavesRef = useRef(pendingConversationSaves)
  const [pendingMessageUpserts] = useState(() => new Map<string, PendingMessageUpsert[]>())
  const pendingMessageUpsertsRef = useRef(pendingMessageUpserts)
  const [pendingMessageSaves] = useState(() => new Map<string, PendingMessageSave>())
  const pendingMessageSavesRef = useRef(pendingMessageSaves)
  const [pendingMessageUiStateSaves] = useState(() => new Map<string, PendingMessageUiStateSave>())

  // Conversation metadata, insertion and state writes stay ordered only within their scope.
  // Failed writes retain their payload and reject flush; another enqueue or flush retries them.
  const [conversationSaveQueue] = useState(
    () =>
      new PersistenceDrain(
        () => pendingConversationSaves.keys(),
        (key) => pendingConversationSaves.has(key),
        async (conversationId) => {
          while (true) {
            const payload = pendingConversationSaves.get(conversationId)
            if (!payload) return
            pendingConversationSaves.delete(conversationId)
            try {
              await saveConversationMeta(payload)
            } catch (error) {
              if (!pendingConversationSaves.has(conversationId)) {
                pendingConversationSaves.set(conversationId, payload)
              }
              console.error('Failed to save conversation metadata to SQLite', error)
              throw error
            }
          }
        }
      )
  )

  const waitForConversationSaves = useCallback(
    (conversationId: string) => conversationSaveQueue.flush(conversationId),
    [conversationSaveQueue]
  )

  const [messageUpsertQueue] = useState(
    () =>
      new PersistenceDrain(
        () => pendingMessageUpserts.keys(),
        (key) => pendingMessageUpserts.has(key),
        async (conversationId) => {
          while (true) {
            const pendingUpserts = pendingMessageUpserts.get(conversationId)
            const payload = pendingUpserts?.[0]
            if (!payload) return
            if (pendingUpserts.length > 1) {
              pendingMessageUpserts.set(conversationId, pendingUpserts.slice(1))
            } else {
              pendingMessageUpserts.delete(conversationId)
            }
            try {
              await waitForConversationSaves(conversationId)
              await upsertChatMessages(conversationId, payload.messages, payload.positionOffset)
            } catch (error) {
              // Inserts preserve FIFO order, including accepted requests behind this failure.
              pendingMessageUpserts.set(conversationId, [
                payload,
                ...(pendingMessageUpserts.get(conversationId) ?? [])
              ])
              console.error('Failed to upsert chat messages to SQLite', error)
              throw error
            }
          }
        }
      )
  )

  const waitForMessageUpserts = useCallback(
    (conversationId: string) => messageUpsertQueue.flush(conversationId),
    [messageUpsertQueue]
  )

  const enqueueConversationMetaSave = useCallback(
    (conversation: ChatConversation) => {
      pendingConversationSaves.set(conversation.id, conversation)
      conversationSaveQueue.start(conversation.id)
    },
    [conversationSaveQueue, pendingConversationSaves]
  )

  const enqueueChatMessagesUpsert = useCallback(
    (conversationId: string, messages: ChatMessage[], positionOffset: number) => {
      pendingMessageUpserts.set(conversationId, [
        ...(pendingMessageUpserts.get(conversationId) ?? []),
        { messages, positionOffset }
      ])
      messageUpsertQueue.start(conversationId)
    },
    [messageUpsertQueue, pendingMessageUpserts]
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
      if (!message.agentRun?.coreCheckpoint) {
        messagePersistenceQueue.persistNow(conversationId, message)
      }
    },
    [messagePersistenceQueue]
  )

  const enqueueChatMessageCheckpoint = useCallback(
    (conversationId: string, message: ChatMessage) => {
      if (!message.agentRun?.coreCheckpoint) {
        messagePersistenceQueue.scheduleCheckpoint(conversationId, message)
      }
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

  const [messageUiStateSaveQueue] = useState(
    () =>
      new PersistenceDrain(
        () => pendingMessageUiStateSaves.keys(),
        (key) => pendingMessageUiStateSaves.has(key),
        async (key) => {
          while (true) {
            const payload = pendingMessageUiStateSaves.get(key)
            if (!payload) return
            pendingMessageUiStateSaves.delete(key)
            try {
              await waitForConversationSaves(payload.conversationId)
              await waitForMessageUpserts(payload.conversationId)
              await messagePersistenceQueue.flushMessage(payload.conversationId, payload.messageId)
              await saveChatMessageUiState(
                payload.conversationId,
                payload.messageId,
                payload.uiState
              )
            } catch (error) {
              if (!pendingMessageUiStateSaves.has(key)) pendingMessageUiStateSaves.set(key, payload)
              console.error('Failed to save chat message UI state to SQLite', error)
              throw error
            }
          }
        }
      )
  )

  const waitForMessageStateSaves = useCallback(
    async (conversationId: string) => {
      const matches = (key: string) => key.startsWith(`${conversationId}:`)
      while (true) {
        await messagePersistenceQueue.flushConversation(conversationId)
        if (!messageUiStateSaveQueue.hasMatching(matches)) return
        await messageUiStateSaveQueue.flushMatching(matches)
      }
    },
    [messagePersistenceQueue, messageUiStateSaveQueue]
  )

  const sealAndFlushChatMessageStateSaves = useCallback(async () => {
    const all = () => true
    while (true) {
      await Promise.all([
        messagePersistenceQueue.sealAndFlushAll(),
        conversationSaveQueue.flushMatching(all),
        messageUpsertQueue.flushMatching(all),
        messageUiStateSaveQueue.flushMatching(all)
      ])
      if (
        !messagePersistenceQueue.hasWork() &&
        !conversationSaveQueue.hasMatching(all) &&
        !messageUpsertQueue.hasMatching(all) &&
        !messageUiStateSaveQueue.hasMatching(all)
      ) {
        return
      }
    }
  }, [conversationSaveQueue, messagePersistenceQueue, messageUiStateSaveQueue, messageUpsertQueue])

  const enqueueChatMessageUiStateSave = useCallback(
    (conversationId: string, messageId: string, uiState: ChatMessageUiState | undefined) => {
      const key = `${conversationId}:${messageId}`
      pendingMessageUiStateSaves.set(key, { conversationId, messageId, uiState })
      messageUiStateSaveQueue.start(key)
    },
    [messageUiStateSaveQueue, pendingMessageUiStateSaves]
  )

  useEffect(() => {
    // Lifecycle notifications cannot await a durable result. The queue retains failures and
    // reports them through its adapter; explicit action/quit boundaries still receive rejection.
    const flush = () => void messagePersistenceQueue.flushAll().catch(() => undefined)
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
