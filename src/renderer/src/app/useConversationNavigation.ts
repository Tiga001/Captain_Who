import { useCallback, type Dispatch, type SetStateAction } from 'react'
import { HostInvocationError } from '@mycopilot/host-api'
import { parseStorageForkConversationErrorData } from '@mycopilot/protocol'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin
} from '../features/chat/chatTypes'
import {
  forkConversation,
  loadConversation,
  saveComposerDraft,
  saveConversationMeta
} from '../features/storage/storageClient'
import { createComposerDraft, createId } from './chatMessageFactory'

type MutableRef<T> = { current: T }

interface ConversationNavigationMessages {
  activeCommandSession: string
  continueInNewTaskFailed: string
  originArchived: string
  originMissing: string
  originOpenFailed: string
}

interface UseConversationNavigationOptions {
  activeConversationIdRef: MutableRef<string | null>
  conversationScrollPositionsRef: MutableRef<Map<string, number>>
  conversationsRef: MutableRef<ChatConversation[]>
  drafts: Record<string, ChatComposerDraft>
  hydrateConversation: (conversationId: string) => Promise<ChatConversation | null>
  messages: ConversationNavigationMessages
  setActiveConversationId: Dispatch<SetStateAction<string | null>>
  setActiveConversationInitialScrollTop: Dispatch<SetStateAction<number | null>>
  setConversationScrollToBottomSignal: Dispatch<SetStateAction<number>>
  setConversationsWithRef: Dispatch<SetStateAction<ChatConversation[]>>
  setDraftsWithRef: Dispatch<SetStateAction<Record<string, ChatComposerDraft>>>
  setScrollTargetMessageId: Dispatch<SetStateAction<string | null>>
  setSettingsOpen: Dispatch<SetStateAction<boolean>>
  showToast: (message: string) => void
}

