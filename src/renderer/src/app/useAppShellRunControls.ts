/* eslint-disable react-hooks/exhaustive-deps -- extracted callbacks keep AppShell's original dependency arrays; omitted inputs are stable refs. */
import { useCallback, type MutableRefObject } from 'react'
import type { AgentEvent } from '@mycopilot/protocol'
import type { Translate } from '../config/translationFormat'
import { getUserFacingErrorMessage } from '../errors/userFacingError'
import { cancelAgentRun, steerAgentRun } from '../features/agent/agentClient'
import {
  applyAgentEventToChatMessage,
  applyOptimisticGuidanceToChatMessage
} from '../features/agentRun/agentEventReducer'
import { isAssistantMessageGenerating } from '../features/chat/assistantGeneration'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatQueuedMessage
} from '../features/chat/chatTypes'
import type { ActiveRunBinding, AutoSubmitQueuedMessage } from './appTypes'

type AppShellRuntime = ReturnType<(typeof import('./useAppShellRuntime'))['useAppShellRuntime']>

interface UseAppShellRunControlsOptions {
  activeConversationId: string | null
  activeConversationIdRef: MutableRefObject<string | null>
  activeRunBindingsRef: MutableRefObject<Map<string, ActiveRunBinding>>
  autoSubmitQueuedMessageRef: MutableRefObject<AutoSubmitQueuedMessage>
  conversations: ChatConversation[]
  conversationsRef: MutableRefObject<ChatConversation[]>
  draftsRef: MutableRefObject<Record<string, ChatComposerDraft>>
  flushRunMessagePersistence: AppShellRuntime['flushRunMessagePersistence']
  mutateDraft: AppShellRuntime['mutateDraft']
  pendingGuidancePayloadsRef: MutableRefObject<
    Map<
      string,
      {
        assistantMessageId: string
        conversationId: string
        index: number
        message: ChatQueuedMessage
      }
    >
  >
  removeQueuedMessageByClientId: AppShellRuntime['removeQueuedMessageByClientId']
  restoreRejectedGuidance: AppShellRuntime['restoreRejectedGuidance']
  scheduleStoppedRunReconciliation: AppShellRuntime['scheduleStoppedRunReconciliation']
  stopRequestedPendingMessageIdsRef: MutableRefObject<Set<string>>
  stopRequestedRunIdsRef: MutableRefObject<Set<string>>
  t: Translate
  updateAssistantMessage: AppShellRuntime['updateAssistantMessage']
}

