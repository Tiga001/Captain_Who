import { useCallback, useRef } from 'react'
import type { ChatConversation, ChatMessage } from '../features/chat/chatTypes'
import {
  saveChatMessageState,
  saveConversationMeta,
  upsertChatMessages
} from '../features/storage/storageClient'
import type { PendingMessageSave } from './AppShellSupport'

type PendingMessageUpsert = {
  messages: ChatMessage[]
  positionOffset: number
}

export function useConversationPersistence() {
  const conversationSaveQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingConversationSavesRef = useRef<Map<string, ChatConversation>>(new Map())
  const messageUpsertQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingMessageUpsertsRef = useRef<Map<string, PendingMessageUpsert[]>>(new Map())
  const messageSaveQueuesRef = useRef<Map<string, Promise<void>>>(new Map())
  const pendingMessageSavesRef = useRef<Map<string, PendingMessageSave>>(new Map())

  const waitForConversationSaves = useCallback(async (conversationId: string) => {
    while (true) {
      const pendingSave = conversationSaveQueuesRef.current.get(conversationId)
      if (!pendingSave) return
      await pendingSave
    }
  }, [])

  const waitForMessageUpserts = useCallback(async (conversationId: string) => {
    while (true) {
      const pendingUpsert = messageUpsertQueuesRef.current.get(conversationId)
      if (!pendingUpsert) return
      await pendingUpsert
    }
  }, [])

  const waitForMessageStateSaves = useCallback(async (conversationId: string) => {
    while (true) {
      const pendingSaves = [...messageSaveQueuesRef.current.entries()]
        .filter(([key]) => key.startsWith(`${conversationId}:`))
        .map(([, pendingSave]) => pendingSave)
      if (pendingSaves.length === 0) return
      await Promise.allSettled(pendingSaves)
    }
  }, [])

  // Conversation metadata, message insertion, and message-state updates use independent queues.
  // Waiting at each boundary preserves storage ordering without blocking unrelated conversations.
  const enqueueConversationMetaSave = useCallback((conversation: ChatConversation) => {
    const conversationId = conversation.id
    pendingConversationSavesRef.current.set(conversationId, conversation)

    if (conversationSaveQueuesRef.current.has(conversationId)) {
      return
    }

    const drainSaves = async () => {
      while (true) {
        const payload = pendingConversationSavesRef.current.get(conversationId)
        if (!payload) return

        pendingConversationSavesRef.current.delete(conversationId)
        try {
          await saveConversationMeta(payload)
        } catch (error) {
          console.error('Failed to save conversation metadata to SQLite', error)
        }
      }
    }

    const nextSave = drainSaves().finally(() => {
      conversationSaveQueuesRef.current.delete(conversationId)
    })
    conversationSaveQueuesRef.current.set(conversationId, nextSave)
  }, [])

  const enqueueChatMessagesUpsert = useCallback(
    (conversationId: string, messages: ChatMessage[], positionOffset: number) => {
      const pendingUpserts = pendingMessageUpsertsRef.current.get(conversationId) ?? []
      pendingMessageUpsertsRef.current.set(conversationId, [
        ...pendingUpserts,
        { messages, positionOffset }
      ])

      if (messageUpsertQueuesRef.current.has(conversationId)) {
        return
      }

      const drainUpserts = async () => {
        while (true) {
          const pendingUpserts = pendingMessageUpsertsRef.current.get(conversationId) ?? []
          const payload = pendingUpserts[0]
          if (!payload) return

          const remainingUpserts = pendingUpserts.slice(1)
          if (remainingUpserts.length > 0) {
            pendingMessageUpsertsRef.current.set(conversationId, remainingUpserts)
          } else {
            pendingMessageUpsertsRef.current.delete(conversationId)
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
        messageUpsertQueuesRef.current.delete(conversationId)
      })
      messageUpsertQueuesRef.current.set(conversationId, nextUpsert)
    },
    [waitForConversationSaves]
  )

  const enqueueChatMessageStateSave = useCallback(
    (conversationId: string, message: ChatMessage) => {
      const key = `${conversationId}:${message.id}`
      pendingMessageSavesRef.current.set(key, { conversationId, message })

      if (messageSaveQueuesRef.current.has(key)) {
        return
      }

      const drainSaves = async () => {
        while (true) {
          const payload = pendingMessageSavesRef.current.get(key)
          if (!payload) return

          pendingMessageSavesRef.current.delete(key)
          try {
            await waitForConversationSaves(payload.conversationId)
            await waitForMessageUpserts(payload.conversationId)
            await saveChatMessageState(payload.conversationId, payload.message)
          } catch (error) {
            console.error('Failed to save chat message state to SQLite', error)
          }
        }
      }

      const nextSave = drainSaves().finally(() => {
        messageSaveQueuesRef.current.delete(key)
      })
      messageSaveQueuesRef.current.set(key, nextSave)
    },
    [waitForConversationSaves, waitForMessageUpserts]
  )

  return {
    enqueueChatMessagesUpsert,
    enqueueChatMessageStateSave,
    enqueueConversationMetaSave,
    pendingConversationSavesRef,
    pendingMessageSavesRef,
    pendingMessageUpsertsRef,
    waitForConversationSaves,
    waitForMessageStateSaves,
    waitForMessageUpserts
  }
}