export function useConversationNavigation({
  activeConversationIdRef,
  conversationScrollPositionsRef,
  conversationsRef,
  drafts,
  hydrateConversation,
  messages,
  setActiveConversationId,
  setActiveConversationInitialScrollTop,
  setConversationScrollToBottomSignal,
  setConversationsWithRef,
  setDraftsWithRef,
  setScrollTargetMessageId,
  setSettingsOpen,
  showToast
}: UseConversationNavigationOptions) {
  const selectConversation = useCallback(
    (conversationId: string, messageId?: string | null, loadedConversation?: ChatConversation) => {
      activeConversationIdRef.current = conversationId
      const selectedConversation =
        loadedConversation ??
        conversationsRef.current.find((conversation) => conversation.id === conversationId)
      const shouldRestoreRememberedPosition = !messageId && !selectedConversation?.unreadAt

      setScrollTargetMessageId(messageId ?? null)
      setActiveConversationInitialScrollTop(
        shouldRestoreRememberedPosition
          ? (conversationScrollPositionsRef.current.get(conversationId) ?? null)
          : null
      )

      let conversationToSave: ChatConversation | null = null
      let loadedConversationFound = false
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation
        loadedConversationFound = true
        const selected = loadedConversation ?? conversation
        if (!selected.unreadAt) return selected

        conversationToSave = {
          ...selected,
          unreadAt: null
        }
        return conversationToSave
      })
      if (loadedConversation && !loadedConversationFound) {
        const selected = loadedConversation.unreadAt
          ? { ...loadedConversation, unreadAt: null }
          : loadedConversation
        if (loadedConversation.unreadAt) conversationToSave = selected
        nextConversations.unshift(selected)
      }

      if (loadedConversation || conversationToSave) {
        setConversationsWithRef(nextConversations)
      }
      if (conversationToSave) {
        void saveConversationMeta(conversationToSave)
      }

      setActiveConversationId(conversationId)
      if (!loadedConversation) {
        void hydrateConversation(conversationId)
      }
    },
    [
      activeConversationIdRef,
      conversationScrollPositionsRef,
      conversationsRef,
      hydrateConversation,
      setActiveConversationId,
      setActiveConversationInitialScrollTop,
      setConversationsWithRef,
      setScrollTargetMessageId
    ]
  )

  const continueInNewTask = useCallback(
    async (sourceConversationId: string, throughAssistantMessageId: string) => {
      try {
        const newConversation = await forkConversation(
          sourceConversationId,
          throughAssistantMessageId,
          createId('conversation-fork-request')
        )
        const sourceDraft =
          drafts[sourceConversationId] ??
          createComposerDraft({
            modelId: newConversation.modelId ?? undefined,
            projectId: newConversation.projectId
          })
        const newDraft = createComposerDraft({
          modelId: newConversation.modelId ?? sourceDraft.modelId,
          permissionMode: sourceDraft.permissionMode,
          projectId: newConversation.projectId
        })

        setConversationsWithRef((currentConversations) => [
          newConversation,
          ...currentConversations.filter((conversation) => conversation.id !== newConversation.id)
        ])
        setDraftsWithRef((currentDrafts) => ({
          ...currentDrafts,
          [newConversation.id]: newDraft
        }))
        void saveComposerDraft(newConversation.id, newDraft)
        conversationScrollPositionsRef.current.delete(newConversation.id)
        activeConversationIdRef.current = newConversation.id
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
        setConversationScrollToBottomSignal((signal) => signal + 1)
        setActiveConversationId(newConversation.id)
        setSettingsOpen(false)
      } catch (error) {
        console.error('Failed to continue conversation in a new task', error)
        showToast(
          resolveConversationForkErrorMessage(
            error,
            messages.activeCommandSession,
            messages.continueInNewTaskFailed
          )
        )
      }
    },
    [
      activeConversationIdRef,
      conversationScrollPositionsRef,
      drafts,
      messages.continueInNewTaskFailed,
      messages.activeCommandSession,
      setActiveConversationId,
      setActiveConversationInitialScrollTop,
      setConversationScrollToBottomSignal,
      setConversationsWithRef,
      setDraftsWithRef,
      setScrollTargetMessageId,
      setSettingsOpen,
      showToast
    ]
  )

  const openContinuationOrigin = useCallback(
    async (origin: ChatConversationContinuationOrigin) => {
      try {
        const sourceConversation = await loadConversation(origin.sourceConversationId)
        if (!sourceConversation) {
          showToast(messages.originMissing)
          return
        }
        if (sourceConversation.archivedAt !== null && sourceConversation.archivedAt !== undefined) {
          showToast(messages.originArchived)
          return
        }
        selectConversation(origin.sourceConversationId, origin.sourceMessageId, sourceConversation)
      } catch (error) {
        console.error('Failed to open continuation origin', error)
        showToast(messages.originOpenFailed)
      }
    },
    [
      messages.originArchived,
      messages.originMissing,
      messages.originOpenFailed,
      selectConversation,
      showToast
    ]
  )

  const rememberConversationScrollPosition = useCallback(
    (conversationId: string, scrollTop: number) => {
      conversationScrollPositionsRef.current.set(conversationId, scrollTop)
    },
    [conversationScrollPositionsRef]
  )

  const patchConversation = useCallback(
    (conversationId: string, patch: Partial<ChatConversation>) => {
      let nextConversation: ChatConversation | null = null
      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation

        nextConversation = { ...conversation, ...patch }
        return nextConversation
      })

      if (nextConversation) {
        setConversationsWithRef(nextConversations)
        void saveConversationMeta(nextConversation)
      }
    },
    [conversationsRef, setConversationsWithRef]
  )

  const archiveConversations = useCallback(
    (predicate: (conversation: ChatConversation) => boolean) => {
      const archivedAt = Date.now()
      setConversationsWithRef((currentConversations) =>
        currentConversations.map((conversation) => {
          if (conversation.archivedAt || !predicate(conversation)) return conversation

          const nextConversation = {
            ...conversation,
            archivedAt,
            unreadAt: null
          }
          void saveConversationMeta(nextConversation)
          return nextConversation
        })
      )
    },
    [setConversationsWithRef]
  )

  return {
    archiveConversations,
    continueInNewTask,
    openContinuationOrigin,
    patchConversation,
    rememberConversationScrollPosition,
    selectConversation
  }
}

function resolveConversationForkErrorMessage(
  error: unknown,
  activeCommandSessionMessage: string,
  fallbackMessage: string
): string {
  if (!(error instanceof HostInvocationError)) return fallbackMessage

  try {
    const data = parseStorageForkConversationErrorData(error.data)
    if (data.code === 'active_command_session') return activeCommandSessionMessage
  } catch {
    // Unknown or malformed recovery data must never expose the underlying Host/Core message.
  }

  return fallbackMessage
}
