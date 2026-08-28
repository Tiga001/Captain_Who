import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import type { AgentContextWindowSnapshot, AgentEvent, SkillSelection } from '@mycopilot/protocol'
import {
  cancelAgentRun,
  getAgentCommandSession,
  listAgentCommandSessions,
  listPendingAgentActions,
  onAgentEvent,
  rewriteConversationTurn,
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
import { DEFAULT_AGENT_MAX_TOKENS } from '../features/agentRun/constants'
import {
  applyAgentEventToChatMessage,
  applyAgentCommandSessionSnapshotToChatMessage,
  ensureAgentRun,
  markMissingAgentCommandSessionOutcomeUnknown,
  removeGuidanceFromChatMessage,
  settleAgentRunToolActivities,
  shouldTouchConversationForAgentEvent
} from '../features/agentRun/agentEventReducer'
import { shouldHydratePendingAgentAction } from '../features/agentRun/agentActionUtils'
import {
  planSkillActivationRecovery,
  type SkillActivationRecoveryPlan
} from '../features/skills/skillActivationRecovery'
import { mergeActivatedSkillSummaries } from '../features/skills/activatedSkillInventory'
import type { Translate } from '../config/translationFormat'
import { getAgentInterruptionReason } from '../errors/userFacingError'
import { createComposerDraft, mergeConversationMessageFromBackend } from './chatMessageFactory'
import type { ActiveRunBinding } from './appTypes'
import {
  STREAM_DELTA_FLUSH_MS,
  STREAM_DELTA_MAX_BUFFER_CHARS,
  type PendingMessageDelta
} from './AppShellSupport'

const STOP_RECONCILIATION_DELAYS_MS = [400, 1500, 4000] as const
const RUN_RECONCILIATION_READ_TIMEOUT_MS = 1500
const MAX_RETIRED_AGENT_RUN_IDS = 1024
const MAX_BUFFERED_AGENT_RUNS = 128
const MAX_BUFFERED_AGENT_EVENTS_PER_RUN = 128
const MAX_UNCONFIRMED_STOPPED_RUNS = 128
const COMMAND_SESSION_HYDRATION_MAX_BYTES = 256 * 1024
const COMMAND_SESSION_HYDRATION_RETRY_DELAYS_MS = [500, 1500, 4000] as const
const REWRITE_CONVERSATION_LOAD_ATTEMPTS = 2
const REWRITE_CONVERSATION_LOAD_TIMEOUT_MS = 750
const TERMINAL_COMMAND_SESSION_STATUSES = new Set([
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
])

type AgentCommandSessionEvent = Extract<
  AgentEvent,
  {
    type: 'command_started' | 'command_output' | 'command_exited' | 'command_interrupted'
  }
>

function isAgentCommandSessionEvent(event: AgentEvent): event is AgentCommandSessionEvent {
  return (
    event.type === 'command_started' ||
    event.type === 'command_output' ||
    event.type === 'command_exited' ||
    event.type === 'command_interrupted'
  )
}

function isTerminalCommandSessionStatus(status: string) {
  return TERMINAL_COMMAND_SESSION_STATUSES.has(status)
}

function isSettledRewriteAssistant(message: ChatMessage | undefined): boolean {
  if (!message || message.role !== 'assistant') return false
  if (message.status === 'sent' || message.status === 'error') return true
  return (
    message.agentRun?.status === 'completed' ||
    message.agentRun?.status === 'failed' ||
    message.agentRun?.status === 'cancelled' ||
    message.agentRun?.status === 'idle'
  )
}

function rewriteAssistantFromTurnOutput(
  output: Awaited<ReturnType<typeof rewriteConversationTurn>>
): ChatMessage {
  const message = mergeConversationMessageFromBackend(
    {
      id: output.assistantMessage.id,
      role: 'assistant',
      content: output.assistantMessage.content,
      createdAt: output.assistantMessage.createdAt,
      status: undefined
    },
    output.assistantMessage
  )
  if (message.status !== 'sent' && message.status !== 'error') return message

  return {
    ...message,
    agentRun: ensureAgentRun(
      undefined,
      output.runId,
      message.status === 'sent' ? 'completed' : 'failed'
    )
  }
}

interface CommandSessionRefreshCandidate {
  assistantMessageId: string
  callId: string
  sessionId: string
}

function runningCommandReceiptSessionId(message: ChatMessage, callId: string) {
  const result = message.agentRun?.toolResults.find(
    (candidate) => candidate.callId === callId && candidate.tool === 'run_command'
  )
  if (
    !result?.ok ||
    !result.result ||
    typeof result.result !== 'object' ||
    Array.isArray(result.result)
  ) {
    return undefined
  }
  const receipt = result.result as Record<string, unknown>
  return receipt.status === 'running' && typeof receipt.sessionId === 'string'
    ? receipt.sessionId
    : undefined
}

function captureCommandSessionRefreshCandidates(
  conversation: ChatConversation | undefined
): CommandSessionRefreshCandidate[] {
  const candidates: CommandSessionRefreshCandidate[] = []
  for (const message of conversation?.messages ?? []) {
    const run = message.agentRun
    if (!run) continue
    for (const call of run.toolCalls) {
      if (call.tool !== 'run_command') continue
      const session = run.commandSessions?.[call.id]
      if (session && isTerminalCommandSessionStatus(session.status)) continue
      const sessionId = session?.sessionId ?? runningCommandReceiptSessionId(message, call.id)
      if (!sessionId) continue
      candidates.push({ assistantMessageId: message.id, callId: call.id, sessionId })
    }
  }
  return candidates
}

function isSameRunBinding(
  current: ActiveRunBinding | undefined,
  expected: ActiveRunBinding
): current is ActiveRunBinding {
  return (
    current?.conversationId === expected.conversationId &&
    current.pendingMessageId === expected.pendingMessageId
  )
}

function loadConversationForRunReconciliation(
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
    const timeoutId = window.setTimeout(() => finish(null), RUN_RECONCILIATION_READ_TIMEOUT_MS)
    void loadConversation(conversationId).then(finish, () => finish(null))
  })
}

