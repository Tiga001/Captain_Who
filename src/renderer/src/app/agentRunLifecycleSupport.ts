import type { AgentContextWindowSnapshot, AgentEvent } from '@mycopilot/protocol'
import type { Dispatch, MutableRefObject, SetStateAction } from 'react'
import { rewriteConversationTurn } from '../features/agent/agentClient'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatQueuedMessage
} from '../features/chat/chatTypes'
import { ensureAgentRun } from '../features/agentRun/agentEventReducer'
import type { SkillActivationRecoveryPlan } from '../features/skills/skillActivationRecovery'
import { loadConversation } from '../features/storage/storageClient'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import type { Translate } from '../config/translationFormat'
import { mergeConversationMessageFromBackend } from './chatMessageFactory'
import type { ActiveRunBinding, AutoSubmitQueuedMessage } from './appTypes'
import type { PendingMessageDelta } from './AppShellSupport'

export const STOP_RECONCILIATION_DELAYS_MS = [400, 1500, 4000] as const
export const RUN_RECONCILIATION_READ_TIMEOUT_MS = 1500
export const MAX_RETIRED_AGENT_RUN_IDS = 1024
export const MAX_BUFFERED_AGENT_RUNS = 128
export const MAX_BUFFERED_AGENT_EVENTS_PER_RUN = 128
export const MAX_UNCONFIRMED_STOPPED_RUNS = 128
export const COMMAND_SESSION_HYDRATION_MAX_BYTES = 256 * 1024
export const COMMAND_SESSION_HYDRATION_RETRY_DELAYS_MS = [500, 1500, 4000] as const
export const REWRITE_CONVERSATION_LOAD_ATTEMPTS = 2
export const REWRITE_CONVERSATION_LOAD_TIMEOUT_MS = 750
export const TERMINAL_COMMAND_SESSION_STATUSES = new Set([
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
])

export type AgentCommandSessionEvent = Extract<
  AgentEvent,
  {
    type: 'command_started' | 'command_output' | 'command_exited' | 'command_interrupted'
  }
>

export function isAgentCommandSessionEvent(event: AgentEvent): event is AgentCommandSessionEvent {
  return (
    event.type === 'command_started' ||
    event.type === 'command_output' ||
    event.type === 'command_exited' ||
    event.type === 'command_interrupted'
  )
}

export function isTerminalCommandSessionStatus(status: string) {
  return TERMINAL_COMMAND_SESSION_STATUSES.has(status)
}

export function isSettledRewriteAssistant(message: ChatMessage | undefined): boolean {
  if (!message || message.role !== 'assistant') return false
  if (message.status === 'sent' || message.status === 'error') return true
  return (
    message.agentRun?.status === 'completed' ||
    message.agentRun?.status === 'failed' ||
    message.agentRun?.status === 'cancelled' ||
    message.agentRun?.status === 'idle'
  )
}

export function rewriteAssistantFromTurnOutput(
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

export interface CommandSessionRefreshCandidate {
  assistantMessageId: string
  callId: string
  sessionId: string
}

export function runningCommandReceiptSessionId(message: ChatMessage, callId: string) {
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

export function captureCommandSessionRefreshCandidates(
  conversation: ChatConversation | undefined
): CommandSessionRefreshCandidate[] {
  const candidates: CommandSessionRefreshCandidate[] = []
  for (const message of getCommandSessionOwnerMessages(conversation)) {
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

export function getCommandSessionOwnerMessages(
  conversation: ChatConversation | undefined
): ChatMessage[] {
  const messages = conversation?.messages ?? []
  const boundaryMessageId = conversation?.continuationOrigin?.boundaryMessageId
  if (!boundaryMessageId) return messages
  const boundaryIndex = messages.findIndex((message) => message.id === boundaryMessageId)
  // Copied history is an immutable snapshot, including its command cards. Only messages
  // created after the Host-provided fork boundary can own live Sessions in the new task.
  return messages.slice(boundaryIndex + 1)
}

export function isSameRunBinding(
  current: ActiveRunBinding | undefined,
  expected: ActiveRunBinding
): current is ActiveRunBinding {
  return (
    current?.conversationId === expected.conversationId &&
    current.pendingMessageId === expected.pendingMessageId
  )
}

export function loadConversationForRunReconciliation(
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

export function isTerminalRunMessage(
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

export function mergeAuthoritativeTerminalMessage(
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

export interface AgentRunLifecycleRefs {
  activeRunBindings: MutableRefObject<Map<string, ActiveRunBinding>>
  autoSubmitQueuedMessage: MutableRefObject<AutoSubmitQueuedMessage>
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

export interface UseAgentRunLifecycleOptions {
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
  enqueueChatMessageCheckpoint: (conversationId: string, message: ChatMessage) => void
  enqueueChatMessageStateSave: (conversationId: string, message: ChatMessage) => void
  enqueueConversationMetaSave: (conversation: ChatConversation) => void
  flushChatMessageStateSave: (conversationId: string, messageId: string) => Promise<void>
  flushConversationMessageStateSaves: (conversationId: string) => Promise<void>
  recordContextWindowSnapshot: (
    conversationId: string,
    modelConfigId: string,
    snapshot: AgentContextWindowSnapshot
  ) => void
  reconcileFailedSkillActivation: (
    scopeId: string,
    recovery: SkillActivationRecoveryPlan,
    fallback: Pick<ChatComposerDraft, 'modelId' | 'permissionMode' | 'projectId'>
  ) => void
  refs: AgentRunLifecycleRefs
  requestSkillCatalogRefresh: (scopeId: string) => void
  sealAndFlushChatMessageStateSaves: () => Promise<void>
  showToast: (message: string) => void
  t: Translate
  uiPreferences: Pick<UiPreferencesSnapshot, 'customPermissions'>
}

export interface RewriteConversationTurnStart {
  requestId: string
  sourceAssistantMessageId: string
  sourceUserMessageId: string
}

export function loadConversationForRewrite(
  conversationId: string
): Promise<ChatConversation | null> {
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
