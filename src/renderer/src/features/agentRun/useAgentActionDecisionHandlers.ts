import { useCallback, type RefObject } from 'react'
import type { AgentApprovalScope, AgentProposedAction } from '@mycopilot/protocol'
import type { ChatConversation, ChatMessage } from '../chat/chatTypes'
import { approveAgentAction, cancelAgentAction, rejectAgentAction } from '../agent/agentClient'
import { getAgentActionId } from './agentActionUtils'
import {
  applyAgentActionDecisionToChatMessage,
  applyAgentActionExecutionToChatMessage
} from './agentEventReducer'
import { applyAuthoritativePendingActionDecision } from './pendingActionDecision'

type UpdateAssistantMessage = (
  conversationId: string,
  messageId: string,
  updater: (message: ChatMessage) => ChatMessage,
  options?: { persist?: boolean; touchConversation?: boolean }
) => void

interface UseAgentActionDecisionHandlersOptions {
  activeConversationId: string | null
  conversationsRef: RefObject<ChatConversation[]>
  flushMessagePersistence?: (
    conversationId: string,
    messageId: string,
    runId?: string
  ) => Promise<void>
  updateAssistantMessage: UpdateAssistantMessage
}

function getRunId(
  conversations: ChatConversation[],
  conversationId: string,
  messageId: string
): string | undefined {
  return (
    conversations
      .find((conversation) => conversation.id === conversationId)
      ?.messages.find((message) => message.id === messageId)?.agentRun?.runId ?? undefined
  )
}

export function useAgentActionDecisionHandlers({
  activeConversationId,
  conversationsRef,
  flushMessagePersistence,
  updateAssistantMessage
}: UseAgentActionDecisionHandlersOptions) {
  const handleApproveAgentAction = useCallback(
    (messageId: string, action: AgentProposedAction, approvalScope: AgentApprovalScope) => {
      if (!activeConversationId) return false
      const conversationId = activeConversationId
      const actionId = getAgentActionId(action)
      const runId = getRunId(conversationsRef.current, conversationId, messageId)
      const persistenceBoundary =
        flushMessagePersistence?.(conversationId, messageId, runId) ?? Promise.resolve()

      return persistenceBoundary
        .then(() =>
          applyAuthoritativePendingActionDecision({
            runId,
            invoke: (authoritativeRunId) =>
              approveAgentAction(authoritativeRunId, actionId, approvalScope),
            apply: (execution) => {
              updateAssistantMessage(
                conversationId,
                messageId,
                (message) =>
                  applyAgentActionExecutionToChatMessage(message, execution, undefined, action),
                { touchConversation: true }
              )
            },
            onError: (error) => {
              logAgentActionDecisionError('approve', action, error)
            },
            onMissingRunId: () => {
              console.warn('Cannot approve agent action without a run id', { actionId, messageId })
            }
          })
        )
        .then((outcome) => outcome.status === 'applied')
    },
    [activeConversationId, conversationsRef, flushMessagePersistence, updateAssistantMessage]
  )

  const handleRejectAgentAction = useCallback(
    (messageId: string, action: AgentProposedAction, message?: string) => {
      if (!activeConversationId) return false
      const conversationId = activeConversationId
      const actionId = getAgentActionId(action)
      const runId = getRunId(conversationsRef.current, conversationId, messageId)
      const persistenceBoundary =
        flushMessagePersistence?.(conversationId, messageId, runId) ?? Promise.resolve()

      return persistenceBoundary
        .then(() =>
          applyAuthoritativePendingActionDecision({
            runId,
            invoke: (authoritativeRunId) =>
              rejectAgentAction(authoritativeRunId, actionId, message),
            apply: (execution) => {
              updateAssistantMessage(
                conversationId,
                messageId,
                (currentMessage) =>
                  applyAgentActionExecutionToChatMessage(
                    currentMessage,
                    execution,
                    message,
                    action
                  ),
                { touchConversation: true }
              )
            },
            onError: (error) => {
              logAgentActionDecisionError('reject', action, error)
            },
            onMissingRunId: () => {
              console.warn('Cannot reject agent action without a run id', { actionId, messageId })
            }
          })
        )
        .then((outcome) => outcome.status === 'applied')
    },
    [activeConversationId, conversationsRef, flushMessagePersistence, updateAssistantMessage]
  )

  const handleCancelAgentAction = useCallback(
    (messageId: string, action: AgentProposedAction) => {
      if (!activeConversationId) return false
      const conversationId = activeConversationId
      const actionId = getAgentActionId(action)
      const runId = getRunId(conversationsRef.current, conversationId, messageId)
      const persistenceBoundary =
        flushMessagePersistence?.(conversationId, messageId, runId) ?? Promise.resolve()

      return persistenceBoundary
        .then(() =>
          applyAuthoritativePendingActionDecision({
            runId,
            invoke: (authoritativeRunId) => cancelAgentAction(authoritativeRunId, actionId),
            isAccepted: (cancelled) => cancelled,
            apply: () => {
              updateAssistantMessage(
                conversationId,
                messageId,
                (message) => applyAgentActionDecisionToChatMessage(message, action, 'rejected'),
                { touchConversation: true }
              )
            },
            onError: (error) => {
              logAgentActionDecisionError('cancel', action, error)
            },
            onMissingRunId: () => {
              console.warn('Cannot cancel agent action without a run id', { actionId, messageId })
            },
            onNotAccepted: () => {
              console.warn('Agent action cancellation was not accepted', { actionId, runId })
            }
          })
        )
        .then((outcome) => outcome.status === 'applied')
    },
    [activeConversationId, conversationsRef, flushMessagePersistence, updateAssistantMessage]
  )

  return {
    handleApproveAgentAction,
    handleCancelAgentAction,
    handleRejectAgentAction
  }
}

function logAgentActionDecisionError(
  operation: 'approve' | 'cancel' | 'reject',
  action: AgentProposedAction,
  error: unknown
): void {
  if (
    action.type === 'file_change' ||
    action.type === 'mcp_tool_call' ||
    action.type === 'builtin_capability_activation' ||
    action.type === 'builtin_mcp_tool_approval' ||
    action.type === 'browser_risk_approval'
  ) {
    // Protected Host actions may carry internal error details. Keep the entire
    // Error/cause/stack out of Renderer logs; dedicated approval UIs receive only safe views.
    console.error(`Failed to ${operation} protected Agent action`)
    return
  }
  console.error(`Failed to ${operation} agent action`, error)
}
