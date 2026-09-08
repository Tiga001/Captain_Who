/* eslint-disable react-hooks/exhaustive-deps -- extracted callbacks keep AppShell's original dependency arrays; omitted inputs are stable refs and React dispatchers. */
import {
  useCallback,
  useEffect,
  useRef,
  useState,
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
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import { loadInputAttachments } from '../features/storage/storageClient'
import { useProviderTransition } from '../features/agentRun/useProviderTransition'
import { isAssistantReplySettled } from '../features/chat/assistantGeneration'
import type { AutoSubmitQueuedMessage } from './appTypes'
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
  | {
      kind: 'composer'
      message: string
      options: ChatSubmitOptions
      draftSnapshot?: ChatComposerDraft
    }
  | { kind: 'queued_message' }

type RequestAssistantResponse = ReturnType<
  (typeof import('./useAgentRunLifecycle'))['useAgentRunLifecycle']
>['requestAssistantResponse']

function clearSubmittedComposerDraft(
  currentDraft: ChatComposerDraft,
  submittedDraft: ChatComposerDraft,
  options: Pick<ChatSubmitOptions, 'modelId' | 'permissionMode' | 'projectId'>,
  now: number
): ChatComposerDraft {
  return {
    ...currentDraft,
    message: currentDraft.message === submittedDraft.message ? '' : currentDraft.message,
    attachments:
      currentDraft.attachments === submittedDraft.attachments ? [] : currentDraft.attachments,
    skills: currentDraft.skills === submittedDraft.skills ? [] : currentDraft.skills,
    modelId:
      currentDraft.modelId === submittedDraft.modelId ? options.modelId : currentDraft.modelId,
    permissionMode:
      currentDraft.permissionMode === submittedDraft.permissionMode
        ? options.permissionMode
        : currentDraft.permissionMode,
    projectId:
      currentDraft.projectId === submittedDraft.projectId
        ? options.projectId
        : currentDraft.projectId,
    updatedAt: Math.max(now, currentDraft.updatedAt + 1)
  }
}

interface UseAppShellMessageSubmissionOptions {
  activeConversationIdRef: MutableRefObject<string | null>
  activeDraft: ChatComposerDraft
  activeDraftSelectedModel: ModelConfig | null
  autoSubmitQueuedMessageRef: MutableRefObject<AutoSubmitQueuedMessage>
  conversations: ChatConversation[]
  conversationsRef: MutableRefObject<ChatConversation[]>
  drafts: Record<string, ChatComposerDraft>
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
  waitForMessageStateSaves: (conversationId: string) => Promise<void>
  waitForRunSettlement: (conversationId: string) => Promise<void>
}

export function useAppShellMessageSubmission({
  activeConversationIdRef,
  activeDraft,
  activeDraftSelectedModel,
  autoSubmitQueuedMessageRef,
  conversations,
  conversationsRef,
  drafts,
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
  waitForMessageUpserts,
  waitForMessageStateSaves,
  waitForRunSettlement
}: UseAppShellMessageSubmissionOptions) {
  // Queue execution is opt-in for each conversation during this app session.
  // A fresh token on every enable invalidates work that was awaiting a previous enable.
  const queueAutoSendTokensRef = useRef(new Map<string, symbol>())
  const queueSubmissionAttemptsRef = useRef(new Map<string, { retryRequested: boolean }>())
  const queueProviderWaitsRef = useRef(new Set<string>())
  const queueMountedRef = useRef(false)
  const queueWakeSnapshotsRef = useRef(
    new Map<string, { head: ChatQueuedMessage | undefined; failure: string | undefined }>()
  )
  const [queueAutoSendConversationIds, setQueueAutoSendConversationIds] = useState(
    () => new Set<string>()
  )

  const pauseQueueAutoSend = useCallback((conversationId: string) => {
    queueWakeSnapshotsRef.current.delete(conversationId)
    queueProviderWaitsRef.current.delete(conversationId)
    if (!queueAutoSendTokensRef.current.delete(conversationId)) return
    setQueueAutoSendConversationIds(new Set(queueAutoSendTokensRef.current.keys()))
    if (
      pendingProviderTransitionSubmissionsRef.current.get(conversationId)?.kind === 'queued_message'
    ) {
      pendingProviderTransitionSubmissionsRef.current.delete(conversationId)
    }
  }, [])

  const submitMessageToConversation = useCallback(
    (
      targetConversationId: string | null,
      message: string,
      options: ChatSubmitOptions,
      behavior: {
        activate: boolean
        preserveComposerContent: boolean
        draftSnapshot?: ChatComposerDraft
      }
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
      if (behavior.activate) {
        activeConversationIdRef.current = conversationId
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)
        setConversationScrollToBottomSignal((signal) => signal + 1)
        setActiveConversationId(conversationId)
      }
      const currentDraft = draftsRef.current[conversationId] ?? createComposerDraft()
      // A queued item owns its frozen settings, not the composer's next-turn choices.
      // For composer sends, asynchronous Provider work must not clear newer input or
      // overwrite choices made after the user submitted this particular turn.
      if (!behavior.preserveComposerContent) {
        updateDraft(
          conversationId,
          clearSubmittedComposerDraft(
            currentDraft,
            behavior.draftSnapshot ?? currentDraft,
            options,
            now
          )
        )
      }
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
      ).then((committed) => {
        if (committed) return
        pauseQueueAutoSend(conversationId)

        // Host owns accepted turns. Persist only a failed local start so its input,
        // attachments and error remain recoverable; never queue the optimistic pair
        // alongside startConversationTurn, where it could arrive after Host acceptance.
        const currentConversation = conversationsRef.current.find(
          (conversation) => conversation.id === conversationId
        )
        const userIndex = currentConversation?.messages.findIndex(
          (message) => message.id === userMessage.id
        )
        if (!currentConversation || userIndex === undefined || userIndex < 0) return
        const failedAssistant = currentConversation.messages[userIndex + 1]
        if (
          failedAssistant?.id !== assistantMessage.id ||
          failedAssistant.agentRun?.runId ||
          !['failed', 'cancelled'].includes(failedAssistant.agentRun?.status ?? '')
        ) {
          return
        }
        enqueueChatMessagesUpsert(
          conversationId,
          [currentConversation.messages[userIndex], failedAssistant],
          userIndex
        )
      })
      return true
    },
    [
      enqueueChatMessagesUpsert,
      enqueueConversationMetaSave,
      requestAssistantResponse,
      pauseQueueAutoSend,
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
      queueProviderWaitsRef.current.delete(operation.conversationId)
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

      const pendingSubmission = pendingProviderTransitionSubmissionsRef.current.get(
        operation.conversationId
      )
      const currentDraft = draftsRef.current[operation.conversationId]
      if (
        currentDraft &&
        pendingSubmission?.kind !== 'queued_message' &&
        currentDraft.modelId === operation.targetModelId
      ) {
        updateDraft(operation.conversationId, {
          ...currentDraft,
          modelId: operation.modelId,
          updatedAt: Math.max(operation.conversationUpdatedAt, currentDraft.updatedAt + 1)
        })
      }

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
          preserveComposerContent: false,
          draftSnapshot: pendingSubmission.draftSnapshot
        }
      )
    },
    [setConversationsWithRef, submitMessageToConversation, updateDraft]
  )

  const handleProviderTransitionFailed = useCallback(
    (operation: Extract<AgentProviderTransitionOperation, { status: 'failed' }>) => {
      queueProviderWaitsRef.current.delete(operation.conversationId)
      const pendingSubmission = pendingProviderTransitionSubmissionsRef.current.get(
        operation.conversationId
      )
      // A composer submission is an intent tied to the failed attempt. Retrying the transition
      // switches models only; it must never replay stale text over a newer user-edited draft.
      if (pendingSubmission?.kind === 'composer') {
        pendingProviderTransitionSubmissionsRef.current.delete(operation.conversationId)
      }
      if (pendingSubmission?.kind === 'queued_message') {
        pauseQueueAutoSend(operation.conversationId)
      }
    },
    [pauseQueueAutoSend]
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
      const draftSnapshot = conversationId ? draftsRef.current[conversationId] : undefined
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
            preserveComposerContent: false,
            draftSnapshot
          }
        )
      }
      if (outcome.status === 'confirmation_required' || outcome.status === 'running') {
        pendingProviderTransitionSubmissionsRef.current.set(conversationId, {
          kind: 'composer',
          message,
          options,
          draftSnapshot
        })
      }
      return false
    },
    [requestProviderTransition, submitMessageToConversation, waitForConversationSaves]
  )

  const readQueueWakeSnapshot = useCallback((conversationId: string) => {
    const conversation = conversationsRef.current.find((item) => item.id === conversationId)
    const latestAssistant = conversation?.messages.findLast(
      (message) => message.role === 'assistant'
    )
    const status = latestAssistant?.agentRun?.status
    const failure =
      status === 'failed' || status === 'cancelled' ? `${latestAssistant!.id}:${status}` : undefined
    const head = draftsRef.current[conversationId]?.queuedMessages[0]
    const ready =
      conversation &&
      conversation.messagesLoaded !== false &&
      !conversation.archivedAt &&
      conversation.pendingArchivedAt === undefined &&
      !queueProviderWaitsRef.current.has(conversationId) &&
      !editRewriteInFlightRef.current.has(conversationId) &&
      pendingProviderTransitionSubmissionsRef.current.get(conversationId)?.kind !== 'composer' &&
      conversation.messages.every(
        (message) => message.role !== 'assistant' || isAssistantReplySettled(message)
      ) &&
      head?.status !== 'submitting'
    return { head: ready ? head : undefined, failure }
  }, [])

  const submitNextQueuedMessage = useCallback(
    async (conversationId: string) => {
      const token = queueAutoSendTokensRef.current.get(conversationId)
      if (!token) return
      const existingAttempt = queueSubmissionAttemptsRef.current.get(conversationId)
      if (existingAttempt) {
        existingAttempt.retryRequested = true
        return
      }

      const getEligibleHead = () => {
        if (
          !queueMountedRef.current ||
          queueAutoSendTokensRef.current.get(conversationId) !== token
        )
          return
        return readQueueWakeSnapshot(conversationId).head
      }
      let queuedMessage = getEligibleHead()
      if (!queuedMessage) return

      const attempt = { retryRequested: false }
      queueSubmissionAttemptsRef.current.set(conversationId, attempt)
      try {
        await waitForRunSettlement(conversationId)
        await waitForMessageStateSaves(conversationId)
        await waitForConversationSaves(conversationId)
        // Ordering and payload may have changed while persistence was finishing.
        queuedMessage = getEligibleHead()
        if (!queuedMessage) return
        // Keep a queue-level continuation while a model switch awaits confirmation or completes;
        // the queue may be reordered or edited before that operation settles.
        pendingProviderTransitionSubmissionsRef.current.set(conversationId, {
          kind: 'queued_message'
        })
        const transitionOutcome = await requestProviderTransition(
          conversationId,
          queuedMessage.modelId,
          {
            shouldContinue: () => getEligibleHead() === queuedMessage,
            reportBlocked: false,
            allowUnchangedModel: true
          }
        )
        if (
          transitionOutcome.status === 'confirmation_required' ||
          transitionOutcome.status === 'running'
        ) {
          const stillWaiting =
            queueAutoSendTokensRef.current.get(conversationId) === token &&
            pendingProviderTransitionSubmissionsRef.current.get(conversationId)?.kind ===
              'queued_message'
          if (stillWaiting) {
            queueProviderWaitsRef.current.add(conversationId)
          }
          // A completion notification may precede the invocation's running reply. Its callback
          // already removed the continuation; do not recreate a wait with no future completion.
          attempt.retryRequested = !stillWaiting
          return
        }
        const currentHead = getEligibleHead()
        if (!currentHead) return
        if (currentHead !== queuedMessage) {
          attempt.retryRequested = true
          return
        }
        if (transitionOutcome.status !== 'completed' && transitionOutcome.status !== 'ready') {
          // Busy/approval responses wait for a new Host completion or queue/readiness change.
          // A timer cannot establish that the previous Turn has released its ownership.
          if (
            transitionOutcome.status === 'failed' ||
            (transitionOutcome.status === 'blocked' &&
              transitionOutcome.reason === 'unsupported_target')
          ) {
            pauseQueueAutoSend(conversationId)
            if (transitionOutcome.status === 'blocked') {
              showProviderTransitionBlocked(transitionOutcome.reason)
            }
          }
          return
        }
        pendingProviderTransitionSubmissionsRef.current.delete(conversationId)

        const submitted = submitMessageToConversation(
          conversationId,
          currentHead.content,
          {
            attachments: currentHead.attachments,
            modelId:
              transitionOutcome.status === 'ready'
                ? transitionOutcome.modelId
                : transitionOutcome.operation.modelId,
            permissionMode: currentHead.permissionMode,
            projectId: currentHead.projectId,
            skills: currentHead.skills
          },
          {
            activate: activeConversationIdRef.current === conversationId,
            preserveComposerContent: true
          }
        )
        if (submitted) {
          mutateDraft(conversationId, (currentDraft) => ({
            ...currentDraft,
            queuedMessages: currentDraft.queuedMessages.filter(
              (message) => message.id !== currentHead.id
            )
          }))
        }
      } catch {
        if (queueAutoSendTokensRef.current.get(conversationId) === token) {
          pauseQueueAutoSend(conversationId)
          showToast(t('chat.commands.failed'))
        }
      } finally {
        queueSubmissionAttemptsRef.current.delete(conversationId)
        if (
          queueMountedRef.current &&
          attempt.retryRequested &&
          queueAutoSendTokensRef.current.has(conversationId)
        ) {
          queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
        }
      }
    },
    [
      mutateDraft,
      pauseQueueAutoSend,
      readQueueWakeSnapshot,
      requestProviderTransition,
      showProviderTransitionBlocked,
      showToast,
      submitMessageToConversation,
      t,
      waitForConversationSaves,
      waitForMessageStateSaves,
      waitForRunSettlement
    ]
  )
  useEffect(() => {
    autoSubmitQueuedMessageRef.current = (conversationId, action) => {
      if (action === 'pause') pauseQueueAutoSend(conversationId)
      else void submitNextQueuedMessage(conversationId)
    }
  }, [pauseQueueAutoSend, submitNextQueuedMessage])

  useEffect(() => {
    queueMountedRef.current = true
    // Fast Refresh replays effects while retaining the visible switch state. Resume enabled
    // queues through the dispatcher rather than clearing only their execution tokens.
    for (const conversationId of queueAutoSendTokensRef.current.keys()) {
      queueMicrotask(() => autoSubmitQueuedMessageRef.current(conversationId))
    }
    return () => {
      queueMountedRef.current = false
      for (const conversationId of queueAutoSendTokensRef.current.keys()) {
        queueAutoSendTokensRef.current.set(conversationId, Symbol())
      }
      autoSubmitQueuedMessageRef.current = () => undefined
    }
  }, [])

  useEffect(() => {
    for (const conversationId of queueAutoSendConversationIds) {
      const previous = queueWakeSnapshotsRef.current.get(conversationId)
      const current = readQueueWakeSnapshot(conversationId)
      queueWakeSnapshotsRef.current.set(conversationId, current)
      if (previous && current.failure && previous.failure !== current.failure) {
        pauseQueueAutoSend(conversationId)
      } else if (current.head && current.head !== previous?.head) {
        // A queue item can arrive after Done (for example after attachment preparation), and
        // an authoritative reload can settle a Run without replaying Done. Observe both inputs.
        autoSubmitQueuedMessageRef.current(conversationId)
      }
    }
  }, [
    conversations,
    drafts,
    queueAutoSendConversationIds,
    pauseQueueAutoSend,
    readQueueWakeSnapshot
  ])

  const toggleQueueAutoSend = useCallback(
    (conversationId: string) => {
      if (queueAutoSendTokensRef.current.has(conversationId)) {
        pauseQueueAutoSend(conversationId)
      } else {
        queueAutoSendTokensRef.current.set(conversationId, Symbol())
        queueWakeSnapshotsRef.current.set(conversationId, readQueueWakeSnapshot(conversationId))
        setQueueAutoSendConversationIds(new Set(queueAutoSendTokensRef.current.keys()))
        void submitNextQueuedMessage(conversationId)
      }
    },
    [pauseQueueAutoSend, readQueueWakeSnapshot, submitNextQueuedMessage]
  )

  const submitEditedLastUserMessage = useCallback(
    async (messageId: string, content: string) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) {
        throw new Error(t('chat.editNoConversation'))
      }
      const draftSnapshot = draftsRef.current[conversationId] ?? activeDraft

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
        const currentDraft = draftsRef.current[conversationId] ?? draftSnapshot
        updateDraft(
          conversationId,
          clearSubmittedComposerDraft(currentDraft, draftSnapshot, rewriteAttempt, Date.now())
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
    queueAutoSendConversationIds,
    retryProviderTransition,
    submitEditedLastUserMessage,
    submitMessage,
    toggleQueueAutoSend
  }
}
