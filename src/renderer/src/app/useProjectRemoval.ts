import { useCallback, type Dispatch, type SetStateAction } from 'react'
import { cancelAgentAction, cancelAgentRun } from '../features/agent/agentClient'
import {
  getAgentActionApprovalStatus,
  getAgentActionId
} from '../features/agentRun/agentActionUtils'
import {
  ensureAgentRun,
  settleAgentRunToolActivities
} from '../features/agentRun/agentEventReducer'
import type { ChatComposerDraft, ChatConversation, ChatMessage } from '../features/chat/chatTypes'
import { saveUiPreferences, type UiPreferencesSnapshot } from '../features/storage/storageClient'
import type { ActiveRunBinding } from './appTypes'
import type { PendingMessageSave } from './AppShellSupport'
import { createComposerDraft } from './chatMessageFactory'

type MutableRef<T> = { current: T }

interface UseProjectRemovalOptions {
  activeConversationIdRef: MutableRef<string | null>
  activeRunBindingsRef: MutableRef<Map<string, ActiveRunBinding>>
  cancelledPendingMessageIdsRef: MutableRef<Set<string>>
  cancelledRunIdsRef: MutableRef<Set<string>>
  cleanupRunBinding(runId: string): void
  conversationScrollPositionsRef: MutableRef<Map<string, number>>
  conversationsRef: MutableRef<ChatConversation[]>
  deleteProject(projectId: string): Promise<void>
  editSubmissionSeqRef: MutableRef<number>
  enqueueChatMessageStateSave(conversationId: string, message: ChatMessage): void
  pendingConversationSavesRef: MutableRef<Map<string, ChatConversation>>
  pendingMessageSavesRef: MutableRef<Map<string, PendingMessageSave>>
  pendingMessageUpsertsRef: MutableRef<Map<string, unknown>>
  removeFailedMessage: string
  setActiveConversationId: Dispatch<SetStateAction<string | null>>
  setActiveConversationInitialScrollTop: Dispatch<SetStateAction<number | null>>
  setConversationsWithRef: Dispatch<SetStateAction<ChatConversation[]>>
  setDraftsWithRef: Dispatch<SetStateAction<Record<string, ChatComposerDraft>>>
  setScrollTargetMessageId: Dispatch<SetStateAction<string | null>>
  setUiPreferences: Dispatch<SetStateAction<UiPreferencesSnapshot>>
  showToast(message: string): void
  waitForConversationSaves(conversationId: string): Promise<void>
  waitForMessageStateSaves(conversationId: string): Promise<void>
  waitForMessageUpserts(conversationId: string): Promise<void>
}

