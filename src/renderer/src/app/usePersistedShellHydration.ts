import { useEffect, type Dispatch, type SetStateAction } from 'react'
import type { ChatComposerDraft, ChatConversation } from '../features/chat/chatTypes'
import {
  loadComposerDrafts,
  loadConversationMetas,
  loadUiPreferences,
  type UiPreferencesSnapshot
} from '../features/storage/storageClient'
import { NEW_CONVERSATION_DRAFT_ID } from './appConstants'
import { createComposerDraft } from './chatMessageFactory'

interface StartupStage {
  attempt: number
  markFailed(error: unknown): void
  markPending(): void
  markReady(): void
}

interface UsePersistedShellHydrationOptions {
  composerDraftsStage: StartupStage
  conversationMetasStage: StartupStage
  setConversations: Dispatch<SetStateAction<ChatConversation[]>>
  setDrafts: Dispatch<SetStateAction<Record<string, ChatComposerDraft>>>
  setUiPreferences: Dispatch<SetStateAction<UiPreferencesSnapshot>>
  uiPreferencesStage: StartupStage
}

export function usePersistedShellHydration({
  composerDraftsStage,
  conversationMetasStage,
  setConversations,
  setDrafts,
  setUiPreferences,
  uiPreferencesStage
}: UsePersistedShellHydrationOptions): void {
  const {
    attempt: uiPreferencesAttempt,
    markFailed: markUiPreferencesFailed,
    markPending: markUiPreferencesPending,
    markReady: markUiPreferencesReady
  } = uiPreferencesStage
  const {
    attempt: composerDraftsAttempt,
    markFailed: markComposerDraftsFailed,
    markPending: markComposerDraftsPending,
    markReady: markComposerDraftsReady
  } = composerDraftsStage
  const {
    attempt: conversationMetasAttempt,
    markFailed: markConversationMetasFailed,
    markPending: markConversationMetasPending,
    markReady: markConversationMetasReady
  } = conversationMetasStage

  useEffect(() => {
    let cancelled = false
    markUiPreferencesPending()
    void loadUiPreferences()
      .then((preferences) => {
        if (!cancelled) {
          setUiPreferences(preferences)
          markUiPreferencesReady()
        }
      })
      .catch((error) => {
        if (!cancelled) {
          markUiPreferencesFailed(error)
          console.error('Failed to load UI preferences', error)
        }
      })
    return () => {
      cancelled = true
    }
  }, [
    setUiPreferences,
    markUiPreferencesFailed,
    markUiPreferencesPending,
    markUiPreferencesReady,
    uiPreferencesAttempt
  ])

  useEffect(() => {
    let cancelled = false
    markComposerDraftsPending()
    void loadComposerDrafts()
      .then((storedDrafts) => {
        if (cancelled) return
        setDrafts({
          [NEW_CONVERSATION_DRAFT_ID]: createComposerDraft(),
          ...storedDrafts
        })
        markComposerDraftsReady()
      })
      .catch((error) => {
        if (!cancelled) {
          markComposerDraftsFailed(error)
          console.error('Failed to load composer drafts', error)
        }
      })
    return () => {
      cancelled = true
    }
  }, [
    composerDraftsAttempt,
    markComposerDraftsFailed,
    markComposerDraftsPending,
    markComposerDraftsReady,
    setDrafts
  ])

  useEffect(() => {
    let cancelled = false
    markConversationMetasPending()
    void loadConversationMetas()
      .then((storedConversations) => {
        if (cancelled) return

        setConversations((currentConversations) => {
          const currentById = new Map(
            currentConversations.map((conversation) => [conversation.id, conversation])
          )
          const storedIds = new Set(storedConversations.map((conversation) => conversation.id))
          return [
            ...storedConversations.map((conversation) => {
              const current = currentById.get(conversation.id)
              return current && current.messagesLoaded !== false ? current : conversation
            }),
            ...currentConversations.filter((conversation) => !storedIds.has(conversation.id))
          ]
        })
        markConversationMetasReady()
      })
      .catch((error) => {
        if (!cancelled) {
          markConversationMetasFailed(error)
          console.error('Failed to load conversation metadata', error)
        }
      })
    return () => {
      cancelled = true
    }
  }, [
    conversationMetasAttempt,
    markConversationMetasFailed,
    markConversationMetasPending,
    markConversationMetasReady,
    setConversations
  ])
}
