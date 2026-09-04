import type { AgentEvent, SkillSelection } from '@mycopilot/protocol'
import { useCallback, type MutableRefObject } from 'react'
import {
  cancelAgentRun,
  rewriteConversationTurn,
  startConversationTurn
} from '../features/agent/agentClient'
import { resolveChatPermissions } from '../features/chat/chatPermissions'
import type {
  ChatConversation,
  ChatMessage,
  ChatPermissionMode,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import { DEFAULT_AGENT_MAX_TOKENS } from '../features/agentRun/constants'
import {
  ensureAgentRun,
  settleAgentRunToolActivities
} from '../features/agentRun/agentEventReducer'
import { mergeActivatedSkillSummaries } from '../features/skills/activatedSkillInventory'
import { planSkillActivationRecovery } from '../features/skills/skillActivationRecovery'
import { getAgentInterruptionReason } from '../errors/userFacingError'
import type { ActiveRunBinding } from './appTypes'
import { mergeConversationMessageFromBackend } from './chatMessageFactory'
import {
  REWRITE_CONVERSATION_LOAD_ATTEMPTS,
  isSettledRewriteAssistant,
  loadConversationForRewrite,
  rewriteAssistantFromTurnOutput,
  type RewriteConversationTurnStart,
  type UseAgentRunLifecycleOptions
} from './agentRunLifecycleSupport'

interface UseRequestAssistantResponseOptions {
  activeConversationIdRef: MutableRefObject<string | null>
  activeRunBindingMap: Map<string, ActiveRunBinding>
  bufferedAgentEventMap: Map<string, AgentEvent[]>
  cancelBackendAgentRun: (runId: string) => void
  cancelledPendingMessageIdSet: Set<string>
  cancelledRunIdSet: Set<string>
  contextWindowIndicatorEnabled: boolean
  conversationsRef: MutableRefObject<ChatConversation[]>
  enqueueChatMessageStateSave: UseAgentRunLifecycleOptions['enqueueChatMessageStateSave']
  handleBoundAgentEvent: (
    conversationId: string,
    assistantMessageId: string,
    event: AgentEvent
  ) => void
  locallyUnconfirmedStoppedRunIdSet: Set<string>
  reconcileFailedSkillActivation: UseAgentRunLifecycleOptions['reconcileFailedSkillActivation']
  requestSkillCatalogRefresh: UseAgentRunLifecycleOptions['requestSkillCatalogRefresh']
  retiredAgentRunIdSet: Set<string>
  scheduleStoppedRunReconciliation: (runId: string, binding: ActiveRunBinding) => void
  setActiveConversationId: UseAgentRunLifecycleOptions['conversationState']['setActiveConversationId']
  setConversations: UseAgentRunLifecycleOptions['conversationState']['setConversations']
  stopRequestedPendingMessageIdSet: Set<string>
  stopRequestedRunIdSet: Set<string>
  uiPreferences: UseAgentRunLifecycleOptions['uiPreferences']
  updateAssistantMessage: (
    conversationId: string,
    messageId: string,
    updater: (message: ChatMessage) => ChatMessage,
    options?: { checkpoint?: boolean; persist?: boolean; touchConversation?: boolean }
  ) => void
}

export function useRequestAssistantResponse({
  activeConversationIdRef,
  activeRunBindingMap,
  bufferedAgentEventMap,
  cancelBackendAgentRun,
  cancelledPendingMessageIdSet,
  cancelledRunIdSet,
  contextWindowIndicatorEnabled,
  conversationsRef,
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
  uiPreferences,
  updateAssistantMessage
}: UseRequestAssistantResponseOptions) {
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

  return requestAssistantResponse
}
