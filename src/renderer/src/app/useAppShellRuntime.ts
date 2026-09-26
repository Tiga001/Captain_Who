/* eslint-disable react-hooks/exhaustive-deps -- extracted hooks keep AppShell's original dependency arrays; omitted inputs are stable refs and React dispatchers. */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction
} from 'react'
import type { AgentEvent, SkillSelection } from '@mycopilot/protocol'
import type { Translate } from '../config/translationFormat'
import { getUserFacingErrorMessage } from '../errors/userFacingError'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatQueuedMessage
} from '../features/chat/chatTypes'
import {
  loadConversation,
  loadConversationMetas,
  loadInputAttachments,
  saveUiPreferences,
  type UiPreferencesSnapshot
} from '../features/storage/storageClient'
import type { SkillActivationRecoveryPlan } from '../features/skills/skillActivationRecovery'
import { reconcileSkillActivationSelections } from '../features/skills/skillActivationRecovery'
import { mergeSkillSelections } from '../features/skills/skillSelection'
import { hostClient } from '../host/hostClient'
import type { ActiveRunBinding, AutoSubmitQueuedMessage } from './appTypes'
import type { PendingMessageDelta } from './AppShellSupport'
import { createComposerDraft, synchronizeComposerDraftForScope } from './chatMessageFactory'
import { getRecoverableGuidanceSignature, isRecoverableGuidanceItem } from './recoverableGuidance'
import { useAgentRunLifecycle } from './useAgentRunLifecycle'
import { usePersistedShellHydration } from './usePersistedShellHydration'
import { useHumanInteractionConversationSync } from './useHumanInteractionConversationSync'

type StartupStage = ReturnType<
  (typeof import('../features/startup/AppStartupContext'))['useAppStartupStage']
>
type ComposerDraftPersistence = ReturnType<
  (typeof import('./useComposerDraftPersistence'))['useComposerDraftPersistence']
>
type ConversationPersistence = ReturnType<
  (typeof import('./useConversationPersistence'))['useConversationPersistence']
>
type ContextWindowSnapshots = ReturnType<
  (typeof import('../features/agentRun/useContextWindowSnapshots'))['useContextWindowSnapshots']
>

interface UseAppShellRuntimeOptions {
  activeConversationId: string | null
  activeConversationIdRef: MutableRefObject<string | null>
  visibleConversationId?: string | null
  activeDraftId: string
  activeRunBindingsRef: MutableRefObject<Map<string, ActiveRunBinding>>
  automationConversationMetaRefreshEpochRef: MutableRefObject<number>
  autoSubmitQueuedMessageRef: MutableRefObject<AutoSubmitQueuedMessage>
  bufferedAgentEventsRef: MutableRefObject<Map<string, AgentEvent[]>>
  cancelledPendingMessageIdsRef: MutableRefObject<Set<string>>
  cancelledRunIdsRef: MutableRefObject<Set<string>>
  composerDraftsStartupAttempt: StartupStage['attempt']
  conversationDetailEpochRef: MutableRefObject<Map<string, number>>
  conversationDetailRequestsRef: MutableRefObject<Map<string, Promise<ChatConversation | null>>>
  conversationMetasStartupAttempt: StartupStage['attempt']
  conversations: ChatConversation[]
  conversationsRef: MutableRefObject<ChatConversation[]>
  contextWindowIndicatorEnabled: boolean
  draftsRef: MutableRefObject<Record<string, ChatComposerDraft>>
  enqueueChatMessageCheckpoint: ConversationPersistence['enqueueChatMessageCheckpoint']
  enqueueChatMessageStateSave: ConversationPersistence['enqueueChatMessageStateSave']
  enqueueConversationMetaSave: ConversationPersistence['enqueueConversationMetaSave']
  flushChatMessageStateSave: ConversationPersistence['flushChatMessageStateSave']
  flushConversationMessageStateSaves: ConversationPersistence['flushConversationMessageStateSaves']
  flushDraft: ComposerDraftPersistence['flushDraft']
  locallyUnconfirmedStoppedRunIdsRef: MutableRefObject<Set<string>>
  markComposerDraftsStartupFailed: StartupStage['markFailed']
  markComposerDraftsStartupPending: StartupStage['markPending']
  markComposerDraftsStartupReady: StartupStage['markReady']
  markConversationMetasStartupFailed: StartupStage['markFailed']
  markConversationMetasStartupPending: StartupStage['markPending']
  markConversationMetasStartupReady: StartupStage['markReady']
  markUiPreferencesStartupFailed: StartupStage['markFailed']
  markUiPreferencesStartupPending: StartupStage['markPending']
  markUiPreferencesStartupReady: StartupStage['markReady']
  pendingActionsHydratedRef: MutableRefObject<Set<string>>
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
  pendingMessageDeltasRef: MutableRefObject<Map<string, PendingMessageDelta>>
  persistDraftNow: ComposerDraftPersistence['persistDraftNow']
  previousActiveDraftIdRef: MutableRefObject<string>
  recordContextWindowSnapshot: ContextWindowSnapshots['recordSnapshot']
  recoveredGuidanceKeysRef: MutableRefObject<Set<string>>
  retiredAgentRunIdsRef: MutableRefObject<Set<string>>
  scheduleMessageSave: ComposerDraftPersistence['scheduleMessageSave']
  sealAndFlushChatMessageStateSaves: ConversationPersistence['sealAndFlushChatMessageStateSaves']
  setActiveConversationId: Dispatch<SetStateAction<string | null>>
  setConversationLoadErrors: Dispatch<SetStateAction<Record<string, string>>>
  setConversations: Dispatch<SetStateAction<ChatConversation[]>>
  setDrafts: Dispatch<SetStateAction<Record<string, ChatComposerDraft>>>
  setSkillCatalogRefreshTokens: Dispatch<SetStateAction<Record<string, number>>>
  setUiPreferences: Dispatch<SetStateAction<UiPreferencesSnapshot>>
  showToast: (message: string) => void
  stopReconciliationTimersRef: MutableRefObject<Map<string, number>>
  stopRequestedPendingMessageIdsRef: MutableRefObject<Set<string>>
  stopRequestedRunIdsRef: MutableRefObject<Set<string>>
  t: Translate
  uiPreferences: UiPreferencesSnapshot
  uiPreferencesStartupAttempt: StartupStage['attempt']
}