export function useAppShellRunControls({
  activeConversationId,
  activeConversationIdRef,
  activeRunBindingsRef,
  autoSubmitQueuedMessageRef,
  conversations,
  conversationsRef,
  draftsRef,
  flushRunMessagePersistence,
  mutateDraft,
  pendingGuidancePayloadsRef,
  removeQueuedMessageByClientId,
  restoreRejectedGuidance,
  scheduleStoppedRunReconciliation,
  stopRequestedPendingMessageIdsRef,
  stopRequestedRunIdsRef,
  t,
  updateAssistantMessage
}: UseAppShellRunControlsOptions) {
  const stopActiveGeneration = useCallback(() => {
    if (!activeConversationId) return
    autoSubmitQueuedMessageRef.current(activeConversationId, 'pause')
    const activeConversationSnapshot = conversations.find(
      (conversation) => conversation.id === activeConversationId
    )
    const pendingMessage = [...(activeConversationSnapshot?.messages ?? [])]
      .reverse()
      .find(isAssistantMessageGenerating)

    if (!pendingMessage) return
    const runId = pendingMessage.agentRun?.runId
    if (!runId) {
      stopRequestedPendingMessageIdsRef.current.add(pendingMessage.id)
      return
    }
    if (stopRequestedRunIdsRef.current.has(runId)) return

    void flushRunMessagePersistence(activeConversationId, pendingMessage.id, runId)

    // The backend emits terminal Done only after the assistant message and trace have been
    // committed atomically. Keep the binding alive and let that event authoritatively settle the
    // UI instead of persisting a renderer-invented cancelled state ahead of durable storage.
    stopRequestedRunIdsRef.current.add(runId)
    const binding = activeRunBindingsRef.current.get(runId)
    if (binding) scheduleStoppedRunReconciliation(runId, binding)
    void cancelAgentRun(runId).catch(() => {
      console.error('Failed to cancel agent run')
    })
  }, [
    activeConversationId,
    conversations,
    flushRunMessagePersistence,
    scheduleStoppedRunReconciliation
  ])

  const guideQueuedMessage = useCallback(
    (queuedMessage: ChatQueuedMessage) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) return
      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      const assistantMessage = [...(conversation?.messages ?? [])]
        .reverse()
        .find(isAssistantMessageGenerating)
      const runId = assistantMessage?.agentRun?.runId
      if (!assistantMessage || !runId || assistantMessage.agentRun?.status !== 'running') {
        mutateDraft(conversationId, (draft) => ({
          ...draft,
          queuedMessages: draft.queuedMessages.map((message) =>
            message.id === queuedMessage.id
              ? {
                  ...message,
                  status: 'error',
                  error: t('chat.guidanceFailed')
                }
              : message
          )
        }))
        return
      }

      const queueIndex =
        draftsRef.current[conversationId]?.queuedMessages.findIndex(
          (message) => message.id === queuedMessage.id
        ) ?? -1
      if (queueIndex < 0 || queuedMessage.status === 'submitting') return

      const submittingMessage: ChatQueuedMessage = {
        ...queuedMessage,
        status: 'submitting',
        error: undefined
      }
      pendingGuidancePayloadsRef.current.set(queuedMessage.clientMessageId, {
        assistantMessageId: assistantMessage.id,
        conversationId,
        index: queueIndex,
        message: queuedMessage
      })
      mutateDraft(conversationId, (draft) => ({
        ...draft,
        queuedMessages: draft.queuedMessages.map((message) =>
          message.id === queuedMessage.id ? submittingMessage : message
        )
      }))
      void flushRunMessagePersistence(conversationId, assistantMessage.id, runId)
      updateAssistantMessage(
        conversationId,
        assistantMessage.id,
        (message) => applyOptimisticGuidanceToChatMessage(message, queuedMessage, runId),
        { touchConversation: true }
      )

      void steerAgentRun({
        conversationId,
        expectedRunId: runId,
        clientMessageId: queuedMessage.clientMessageId,
        content: queuedMessage.content,
        attachments: queuedMessage.attachments
      })
        .then((output) => {
          if (output.status === 'rejected') {
            restoreRejectedGuidance(
              conversationId,
              assistantMessage.id,
              queuedMessage.clientMessageId,
              output.message || t('chat.guidanceFailed')
            )
            queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
            return
          }

          removeQueuedMessageByClientId(conversationId, queuedMessage.clientMessageId)
          const attachments = queuedMessage.attachments.map((attachment) => ({
            id: attachment.id,
            kind: attachment.kind,
            name: attachment.name,
            mimeType: attachment.mimeType,
            sizeBytes: attachment.sizeBytes
          }))
          const acceptedEvent: AgentEvent =
            output.status === 'applied'
              ? {
                  type: 'guidance_applied',
                  runId,
                  guidanceId: output.guidanceId,
                  clientMessageId: queuedMessage.clientMessageId,
                  content: queuedMessage.content,
                  attachments,
                  createdAt: queuedMessage.createdAt,
                  sequence: 0
                }
              : {
                  type: 'guidance_queued',
                  runId,
                  guidanceId: output.guidanceId,
                  clientMessageId: queuedMessage.clientMessageId,
                  content: queuedMessage.content,
                  attachments,
                  createdAt: queuedMessage.createdAt
                }
          updateAssistantMessage(
            conversationId,
            assistantMessage.id,
            (message) => applyAgentEventToChatMessage(message, acceptedEvent),
            { touchConversation: true }
          )
          if (output.status === 'applied') {
            pendingGuidancePayloadsRef.current.delete(queuedMessage.clientMessageId)
          }
          queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
        })
        .catch((error) => {
          restoreRejectedGuidance(
            conversationId,
            assistantMessage.id,
            queuedMessage.clientMessageId,
            getUserFacingErrorMessage(error, t, 'chat.guidanceFailed')
          )
          queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
        })
    },
    [
      flushRunMessagePersistence,
      mutateDraft,
      removeQueuedMessageByClientId,
      restoreRejectedGuidance,
      t,
      updateAssistantMessage
    ]
  )

  return { guideQueuedMessage, stopActiveGeneration }
}
