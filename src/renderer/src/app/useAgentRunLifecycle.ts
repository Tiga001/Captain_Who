import {
  useCallback,
  useEffect,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import type { AgentContextWindowSnapshot, AgentEvent, SkillSelection } from '@mycopilot/protocol'
import {
  cancelAgentRun,
  listPendingAgentActions,
  onAgentEvent,
  startConversationTurn
} from '../features/agent/agentClient'
import { resolveChatPermissions } from '../features/chat/chatPermissions'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatPermissionMode,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import {
  loadConversation,
  loadInputAttachments,
  saveConversationMeta
} from '../features/storage/storageClient'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { DEFAULT_AGENT_MAX_TOKENS, THINKING_PLACEHOLDER } from '../features/agentRun/constants'
import {
  applyAgentEventToChatMessage,
  ensureAgentRun,
  removeGuidanceFromChatMessage,
  settleAgentRunToolActivities,
  shouldTouchConversationForAgentEvent
} from '../features/agentRun/agentEventReducer'
import {
  planSkillActivationRecovery,
  type SkillActivationRecoveryPlan
} from '../features/skills/skillActivationRecovery'
import { mergeActivatedSkillSummaries } from '../features/skills/activatedSkillInventory'
import type { Translate } from '../config/translationFormat'
import { createComposerDraft, mergeConversationMessageFromBackend } from './chatMessageFactory'
import type { ActiveRunBinding } from './appTypes'
import {
  STREAM_DELTA_FLUSH_MS,
  STREAM_DELTA_MAX_BUFFER_CHARS,
  type PendingMessageDelta
} from './AppShellSupport'

const STOP_RECONCILIATION_DELAYS_MS = [400, 1500, 4000] as const
const STOP_RECONCILIATION_READ_TIMEOUT_MS = 1500
const MAX_RETIRED_AGENT_RUN_IDS = 1024
const MAX_BUFFERED_AGENT_RUNS = 128
const MAX_BUFFERED_AGENT_EVENTS_PER_RUN = 128
const MAX_UNCONFIRMED_STOPPED_RUNS = 128

function isSameRunBinding(
  current: ActiveRunBinding | undefined,
  expected: ActiveRunBinding
): current is ActiveRunBinding {
  return (
    current?.conversationId === expected.conversationId &&
    current.pendingMessageId === expected.pendingMessageId
  )
}

function loadConversationForStopReconciliation(
  conversationId: string
): Promise<ChatConversation | null> {
  return new Promise((resolve) => {
    let settled = false
    const finish = (conversation: ChatConversation | null) => {
      if (settled) return
      settled = true
      window.clearTimeout(timeoutId)
      resolve(conversation)
    }
    const timeoutId = window.setTimeout(() => finish(null), STOP_RECONCILIATION_READ_TIMEOUT_MS)
    void loadConversation(conversationId).then(finish, () => finish(null))
  })
}

interface AgentRunLifecycleRefs {
  activeRunBindings: MutableRefObject<Map<string, ActiveRunBinding>>
  autoSubmitQueuedMessage: MutableRefObject<(conversationId: string) => void>
  bufferedAgentEvents: MutableRefObject<Map<string, AgentEvent[]>>
  cancelledPendingMessageIds: MutableRefObject<Set<string>>
  cancelledRunIds: MutableRefObject<Set<string>>
  locallyUnconfirmedStoppedRunIds: MutableRefObject<Set<string>>
  pendingActionsHydrated: MutableRefObject<Set<string>>
  pendingGuidancePayloads: MutableRefObject<
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
  pendingMessageDeltas: MutableRefObject<Map<string, PendingMessageDelta>>
  retiredAgentRunIds: MutableRefObject<Set<string>>
  stopReconciliationTimers: MutableRefObject<Map<string, number>>
  stopRequestedPendingMessageIds: MutableRefObject<Set<string>>
  stopRequestedRunIds: MutableRefObject<Set<string>>
}

interface UseAgentRunLifecycleOptions {
  contextWindowIndicatorEnabled: boolean
  conversationState: {
    activeConversationIdRef: MutableRefObject<string | null>
    conversations: ChatConversation[]
    conversationsRef: MutableRefObject<ChatConversation[]>
    setActiveConversationId: Dispatch<SetStateAction<string | null>>
    setConversations: (value: SetStateAction<ChatConversation[]>) => void
  }
  draftState: {
    draftsRef: MutableRefObject<Record<string, ChatComposerDraft>>
    mutateDraft: (
      scopeId: string,
      updater: (draft: ChatComposerDraft) => ChatComposerDraft
    ) => ChatComposerDraft
  }
  enqueueChatMessageStateSave: (conversationId: string, message: ChatMessage) => void
  recordContextWindowSnapshot: (
    conversationId: string,
    snapshot: AgentContextWindowSnapshot
  ) => void
  reconcileFailedSkillActivation: (
    scopeId: string,
    recovery: SkillActivationRecoveryPlan,
    fallback: Pick<ChatComposerDraft, 'modelId' | 'permissionMode' | 'projectId'>
  ) => void
  refs: AgentRunLifecycleRefs
  requestSkillCatalogRefresh: (scopeId: string) => void
  showToast: (message: string) => void
  t: Translate
  uiPreferences: Pick<UiPreferencesSnapshot, 'customPermissions'>
}

export function useAgentRunLifecycle({
  contextWindowIndicatorEnabled,
  conversationState,
  draftState,
  enqueueChatMessageStateSave,
  recordContextWindowSnapshot,
  reconcileFailedSkillActivation,
  refs,
  requestSkillCatalogRefresh,
  showToast,
  t,
  uiPreferences
}: UseAgentRunLifecycleOptions) {
  const {
    activeConversationIdRef,
    conversations,
    conversationsRef,
    setActiveConversationId,
    setConversations
  } = conversationState
  const { draftsRef, mutateDraft } = draftState
  const {
    activeRunBindings,
    autoSubmitQueuedMessage,
    bufferedAgentEvents,
    cancelledPendingMessageIds,
    cancelledRunIds,
    locallyUnconfirmedStoppedRunIds,
    pendingActionsHydrated,
    pendingGuidancePayloads,
    pendingMessageDeltas,
    retiredAgentRunIds,
    stopReconciliationTimers,
    stopRequestedPendingMessageIds,
    stopRequestedRunIds
  } = refs

  // These refs own stable Map/Set instances for the lifetime of AppShell. Capture the containers
  // once per render so React Compiler can reason about callback and cleanup dependencies without
  // treating mutable `.current` reads as a changing memoization boundary.
  const activeRunBindingMap = activeRunBindings.current
  const bufferedAgentEventMap = bufferedAgentEvents.current
  const cancelledPendingMessageIdSet = cancelledPendingMessageIds.current
  const cancelledRunIdSet = cancelledRunIds.current
  const locallyUnconfirmedStoppedRunIdSet = locallyUnconfirmedStoppedRunIds.current
  const pendingActionsHydratedSet = pendingActionsHydrated.current
  const pendingGuidancePayloadMap = pendingGuidancePayloads.current
  const pendingMessageDeltaMap = pendingMessageDeltas.current
  const retiredAgentRunIdSet = retiredAgentRunIds.current
  const stopReconciliationTimerMap = stopReconciliationTimers.current
  const stopRequestedPendingMessageIdSet = stopRequestedPendingMessageIds.current
  const stopRequestedRunIdSet = stopRequestedRunIds.current

  const clearPendingMessageDelta = useCallback(
    (runId: string) => {
      const pendingDelta = pendingMessageDeltaMap.get(runId)
      if (!pendingDelta) return

      window.clearTimeout(pendingDelta.timerId)
      pendingMessageDeltaMap.delete(runId)
    },
    [pendingMessageDeltaMap]
  )

  const cleanupRunBinding = useCallback(
    (runId: string) => {
      const reconciliationTimer = stopReconciliationTimerMap.get(runId)
      if (reconciliationTimer !== undefined) {
        window.clearTimeout(reconciliationTimer)
        stopReconciliationTimerMap.delete(runId)
      }
      activeRunBindingMap.delete(runId)
      bufferedAgentEventMap.delete(runId)
      locallyUnconfirmedStoppedRunIdSet.delete(runId)
      retiredAgentRunIdSet.add(runId)
      if (retiredAgentRunIdSet.size > MAX_RETIRED_AGENT_RUN_IDS) {
        const oldestRunId = retiredAgentRunIdSet.values().next().value
        if (oldestRunId) retiredAgentRunIdSet.delete(oldestRunId)
      }
      clearPendingMessageDelta(runId)
    },
    [
      activeRunBindingMap,
      bufferedAgentEventMap,
      clearPendingMessageDelta,
      locallyUnconfirmedStoppedRunIdSet,
      retiredAgentRunIdSet,
      stopReconciliationTimerMap
    ]
  )

  const cancelBackendAgentRun = useCallback((runId: string) => {
    void cancelAgentRun(runId).catch((error) => {
      console.error('Failed to cancel agent run', error)
    })
  }, [])

  const updateAssistantMessage = useCallback(
    (
      conversationId: string,
      messageId: string,
      updater: (message: ChatMessage) => ChatMessage,
      options: { persist?: boolean; touchConversation?: boolean } = {}
    ) => {
      let messageToSave: ChatMessage | null = null
      let conversationMetaToSave: ChatConversation | null = null
      const timestamp = Date.now()

      const nextConversations = conversationsRef.current.map((conversation) => {
        if (conversation.id !== conversationId) return conversation

        const nextConversation = {
          ...conversation,
          messages: conversation.messages.map((message) => {
            if (message.id !== messageId) return message
            messageToSave = updater(message)
            return messageToSave
          }),
          updatedAt: options.touchConversation ? timestamp : conversation.updatedAt
        }

        conversationMetaToSave = nextConversation
        return nextConversation
      })

      setConversations(nextConversations)

      if (messageToSave && options.persist !== false) {
        enqueueChatMessageStateSave(conversationId, messageToSave)
      }
      if (options.touchConversation && conversationMetaToSave) {
        void saveConversationMeta(conversationMetaToSave)
      }
    },
    [conversationsRef, enqueueChatMessageStateSave, setConversations]
  )

  const removeQueuedMessageByClientId = useCallback(
    (conversationId: string, clientMessageId: string) => {
      mutateDraft(conversationId, (draft) => ({
        ...draft,
        queuedMessages: draft.queuedMessages.filter(
          (message) => message.clientMessageId !== clientMessageId
        )
      }))
    },
    [mutateDraft]
  )

  const restoreRejectedGuidance = useCallback(
    (
      conversationId: string,
      assistantMessageId: string,
      clientMessageId: string,
      errorMessage: string,
      fallback?: {
        content: string
        attachments: Array<{ id: string }>
        createdAt: number
      }
    ) => {
      const pending = pendingGuidancePayloadMap.get(clientMessageId)
      pendingGuidancePayloadMap.delete(clientMessageId)
      const alreadyRestored = draftsRef.current[conversationId]?.queuedMessages.find(
        (message) => message.clientMessageId === clientMessageId
      )

      const restore = (message: ChatQueuedMessage, preferredIndex: number) => {
        mutateDraft(conversationId, (draft) => {
          const withoutMessage = draft.queuedMessages.filter(
            (candidate) => candidate.clientMessageId !== clientMessageId
          )
          const insertAt = Math.min(Math.max(0, preferredIndex), withoutMessage.length)
          withoutMessage.splice(insertAt, 0, {
            ...message,
            status: 'error',
            error: errorMessage
          })
          return {
            ...draft,
            queuedMessages: withoutMessage
          }
        })
      }

      updateAssistantMessage(
        conversationId,
        assistantMessageId,
        (message) => removeGuidanceFromChatMessage(message, clientMessageId),
        { touchConversation: true }
      )

      if (!pending && alreadyRestored) {
        restore(
          alreadyRestored,
          draftsRef.current[conversationId].queuedMessages.indexOf(alreadyRestored)
        )
        return
      }
      if (pending) {
        restore(pending.message, pending.index)
        return
      }
      if (!fallback) return

      void loadInputAttachments(fallback.attachments.map((attachment) => attachment.id))
        .then((attachments) => {
          const draft = draftsRef.current[conversationId] ?? createComposerDraft()
          restore(
            {
              id: `queued-message-${fallback.createdAt}-${Math.random().toString(36).slice(2, 8)}`,
              clientMessageId,
              content: fallback.content,
              attachments,
              modelId: draft.modelId,
              permissionMode: draft.permissionMode,
              projectId: draft.projectId,
              skills: [],
              status: 'error',
              error: errorMessage,
              createdAt: fallback.createdAt
            },
            draft.queuedMessages.length
          )
        })
        .catch((error) => {
          console.error('Failed to restore rejected guidance attachments', error)
          const draft = draftsRef.current[conversationId] ?? createComposerDraft()
          restore(
            {
              id: `queued-message-${fallback.createdAt}-${Math.random().toString(36).slice(2, 8)}`,
              clientMessageId,
              content: fallback.content,
              attachments: [],
              modelId: draft.modelId,
              permissionMode: draft.permissionMode,
              projectId: draft.projectId,
              skills: [],
              status: 'error',
              error: errorMessage,
              createdAt: fallback.createdAt
            },
            draft.queuedMessages.length
          )
        })
    },
    [draftsRef, mutateDraft, pendingGuidancePayloadMap, updateAssistantMessage]
  )

  const flushPendingMessageDelta = useCallback(
    (runId: string) => {
      const pendingDelta = pendingMessageDeltaMap.get(runId)
      if (!pendingDelta) return

      window.clearTimeout(pendingDelta.timerId)
      pendingMessageDeltaMap.delete(runId)
      updateAssistantMessage(
        pendingDelta.conversationId,
        pendingDelta.messageId,
        (message) =>
          applyAgentEventToChatMessage(message, {
            type: 'message_delta',
            runId,
            streamId: pendingDelta.streamId,
            delta: pendingDelta.delta
          }),
        { touchConversation: false }
      )
    },
    [pendingMessageDeltaMap, updateAssistantMessage]
  )

  const bufferMessageDelta = useCallback(
    (
      conversationId: string,
      messageId: string,
      agentEvent: AgentEvent & { type: 'message_delta' }
    ) => {
      const pendingDelta = pendingMessageDeltaMap.get(agentEvent.runId)

      if (pendingDelta) {
        pendingDelta.conversationId = conversationId
        pendingDelta.messageId = messageId
        pendingDelta.delta += agentEvent.delta

        if (
          pendingDelta.delta.includes('\n') ||
          pendingDelta.delta.length >= STREAM_DELTA_MAX_BUFFER_CHARS
        ) {
          flushPendingMessageDelta(agentEvent.runId)
        }
        return
      }

      const timerId = window.setTimeout(() => {
        flushPendingMessageDelta(agentEvent.runId)
      }, STREAM_DELTA_FLUSH_MS)
      pendingMessageDeltaMap.set(agentEvent.runId, {
        conversationId,
        delta: agentEvent.delta,
        messageId,
        streamId: agentEvent.streamId,
        timerId
      })
    },
    [flushPendingMessageDelta, pendingMessageDeltaMap]
  )

  useEffect(() => {
    const newlyHydratedConversationIds = conversations
      .filter(
        (conversation) =>
          conversation.messagesLoaded !== false && !pendingActionsHydratedSet.has(conversation.id)
      )
      .map((conversation) => conversation.id)
    if (newlyHydratedConversationIds.length === 0) return

    for (const conversationId of newlyHydratedConversationIds) {
      pendingActionsHydratedSet.add(conversationId)
    }
    const newlyHydratedConversationIdSet = new Set(newlyHydratedConversationIds)

    void listPendingAgentActions()
      .then((pendingActions) => {
        for (const pendingAction of pendingActions) {
          if (!pendingAction.conversationId || !pendingAction.assistantMessageId) continue
          const conversationExists = conversationsRef.current.some(
            (conversation) => conversation.id === pendingAction.conversationId
          )
          if (!conversationExists) continue

          activeRunBindingMap.set(pendingAction.runId, {
            conversationId: pendingAction.conversationId,
            pendingMessageId: pendingAction.assistantMessageId
          })
          if (!newlyHydratedConversationIdSet.has(pendingAction.conversationId)) continue
          updateAssistantMessage(
            pendingAction.conversationId,
            pendingAction.assistantMessageId,
            (message) =>
              applyAgentEventToChatMessage(message, {
                type: 'approval_required',
                runId: pendingAction.runId,
                action: pendingAction.action
              }),
            { touchConversation: false }
          )
        }
      })
      .catch((error) => {
        for (const conversationId of newlyHydratedConversationIds) {
          pendingActionsHydratedSet.delete(conversationId)
        }
        console.error('Failed to hydrate pending agent actions', error)
      })
  }, [
    activeRunBindingMap,
    conversations,
    conversationsRef,
    pendingActionsHydratedSet,
    updateAssistantMessage
  ])

  const handleBoundAgentEvent = useCallback(
    (conversationId: string, assistantMessageId: string, agentEvent: AgentEvent) => {
      if (agentEvent.runId && cancelledRunIdSet.has(agentEvent.runId)) return
      if (cancelledPendingMessageIdSet.has(assistantMessageId)) return

      if (agentEvent.type === 'message_delta') {
        bufferMessageDelta(conversationId, assistantMessageId, agentEvent)
        return
      }

      if (agentEvent.type === 'tool_input_progress') return

      if (
        agentEvent.type === 'file_write_preview_updated' ||
        agentEvent.type === 'file_write_preview_cleared'
      ) {
        updateAssistantMessage(
          conversationId,
          assistantMessageId,
          (message) => applyAgentEventToChatMessage(message, agentEvent),
          { persist: false, touchConversation: false }
        )
        return
      }

      if (agentEvent.runId) {
        flushPendingMessageDelta(agentEvent.runId)
      }

      if (agentEvent.type === 'guidance_queued' || agentEvent.type === 'guidance_applied') {
        removeQueuedMessageByClientId(conversationId, agentEvent.clientMessageId)
        if (agentEvent.type === 'guidance_applied') {
          pendingGuidancePayloadMap.delete(agentEvent.clientMessageId)
        }
      }

      if (agentEvent.type === 'guidance_rejected') {
        restoreRejectedGuidance(
          conversationId,
          assistantMessageId,
          agentEvent.clientMessageId,
          agentEvent.message || t('chat.guidanceFailed'),
          {
            content: agentEvent.content,
            attachments: [],
            createdAt: agentEvent.createdAt
          }
        )
        window.setTimeout(() => autoSubmitQueuedMessage.current(conversationId), 0)
        return
      }

      updateAssistantMessage(
        conversationId,
        assistantMessageId,
        (message) => {
          const shouldAcceptAuthoritativeTerminal =
            agentEvent.type === 'done' && locallyUnconfirmedStoppedRunIdSet.has(agentEvent.runId)
          const messageForEvent =
            shouldAcceptAuthoritativeTerminal && message.agentRun
              ? {
                  ...message,
                  status: 'pending' as const,
                  agentRun: {
                    ...message.agentRun,
                    status: 'running' as const
                  }
                }
              : message
          return applyAgentEventToChatMessage(messageForEvent, agentEvent)
        },
        { touchConversation: shouldTouchConversationForAgentEvent(agentEvent) }
      )

      if (agentEvent.type === 'done') {
        stopRequestedRunIdSet.delete(agentEvent.runId)
        if (agentEvent.success && agentEvent.status !== 'waiting_for_approval') {
          const completedAt = Date.now()
          let conversationToSave: ChatConversation | null = null
          const nextConversations = conversationsRef.current.map((conversation) => {
            if (
              conversation.id !== conversationId ||
              activeConversationIdRef.current === conversationId
            ) {
              return conversation
            }

            conversationToSave = {
              ...conversation,
              unreadAt: completedAt
            }
            return conversationToSave
          })

          if (conversationToSave) {
            setConversations(nextConversations)
            void saveConversationMeta(conversationToSave)
          }
        }

        if (agentEvent.status !== 'waiting_for_approval') {
          cleanupRunBinding(agentEvent.runId)
        }
        if (agentEvent.status === 'completed' || agentEvent.status === 'cancelled') {
          window.setTimeout(() => autoSubmitQueuedMessage.current(conversationId), 0)
        }
      }

      if (agentEvent.type === 'error' && !agentEvent.recoverable && agentEvent.runId) {
        stopRequestedRunIdSet.delete(agentEvent.runId)
        cleanupRunBinding(agentEvent.runId)
      }
    },
    [
      activeConversationIdRef,
      autoSubmitQueuedMessage,
      bufferMessageDelta,
      cancelledPendingMessageIdSet,
      cancelledRunIdSet,
      cleanupRunBinding,
      conversationsRef,
      flushPendingMessageDelta,
      locallyUnconfirmedStoppedRunIdSet,
      pendingGuidancePayloadMap,
      removeQueuedMessageByClientId,
      restoreRejectedGuidance,
      setConversations,
      stopRequestedRunIdSet,
      t,
      updateAssistantMessage
    ]
  )

  const markStoppedRunStatusUnknown = useCallback(
    (runId: string, binding: ActiveRunBinding) => {
      if (!isSameRunBinding(activeRunBindingMap.get(runId), binding)) return

      const settledAt = Date.now()
      const safeError = t('chat.stopStatusUnknown')
      locallyUnconfirmedStoppedRunIdSet.add(runId)
      if (locallyUnconfirmedStoppedRunIdSet.size > MAX_UNCONFIRMED_STOPPED_RUNS) {
        const oldestRunId = locallyUnconfirmedStoppedRunIdSet.values().next().value
        if (oldestRunId && oldestRunId !== runId) cleanupRunBinding(oldestRunId)
      }
      stopRequestedRunIdSet.delete(runId)
      updateAssistantMessage(
        binding.conversationId,
        binding.pendingMessageId,
        (message) => {
          const run = ensureAgentRun(message.agentRun, runId)
          return {
            ...message,
            status: 'error',
            agentRun: {
              ...settleAgentRunToolActivities(
                {
                  ...run,
                  completedAt: settledAt,
                  error: safeError
                },
                'failed',
                settledAt
              ),
              completedAt: settledAt,
              error: safeError
            }
          }
        },
        {
          // This is a Renderer-only fail-safe when authoritative storage cannot be reached. Never
          // overwrite the backend record with a guessed terminal state.
          persist: false,
          touchConversation: false
        }
      )
      showToast(safeError)
    },
    [
      activeRunBindingMap,
      cleanupRunBinding,
      locallyUnconfirmedStoppedRunIdSet,
      showToast,
      stopRequestedRunIdSet,
      t,
      updateAssistantMessage
    ]
  )

  const scheduleStoppedRunReconciliation = useCallback(
    (runId: string, binding: ActiveRunBinding) => {
      const scheduleAttempt = (attempt: number) => {
        const existingTimer = stopReconciliationTimerMap.get(runId)
        if (existingTimer !== undefined) window.clearTimeout(existingTimer)

        const timerId = window.setTimeout(() => {
          stopReconciliationTimerMap.delete(runId)
          const currentBinding = activeRunBindingMap.get(runId)
          if (!isSameRunBinding(currentBinding, binding)) return

          void loadConversationForStopReconciliation(binding.conversationId)
            .then((storedConversation) => {
              // A terminal event may have cleaned up this binding while storage was loading. Never
              // let that older snapshot overwrite the newer authoritative event.
              if (!isSameRunBinding(activeRunBindingMap.get(runId), binding)) return

              const storedMessage = storedConversation?.messages.find(
                (message) => message.id === binding.pendingMessageId
              )
              const storedStatus = storedMessage?.agentRun?.status
              const isTerminal =
                storedStatus === 'completed' ||
                storedStatus === 'failed' ||
                storedStatus === 'cancelled'
              if (storedMessage?.agentRun?.runId === runId && isTerminal) {
                setConversations((currentConversations) =>
                  currentConversations.map((conversation) =>
                    conversation.id !== binding.conversationId
                      ? conversation
                      : {
                          ...conversation,
                          messages: conversation.messages.map((message) =>
                            message.id !== binding.pendingMessageId
                              ? message
                              : {
                                  ...storedMessage,
                                  uiState: message.uiState ?? storedMessage.uiState
                                }
                          )
                        }
                  )
                )
                stopRequestedRunIdSet.delete(runId)
                cleanupRunBinding(runId)
                return
              }

              if (attempt + 1 < STOP_RECONCILIATION_DELAYS_MS.length) {
                scheduleAttempt(attempt + 1)
                return
              }
              markStoppedRunStatusUnknown(runId, binding)
            })
            .catch(() => {
              if (!isSameRunBinding(activeRunBindingMap.get(runId), binding)) return
              if (attempt + 1 < STOP_RECONCILIATION_DELAYS_MS.length) {
                scheduleAttempt(attempt + 1)
                return
              }
              markStoppedRunStatusUnknown(runId, binding)
            })
        }, STOP_RECONCILIATION_DELAYS_MS[attempt])
        stopReconciliationTimerMap.set(runId, timerId)
      }

      scheduleAttempt(0)
    },
    [
      activeRunBindingMap,
      cleanupRunBinding,
      markStoppedRunStatusUnknown,
      setConversations,
      stopReconciliationTimerMap,
      stopRequestedRunIdSet
    ]
  )

  useEffect(() => {
    return onAgentEvent((agentEvent) => {
      if (agentEvent.type === 'context_window_updated') {
        const conversationId = agentEvent.conversationId
        if (conversationId) recordContextWindowSnapshot(conversationId, agentEvent.snapshot)
        return
      }

      const runId = agentEvent.runId
      if (!runId) return
      if (cancelledRunIdSet.has(runId)) return
      if (retiredAgentRunIdSet.has(runId)) return

      const binding = activeRunBindingMap.get(runId)
      if (!binding) {
        const bufferedEvents = bufferedAgentEventMap.get(runId) ?? []
        if (bufferedEvents.length === 0 && bufferedAgentEventMap.size >= MAX_BUFFERED_AGENT_RUNS) {
          const oldestRunId = bufferedAgentEventMap.keys().next().value
          if (oldestRunId) bufferedAgentEventMap.delete(oldestRunId)
        }
        bufferedAgentEventMap.set(
          runId,
          [...bufferedEvents, agentEvent].slice(-MAX_BUFFERED_AGENT_EVENTS_PER_RUN)
        )
        return
      }

      handleBoundAgentEvent(binding.conversationId, binding.pendingMessageId, agentEvent)
    })
  }, [
    activeRunBindingMap,
    bufferedAgentEventMap,
    cancelledRunIdSet,
    handleBoundAgentEvent,
    recordContextWindowSnapshot,
    retiredAgentRunIdSet
  ])

  useEffect(() => {
    return () => {
      for (const pendingDelta of pendingMessageDeltaMap.values()) {
        window.clearTimeout(pendingDelta.timerId)
      }
      pendingMessageDeltaMap.clear()
    }
  }, [pendingMessageDeltaMap])

  useEffect(() => {
    return () => {
      for (const timerId of stopReconciliationTimerMap.values()) {
        window.clearTimeout(timerId)
      }
      stopReconciliationTimerMap.clear()
      activeRunBindingMap.clear()
    }
  }, [activeRunBindingMap, stopReconciliationTimerMap])

  const requestAssistantResponse = useCallback(
    async (
      conversationId: string,
      userMessageId: string,
      assistantMessageId: string,
      content: string,
      modelId: string,
      projectId: string | null,
      permissionMode: ChatPermissionMode,
      attachments: ChatSubmitOptions['attachments'],
      skills: readonly SkillSelection[],
      title?: string
    ) => {
      updateAssistantMessage(conversationId, assistantMessageId, (message) => ({
        ...message,
        agentRun: ensureAgentRun(message.agentRun, null, 'starting')
      }))

      try {
        const startOutput = await startConversationTurn({
          assistantMessageId,
          attachments,
          content,
          contextWindowIndicatorEnabled,
          conversationId,
          maxTokens: DEFAULT_AGENT_MAX_TOKENS,
          modelId,
          permissions: resolveChatPermissions(permissionMode, uiPreferences.customPermissions),
          projectId,
          skills: skills.length > 0 ? [...skills] : undefined,
          title,
          userMessageId
        })

        if (cancelledPendingMessageIdSet.has(assistantMessageId)) {
          cancelledPendingMessageIdSet.delete(assistantMessageId)
          cancelledRunIdSet.add(startOutput.runId)
          cancelBackendAgentRun(startOutput.runId)
          bufferedAgentEventMap.delete(startOutput.runId)
          return
        }

        const stopWasRequested = stopRequestedPendingMessageIdSet.delete(assistantMessageId)

        const resolvedConversationId = startOutput.conversationId
        const resolvedAssistantMessageId = startOutput.assistantMessageId
        let resolvedAssistantMessage: ChatMessage | null = null

        const nextConversations = conversationsRef.current.map((conversation) =>
          conversation.id === conversationId
            ? {
                ...conversation,
                id: resolvedConversationId,
                messages: conversation.messages.map((message) => {
                  if (message.id === userMessageId) {
                    return mergeConversationMessageFromBackend(message, startOutput.userMessage)
                  }

                  if (message.id === assistantMessageId) {
                    const mergedMessage = mergeConversationMessageFromBackend(
                      message,
                      startOutput.assistantMessage
                    )
                    resolvedAssistantMessage = {
                      ...mergedMessage,
                      content: mergedMessage.content || message.content || THINKING_PLACEHOLDER,
                      status: 'pending' as const,
                      agentRun: {
                        ...ensureAgentRun(mergedMessage.agentRun, startOutput.runId, 'running'),
                        activatedSkills: mergeActivatedSkillSummaries(
                          [],
                          startOutput.activatedSkills
                        ),
                        skillActivationRevision: startOutput.skillActivationRevision,
                        explicitSkillSelections: [...skills]
                      }
                    }
                    return resolvedAssistantMessage
                  }

                  return message
                })
              }
            : conversation
        )
        setConversations(nextConversations)
        if (resolvedAssistantMessage) {
          enqueueChatMessageStateSave(resolvedConversationId, resolvedAssistantMessage)
        }

        if (resolvedConversationId !== conversationId) {
          setActiveConversationId((currentActiveConversationId) =>
            currentActiveConversationId === conversationId
              ? resolvedConversationId
              : currentActiveConversationId
          )
          if (activeConversationIdRef.current === conversationId) {
            activeConversationIdRef.current = resolvedConversationId
          }
        }

        retiredAgentRunIdSet.delete(startOutput.runId)
        locallyUnconfirmedStoppedRunIdSet.delete(startOutput.runId)
        activeRunBindingMap.set(startOutput.runId, {
          conversationId: resolvedConversationId,
          pendingMessageId: resolvedAssistantMessageId
        })

        const bufferedEvents = bufferedAgentEventMap.get(startOutput.runId) ?? []
        bufferedAgentEventMap.delete(startOutput.runId)
        bufferedEvents.forEach((agentEvent) => {
          handleBoundAgentEvent(resolvedConversationId, resolvedAssistantMessageId, agentEvent)
        })

        if (stopWasRequested && activeRunBindingMap.has(startOutput.runId)) {
          stopRequestedRunIdSet.add(startOutput.runId)
          const binding = activeRunBindingMap.get(startOutput.runId)
          if (binding) scheduleStoppedRunReconciliation(startOutput.runId, binding)
          void cancelAgentRun(startOutput.runId).catch(() => {
            console.error('Failed to cancel agent run')
          })
        }
      } catch (error) {
        if (cancelledPendingMessageIdSet.has(assistantMessageId)) {
          cancelledPendingMessageIdSet.delete(assistantMessageId)
          return
        }
        if (stopRequestedPendingMessageIdSet.delete(assistantMessageId)) {
          const stoppedAt = Date.now()
          updateAssistantMessage(
            conversationId,
            assistantMessageId,
            (currentMessage) => ({
              ...currentMessage,
              content:
                currentMessage.content && currentMessage.content !== THINKING_PLACEHOLDER
                  ? currentMessage.content
                  : '',
              status: 'sent',
              agentRun: settleAgentRunToolActivities(
                {
                  ...ensureAgentRun(currentMessage.agentRun, null, 'cancelled'),
                  completedAt: stoppedAt,
                  todo: undefined
                },
                'cancelled',
                stoppedAt
              )
            }),
            { touchConversation: true }
          )
          return
        }

        const message = error instanceof Error ? error.message : String(error)
        const recovery = planSkillActivationRecovery(error, skills)
        reconcileFailedSkillActivation(conversationId, recovery, {
          modelId,
          permissionMode,
          projectId
        })
        if (recovery.refreshCatalog) {
          requestSkillCatalogRefresh(conversationId)
        }
        updateAssistantMessage(
          conversationId,
          assistantMessageId,
          (currentMessage) => ({
            ...currentMessage,
            content: message,
            status: 'error',
            agentRun: {
              ...ensureAgentRun(currentMessage.agentRun, null, 'failed'),
              error: message
            }
          }),
          { touchConversation: true }
        )
      }
    },
    [
      activeConversationIdRef,
      activeRunBindingMap,
      bufferedAgentEventMap,
      cancelBackendAgentRun,
      cancelledPendingMessageIdSet,
      cancelledRunIdSet,
      conversationsRef,
      contextWindowIndicatorEnabled,
      enqueueChatMessageStateSave,
      handleBoundAgentEvent,
      locallyUnconfirmedStoppedRunIdSet,
      reconcileFailedSkillActivation,
      requestSkillCatalogRefresh,
      retiredAgentRunIdSet,
      scheduleStoppedRunReconciliation,
      setActiveConversationId,
      setConversations,
      stopRequestedPendingMessageIdSet,
      stopRequestedRunIdSet,
      uiPreferences.customPermissions,
      updateAssistantMessage
    ]
  )

  return {
    cleanupRunBinding,
    removeQueuedMessageByClientId,
    requestAssistantResponse,
    restoreRejectedGuidance,
    scheduleStoppedRunReconciliation,
    updateAssistantMessage
  }
}
