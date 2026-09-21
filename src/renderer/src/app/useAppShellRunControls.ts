/* eslint-disable react-hooks/exhaustive-deps -- extracted callbacks keep AppShell's original dependency arrays; omitted inputs are stable refs. */
import { useCallback, useRef, type MutableRefObject } from 'react'
import type { AgentEvent } from '@mycopilot/protocol'
import type { Translate } from '../config/translationFormat'
import { getUserFacingErrorMessage } from '../errors/userFacingError'
import { cancelAgentRun, steerAgentRun } from '../features/agent/agentClient'
import {
  applyAgentEventToChatMessage,
  applyOptimisticGuidanceToChatMessage,
  removeGuidanceFromChatMessage
} from '../features/agentRun/agentEventReducer'
import { isAssistantMessageGenerating } from '../features/chat/assistantGeneration'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatQueuedMessage
} from '../features/chat/chatTypes'
import type { ActiveRunBinding, AutoSubmitQueuedMessage } from './appTypes'
import { createId } from './chatMessageFactory'

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
  supersedeRejectedGuidance: AppShellRuntime['supersedeRejectedGuidance']
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
  supersedeRejectedGuidance,
  scheduleStoppedRunReconciliation,
  stopRequestedPendingMessageIdsRef,
  stopRequestedRunIdsRef,
  t,
  updateAssistantMessage
}: UseAppShellRunControlsOptions) {
  const guidanceSubmissionsRef = useRef(new Set<string>())
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
    (selectedMessage: ChatQueuedMessage) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) return
      const queuedMessage = draftsRef.current[conversationId]?.queuedMessages.find(
        (message) => message.id === selectedMessage.id
      )
      const submissionKey = `${conversationId}:${selectedMessage.id}`
      if (
        !queuedMessage ||
        queuedMessage.status === 'submitting' ||
        guidanceSubmissionsRef.current.has(submissionKey)
      )
        return
      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      const assistantMessage = [...(conversation?.messages ?? [])]
        .reverse()
        .find(isAssistantMessageGenerating)
      const runId = assistantMessage?.agentRun?.runId
      if (
        !assistantMessage ||
        !runId ||
        !['running', 'waiting_for_approval'].includes(assistantMessage.agentRun?.status ?? '')
      ) {
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
      if (queueIndex < 0) return

      guidanceSubmissionsRef.current.add(submissionKey)
      let submittedMessage = queuedMessage
      const projectSubmission = () => {
        pendingGuidancePayloadsRef.current.set(submittedMessage.clientMessageId, {
          assistantMessageId: assistantMessage.id,
          conversationId,
          index: queueIndex,
          message: submittedMessage
        })
        mutateDraft(conversationId, (draft) => ({
          ...draft,
          queuedMessages: draft.queuedMessages.map((message) =>
            message.id === submittedMessage.id
              ? { ...submittedMessage, status: 'submitting', error: undefined }
              : message
          )
        }))
        updateAssistantMessage(
          conversationId,
          assistantMessage.id,
          (message) => applyOptimisticGuidanceToChatMessage(message, submittedMessage, runId),
          { touchConversation: true }
        )
      }
      const send = () =>
        steerAgentRun({
          conversationId,
          expectedRunId: runId,
          clientMessageId: submittedMessage.clientMessageId,
          content: submittedMessage.content,
          attachments: submittedMessage.attachments,
          folderReferences: submittedMessage.folderReferences ?? []
        })
      void flushRunMessagePersistence(conversationId, assistantMessage.id, runId)
      projectSubmission()

      void send()
        .then(async (output) => {
          // An error row can also mean a lost acknowledgement. Replay its original identity
          // first; only an authoritative terminal refusal permits this explicit manual resend
          // to use a new identity. Identity conflicts never prove the original was unapplied.
          if (
            queuedMessage.status !== 'error' ||
            output.status !== 'rejected' ||
            output.rejectionCode !== 'run_not_steerable' ||
            stopRequestedRunIdsRef.current.has(runId)
          )
            return output
          const currentMessage = draftsRef.current[conversationId]?.queuedMessages.find(
            (message) => message.id === queuedMessage.id
          )
          const currentRun = conversationsRef.current
            .find((candidate) => candidate.id === conversationId)
            ?.messages.find((message) => message.id === assistantMessage.id)?.agentRun
          if (
            currentMessage?.clientMessageId !== queuedMessage.clientMessageId ||
            currentRun?.runId !== runId ||
            !['running', 'waiting_for_approval'].includes(currentRun.status)
          )
            return output

          supersedeRejectedGuidance(queuedMessage.clientMessageId)
          updateAssistantMessage(
            conversationId,
            assistantMessage.id,
            (message) => removeGuidanceFromChatMessage(message, queuedMessage.clientMessageId),
            { touchConversation: true }
          )
          submittedMessage = { ...queuedMessage, clientMessageId: createId('guidance') }
          projectSubmission()
          return send()
        })
        .then((output) => {
          if (output.status === 'rejected') {
            restoreRejectedGuidance(
              conversationId,
              assistantMessage.id,
              submittedMessage.clientMessageId,
              output.message || t('chat.guidanceFailed')
            )
            queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
            return
          }

          removeQueuedMessageByClientId(conversationId, submittedMessage.clientMessageId)
          const attachments = submittedMessage.attachments.map((attachment) => ({
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
                  clientMessageId: submittedMessage.clientMessageId,
                  content: submittedMessage.content,
                  attachments,
                  createdAt: submittedMessage.createdAt,
                  sequence: 0
                }
              : {
                  type: 'guidance_queued',
                  runId,
                  guidanceId: output.guidanceId,
                  clientMessageId: submittedMessage.clientMessageId,
                  content: submittedMessage.content,
                  attachments,
                  createdAt: submittedMessage.createdAt
                }
          updateAssistantMessage(
            conversationId,
            assistantMessage.id,
            (message) => applyAgentEventToChatMessage(message, acceptedEvent),
            { touchConversation: true }
          )
          if (output.status === 'applied') {
            pendingGuidancePayloadsRef.current.delete(submittedMessage.clientMessageId)
          }
          queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
        })
        .catch((error) => {
          // A notification can acknowledge acceptance before the RPC transport loses its reply.
          // That authoritative receipt wins; do not turn an accepted input back into an error row.
          const accepted = conversationsRef.current
            .find((candidate) => candidate.id === conversationId)
            ?.messages.find((message) => message.id === assistantMessage.id)
            ?.agentRun?.timeline.some(
              (item) =>
                item.type === 'user_guidance' &&
                item.clientMessageId === submittedMessage.clientMessageId &&
                (item.status === 'queued' || item.status === 'applied')
            )
          if (accepted) return
          restoreRejectedGuidance(
            conversationId,
            assistantMessage.id,
            submittedMessage.clientMessageId,
            getUserFacingErrorMessage(error, t, 'chat.guidanceFailed')
          )
          queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
        })
        .finally(() => {
          guidanceSubmissionsRef.current.delete(submissionKey)
        })
    },
    [
      flushRunMessagePersistence,
      mutateDraft,
      removeQueuedMessageByClientId,
      restoreRejectedGuidance,
      supersedeRejectedGuidance,
      t,
      updateAssistantMessage
    ]
  )

  return { guideQueuedMessage, stopActiveGeneration }
}