function isTerminalRunMessage(
  message: ChatMessage | undefined,
  runId: string
): message is ChatMessage {
  if (message?.agentRun?.runId !== runId) return false
  return (
    message.agentRun.status === 'completed' ||
    message.agentRun.status === 'failed' ||
    message.agentRun.status === 'cancelled'
  )
}

function mergeAuthoritativeTerminalMessage(
  currentMessage: ChatMessage,
  storedMessage: ChatMessage
): ChatMessage {
  const storedRun = storedMessage.agentRun
  const currentRun = currentMessage.agentRun
  if (!storedRun || !currentRun) {
    return {
      ...storedMessage,
      uiState: currentMessage.uiState ?? storedMessage.uiState
    }
  }

  const commandSessions =
    storedRun.commandSessions || currentRun.commandSessions
      ? { ...storedRun.commandSessions, ...currentRun.commandSessions }
      : undefined
  const commandOutputPreviews =
    storedRun.commandOutputPreviews || currentRun.commandOutputPreviews
      ? { ...storedRun.commandOutputPreviews, ...currentRun.commandOutputPreviews }
      : undefined

  return {
    ...storedMessage,
    uiState: currentMessage.uiState ?? storedMessage.uiState,
    agentRun: {
      ...storedRun,
      ...(commandSessions ? { commandSessions } : {}),
      ...(commandOutputPreviews ? { commandOutputPreviews } : {})
    }
  }
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
    activeConversationId: string | null
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

interface RewriteConversationTurnStart {
  requestId: string
  sourceAssistantMessageId: string
  sourceUserMessageId: string
}

function loadConversationForRewrite(conversationId: string): Promise<ChatConversation | null> {
  return new Promise((resolve, reject) => {
    let settled = false
    const finish = (conversation: ChatConversation | null, error?: unknown) => {
      if (settled) return
      settled = true
      window.clearTimeout(timeoutId)
      if (error !== undefined) reject(error)
      else resolve(conversation)
    }
    const timeoutId = window.setTimeout(
      () => finish(null, new Error('Timed out loading the rewritten conversation')),
      REWRITE_CONVERSATION_LOAD_TIMEOUT_MS
    )
    void loadConversation(conversationId).then(
      (conversation) => finish(conversation),
      (error) => finish(null, error)
    )
  })
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
    activeConversationId,
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
  const commandSessionHydratedConversationSetRef = useRef<Set<string>>(new Set())
  const commandSessionHydrationEpochRef = useRef<Map<string, number>>(new Map())
  const commandSessionHydrationRetryCountRef = useRef<Map<string, number>>(new Map())
  const commandSessionHydrationRetryTimerRef = useRef<Map<string, number>>(new Map())
  const [commandSessionHydrationRetryRevision, setCommandSessionHydrationRetryRevision] =
    useState(0)
  const lastCommandSessionActiveConversationIdRef = useRef<string | null>(null)
  const commandSessionHydrationMountedRef = useRef(true)

  useEffect(() => {
    const hydratedConversationSet = commandSessionHydratedConversationSetRef.current
    const hydrationEpochMap = commandSessionHydrationEpochRef.current
    const retryCountMap = commandSessionHydrationRetryCountRef.current
    const retryTimerMap = commandSessionHydrationRetryTimerRef.current
    commandSessionHydrationMountedRef.current = true
    return () => {
      commandSessionHydrationMountedRef.current = false
      hydratedConversationSet.clear()
      retryCountMap.clear()
      for (const timerId of retryTimerMap.values()) window.clearTimeout(timerId)
      retryTimerMap.clear()
      for (const [conversationId, epoch] of hydrationEpochMap) {
        hydrationEpochMap.set(conversationId, epoch + 1)
      }
    }
  }, [])

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
          updatedAt: options.touchConversation
            ? Math.max(timestamp, conversation.updatedAt + 1)
            : conversation.updatedAt
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

  const reconcileTerminalRunFromStorage = useCallback(
    (conversationId: string, assistantMessageId: string, runId: string) => {
      void loadConversationForRunReconciliation(conversationId).then((storedConversation) => {
        const storedMessage = storedConversation?.messages.find(
          (message) => message.id === assistantMessageId
        )
        const hasAuthoritativeTerminal = isTerminalRunMessage(storedMessage, runId)
        let messageToSave: ChatMessage | null = null

        setConversations((currentConversations) =>
          currentConversations.map((conversation) => {
            if (conversation.id !== conversationId) return conversation

            return {
              ...conversation,
              messages: conversation.messages.map((currentMessage) => {
                if (
                  currentMessage.id !== assistantMessageId ||
                  currentMessage.agentRun?.runId !== runId
                ) {
                  return currentMessage
                }

                // A terminal done notification is emitted only after the backend has committed the
                // complete assistant trace. Replacing the bounded live projection here prevents an
                // early Skill/resource event from being lost when pre-binding buffering overflowed.
                // Managed command sessions outlive the Agent Run, so retain their newer live view.
                messageToSave = hasAuthoritativeTerminal
                  ? mergeAuthoritativeTerminalMessage(currentMessage, storedMessage)
                  : currentMessage
                return messageToSave
              })
            }
          })
        )

        // Fence every older renderer write with the reconciled terminal message. The fallback is
        // retained for tests and defensive compatibility if storage cannot return the committed row.
        if (messageToSave) enqueueChatMessageStateSave(conversationId, messageToSave)
      })
    },
    [enqueueChatMessageStateSave, setConversations]
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
          if (!shouldHydratePendingAgentAction(pendingAction)) continue
          if (!pendingAction.conversationId || !pendingAction.assistantMessageId) continue
          const conversation = conversationsRef.current.find(
            (candidate) => candidate.id === pendingAction.conversationId
          )
          if (!conversation) continue
          const message = conversation.messages.find(
            (candidate) => candidate.id === pendingAction.assistantMessageId
          )
          if (
            pendingAction.toolName === 'run_command' &&
            pendingAction.toolCallId &&
            message?.agentRun?.commandSessions?.[pendingAction.toolCallId]
          ) {
            // This list response was captured before the Session authority advanced the same call.
            // Do not resurrect either its approval card or an obsolete active Run binding.
            continue
          }

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

  useEffect(() => {
    const hydratedConversationSet = commandSessionHydratedConversationSetRef.current
    const hydrationEpochMap = commandSessionHydrationEpochRef.current
    const retryCountMap = commandSessionHydrationRetryCountRef.current
    const retryTimerMap = commandSessionHydrationRetryTimerRef.current
    const loadedConversationIds = new Set(
      conversations
        .filter((conversation) => conversation.messagesLoaded !== false)
        .map((conversation) => conversation.id)
    )
    for (const conversationId of hydratedConversationSet) {
      if (loadedConversationIds.has(conversationId)) continue
      hydratedConversationSet.delete(conversationId)
      hydrationEpochMap.set(conversationId, (hydrationEpochMap.get(conversationId) ?? 0) + 1)
    }
    const conversationIds = new Set(
      conversations
        .filter(
          (conversation) =>
            conversation.messagesLoaded !== false && !hydratedConversationSet.has(conversation.id)
        )
        .map((conversation) => conversation.id)
    )
    const activeConversationChanged =
      activeConversationId !== lastCommandSessionActiveConversationIdRef.current
    lastCommandSessionActiveConversationIdRef.current = activeConversationId
    if (
      activeConversationChanged &&
      activeConversationId &&
      loadedConversationIds.has(activeConversationId)
    ) {
      // Reopening a previously loaded conversation refreshes its Host-owned Session projection.
      // Live events normally keep inactive conversations current, while this closes any IPC gap.
      conversationIds.add(activeConversationId)
    }

    for (const conversationId of conversationIds) {
      hydratedConversationSet.add(conversationId)
      const requestEpoch = (hydrationEpochMap.get(conversationId) ?? 0) + 1
      hydrationEpochMap.set(conversationId, requestEpoch)
      const pendingRetryTimer = retryTimerMap.get(conversationId)
      if (pendingRetryTimer !== undefined) {
        window.clearTimeout(pendingRetryTimer)
        retryTimerMap.delete(conversationId)
      }
      // Capture only identities which existed before issuing the list request. The success
      // reconciliation below must never classify a Session created while this request is in flight
      // as missing from an older Host snapshot.
      const refreshCandidates = captureCommandSessionRefreshCandidates(
        conversationsRef.current.find(
          (candidate) => candidate.id === conversationId && candidate.messagesLoaded !== false
        )
      )
      const scheduleHydrationRetry = (error: unknown) => {
        if (
          !commandSessionHydrationMountedRef.current ||
          hydrationEpochMap.get(conversationId) !== requestEpoch ||
          retryTimerMap.has(conversationId)
        ) {
          return
        }
        const retryCount = retryCountMap.get(conversationId) ?? 0
        const retryDelay = COMMAND_SESSION_HYDRATION_RETRY_DELAYS_MS[retryCount]
        if (retryDelay !== undefined) {
          retryCountMap.set(conversationId, retryCount + 1)
          const timerId = window.setTimeout(() => {
            retryTimerMap.delete(conversationId)
            if (
              !commandSessionHydrationMountedRef.current ||
              hydrationEpochMap.get(conversationId) !== requestEpoch
            ) {
              return
            }
            hydratedConversationSet.delete(conversationId)
            setCommandSessionHydrationRetryRevision((revision) => revision + 1)
          }, retryDelay)
          retryTimerMap.set(conversationId, timerId)
        }
        console.error('Failed to hydrate managed command Sessions', error)
      }

      void listAgentCommandSessions({ conversationId })
        .then(async ({ sessions }) => {
          if (
            !commandSessionHydrationMountedRef.current ||
            hydrationEpochMap.get(conversationId) !== requestEpoch
          ) {
            return
          }
          const conversation = conversationsRef.current.find(
            (candidate) => candidate.id === conversationId && candidate.messagesLoaded !== false
          )
          if (!conversation) return

          const matchingSessions = sessions.filter(
            (session) =>
              session.conversationId === conversationId &&
              conversation.messages.some((message) => {
                const run = message.agentRun
                if (!run || message.id !== session.assistantMessageId) return false
                return (
                  (run.runId === null || run.runId === session.originRunId) &&
                  run.toolCalls.some(
                    (call) => call.id === session.callId && call.tool === 'run_command'
                  )
                )
              })
          )

          // The list projection is already authoritative for process state. Apply it before
          // loading transcript bytes so a failed or slow transcript read cannot leave a command
          // looking permanently active (or permanently awaiting approval) after reload.
          for (const session of matchingSessions) {
            updateAssistantMessage(
              conversationId,
              session.assistantMessageId,
              (message) => applyAgentCommandSessionSnapshotToChatMessage(message, session),
              {
                persist: isTerminalCommandSessionStatus(session.status),
                touchConversation: false
              }
            )
          }

          const listedSessionIds = new Set(matchingSessions.map((session) => session.sessionId))
          const reconciledAt = Date.now()
          for (const candidate of refreshCandidates) {
            if (listedSessionIds.has(candidate.sessionId)) continue
            updateAssistantMessage(
              conversationId,
              candidate.assistantMessageId,
              (message) =>
                markMissingAgentCommandSessionOutcomeUnknown(
                  message,
                  candidate.callId,
                  candidate.sessionId,
                  reconciledAt
                ),
              { persist: true, touchConversation: false }
            )
          }

          const transcriptResults = await Promise.allSettled(
            matchingSessions.map((session) =>
              getAgentCommandSession({
                conversationId,
                sessionId: session.sessionId,
                afterSequence: 0,
                maxBytes: COMMAND_SESSION_HYDRATION_MAX_BYTES
              })
            )
          )
          if (
            !commandSessionHydrationMountedRef.current ||
            hydrationEpochMap.get(conversationId) !== requestEpoch
          ) {
            return
          }
          if (
            !conversationsRef.current.some(
              (candidate) => candidate.id === conversationId && candidate.messagesLoaded !== false
            )
          ) {
            return
          }

          for (let index = 0; index < transcriptResults.length; index += 1) {
            const result = transcriptResults[index]
            const listedSession = matchingSessions[index]
            if (result.status !== 'fulfilled' || !listedSession) continue
            const { session, transcript } = result.value
            if (
              session.conversationId !== conversationId ||
              session.sessionId !== listedSession.sessionId ||
              session.callId !== listedSession.callId ||
              session.assistantMessageId !== listedSession.assistantMessageId ||
              session.originRunId !== listedSession.originRunId
            ) {
              continue
            }

            updateAssistantMessage(
              conversationId,
              session.assistantMessageId,
              (message) =>
                applyAgentCommandSessionSnapshotToChatMessage(message, session, transcript),
              {
                persist: isTerminalCommandSessionStatus(session.status),
                touchConversation: false
              }
            )
          }
          const transcriptFailure = transcriptResults.find(
            (result): result is PromiseRejectedResult => result.status === 'rejected'
          )
          if (transcriptFailure) {
            // Status snapshots above are already authoritative and remain applied. Retry the
            // bounded transcript read without rolling back process state.
            scheduleHydrationRetry(transcriptFailure.reason)
          } else {
            retryCountMap.delete(conversationId)
          }
        })
        .catch(scheduleHydrationRetry)
    }
  }, [
    activeConversationId,
    commandSessionHydrationRetryRevision,
    conversations,
    conversationsRef,
    updateAssistantMessage
  ])

  const handleBoundAgentEvent = useCallback(
    (conversationId: string, assistantMessageId: string, agentEvent: AgentEvent) => {
      const isCommandSessionEvent = isAgentCommandSessionEvent(agentEvent)
      const isTerminalCommandSessionEvent =
        agentEvent.type === 'command_exited' || agentEvent.type === 'command_interrupted'
      // Once run_command has handed a process to the Session registry, its lifecycle is no longer
      // owned by the originating Agent Run. A later stop/retirement of that Run must not suppress
      // process output or its terminal state on the original command card.
      if (!isCommandSessionEvent && agentEvent.runId && cancelledRunIdSet.has(agentEvent.runId)) {
        return
      }
      if (!isCommandSessionEvent && cancelledPendingMessageIdSet.has(assistantMessageId)) return

      if (agentEvent.type === 'message_delta') {
        bufferMessageDelta(conversationId, assistantMessageId, agentEvent)
        return
      }

      if (agentEvent.type === 'tool_input_progress') return

      if (
        agentEvent.type === 'file_change_preview_updated' ||
        agentEvent.type === 'file_change_preview_cleared'
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
        {
          // Retry countdowns are Host-authoritative transient UI state. Persisting retryAt would
          // resurrect a stale countdown after reload; durable Run state remains unchanged.
          persist:
            agentEvent.type !== 'llm_retry' &&
            (!isCommandSessionEvent || isTerminalCommandSessionEvent) &&
            !(agentEvent.type === 'done' && agentEvent.status !== 'waiting_for_approval'),
          touchConversation:
            !isCommandSessionEvent && shouldTouchConversationForAgentEvent(agentEvent)
        }
      )

      if (agentEvent.type === 'done') {
        stopRequestedRunIdSet.delete(agentEvent.runId)
        if (agentEvent.status !== 'waiting_for_approval') {
          reconcileTerminalRunFromStorage(conversationId, assistantMessageId, agentEvent.runId)
        }
        if (agentEvent.success && agentEvent.status !== 'waiting_for_approval') {
          const completedAt = Date.now()
          let conversationToSave: ChatConversation | null = null
          const nextConversations = conversationsRef.current.map((conversation) => {
            if (
              conversation.id !== conversationId ||
              activeConversationIdRef.current === conversationId ||
              (conversation.archivedAt !== null && conversation.archivedAt !== undefined) ||
              conversation.pendingArchivedAt !== undefined
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
      reconcileTerminalRunFromStorage,
      removeQueuedMessageByClientId,
      restoreRejectedGuidance,
      setConversations,
      stopRequestedRunIdSet,
      t,
      updateAssistantMessage
    ]
  )

  useEffect(() => {
    // Automation turns are admitted by core-server, so this Renderer never receives the local
    // startConversationTurn response that normally establishes the Run-to-message binding. When
    // an authoritative conversation refresh reveals such a non-terminal Run, bind it here and
    // replay any events that arrived before the conversation metadata/detail refresh completed.
    for (const conversation of conversations) {
      if (conversation.messagesLoaded === false) continue
      for (const message of conversation.messages) {
        const run = message.agentRun
        const runId = run?.runId
        if (
          !runId ||
          (run.status !== 'queued' &&
            run.status !== 'starting' &&
            run.status !== 'running' &&
            run.status !== 'waiting_for_approval')
        ) {
          continue
        }

        const expectedBinding = {
          conversationId: conversation.id,
          pendingMessageId: message.id
        }
        if (!isSameRunBinding(activeRunBindingMap.get(runId), expectedBinding)) {
          activeRunBindingMap.set(runId, expectedBinding)
        }
        const bufferedEvents = bufferedAgentEventMap.get(runId)
        if (!bufferedEvents?.length) continue
        bufferedAgentEventMap.delete(runId)
        for (const agentEvent of bufferedEvents) {
          handleBoundAgentEvent(conversation.id, message.id, agentEvent)
        }
      }
    }
  }, [activeRunBindingMap, bufferedAgentEventMap, conversations, handleBoundAgentEvent])

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

          void loadConversationForRunReconciliation(binding.conversationId)
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

      // Managed process events carry their durable owner identity. Route them directly to the
      // original assistant message even after cleanupRunBinding retired the Agent Run. This does
      // not create a message and does not revive the assistant/Run pending state.
      if (isAgentCommandSessionEvent(agentEvent)) {
        handleBoundAgentEvent(agentEvent.conversationId, agentEvent.assistantMessageId, agentEvent)
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
      title?: string,
      rewrite?: RewriteConversationTurnStart
    ): Promise<boolean> => {
      if (!rewrite) {
        updateAssistantMessage(conversationId, assistantMessageId, (message) => ({
          ...message,
          agentRun: ensureAgentRun(message.agentRun, null, 'starting')
        }))
      }

      try {
        const turnInput = {
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
        }
        const startOutput = rewrite
          ? await rewriteConversationTurn({
              requestId: rewrite.requestId,
              sourceAssistantMessageId: rewrite.sourceAssistantMessageId,
              sourceUserMessageId: rewrite.sourceUserMessageId,
              turn: turnInput
            })
          : await startConversationTurn(turnInput)

        let rewrittenConversation: ChatConversation | null = null
        if (rewrite) {
          for (let attempt = 0; attempt < REWRITE_CONVERSATION_LOAD_ATTEMPTS; attempt += 1) {
            try {
              const loadedConversation = await loadConversationForRewrite(
                startOutput.conversationId
              )
              if (
                loadedConversation?.id === conversationId &&
                loadedConversation.messages.some(
                  (message) => message.id === startOutput.userMessageId && message.role === 'user'
                ) &&
                loadedConversation.messages.some(
                  (message) =>
                    message.id === startOutput.assistantMessageId && message.role === 'assistant'
                )
              ) {
                rewrittenConversation = loadedConversation
                break
              }
            } catch {
              // The rewrite transaction is already committed. A transient projection read must
              // not turn that known success into a failed edit or strand its running event stream.
            }
          }
        }

        if (cancelledPendingMessageIdSet.has(assistantMessageId)) {
          cancelledPendingMessageIdSet.delete(assistantMessageId)
          cancelledRunIdSet.add(startOutput.runId)
          cancelBackendAgentRun(startOutput.runId)
          bufferedAgentEventMap.delete(startOutput.runId)
          return false
        }

        const stopWasRequested = stopRequestedPendingMessageIdSet.delete(assistantMessageId)

        const resolvedConversationId = startOutput.conversationId
        const resolvedAssistantMessageId = startOutput.assistantMessageId
        let resolvedAssistantMessage: ChatMessage | null = null
        const authoritativeRewriteAssistant = rewrite
          ? (rewrittenConversation?.messages.find(
              (message) =>
                message.id === startOutput.assistantMessageId && message.role === 'assistant'
            ) ?? rewriteAssistantFromTurnOutput(startOutput))
          : undefined
        const rewriteAlreadySettled = isSettledRewriteAssistant(authoritativeRewriteAssistant)

        const currentConversation = conversationsRef.current.find(
          (candidate) => candidate.id === conversationId
        )
        if (!currentConversation) {
          if (rewrite) {
            cancelBackendAgentRun(startOutput.runId)
            bufferedAgentEventMap.delete(startOutput.runId)
          }
          return false
        }
        let conversationToMerge = rewrittenConversation ?? currentConversation
        if (rewrite && !rewrittenConversation) {
          const rewrittenPairAlreadyPresent =
            currentConversation.messages.some(
              (message) => message.id === startOutput.userMessageId && message.role === 'user'
            ) &&
            currentConversation.messages.some(
              (message) =>
                message.id === startOutput.assistantMessageId && message.role === 'assistant'
            )
          if (rewrittenPairAlreadyPresent) {
            conversationToMerge = currentConversation
          } else {
            const sourceUserIndex = currentConversation.messages.findIndex(
              (message) => message.id === rewrite.sourceUserMessageId && message.role === 'user'
            )
            const sourceAssistantIndex = currentConversation.messages.findIndex(
              (message) =>
                message.id === rewrite.sourceAssistantMessageId && message.role === 'assistant'
            )
            const sourcePairIsPresent =
              sourceUserIndex >= 0 && sourceAssistantIndex === sourceUserIndex + 1
            const prefix = sourcePairIsPresent
              ? currentConversation.messages.slice(0, sourceUserIndex)
              : currentConversation.messages
            const suffix = sourcePairIsPresent
              ? currentConversation.messages.slice(sourceAssistantIndex + 1)
              : []
            conversationToMerge = {
              ...currentConversation,
              modelId,
              messages: [
                ...prefix,
                mergeConversationMessageFromBackend(
                  {
                    id: userMessageId,
                    role: 'user',
                    content,
                    createdAt: startOutput.userMessage.createdAt,
                    status: 'sent',
                    attachments
                  },
                  startOutput.userMessage
                ),
                mergeConversationMessageFromBackend(
                  {
                    id: assistantMessageId,
                    role: 'assistant',
                    content: '',
                    createdAt: startOutput.assistantMessage.createdAt,
                    status: 'pending'
                  },
                  startOutput.assistantMessage
                ),
                ...suffix
              ],
              updatedAt: Math.max(
                currentConversation.updatedAt + 1,
                startOutput.userMessage.createdAt,
                startOutput.assistantMessage.createdAt
              ),
              unreadAt: null
            }
          }
        }
        const nextConversations = conversationsRef.current.map((conversation) =>
          conversation.id === conversationId
            ? {
                ...conversationToMerge,
                id: resolvedConversationId,
                messages: conversationToMerge.messages.map((message) => {
                  if (message.id === userMessageId) {
                    return mergeConversationMessageFromBackend(message, startOutput.userMessage)
                  }

                  if (message.id === assistantMessageId) {
                    if (rewriteAlreadySettled && authoritativeRewriteAssistant) {
                      resolvedAssistantMessage = authoritativeRewriteAssistant
                      return authoritativeRewriteAssistant
                    }
                    const mergedMessage = mergeConversationMessageFromBackend(
                      message,
                      startOutput.assistantMessage
                    )
                    resolvedAssistantMessage = {
                      ...mergedMessage,
                      content: mergedMessage.content || message.content,
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
        if (resolvedAssistantMessage && !rewriteAlreadySettled) {
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

        if (rewriteAlreadySettled) {
          activeRunBindingMap.delete(startOutput.runId)
          bufferedAgentEventMap.delete(startOutput.runId)
          locallyUnconfirmedStoppedRunIdSet.delete(startOutput.runId)
          retiredAgentRunIdSet.add(startOutput.runId)
          return true
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
        return true
      } catch (error) {
        if (rewrite) {
          const recovery = planSkillActivationRecovery(error, skills)
          reconcileFailedSkillActivation(conversationId, recovery, {
            modelId,
            permissionMode,
            projectId
          })
          if (recovery.refreshCatalog) {
            requestSkillCatalogRefresh(conversationId)
          }
          throw error
        }
        if (cancelledPendingMessageIdSet.has(assistantMessageId)) {
          cancelledPendingMessageIdSet.delete(assistantMessageId)
          return false
        }
        if (stopRequestedPendingMessageIdSet.delete(assistantMessageId)) {
          const stoppedAt = Date.now()
          updateAssistantMessage(
            conversationId,
            assistantMessageId,
            (currentMessage) => ({
              ...currentMessage,
              content: currentMessage.content,
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
          return false
        }

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
          (currentMessage) => {
            const failedAt = Date.now()
            return {
              ...currentMessage,
              status: 'sent',
              agentRun: settleAgentRunToolActivities(
                {
                  ...ensureAgentRun(currentMessage.agentRun, null, 'failed'),
                  status: 'failed',
                  completedAt: failedAt,
                  todo: undefined,
                  interruption: { reason: getAgentInterruptionReason(error) },
                  error: undefined,
                  llmRetry: undefined
                },
                'failed',
                failedAt
              )
            }
          },
          { touchConversation: true }
        )
        return false
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