export function useAppShellRuntime({
  activeConversationId,
  activeConversationIdRef,
  visibleConversationId,
  activeDraftId,
  activeRunBindingsRef,
  automationConversationMetaRefreshEpochRef,
  autoSubmitQueuedMessageRef,
  bufferedAgentEventsRef,
  cancelledPendingMessageIdsRef,
  cancelledRunIdsRef,
  composerDraftsStartupAttempt,
  conversationDetailEpochRef,
  conversationDetailRequestsRef,
  conversationMetasStartupAttempt,
  conversations,
  conversationsRef,
  contextWindowIndicatorEnabled,
  draftsRef,
  enqueueChatMessageCheckpoint,
  enqueueChatMessageStateSave,
  enqueueConversationMetaSave,
  flushChatMessageStateSave,
  flushConversationMessageStateSaves,
  flushDraft,
  locallyUnconfirmedStoppedRunIdsRef,
  markComposerDraftsStartupFailed,
  markComposerDraftsStartupPending,
  markComposerDraftsStartupReady,
  markConversationMetasStartupFailed,
  markConversationMetasStartupPending,
  markConversationMetasStartupReady,
  markUiPreferencesStartupFailed,
  markUiPreferencesStartupPending,
  markUiPreferencesStartupReady,
  pendingActionsHydratedRef,
  pendingGuidancePayloadsRef,
  pendingMessageDeltasRef,
  persistDraftNow,
  previousActiveDraftIdRef,
  recordContextWindowSnapshot,
  recoveredGuidanceKeysRef,
  retiredAgentRunIdsRef,
  scheduleMessageSave,
  sealAndFlushChatMessageStateSaves,
  setActiveConversationId,
  setConversationLoadErrors,
  setConversations,
  setDrafts,
  setSkillCatalogRefreshTokens,
  setUiPreferences,
  showToast,
  stopReconciliationTimersRef,
  stopRequestedPendingMessageIdsRef,
  stopRequestedRunIdsRef,
  t,
  uiPreferences,
  uiPreferencesStartupAttempt
}: UseAppShellRuntimeOptions) {
  const visibleConversationIdRef = useRef(
    visibleConversationId === undefined ? activeConversationId : visibleConversationId
  )
  useLayoutEffect(() => {
    visibleConversationIdRef.current =
      visibleConversationId === undefined ? activeConversationId : visibleConversationId
  }, [activeConversationId, visibleConversationId])

  // Agent tool events arrive faster than React state commits. Keep the ref and state in one
  // update path so an older render snapshot cannot overwrite newer tool-call results.
  const setConversationsWithRef = useCallback((value: SetStateAction<ChatConversation[]>) => {
    const nextConversations =
      typeof value === 'function'
        ? (value as (currentConversations: ChatConversation[]) => ChatConversation[])(
            conversationsRef.current
          )
        : value

    conversationsRef.current = nextConversations
    setConversations(nextConversations)
  }, [])

  const refreshConversationMetasFromAutomation = useCallback(async () => {
    const requestEpoch = automationConversationMetaRefreshEpochRef.current + 1
    automationConversationMetaRefreshEpochRef.current = requestEpoch
    try {
      const storedConversations = await loadConversationMetas()
      if (automationConversationMetaRefreshEpochRef.current !== requestEpoch) return

      const currentById = new Map(
        conversationsRef.current.map((conversation) => [conversation.id, conversation])
      )
      const conversationsToReload = storedConversations.filter((storedConversation) => {
        const current = currentById.get(storedConversation.id)
        if (!current) return false
        return current.messagesLoaded !== false && storedConversation.updatedAt > current.updatedAt
      })
      const reloadedConversations = new Map<string, ChatConversation>()
      await Promise.all(
        conversationsToReload.map(async (conversation) => {
          try {
            const reloaded = await loadConversation(conversation.id)
            if (reloaded) reloadedConversations.set(conversation.id, reloaded)
          } catch (error) {
            console.error('Failed to refresh Automation conversation messages', error)
          }
        })
      )
      if (automationConversationMetaRefreshEpochRef.current !== requestEpoch) return

      // A newly admitted Automation turn can introduce both a new Agent Run binding and a new
      // durable approval. Let the lifecycle re-project pending actions for every authoritative
      // detail we replace below instead of treating an older hydration as final.
      for (const conversationId of reloadedConversations.keys()) {
        pendingActionsHydratedRef.current.delete(conversationId)
      }
      setConversationsWithRef((currentConversations) => {
        const latestCurrentById = new Map(
          currentConversations.map((conversation) => [conversation.id, conversation])
        )
        const storedIds = new Set(storedConversations.map((conversation) => conversation.id))
        return [
          ...storedConversations.map((conversation) => {
            const reloaded = reloadedConversations.get(conversation.id)
            if (reloaded) return { ...reloaded, messagesLoaded: true as const }
            const current = latestCurrentById.get(conversation.id)
            return current && current.messagesLoaded !== false ? current : conversation
          }),
          ...currentConversations.filter((conversation) => !storedIds.has(conversation.id))
        ]
      })
    } catch (error) {
      console.error('Failed to refresh conversation metadata after Automation event', error)
    }
  }, [setConversationsWithRef])

  useEffect(() => {
    const automations = hostClient.automations
    if (!automations?.onEvent || !automations.onResync) return
    const unsubscribeEvent = automations.onEvent((event) => {
      if (event.kind === 'run_updated') void refreshConversationMetasFromAutomation()
    })
    const unsubscribeResync = automations.onResync(() => {
      void refreshConversationMetasFromAutomation()
    })
    return () => {
      unsubscribeEvent()
      unsubscribeResync()
      automationConversationMetaRefreshEpochRef.current += 1
    }
  }, [refreshConversationMetasFromAutomation])

  const setDraftsWithRef = useCallback(
    (value: SetStateAction<Record<string, ChatComposerDraft>>) => {
      const nextDrafts =
        typeof value === 'function'
          ? (
              value as (
                currentDrafts: Record<string, ChatComposerDraft>
              ) => Record<string, ChatComposerDraft>
            )(draftsRef.current)
          : value

      draftsRef.current = nextDrafts
      setDrafts(nextDrafts)
    },
    []
  )

  usePersistedShellHydration({
    composerDraftsStage: {
      attempt: composerDraftsStartupAttempt,
      markFailed: markComposerDraftsStartupFailed,
      markPending: markComposerDraftsStartupPending,
      markReady: markComposerDraftsStartupReady
    },
    conversationMetasStage: {
      attempt: conversationMetasStartupAttempt,
      markFailed: markConversationMetasStartupFailed,
      markPending: markConversationMetasStartupPending,
      markReady: markConversationMetasStartupReady
    },
    setConversations: setConversationsWithRef,
    setDrafts: setDraftsWithRef,
    setUiPreferences,
    uiPreferencesStage: {
      attempt: uiPreferencesStartupAttempt,
      markFailed: markUiPreferencesStartupFailed,
      markPending: markUiPreferencesStartupPending,
      markReady: markUiPreferencesStartupReady
    }
  })

  useLayoutEffect(() => {
    setDrafts((renderedDrafts) =>
      synchronizeComposerDraftForScope(renderedDrafts, draftsRef.current, activeDraftId)
    )
  }, [activeDraftId])

  useEffect(() => {
    const previousScopeId = previousActiveDraftIdRef.current
    previousActiveDraftIdRef.current = activeDraftId
    if (previousScopeId !== activeDraftId) void flushDraft(previousScopeId)
  }, [activeDraftId, flushDraft])

  const hydrateConversation = useCallback(
    (conversationId: string): Promise<ChatConversation | null> => {
      const currentConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      if (currentConversation && currentConversation.messagesLoaded !== false) {
        return Promise.resolve(currentConversation)
      }

      const pendingRequest = conversationDetailRequestsRef.current.get(conversationId)
      if (pendingRequest) return pendingRequest

      const requestEpoch = (conversationDetailEpochRef.current.get(conversationId) ?? 0) + 1
      conversationDetailEpochRef.current.set(conversationId, requestEpoch)
      setConversationLoadErrors((current) => {
        if (!(conversationId in current)) return current
        const next = { ...current }
        delete next[conversationId]
        return next
      })

      const request = loadConversation(conversationId)
        .then((storedConversation) => {
          if (conversationDetailEpochRef.current.get(conversationId) !== requestEpoch) return null
          if (!storedConversation) {
            throw new Error('Conversation not found')
          }

          let hydratedConversation: ChatConversation | null = null
          setConversationsWithRef((currentConversations) => {
            let foundConversation = false
            const nextConversations = currentConversations.map((conversation) => {
              if (conversation.id !== conversationId) return conversation
              foundConversation = true
              // Metadata may have changed while SQLite loaded the message history. Keep the latest
              // sidebar fields and attach only the lazily fetched message payload.
              if (conversation.messagesLoaded !== false) {
                hydratedConversation = conversation
                return conversation
              }
              hydratedConversation = {
                ...storedConversation,
                projectId: conversation.projectId,
                modelId: conversation.modelId,
                title: conversation.title,
                createdAt: conversation.createdAt,
                updatedAt: conversation.updatedAt,
                pinnedAt: conversation.pinnedAt,
                archivedAt: conversation.archivedAt,
                pendingArchivedAt: conversation.pendingArchivedAt,
                unreadAt: conversation.unreadAt,
                messagesLoaded: true
              }
              return hydratedConversation
            })

            if (!foundConversation) {
              hydratedConversation = {
                ...storedConversation,
                messagesLoaded: true
              }
              return [hydratedConversation, ...nextConversations]
            }
            return nextConversations
          })
          return hydratedConversation
        })
        .catch((error) => {
          if (conversationDetailEpochRef.current.get(conversationId) === requestEpoch) {
            setConversationLoadErrors((current) => ({
              ...current,
              [conversationId]: getUserFacingErrorMessage(error, t, 'chat.conversationLoadFailed')
            }))
            console.error('Failed to load conversation messages', error)
          }
          return null
        })
        .finally(() => {
          if (conversationDetailRequestsRef.current.get(conversationId) === request) {
            conversationDetailRequestsRef.current.delete(conversationId)
          }
        })

      conversationDetailRequestsRef.current.set(conversationId, request)
      return request
    },
    [setConversationsWithRef, t]
  )

  useEffect(() => {
    activeConversationIdRef.current = activeConversationId
  }, [activeConversationId])

  const updateUiPreferences = useCallback((patch: Partial<UiPreferencesSnapshot>) => {
    setUiPreferences((currentPreferences) => {
      const nextPreferences = {
        ...currentPreferences,
        ...patch,
        updatedAt: Date.now()
      }
      void saveUiPreferences(nextPreferences)
      return nextPreferences
    })
  }, [])

  const updateDraft = useCallback(
    (scopeId: string, draft: ChatComposerDraft) => {
      setDraftsWithRef((currentDrafts) => ({
        ...currentDrafts,
        [scopeId]: draft
      }))
      void persistDraftNow(scopeId, draft)
    },
    [persistDraftNow, setDraftsWithRef]
  )

  const persistDraftMessageOnly = useCallback(
    (scopeId: string, draft: ChatComposerDraft) => {
      draftsRef.current = {
        ...draftsRef.current,
        [scopeId]: draft
      }
      scheduleMessageSave(scopeId, draft)
    },
    [scheduleMessageSave]
  )

  const mutateDraft = useCallback(
    (scopeId: string, updater: (draft: ChatComposerDraft) => ChatComposerDraft) => {
      const currentDraft = draftsRef.current[scopeId] ?? createComposerDraft()
      const nextDraft = {
        ...updater(currentDraft),
        updatedAt: Math.max(Date.now(), currentDraft.updatedAt + 1)
      }
      setDraftsWithRef({
        ...draftsRef.current,
        [scopeId]: nextDraft
      })
      void persistDraftNow(scopeId, nextDraft)
      return nextDraft
    },
    [persistDraftNow, setDraftsWithRef]
  )

  const recoverableGuidanceSignature = getRecoverableGuidanceSignature(conversations)

  useHumanInteractionConversationSync({
    activeConversationId,
    conversationsRef,
    setConversations: setConversationsWithRef
  })

  useEffect(() => {
    for (const conversation of conversationsRef.current) {
      if (conversation.messagesLoaded === false) continue
      const recoverableItems = conversation.messages.flatMap((message) =>
        (message.agentRun?.timeline ?? []).filter(isRecoverableGuidanceItem)
      )
      const unseenItems = recoverableItems.filter((item) => {
        const key = `${conversation.id}:${item.clientMessageId}`
        if (recoveredGuidanceKeysRef.current.has(key)) return false
        recoveredGuidanceKeysRef.current.add(key)
        return true
      })
      if (unseenItems.length === 0) continue

      void Promise.all(
        unseenItems.map(async (item) => {
          try {
            return {
              item,
              attachments: await loadInputAttachments(
                item.attachments.map((attachment) => attachment.id)
              ),
              attachmentRecoveryFailed: false
            }
          } catch (error) {
            console.error('Failed to recover interrupted guidance attachments', error)
            return {
              item,
              attachments: [],
              attachmentRecoveryFailed: item.attachments.length > 0
            }
          }
        })
      ).then((recoveredItems) => {
        if (!conversationsRef.current.some((candidate) => candidate.id === conversation.id)) return
        const recovered = recoveredItems.sort(
          (left, right) => left.item.createdAt - right.item.createdAt
        )
        mutateDraft(conversation.id, (draft) => {
          const existingClientMessageIds = new Set(
            draft.queuedMessages.map((message) => message.clientMessageId)
          )
          const recoveredMessages = recovered
            .filter(({ item }) => !existingClientMessageIds.has(item.clientMessageId))
            .map(({ item, attachments, attachmentRecoveryFailed }) => ({
              id: `recovered-guidance-${item.guidanceId ?? item.clientMessageId}`,
              clientMessageId: item.clientMessageId,
              content: item.content,
              attachments,
              folderReferences: item.folderReferences ?? [],
              modelId: conversation.modelId ?? draft.modelId,
              permissionMode: draft.permissionMode,
              projectId: conversation.projectId,
              skills: [],
              status: 'error' as const,
              error: attachmentRecoveryFailed
                ? t('chat.guidanceRecoveryAttachmentFailed')
                : t('chat.guidanceInterrupted'),
              createdAt: item.createdAt
            }))
          const recoveredIds = new Set(recoverableItems.map((item) => item.clientMessageId))
          return {
            ...draft,
            queuedMessages: [
              ...recoveredMessages,
              ...draft.queuedMessages.map((message) =>
                recoveredIds.has(message.clientMessageId)
                  ? {
                      ...message,
                      status: 'error' as const,
                      error: t('chat.guidanceInterrupted')
                    }
                  : message
              )
            ]
          }
        })
      })
    }
  }, [mutateDraft, recoverableGuidanceSignature, t])

  const restoreSubmittedSkills = useCallback(
    (
      scopeId: string,
      submittedSkills: readonly SkillSelection[],
      fallback: Pick<ChatComposerDraft, 'modelId' | 'permissionMode' | 'projectId'>
    ) => {
      if (submittedSkills.length === 0) return

      const currentDraft = draftsRef.current[scopeId] ?? createComposerDraft(fallback)
      const nextDraft = {
        ...currentDraft,
        // A user may already have started composing the next turn. Preserve that live choice for
        // duplicate ids and only restore submitted selections that are currently missing.
        skills: mergeSkillSelections(currentDraft.skills, submittedSkills),
        updatedAt: Math.max(Date.now(), currentDraft.updatedAt + 1)
      }
      setDraftsWithRef({ ...draftsRef.current, [scopeId]: nextDraft })
      void persistDraftNow(scopeId, nextDraft)
    },
    [persistDraftNow, setDraftsWithRef]
  )

  const reconcileFailedSkillActivation = useCallback(
    (
      scopeId: string,
      recovery: SkillActivationRecoveryPlan,
      fallback: Pick<ChatComposerDraft, 'modelId' | 'permissionMode' | 'projectId'>
    ) => {
      if (recovery.draftPolicy !== 'rejectSelection' && recovery.selectionsToRestore.length === 0) {
        return
      }

      const currentDraft = draftsRef.current[scopeId] ?? createComposerDraft(fallback)
      const nextDraft = {
        ...currentDraft,
        skills: reconcileSkillActivationSelections(currentDraft.skills, recovery),
        updatedAt: Math.max(Date.now(), currentDraft.updatedAt + 1)
      }
      setDraftsWithRef({ ...draftsRef.current, [scopeId]: nextDraft })
      void persistDraftNow(scopeId, nextDraft)
    },
    [persistDraftNow, setDraftsWithRef]
  )

  const requestSkillCatalogRefresh = useCallback(
    (scopeId: string) => {
      setSkillCatalogRefreshTokens((currentTokens) => ({
        ...currentTokens,
        [scopeId]: (currentTokens[scopeId] ?? 0) + 1
      }))
    },
    [setSkillCatalogRefreshTokens]
  )

  const {
    cleanupRunBinding,
    flushRunMessagePersistence,
    removeQueuedMessageByClientId,
    requestAssistantResponse,
    restoreRejectedGuidance,
    supersedeRejectedGuidance,
    scheduleStoppedRunReconciliation,
    updateAssistantMessage,
    waitForRunSettlement
  } = useAgentRunLifecycle({
    contextWindowIndicatorEnabled,
    conversationState: {
      activeConversationId,
      activeConversationIdRef,
      visibleConversationIdRef,
      conversations,
      conversationsRef,
      setActiveConversationId,
      setConversations: setConversationsWithRef
    },
    draftState: {
      draftsRef,
      mutateDraft
    },
    enqueueChatMessageCheckpoint,
    enqueueChatMessageStateSave,
    enqueueConversationMetaSave,
    flushChatMessageStateSave,
    flushConversationMessageStateSaves,
    recordContextWindowSnapshot,
    reconcileFailedSkillActivation,
    refs: {
      activeRunBindings: activeRunBindingsRef,
      autoSubmitQueuedMessage: autoSubmitQueuedMessageRef,
      bufferedAgentEvents: bufferedAgentEventsRef,
      cancelledPendingMessageIds: cancelledPendingMessageIdsRef,
      cancelledRunIds: cancelledRunIdsRef,
      locallyUnconfirmedStoppedRunIds: locallyUnconfirmedStoppedRunIdsRef,
      pendingActionsHydrated: pendingActionsHydratedRef,
      pendingGuidancePayloads: pendingGuidancePayloadsRef,
      pendingMessageDeltas: pendingMessageDeltasRef,
      retiredAgentRunIds: retiredAgentRunIdsRef,
      stopReconciliationTimers: stopReconciliationTimersRef,
      stopRequestedPendingMessageIds: stopRequestedPendingMessageIdsRef,
      stopRequestedRunIds: stopRequestedRunIdsRef
    },
    requestSkillCatalogRefresh,
    sealAndFlushChatMessageStateSaves,
    showToast,
    t,
    uiPreferences
  })

  return {
    cleanupRunBinding,
    flushRunMessagePersistence,
    hydrateConversation,
    mutateDraft,
    persistDraftMessageOnly,
    removeQueuedMessageByClientId,
    requestAssistantResponse,
    restoreRejectedGuidance,
    restoreSubmittedSkills,
    supersedeRejectedGuidance,
    scheduleStoppedRunReconciliation,
    setConversationsWithRef,
    setDraftsWithRef,
    updateAssistantMessage,
    updateDraft,
    waitForRunSettlement,
    updateUiPreferences
  }
}
