/* eslint-disable react-hooks/exhaustive-deps -- extracted callbacks keep AppShell's original dependency arrays; omitted inputs are stable refs and React dispatchers. */
import {
  useCallback,
  useEffect,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import type {
  AgentProviderTransitionOperation,
  AgentProviderTransitionReason,
  SkillSelection
} from '@mycopilot/protocol'
import type { ModelConfig } from '../config/modelConfig'
import type { Translate } from '../config/translationFormat'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import { loadInputAttachments } from '../features/storage/storageClient'
import { useProviderTransition } from '../features/agentRun/useProviderTransition'
import { getEditableLastTurn } from './appShellConversationUtils'
import { buildMessageContentWithAttachments } from './appShellConversationUtils'
import {
  createAssistantMessage,
  createComposerDraft,
  createConversationTitle,
  createId,
  createUserMessage
} from './chatMessageFactory'

interface EditRewriteAttempt {
  assistantMessage: ChatMessage
  attachments: NonNullable<ChatSubmitOptions['attachments']>
  content: string
  modelId: string
  permissionMode: ChatSubmitOptions['permissionMode']
  projectId: string | null
  requestId: string
  skills: SkillSelection[]
  title?: string
  userMessage: ChatMessage
}

type PendingProviderTransitionSubmission =
  | { kind: 'composer'; message: string; options: ChatSubmitOptions }
  | { kind: 'queued_message'; queueMessageId: string }

type RequestAssistantResponse = ReturnType<
  (typeof import('./useAgentRunLifecycle'))['useAgentRunLifecycle']
>['requestAssistantResponse']

interface UseAppShellMessageSubmissionOptions {
  activeConversationIdRef: MutableRefObject<string | null>
  activeDraft: ChatComposerDraft
  activeDraftSelectedModel: ModelConfig | null
  autoSubmitQueuedMessageRef: MutableRefObject<(conversationId: string) => void>
  conversationsRef: MutableRefObject<ChatConversation[]>
  draftsRef: MutableRefObject<Record<string, ChatComposerDraft>>
  editRewriteAttemptsRef: MutableRefObject<Map<string, EditRewriteAttempt>>
  editRewriteInFlightRef: MutableRefObject<Set<string>>
  editSubmissionSeqRef: MutableRefObject<number>
  enabledModels: ModelConfig[]
  enqueueChatMessagesUpsert: (
    conversationId: string,
    messages: ChatMessage[],
    startIndex: number
  ) => void
  enqueueConversationMetaSave: (conversation: ChatConversation) => void
  mutateDraft: (
    scopeId: string,
    updater: (draft: ChatComposerDraft) => ChatComposerDraft
  ) => ChatComposerDraft
  pendingProviderTransitionSubmissionsRef: MutableRefObject<
    Map<string, PendingProviderTransitionSubmission>
  >
  requestAssistantResponse: RequestAssistantResponse
  restoreSubmittedSkills: (
    scopeId: string,
    submittedSkills: readonly SkillSelection[],
    fallback: Pick<ChatComposerDraft, 'modelId' | 'permissionMode' | 'projectId'>
  ) => void
  setActiveConversationId: Dispatch<SetStateAction<string | null>>
  setActiveConversationInitialScrollTop: Dispatch<SetStateAction<number | null>>
  setConversationScrollToBottomSignal: Dispatch<SetStateAction<number>>
  setConversationsWithRef: (value: SetStateAction<ChatConversation[]>) => void
  setScrollTargetMessageId: Dispatch<SetStateAction<string | null>>
  showToast: (message: string) => void
  t: Translate
  updateDraft: (scopeId: string, draft: ChatComposerDraft) => void
  waitForConversationSaves: (conversationId: string) => Promise<void>
  waitForMessageUpserts: (conversationId: string) => Promise<void>
}

export function useAppShellMessageSubmission({
  activeConversationIdRef,
  activeDraft,
  activeDraftSelectedModel,
  autoSubmitQueuedMessageRef,
  conversationsRef,
  draftsRef,
  editRewriteAttemptsRef,
  editRewriteInFlightRef,
  editSubmissionSeqRef,
  enabledModels,
  enqueueChatMessagesUpsert,
  enqueueConversationMetaSave,
  mutateDraft,
  pendingProviderTransitionSubmissionsRef,
  requestAssistantResponse,
  restoreSubmittedSkills,
  setActiveConversationId,
  setActiveConversationInitialScrollTop,
  setConversationScrollToBottomSignal,
  setConversationsWithRef,
  setScrollTargetMessageId,
  showToast,
  t,
  updateDraft,
  waitForConversationSaves,
  waitForMessageUpserts
}: UseAppShellMessageSubmissionOptions) {
  const submitMessageToConversation = useCallback(
    (
      targetConversationId: string | null,
      message: string,
      options: ChatSubmitOptions,
      behavior: { activate: boolean; preserveComposerContent: boolean }
    ): boolean => {
      const targetConversation = targetConversationId
        ? (conversationsRef.current.find(
            (conversation) => conversation.id === targetConversationId
          ) ?? null)
        : null
      if (
        targetConversationId !== null &&
        (!targetConversation ||
          targetConversation.archivedAt ||
          targetConversation.pendingArchivedAt !== undefined ||
          targetConversation.messages.some(
            (message) => message.agentRun?.status === 'waiting_for_user_input'
          ))
      ) {
        return false
      }
      const now = targetConversation
        ? Math.max(Date.now(), targetConversation.updatedAt + 1)
        : Date.now()
      const conversationId = targetConversation?.id ?? createId('conversation')
      const userMessage = createUserMessage(message, options.attachments ?? [])
      const assistantMessage = createAssistantMessage('', 'pending')
      const title = createConversationTitle(message, t('chat.newConversation'))
      const conversationToSave: ChatConversation = targetConversation
        ? {
            ...targetConversation,
            modelId: options.modelId,
            messages: [...targetConversation.messages, userMessage, assistantMessage],
            updatedAt: now
          }
        : {
            id: conversationId,
            projectId: options.projectId,
            modelId: options.modelId,
            title,
            messages: [userMessage, assistantMessage],
            messagesLoaded: true,
            createdAt: now,
            updatedAt: now,
            pinnedAt: null,
            archivedAt: null,
            unreadAt: null
          }

      const existingConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      const nextConversations = existingConversation
        ? conversationsRef.current.map((conversation) =>
            conversation.id === conversationId ? conversationToSave : conversation
          )
        : [conversationToSave, ...conversationsRef.current]

      setConversationsWithRef(nextConversations)
      enqueueConversationMetaSave(conversationToSave)
      enqueueChatMessagesUpsert(
        conversationId,
        [userMessage, assistantMessage],
        targetConversation?.messages.length ?? 0
      )
      if (behavior.activate) {
        activeConversationIdRef.current = conversationId
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
        setConversationScrollToBottomSignal((signal) => signal + 1)
        setActiveConversationId(conversationId)
      }
      const currentDraft = draftsRef.current[conversationId] ?? createComposerDraft()
      updateDraft(
        conversationId,
        behavior.preserveComposerContent
          ? {
              ...currentDraft,
              modelId: options.modelId,
              permissionMode: options.permissionMode,
              projectId: options.projectId,
              updatedAt: now
            }
          : {
              ...createComposerDraft({
                modelId: options.modelId,
                permissionMode: options.permissionMode,
                projectId: options.projectId,
                queuedMessages: currentDraft.queuedMessages
              }),
              updatedAt: now
            }
      )
      void requestAssistantResponse(
        conversationId,
        userMessage.id,
        assistantMessage.id,
        message,
        options.modelId,
        targetConversation?.projectId ?? options.projectId,
        options.permissionMode,
        options.attachments,
        options.skills,
        targetConversation ? undefined : title
      )
      return true
    },
    [
      enqueueChatMessagesUpsert,
      enqueueConversationMetaSave,
      requestAssistantResponse,
      setConversationsWithRef,
      t,
      updateDraft
    ]
  )

  const showProviderTransitionBlocked = useCallback(
    (reason: AgentProviderTransitionReason) => {
      const key =
        reason === 'active_run'
          ? 'chat.modelTransition.blockedActiveRun'
          : reason === 'pending_approval'
            ? 'chat.modelTransition.blockedPendingApproval'
            : 'chat.modelTransition.blockedUnsupportedTarget'
      showToast(t(key))
    },
    [showToast, t]
  )

  const handleProviderTransitionCompleted = useCallback(
    (operation: Extract<AgentProviderTransitionOperation, { status: 'completed' }>) => {
      setConversationsWithRef((currentConversations) =>
        currentConversations.map((conversation) =>
          conversation.id === operation.conversationId
            ? {
                ...conversation,
                modelId: operation.modelId,
                updatedAt: Math.max(conversation.updatedAt, operation.conversationUpdatedAt)
              }
            : conversation
        )
      )

      const currentDraft = draftsRef.current[operation.conversationId]
      if (currentDraft) {
        updateDraft(operation.conversationId, {
          ...currentDraft,
          modelId: operation.modelId,
          updatedAt: Math.max(operation.conversationUpdatedAt, currentDraft.updatedAt + 1)
        })
      }

      const pendingSubmission = pendingProviderTransitionSubmissionsRef.current.get(
        operation.conversationId
      )
      if (!pendingSubmission) return
      const latestConversation = conversationsRef.current.find(
        (conversation) => conversation.id === operation.conversationId
      )
      if (latestConversation?.archivedAt || latestConversation?.pendingArchivedAt !== undefined) {
        // A completed transition may update the archived task's model snapshot, but must not turn
        // a queued/composer intent into a new hidden Turn. The draft/queue remains user-visible
        // when the task is restored.
        pendingProviderTransitionSubmissionsRef.current.delete(operation.conversationId)
        return
      }
      pendingProviderTransitionSubmissionsRef.current.delete(operation.conversationId)

      if (pendingSubmission.kind === 'queued_message') {
        queueMicrotask(() => {
          const nextQueuedMessage = draftsRef.current[operation.conversationId]?.queuedMessages[0]
          if (nextQueuedMessage?.id !== pendingSubmission.queueMessageId) return
          autoSubmitQueuedMessageRef.current(operation.conversationId)
        })
        return
      }
      if (pendingSubmission.options.modelId !== operation.targetModelId) return
      submitMessageToConversation(
        operation.conversationId,
        pendingSubmission.message,
        { ...pendingSubmission.options, modelId: operation.modelId },
        {
          activate: activeConversationIdRef.current === operation.conversationId,
          preserveComposerContent: false
        }
      )
    },
    [setConversationsWithRef, submitMessageToConversation, updateDraft]
  )

  const handleProviderTransitionFailed = useCallback(
    (operation: Extract<AgentProviderTransitionOperation, { status: 'failed' }>) => {
      const pendingSubmission = pendingProviderTransitionSubmissionsRef.current.get(
        operation.conversationId
      )
      // A composer submission is an intent tied to the failed attempt. Retrying the transition
      // switches models only; it must never replay stale text over a newer user-edited draft.
      if (pendingSubmission?.kind === 'composer') {
        pendingProviderTransitionSubmissionsRef.current.delete(operation.conversationId)
      }
    },
    []
  )

  const {
    cancelConfirmation: cancelProviderTransitionConfirmation,
    confirm: confirmProviderTransition,
    loadStatus: loadProviderTransitionStatus,
    request: requestProviderTransition,
    retry: retryProviderTransition,
    store: providerTransitionStore
  } = useProviderTransition({
    onBlocked: showProviderTransitionBlocked,
    onOperationCompleted: handleProviderTransitionCompleted,
    onOperationFailed: handleProviderTransitionFailed,
    onRequestError: () => showToast(t('chat.modelTransition.requestFailed'))
  })

  const submitMessage = useCallback(
    async (message: string, options: ChatSubmitOptions): Promise<boolean> => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) {
        submitMessageToConversation(null, message, options, {
          activate: true,
          preserveComposerContent: false
        })
        return true
      }
      const activeConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      if (
        activeConversation?.archivedAt ||
        activeConversation?.pendingArchivedAt !== undefined ||
        activeConversation?.messages.some(
          (message) => message.agentRun?.status === 'waiting_for_user_input'
        )
      ) {
        return false
      }

      await waitForConversationSaves(conversationId)
      const outcome = await requestProviderTransition(conversationId, options.modelId)
      if (outcome.status === 'completed') {
        return submitMessageToConversation(
          conversationId,
          message,
          { ...options, modelId: outcome.operation.modelId },
          {
            activate: true,
            preserveComposerContent: false
          }
        )
      }
      if (outcome.status === 'confirmation_required' || outcome.status === 'running') {
        pendingProviderTransitionSubmissionsRef.current.set(conversationId, {
          kind: 'composer',
          message,
          options
        })
      }
      return false
    },
    [requestProviderTransition, submitMessageToConversation, waitForConversationSaves]
  )

  const submitNextQueuedMessage = useCallback(
    async (conversationId: string) => {
      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (conversation?.archivedAt || conversation?.pendingArchivedAt !== undefined) return
      const latestAssistant = [...(conversation?.messages ?? [])]
        .reverse()
        .find((message) => message.role === 'assistant')
      if (
        !conversation ||
        !latestAssistant ||
        !['completed', 'cancelled'].includes(latestAssistant.agentRun?.status ?? '')
      ) {
        return
      }

      const draft = draftsRef.current[conversationId]
      const queuedMessage = draft?.queuedMessages[0]
      if (!draft || !queuedMessage || queuedMessage.status === 'submitting') return

      await waitForConversationSaves(conversationId)
      const conversationAfterSave = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (
        conversationAfterSave?.archivedAt ||
        conversationAfterSave?.pendingArchivedAt !== undefined
      ) {
        return
      }
      const transitionOutcome = await requestProviderTransition(
        conversationId,
        queuedMessage.modelId
      )
      if (
        transitionOutcome.status === 'confirmation_required' ||
        transitionOutcome.status === 'running'
      ) {
        pendingProviderTransitionSubmissionsRef.current.set(conversationId, {
          kind: 'queued_message',
          queueMessageId: queuedMessage.id
        })
        return
      }
      if (transitionOutcome.status !== 'completed') return

      const conversationAfterTransition = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (
        conversationAfterTransition?.archivedAt ||
        conversationAfterTransition?.pendingArchivedAt !== undefined
      ) {
        return
      }

      const currentQueuedMessage = draftsRef.current[conversationId]?.queuedMessages[0]
      if (!currentQueuedMessage || currentQueuedMessage.id !== queuedMessage.id) return

      mutateDraft(conversationId, (currentDraft) => ({
        ...currentDraft,
        queuedMessages: currentDraft.queuedMessages.filter(
          (message) => message.id !== queuedMessage.id
        )
      }))
      submitMessageToConversation(
        conversationId,
        queuedMessage.content,
        {
          attachments: queuedMessage.attachments,
          modelId: transitionOutcome.operation.modelId,
          permissionMode: queuedMessage.permissionMode,
          projectId: queuedMessage.projectId,
          skills: queuedMessage.skills
        },
        {
          activate: activeConversationIdRef.current === conversationId,
          preserveComposerContent: true
        }
      )
    },
    [mutateDraft, requestProviderTransition, submitMessageToConversation, waitForConversationSaves]
  )
  useEffect(() => {
    autoSubmitQueuedMessageRef.current = submitNextQueuedMessage
  }, [submitNextQueuedMessage])

  const submitEditedLastUserMessage = useCallback(
    async (messageId: string, content: string) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) {
        throw new Error(t('chat.editNoConversation'))
      }

      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (!conversation) {
        throw new Error(t('chat.editConversationMissing'))
      }

      const editableTurn = getEditableLastTurn(conversation)
      if (!editableTurn || editableTurn.userMessage.id !== messageId) {
        throw new Error(t('chat.editMessageUnavailable'))
      }

      const attachmentIds =
        editableTurn.userMessage.attachments?.map((attachment) => attachment.id) ?? []
      const attachments = await loadInputAttachments(attachmentIds)

      await waitForConversationSaves(conversationId)
      await waitForMessageUpserts(conversationId)

      const latestConversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      if (!latestConversation) {
        throw new Error(t('chat.editConversationMissing'))
      }

      const latestEditableTurn = getEditableLastTurn(latestConversation)
      if (!latestEditableTurn || latestEditableTurn.userMessage.id !== messageId) {
        throw new Error(t('chat.editConversationChanged'))
      }

      const messageContent = buildMessageContentWithAttachments(content, attachments)
      if (!messageContent.trim()) {
        throw new Error(t('chat.emptyMessage'))
      }
      // Editing is one atomic logical replacement inside the current Conversation. Do not run a
      // Provider transition first: an incompatible transition may compact through the very Turn
      // that is about to be replaced, after which the rewrite can no longer be truthful. A model
      // change remains available as an ordinary subsequent Turn.
      const rewriteModel = latestConversation.modelId
        ? (enabledModels.find((model) => model.id === latestConversation.modelId) ?? null)
        : activeDraftSelectedModel
      if (!rewriteModel) {
        throw new Error(t('chat.noEnabledModels'))
      }
      if (
        attachments.some((attachment) => attachment.kind === 'image') &&
        !rewriteModel.supportsImage
      ) {
        throw new Error(t('chat.unsupportedImageWarning'))
      }
      const modelId = rewriteModel.id
      const permissionMode = activeDraft.permissionMode
      const sourceAutoTitle = createConversationTitle(
        latestEditableTurn.userMessage.content,
        t('chat.newConversation')
      )
      const replacementTitle =
        latestConversation.title === sourceAutoTitle
          ? createConversationTitle(messageContent, t('chat.newConversation'))
          : undefined
      const editedSkillSelections =
        latestEditableTurn.assistantMessage.agentRun?.explicitSkillSelections ??
        latestEditableTurn.assistantMessage.agentRun?.activatedSkills?.map((skill) => ({
          id: skill.id,
          revision: skill.revision
        })) ??
        []
      if (editRewriteInFlightRef.current.has(conversationId)) {
        throw new Error(t('chat.editMessageUnavailable'))
      }
      const submissionSeq = (editSubmissionSeqRef.current += 1)
      const rewriteIdentity = JSON.stringify({
        attachmentIds,
        content,
        conversationId,
        modelId,
        permissionMode,
        skills: editedSkillSelections,
        sourceAssistantMessageId: latestEditableTurn.assistantMessage.id,
        sourceUserMessageId: latestEditableTurn.userMessage.id,
        title: replacementTitle
      })
      let rewriteAttempt = editRewriteAttemptsRef.current.get(rewriteIdentity)
      if (!rewriteAttempt) {
        const frozenAttachments = attachments.map((attachment) => ({ ...attachment }))
        const frozenSkills = editedSkillSelections.map((selection) => ({ ...selection }))
        rewriteAttempt = {
          assistantMessage: createAssistantMessage('', 'pending'),
          attachments: frozenAttachments,
          content,
          modelId,
          permissionMode,
          projectId: latestConversation.projectId,
          requestId: createId('conversation-turn-rewrite-request'),
          skills: frozenSkills,
          title: replacementTitle,
          userMessage: createUserMessage(
            buildMessageContentWithAttachments(content, frozenAttachments),
            frozenAttachments
          )
        }
        editRewriteAttemptsRef.current.set(rewriteIdentity, rewriteAttempt)
        if (editRewriteAttemptsRef.current.size > 32) {
          const oldestIdentity = editRewriteAttemptsRef.current.keys().next().value
          if (oldestIdentity) editRewriteAttemptsRef.current.delete(oldestIdentity)
        }
      }
      const { assistantMessage, userMessage } = rewriteAttempt
      editRewriteInFlightRef.current.add(conversationId)
      try {
        const committed = await requestAssistantResponse(
          conversationId,
          userMessage.id,
          assistantMessage.id,
          rewriteAttempt.content,
          rewriteAttempt.modelId,
          rewriteAttempt.projectId,
          rewriteAttempt.permissionMode,
          rewriteAttempt.attachments,
          rewriteAttempt.skills,
          rewriteAttempt.title,
          {
            requestId: rewriteAttempt.requestId,
            sourceAssistantMessageId: latestEditableTurn.assistantMessage.id,
            sourceUserMessageId: latestEditableTurn.userMessage.id
          }
        )
        if (!committed) return
        editRewriteAttemptsRef.current.delete(rewriteIdentity)
        if (editSubmissionSeqRef.current !== submissionSeq) return

        const rewrittenConversation = conversationsRef.current.find(
          (candidate) => candidate.id === conversationId
        )
        if (
          !rewrittenConversation ||
          rewrittenConversation.archivedAt ||
          rewrittenConversation.pendingArchivedAt !== undefined ||
          !rewrittenConversation.messages.some((message) => message.id === userMessage.id) ||
          !rewrittenConversation.messages.some((message) => message.id === assistantMessage.id)
        ) {
          return
        }
        updateDraft(
          conversationId,
          createComposerDraft({
            modelId: rewriteAttempt.modelId,
            permissionMode: rewriteAttempt.permissionMode,
            projectId: rewriteAttempt.projectId
          })
        )
        if (activeConversationIdRef.current === conversationId) {
          setScrollTargetMessageId(null)
          setActiveConversationInitialScrollTop(null)
          setConversationScrollToBottomSignal((signal) => signal + 1)
        }
      } catch (error) {
        if (editSubmissionSeqRef.current === submissionSeq) {
          restoreSubmittedSkills(conversationId, rewriteAttempt.skills, {
            modelId: rewriteAttempt.modelId,
            permissionMode: rewriteAttempt.permissionMode,
            projectId: rewriteAttempt.projectId
          })
        }
        throw error
      } finally {
        editRewriteInFlightRef.current.delete(conversationId)
      }
    },
    [
      activeDraft.permissionMode,
      activeDraftSelectedModel,
      enabledModels,
      requestAssistantResponse,
      restoreSubmittedSkills,
      t,
      updateDraft,
      waitForConversationSaves,
      waitForMessageUpserts
    ]
  )

  return {
    cancelProviderTransitionConfirmation,
    confirmProviderTransition,
    loadProviderTransitionStatus,
    providerTransitionStore,
    retryProviderTransition,
    submitEditedLastUserMessage,
    submitMessage
  }
}