export function useProjectRemoval({
  activeConversationIdRef,
  activeRunBindingsRef,
  cancelledPendingMessageIdsRef,
  cancelledRunIdsRef,
  cleanupRunBinding,
  conversationScrollPositionsRef,
  conversationsRef,
  deleteProject,
  editSubmissionSeqRef,
  enqueueChatMessageStateSave,
  pendingConversationSavesRef,
  pendingMessageSavesRef,
  pendingMessageUpsertsRef,
  removeFailedMessage,
  setActiveConversationId,
  setActiveConversationInitialScrollTop,
  setConversationsWithRef,
  setDraftsWithRef,
  setScrollTargetMessageId,
  setUiPreferences,
  showToast,
  waitForConversationSaves,
  waitForMessageStateSaves,
  waitForMessageUpserts
}: UseProjectRemovalOptions) {
  return useCallback(
    async (projectId: string): Promise<boolean> => {
      const projectConversations = conversationsRef.current.filter(
        (conversation) => conversation.projectId === projectId
      )
      const conversationIds = new Set(projectConversations.map((conversation) => conversation.id))
      const runIds = new Set<string>()
      const pendingActions = new Map<string, { runId: string; actionId: string }>()

      editSubmissionSeqRef.current += 1
      for (const conversation of projectConversations) {
        for (const message of conversation.messages) {
          if (message.role !== 'assistant' || message.status !== 'pending') continue

          cancelledPendingMessageIdsRef.current.add(message.id)
          const pendingRunId = message.agentRun?.runId
          if (pendingRunId) runIds.add(pendingRunId)
          for (const action of message.agentRun?.approvals ?? []) {
            if (pendingRunId && getAgentActionApprovalStatus(action) === 'required') {
              const actionId = getAgentActionId(action)
              pendingActions.set(`${pendingRunId}\u0000${actionId}`, {
                runId: pendingRunId,
                actionId
              })
            }
          }
        }
      }
      for (const [runId, binding] of activeRunBindingsRef.current) {
        if (conversationIds.has(binding.conversationId)) runIds.add(runId)
      }

      runIds.forEach((runId) => {
        cancelledRunIdsRef.current.add(runId)
        cleanupRunBinding(runId)
      })
      await Promise.allSettled([
        ...[...runIds].map((runId) => cancelAgentRun(runId)),
        ...[...pendingActions.values()].map(({ runId, actionId }) =>
          cancelAgentAction(runId, actionId)
        ),
        ...[...conversationIds].map(async (conversationId) => {
          await waitForConversationSaves(conversationId)
          await waitForMessageUpserts(conversationId)
          await waitForMessageStateSaves(conversationId)
        })
      ])

      try {
        await deleteProject(projectId)
      } catch (error) {
        console.error('Failed to remove project', error)
        const cancelledAt = Date.now()
        const reconciledConversations = conversationsRef.current.map((conversation) => {
          if (conversation.projectId !== projectId) return conversation

          return {
            ...conversation,
            messages: conversation.messages.map((message) => {
              if (message.role !== 'assistant' || message.status !== 'pending') return message

              const currentRun = ensureAgentRun(
                message.agentRun,
                message.agentRun?.runId ?? null,
                'cancelled'
              )
              const cancelledMessage: ChatMessage = {
                ...message,
                content: message.content,
                status: 'sent',
                agentRun: settleAgentRunToolActivities(
                  {
                    ...currentRun,
                    completedAt: cancelledAt,
                    todo: undefined
                  },
                  'cancelled',
                  cancelledAt
                )
              }
              enqueueChatMessageStateSave(conversation.id, cancelledMessage)
              return cancelledMessage
            })
          }
        })
        setConversationsWithRef(reconciledConversations)
        showToast(removeFailedMessage)
        return false
      }

      const removedConversationIds = new Set(
        conversationsRef.current
          .filter((conversation) => conversation.projectId === projectId)
          .map((conversation) => conversation.id)
      )
      for (const conversationId of removedConversationIds) {
        conversationScrollPositionsRef.current.delete(conversationId)
        pendingConversationSavesRef.current.delete(conversationId)
        pendingMessageUpsertsRef.current.delete(conversationId)
      }
      for (const [key, pendingSave] of pendingMessageSavesRef.current) {
        if (removedConversationIds.has(pendingSave.conversationId)) {
          pendingMessageSavesRef.current.delete(key)
        }
      }

      setConversationsWithRef((currentConversations) =>
        currentConversations.filter((conversation) => !removedConversationIds.has(conversation.id))
      )
      setDraftsWithRef((currentDrafts) =>
        Object.fromEntries(
          Object.entries(currentDrafts)
            .filter(([scopeId]) => !removedConversationIds.has(scopeId))
            .map(([scopeId, draft]) => [
              scopeId,
              draft.projectId === projectId ? createComposerDraft() : draft
            ])
        )
      )
      setUiPreferences((currentPreferences) => {
        const sidebarProjectOrder = currentPreferences.sidebarProjectOrder.filter(
          (orderedProjectId) => orderedProjectId !== projectId
        )
        if (sidebarProjectOrder.length === currentPreferences.sidebarProjectOrder.length) {
          return currentPreferences
        }

        const nextPreferences = {
          ...currentPreferences,
          sidebarProjectOrder,
          updatedAt: Date.now()
        }
        void saveUiPreferences(nextPreferences)
        return nextPreferences
      })

      if (
        activeConversationIdRef.current &&
        removedConversationIds.has(activeConversationIdRef.current)
      ) {
        activeConversationIdRef.current = null
        setActiveConversationId(null)
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
      }
      return true
    },
    [
      activeConversationIdRef,
      activeRunBindingsRef,
      cancelledPendingMessageIdsRef,
      cancelledRunIdsRef,
      cleanupRunBinding,
      conversationScrollPositionsRef,
      conversationsRef,
      deleteProject,
      editSubmissionSeqRef,
      enqueueChatMessageStateSave,
      pendingConversationSavesRef,
      pendingMessageSavesRef,
      pendingMessageUpsertsRef,
      removeFailedMessage,
      setActiveConversationId,
      setActiveConversationInitialScrollTop,
      setConversationsWithRef,
      setDraftsWithRef,
      setScrollTargetMessageId,
      setUiPreferences,
      showToast,
      waitForConversationSaves,
      waitForMessageStateSaves,
      waitForMessageUpserts
    ]
  )
}
