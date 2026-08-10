import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type SetStateAction
} from 'react'
import { createPortal } from 'react-dom'
import type {
  AgentEvent,
  AgentProviderTransitionOperation,
  AgentProviderTransitionReason,
  SkillSelection
} from '@mycopilot/protocol'
import { ResizeHandle } from '../components/layout/ResizeHandle'
import { LeftSidebar } from './shell/sidebar/LeftSidebar'
import { RightSidebar } from '../features/rightSidebar/RightSidebar'
import { useToast } from '../components/toast/ToastContext'
import { useModelSettings } from '../config/ModelSettingsProvider'
import { useProjectSettings } from '../config/ProjectSettingsProvider'
import { useFrontendConfig } from '../config/FrontendConfigProvider'
import { featureFlags } from '../config/featureFlags'
import { useGitRepositoryCapability } from '../features/gitReview/useGitRepositoryCapability'
import { useAppStartupStage } from '../features/startup/AppStartupContext'
import type {
  RightSidebarCapabilities,
  RightSidebarReviewNavigationRequest
} from '../features/rightSidebar/rightSidebarTypes'
import { ChatConversationPage } from '../features/chat/ChatConversationPage'
import { NewConversationPage } from '../features/chat/NewConversationPage'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatGuidanceTimelineItem,
  ChatMessageUiState,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import { cancelAgentRun, steerAgentRun } from '../features/agent/agentClient'
import {
  reconcileSkillActivationSelections,
  type SkillActivationRecoveryPlan
} from '../features/skills/skillActivationRecovery'
import { mergeSkillSelections } from '../features/skills/skillSelection'
import {
  defaultUiPreferences,
  deleteChatMessages,
  loadConversation,
  loadInputAttachments,
  saveComposerDraft,
  saveConversationMeta,
  saveUiPreferences,
  upsertChatMessages
} from '../features/storage/storageClient'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { THINKING_PLACEHOLDER } from '../features/agentRun/constants'
import { NEW_CONVERSATION_DRAFT_ID } from './appConstants'
import type { ActiveRunBinding } from './appTypes'
import {
  applyAgentEventToChatMessage,
  applyOptimisticGuidanceToChatMessage,
  ensureAgentRun
} from '../features/agentRun/agentEventReducer'
import {
  createAssistantMessage,
  createComposerDraft,
  createConversationTitle,
  createId,
  createUserMessage,
  synchronizeComposerDraftForScope
} from './chatMessageFactory'
import {
  buildMessageContentWithAttachments,
  getActiveRunModelId,
  getActiveRunSkillSelections,
  getEditableLastTurn
} from './appShellConversationUtils'
import { useConversationPersistence } from './useConversationPersistence'
import { useShellLayout } from './useShellLayout'
import { useConversationNavigation } from './useConversationNavigation'
import { usePersistedShellHydration } from './usePersistedShellHydration'
import { useProjectRemoval } from './useProjectRemoval'
import { useAppWindowSettings } from './useAppWindowSettings'
import { AppShellSettingsView } from './AppShellSettingsView'
import { AppShellWorkspace } from './AppShellWorkspace'
import {
  getAppShellPanelStyle,
  getPermissionModeAvailability,
  MainPanelToolbar,
  MaximizedSidebarControls,
  SUPPORTS_NATIVE_FONT_SMOOTHING
} from './AppShellSupport'
import type { PendingMessageDelta } from './AppShellSupport'
import { useAgentActionDecisionHandlers } from '../features/agentRun/useAgentActionDecisionHandlers'
import { useContextWindowSnapshots } from '../features/agentRun/useContextWindowSnapshots'
import { useAgentRunLifecycle } from './useAgentRunLifecycle'
import { useProviderTransition } from '../features/agentRun/useProviderTransition'
import { selectRenderableModelTransitionOperations } from '../features/chat/modelTransitionUiState'

export function AppShell() {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const {
    attempt: uiPreferencesStartupAttempt,
    markFailed: markUiPreferencesStartupFailed,
    markPending: markUiPreferencesStartupPending,
    markReady: markUiPreferencesStartupReady
  } = useAppStartupStage('uiPreferences')
  const {
    attempt: composerDraftsStartupAttempt,
    markFailed: markComposerDraftsStartupFailed,
    markPending: markComposerDraftsStartupPending,
    markReady: markComposerDraftsStartupReady
  } = useAppStartupStage('composerDrafts')
  const {
    attempt: conversationMetasStartupAttempt,
    markFailed: markConversationMetasStartupFailed,
    markPending: markConversationMetasStartupPending,
    markReady: markConversationMetasStartupReady
  } = useAppStartupStage('conversationMetas')
  const { enabledModels, models } = useModelSettings()
  const { projects, deleteProject, renameProject, showProjectInFolder, togglePinProject } =
    useProjectSettings()
  const {
    leftOpen,
    leftWidth,
    openRightSidebar,
    resizeSide,
    rightMaximized,
    rightOpen,
    rightWidth,
    shellRef,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  } = useShellLayout()
  const [rightSidebarReviewNavigationRequest, setRightSidebarReviewNavigationRequest] =
    useState<RightSidebarReviewNavigationRequest | null>(null)
  const rightSidebarReviewNavigationRequestIdRef = useRef(0)
  const {
    appWindowMaximized,
    closeSettings,
    openSettings,
    setSettingsOpen,
    settingsInitialPage,
    settingsOpen
  } = useAppWindowSettings(shellRef)
  const [uiPreferences, setUiPreferences] = useState<UiPreferencesSnapshot>(() =>
    defaultUiPreferences()
  )
  const [conversations, setConversations] = useState<ChatConversation[]>([])
  const conversationsRef = useRef<ChatConversation[]>([])
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null)
  const [activeConversationInitialScrollTop, setActiveConversationInitialScrollTop] = useState<
    number | null
  >(null)
  const [conversationScrollToBottomSignal, setConversationScrollToBottomSignal] = useState(0)
  const [scrollTargetMessageId, setScrollTargetMessageId] = useState<string | null>(null)
  const activeConversationIdRef = useRef<string | null>(null)
  const conversationDetailRequestsRef = useRef<Map<string, Promise<ChatConversation | null>>>(
    new Map()
  )
  const conversationDetailEpochRef = useRef<Map<string, number>>(new Map())
  const [conversationLoadErrors, setConversationLoadErrors] = useState<Record<string, string>>({})
  const conversationScrollPositionsRef = useRef<Map<string, number>>(new Map())
  const activeRunBindingsRef = useRef<Map<string, ActiveRunBinding>>(new Map())
  const bufferedAgentEventsRef = useRef<Map<string, AgentEvent[]>>(new Map())
  const retiredAgentRunIdsRef = useRef<Set<string>>(new Set())
  const locallyUnconfirmedStoppedRunIdsRef = useRef<Set<string>>(new Set())
  const pendingMessageDeltasRef = useRef<Map<string, PendingMessageDelta>>(new Map())
  const {
    enqueueChatMessagesUpsert,
    enqueueChatMessageStateSave,
    enqueueChatMessageUiStateSave,
    enqueueConversationMetaSave,
    pendingConversationSavesRef,
    pendingMessageSavesRef,
    pendingMessageUpsertsRef,
    waitForConversationSaves,
    waitForMessageStateSaves,
    waitForMessageUpserts
  } = useConversationPersistence()
  const pendingActionsHydratedRef = useRef<Set<string>>(new Set())
  const cancelledPendingMessageIdsRef = useRef<Set<string>>(new Set())
  const cancelledRunIdsRef = useRef<Set<string>>(new Set())
  const stopRequestedPendingMessageIdsRef = useRef<Set<string>>(new Set())
  const stopRequestedRunIdsRef = useRef<Set<string>>(new Set())
  const stopReconciliationTimersRef = useRef<Map<string, number>>(new Map())
  const pendingGuidancePayloadsRef = useRef<
    Map<
      string,
      {
        assistantMessageId: string
        conversationId: string
        index: number
        message: ChatQueuedMessage
      }
    >
  >(new Map())
  const recoveredGuidanceKeysRef = useRef<Set<string>>(new Set())
  const autoSubmitQueuedMessageRef = useRef<(conversationId: string) => void>(() => undefined)
  const editSubmissionSeqRef = useRef(0)
  const pendingProviderTransitionSubmissionsRef = useRef<
    Map<
      string,
      | { kind: 'composer'; message: string; options: ChatSubmitOptions }
      | { kind: 'queued_message'; queueMessageId: string }
    >
  >(new Map())
  const [drafts, setDrafts] = useState<Record<string, ChatComposerDraft>>({
    [NEW_CONVERSATION_DRAFT_ID]: createComposerDraft()
  })
  const draftsRef = useRef(drafts)
  const [skillCatalogRefreshTokens, setSkillCatalogRefreshTokens] = useState<
    Record<string, number>
  >({})
  const activeConversation = useMemo(
    () => conversations.find((conversation) => conversation.id === activeConversationId) ?? null,
    [activeConversationId, conversations]
  )
  const activeDraftId = activeConversation?.id ?? NEW_CONVERSATION_DRAFT_ID
  const activeDraft =
    drafts[activeDraftId] ??
    createComposerDraft({ projectId: activeConversation?.projectId ?? null })
  const activeSkillCatalogRefreshToken = skillCatalogRefreshTokens[activeDraftId] ?? 0
  const activeDraftSelectedModel = useMemo(
    () =>
      enabledModels.find((model) => model.id === activeDraft.modelId) ?? enabledModels[0] ?? null,
    [activeDraft.modelId, enabledModels]
  )
  const activeRunModelId = useMemo(
    () => getActiveRunModelId(activeConversation),
    [activeConversation]
  )
  const contextWindowSkills = useMemo(
    () => (activeRunModelId ? getActiveRunSkillSelections(activeConversation) : activeDraft.skills),
    [activeConversation, activeDraft.skills, activeRunModelId]
  )
  // The composer selects the next run. Capacity reporting stays pinned to the immutable model of
  // the current run until that run reaches a terminal state.
  const contextWindowModel = useMemo(
    () =>
      activeRunModelId
        ? (models.find((model) => model.id === activeRunModelId) ?? null)
        : activeDraftSelectedModel,
    [activeDraftSelectedModel, activeRunModelId, models]
  )
  const activeContextWindowKey = activeConversation?.id ?? NEW_CONVERSATION_DRAFT_ID
  const contextWindowModelId = contextWindowModel?.id ?? null
  const contextWindowIndicatorEnabled =
    featureFlags.contextWindowIndicator && uiPreferences.showContextWindowUsage
  const {
    activeSnapshot: activeContextWindowSnapshot,
    recordSnapshot: recordContextWindowSnapshot
  } = useContextWindowSnapshots({
    conversationId: activeConversation?.id,
    customPermissions: uiPreferences.customPermissions,
    enabled: contextWindowIndicatorEnabled,
    modelId: contextWindowModelId,
    permissionMode: activeDraft.permissionMode,
    projectId: activeConversation?.projectId ?? activeDraft.projectId,
    scopeId: activeContextWindowKey,
    skills: contextWindowSkills
  })
  const permissionModeAvailability = getPermissionModeAvailability(uiPreferences)
  const hasUnreadConversations = conversations.some(
    (conversation) => !conversation.archivedAt && Boolean(conversation.unreadAt)
  )
  const rightSidebarWorkspaceProjectId = activeConversation
    ? activeConversation.projectId
    : activeDraft.projectId
  const rightSidebarWorkspaceProject = useMemo(() => {
    if (!rightSidebarWorkspaceProjectId) return null

    return projects.find((project) => project.id === rightSidebarWorkspaceProjectId) ?? null
  }, [rightSidebarWorkspaceProjectId, projects])
  const rightSidebarWorkspaceKeys = useMemo(() => projects.map((project) => project.id), [projects])
  const rightSidebarWorkspacePath = rightSidebarWorkspaceProject?.path?.trim() || undefined
  const gitRepositoryCapability = useGitRepositoryCapability(
    rightSidebarWorkspaceProject?.id,
    rightSidebarWorkspacePath
  )

  const rightSidebarCapabilities = useMemo<RightSidebarCapabilities>(
    () => ({ 'git-repository': gitRepositoryCapability }),
    [gitRepositoryCapability]
  )
  const openLastTurnReview = useCallback(
    (filePath?: string) => {
      const projectId = rightSidebarWorkspaceProject?.id
      if (!projectId) return
      rightSidebarReviewNavigationRequestIdRef.current += 1
      setRightSidebarReviewNavigationRequest({
        kind: 'git-review',
        projectId,
        requestId: rightSidebarReviewNavigationRequestIdRef.current,
        scope: 'lastTurn',
        ...(filePath ? { filePath } : {})
      })
      openRightSidebar()
    },
    [openRightSidebar, rightSidebarWorkspaceProject?.id]
  )
  const rightSidebarMaximizedToolbarControls = useMemo(
    () =>
      rightMaximized ? (
        <MaximizedSidebarControls
          hasUnreadConversations={hasUnreadConversations}
          leftOpen={leftOpen}
          onToggleLeftSidebar={toggleLeftSidebar}
          onToggleRightSidebar={toggleRightSidebar}
          rightOpen={rightOpen}
          t={t}
        />
      ) : null,
    [
      hasUnreadConversations,
      leftOpen,
      rightMaximized,
      rightOpen,
      t,
      toggleLeftSidebar,
      toggleRightSidebar
    ]
  )

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

  const hydrateConversation = useCallback(
    (conversationId: string): Promise<ChatConversation | null> => {
      const currentConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      if (currentConversation?.messagesLoaded !== false) {
        return Promise.resolve(currentConversation ?? null)
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
          setConversationsWithRef((currentConversations) =>
            currentConversations.map((conversation) => {
              if (conversation.id !== conversationId) return conversation
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
                unreadAt: conversation.unreadAt,
                messagesLoaded: true
              }
              return hydratedConversation
            })
          )
          return hydratedConversation
        })
        .catch((error) => {
          if (conversationDetailEpochRef.current.get(conversationId) === requestEpoch) {
            const message = error instanceof Error ? error.message : String(error)
            setConversationLoadErrors((current) => ({ ...current, [conversationId]: message }))
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
    [setConversationsWithRef]
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
      void saveComposerDraft(scopeId, draft)
    },
    [setDraftsWithRef]
  )

  const persistDraftMessageOnly = useCallback((scopeId: string, draft: ChatComposerDraft) => {
    draftsRef.current = {
      ...draftsRef.current,
      [scopeId]: draft
    }
    void saveComposerDraft(scopeId, draft)
  }, [])

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
      void saveComposerDraft(scopeId, nextDraft)
      return nextDraft
    },
    [setDraftsWithRef]
  )

  useEffect(() => {
    for (const conversation of conversations) {
      if (conversation.messagesLoaded === false) continue
      const recoverableItems = conversation.messages.flatMap((message) =>
        (message.agentRun?.timeline ?? []).filter(
          (item): item is ChatGuidanceTimelineItem =>
            item.type === 'user_guidance' &&
            item.status === 'rejected' &&
            item.recoverable === true &&
            item.rejectionCode === 'run_interrupted'
        )
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
  }, [conversations, mutateDraft, t])

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
        updatedAt: Date.now()
      }
      setDraftsWithRef({ ...draftsRef.current, [scopeId]: nextDraft })
      void saveComposerDraft(scopeId, nextDraft)
    },
    [setDraftsWithRef]
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
        updatedAt: Date.now()
      }
      setDraftsWithRef({ ...draftsRef.current, [scopeId]: nextDraft })
      void saveComposerDraft(scopeId, nextDraft)
    },
    [setDraftsWithRef]
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
    removeQueuedMessageByClientId,
    requestAssistantResponse,
    restoreRejectedGuidance,
    scheduleStoppedRunReconciliation,
    updateAssistantMessage
  } = useAgentRunLifecycle({
    contextWindowIndicatorEnabled,
    conversationState: {
      activeConversationId,
      activeConversationIdRef,
      conversations,
      conversationsRef,
      setActiveConversationId,
      setConversations: setConversationsWithRef
    },
    draftState: {
      draftsRef,
      mutateDraft
    },
    enqueueChatMessageStateSave,
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
    showToast,
    t,
    uiPreferences
  })
  const submitMessageToConversation = useCallback(
    (
      targetConversationId: string | null,
      message: string,
      options: ChatSubmitOptions,
      behavior: { activate: boolean; preserveComposerContent: boolean }
    ) => {
      const targetConversation = targetConversationId
        ? (conversationsRef.current.find(
            (conversation) => conversation.id === targetConversationId
          ) ?? null)
        : null
      const now = targetConversation
        ? Math.max(Date.now(), targetConversation.updatedAt + 1)
        : Date.now()
      const conversationId = targetConversation?.id ?? createId('conversation')
      const userMessage = createUserMessage(message, options.attachments ?? [])
      const assistantMessage = createAssistantMessage(THINKING_PLACEHOLDER, 'pending')
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

      await waitForConversationSaves(conversationId)
      const outcome = await requestProviderTransition(conversationId, options.modelId)
      if (outcome.status === 'completed') {
        submitMessageToConversation(
          conversationId,
          message,
          { ...options, modelId: outcome.operation.modelId },
          {
            activate: true,
            preserveComposerContent: false
          }
        )
        return true
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
      if (!activeDraftSelectedModel) {
        throw new Error(t('chat.noEnabledModels'))
      }
      if (
        attachments.some((attachment) => attachment.kind === 'image') &&
        !activeDraftSelectedModel.supportsImage
      ) {
        throw new Error(t('chat.unsupportedImageWarning'))
      }

      await waitForConversationSaves(conversationId)
      const transitionOutcome = await requestProviderTransition(
        conversationId,
        activeDraftSelectedModel.id
      )
      if (transitionOutcome.status !== 'completed') {
        throw new Error(
          transitionOutcome.status === 'confirmation_required' ||
            transitionOutcome.status === 'running'
            ? t('chat.modelTransition.confirmThenRetryEdit')
            : t('chat.modelTransition.requestFailed')
        )
      }

      const now = Date.now()
      const modelId = transitionOutcome.operation.modelId
      const permissionMode = activeDraft.permissionMode
      const editedSkillSelections =
        latestEditableTurn.assistantMessage.agentRun?.explicitSkillSelections ??
        latestEditableTurn.assistantMessage.agentRun?.activatedSkills?.map((skill) => ({
          id: skill.id,
          revision: skill.revision
        })) ??
        []
      const userMessage = createUserMessage(messageContent, attachments)
      const assistantMessage = createAssistantMessage(THINKING_PLACEHOLDER, 'pending')
      const messagesBeforeEditedTurn = latestConversation.messages.slice(
        0,
        latestEditableTurn.userIndex
      )
      const nextConversation: ChatConversation = {
        ...latestConversation,
        messages: [...messagesBeforeEditedTurn, userMessage, assistantMessage],
        modelId,
        updatedAt: now,
        unreadAt: null
      }

      const oldRunId = latestEditableTurn.assistantMessage.agentRun?.runId
      if (oldRunId) {
        cancelledRunIdsRef.current.add(oldRunId)
        cleanupRunBinding(oldRunId)
      }

      cancelledPendingMessageIdsRef.current.add(latestEditableTurn.assistantMessage.id)
      const submissionSeq = (editSubmissionSeqRef.current += 1)
      setScrollTargetMessageId(null)
      setActiveConversationInitialScrollTop(null)
      setConversationScrollToBottomSignal((signal) => signal + 1)
      setConversationsWithRef((currentConversations) =>
        currentConversations.map((candidate) =>
          candidate.id === conversationId ? nextConversation : candidate
        )
      )
      updateDraft(
        conversationId,
        createComposerDraft({
          modelId,
          permissionMode,
          projectId: latestConversation.projectId
        })
      )

      void (async () => {
        try {
          await saveConversationMeta(nextConversation)
          await deleteChatMessages(conversationId, [
            latestEditableTurn.userMessage.id,
            latestEditableTurn.assistantMessage.id
          ])
          await upsertChatMessages(
            conversationId,
            [userMessage, assistantMessage],
            latestEditableTurn.userIndex
          )

          if (editSubmissionSeqRef.current !== submissionSeq) return

          await requestAssistantResponse(
            conversationId,
            userMessage.id,
            assistantMessage.id,
            content,
            modelId,
            latestConversation.projectId,
            permissionMode,
            attachments,
            editedSkillSelections,
            undefined
          )
        } catch (error) {
          if (editSubmissionSeqRef.current !== submissionSeq) return
          const errorMessage = error instanceof Error ? error.message : String(error)
          restoreSubmittedSkills(conversationId, editedSkillSelections, {
            modelId,
            permissionMode,
            projectId: latestConversation.projectId
          })
          updateAssistantMessage(
            conversationId,
            assistantMessage.id,
            (currentMessage) => ({
              ...currentMessage,
              content: errorMessage,
              status: 'error',
              agentRun: {
                ...ensureAgentRun(currentMessage.agentRun, null, 'failed'),
                error: errorMessage
              }
            }),
            { touchConversation: true }
          )
        }
      })()
    },
    [
      activeDraft.permissionMode,
      activeDraftSelectedModel,
      cleanupRunBinding,
      requestAssistantResponse,
      requestProviderTransition,
      restoreSubmittedSkills,
      setConversationsWithRef,
      t,
      updateDraft,
      updateAssistantMessage,
      waitForConversationSaves,
      waitForMessageUpserts
    ]
  )

  const {
    archiveConversations,
    continueInNewTask,
    openContinuationOrigin,
    patchConversation,
    rememberConversationScrollPosition,
    selectConversation
  } = useConversationNavigation({
    activeConversationIdRef,
    conversationScrollPositionsRef,
    conversationsRef,
    drafts,
    hydrateConversation,
    messages: {
      activeCommandSession: t('chat.continueInNewTaskActiveCommand'),
      continueInNewTaskFailed: t('chat.continueInNewTaskFailed'),
      originArchived: t('chat.continuationOriginArchived'),
      originMissing: t('chat.continuationOriginMissing'),
      originOpenFailed: t('chat.continuationOriginOpenFailed')
    },
    setActiveConversationId,
    setActiveConversationInitialScrollTop,
    setConversationScrollToBottomSignal,
    setConversationsWithRef,
    setDraftsWithRef,
    setScrollTargetMessageId,
    setSettingsOpen,
    showToast
  })

  const activeProviderTransitionConversationId = activeConversation?.id
  const activeProviderTransitionMessagesLoaded = activeConversation?.messagesLoaded
  useEffect(() => {
    if (
      !activeProviderTransitionConversationId ||
      activeProviderTransitionMessagesLoaded === false
    ) {
      return
    }
    void loadProviderTransitionStatus(activeProviderTransitionConversationId)
  }, [
    activeProviderTransitionConversationId,
    activeProviderTransitionMessagesLoaded,
    loadProviderTransitionStatus
  ])

  const activeProviderTransitionConfirmation = activeConversation
    ? providerTransitionStore.confirmations[activeConversation.id]
    : undefined
  const activeProviderTransitionOperations = useMemo(
    () =>
      activeProviderTransitionConversationId
        ? selectRenderableModelTransitionOperations(
            providerTransitionStore,
            activeProviderTransitionConversationId
          )
        : [],
    [activeProviderTransitionConversationId, providerTransitionStore]
  )

  const requestConversationModelChange = useCallback(
    async (targetModelId: string) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) return
      pendingProviderTransitionSubmissionsRef.current.delete(conversationId)
      await waitForConversationSaves(conversationId)
      await requestProviderTransition(conversationId, targetModelId)
    },
    [requestProviderTransition, waitForConversationSaves]
  )

  const cancelActiveProviderTransition = useCallback(() => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) return
    pendingProviderTransitionSubmissionsRef.current.delete(conversationId)
    cancelProviderTransitionConfirmation(conversationId)
  }, [cancelProviderTransitionConfirmation])

  const confirmActiveProviderTransition = useCallback(async () => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) return
    await waitForConversationSaves(conversationId)
    await confirmProviderTransition(conversationId)
  }, [confirmProviderTransition, waitForConversationSaves])

  const retryActiveProviderTransition = useCallback(
    async (operation: AgentProviderTransitionOperation) => {
      await waitForConversationSaves(operation.conversationId)
      await retryProviderTransition(operation)
    },
    [retryProviderTransition, waitForConversationSaves]
  )

  const removeProject = useProjectRemoval({
    activeConversationIdRef,
    activeRunBindingsRef,
    cancelledPendingMessageIdsRef,
    cancelledRunIdsRef,
    cleanupRunBinding,
    conversationScrollPositionsRef,
    conversationsRef,
    deleteProject,
    editSubmissionSeqRef,
    enqueueChatMessageStateSave,
    pendingConversationSavesRef,
    pendingMessageSavesRef,
    pendingMessageUpsertsRef,
    removeFailedMessage: t('project.removeFailed'),
    setActiveConversationId,
    setActiveConversationInitialScrollTop,
    setConversationsWithRef,
    setDraftsWithRef,
    setScrollTargetMessageId,
    setUiPreferences,
    showToast,
    waitForConversationSaves,
    waitForMessageStateSaves,
    waitForMessageUpserts
  })

  const stopActiveGeneration = useCallback(() => {
    if (!activeConversationId) return
    const activeConversationSnapshot = conversations.find(
      (conversation) => conversation.id === activeConversationId
    )
    const pendingMessage = [...(activeConversationSnapshot?.messages ?? [])]
      .reverse()
      .find((message) => message.role === 'assistant' && message.status === 'pending')

    if (!pendingMessage) return
    const runId = pendingMessage.agentRun?.runId
    if (!runId) {
      stopRequestedPendingMessageIdsRef.current.add(pendingMessage.id)
      return
    }
    if (stopRequestedRunIdsRef.current.has(runId)) return

    // The backend emits terminal Done only after the assistant message and trace have been
    // committed atomically. Keep the binding alive and let that event authoritatively settle the
    // UI instead of persisting a renderer-invented cancelled state ahead of durable storage.
    stopRequestedRunIdsRef.current.add(runId)
    const binding = activeRunBindingsRef.current.get(runId)
    if (binding) scheduleStoppedRunReconciliation(runId, binding)
    void cancelAgentRun(runId).catch(() => {
      console.error('Failed to cancel agent run')
    })
  }, [activeConversationId, conversations, scheduleStoppedRunReconciliation])

  const guideQueuedMessage = useCallback(
    (queuedMessage: ChatQueuedMessage) => {
      const conversationId = activeConversationIdRef.current
      if (!conversationId) return
      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === conversationId
      )
      const assistantMessage = [...(conversation?.messages ?? [])]
        .reverse()
        .find((message) => message.role === 'assistant' && message.status === 'pending')
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
            window.setTimeout(() => autoSubmitQueuedMessageRef.current(conversationId), 0)
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
          window.setTimeout(() => autoSubmitQueuedMessageRef.current(conversationId), 0)
        })
        .catch((error) => {
          const message = error instanceof Error ? error.message : String(error)
          restoreRejectedGuidance(
            conversationId,
            assistantMessage.id,
            queuedMessage.clientMessageId,
            message || t('chat.guidanceFailed')
          )
          window.setTimeout(() => autoSubmitQueuedMessageRef.current(conversationId), 0)
        })
    },
    [mutateDraft, removeQueuedMessageByClientId, restoreRejectedGuidance, t, updateAssistantMessage]
  )

  const { handleApproveAgentAction, handleCancelAgentAction, handleRejectAgentAction } =
    useAgentActionDecisionHandlers({
      activeConversationId,
      conversationsRef,
      updateAssistantMessage
    })

  return (
    <AppShellWorkspace
      ref={shellRef}
      className="app-shell"
      data-left-open={leftOpen ? 'true' : 'false'}
      data-native-font-smoothing={
        SUPPORTS_NATIVE_FONT_SMOOTHING && uiPreferences.nativeFontSmoothing ? 'true' : undefined
      }
      data-right-maximized={rightMaximized ? 'true' : undefined}
      data-right-open={rightOpen ? 'true' : 'false'}
      data-translucent-sidebar={uiPreferences.translucentSidebar ? 'true' : undefined}
      data-window-maximized={appWindowMaximized ? 'true' : undefined}
      settingsOpen={settingsOpen}
      style={getAppShellPanelStyle(leftOpen, leftWidth, rightOpen, rightWidth, uiPreferences)}
    >
      <header className="window-toolbar" data-drag-region />

      <aside className="side-panel side-panel--left">
        <div className="side-panel__surface">
          <LeftSidebar
            activeConversationId={activeConversationId}
            conversations={conversations}
            projects={projects}
            uiPreferences={uiPreferences}
            onArchiveAllProjectConversations={() => {
              const projectIds = new Set(projects.map((project) => project.id))
              archiveConversations(
                (conversation) =>
                  Boolean(conversation.projectId) && projectIds.has(conversation.projectId!)
              )
            }}
            onArchiveAllRootConversations={() => {
              const projectIds = new Set(projects.map((project) => project.id))
              archiveConversations(
                (conversation) => !conversation.projectId || !projectIds.has(conversation.projectId)
              )
            }}
            onArchiveConversation={(conversationId) =>
              patchConversation(conversationId, { archivedAt: Date.now() })
            }
            onArchiveProjectConversations={(projectId) =>
              archiveConversations((conversation) => conversation.projectId === projectId)
            }
            onMarkConversationUnread={(conversationId) =>
              patchConversation(conversationId, { unreadAt: Date.now() })
            }
            onNewConversation={(projectId = null) => {
              activeConversationIdRef.current = null
              setActiveConversationId(null)
              setScrollTargetMessageId(null)
              setActiveConversationInitialScrollTop(null)
              updateDraft(NEW_CONVERSATION_DRAFT_ID, createComposerDraft({ projectId }))
            }}
            onOpenSettings={() => openSettings('general')}
            onRemoveProject={removeProject}
            onRenameConversation={(conversationId, title) =>
              patchConversation(conversationId, { title })
            }
            onRenameProject={renameProject}
            onSelectConversation={selectConversation}
            onShowProjectInFolder={showProjectInFolder}
            onTogglePinConversation={(conversationId) => {
              const conversation = conversations.find(
                (candidate) => candidate.id === conversationId
              )
              patchConversation(conversationId, {
                pinnedAt: conversation?.pinnedAt ? null : Date.now()
              })
            }}
            onTogglePinProject={togglePinProject}
            onUiPreferencesChange={updateUiPreferences}
          />
        </div>
      </aside>

      {leftOpen && <ResizeHandle side="left" onResize={(deltaX) => resizeSide('left', deltaX)} />}

      <main className="main-panel" aria-label={t('app.mainWorkspace')}>
        <MainPanelToolbar
          hasUnreadConversations={hasUnreadConversations}
          leftOpen={leftOpen}
          onToggleLeftSidebar={toggleLeftSidebar}
          onToggleRightSidebar={toggleRightSidebar}
          rightOpen={rightOpen}
          t={t}
          title={activeConversation?.title}
        />

        <div className="main-panel__surface">
          {activeConversation ? (
            activeConversation.messagesLoaded === false ? (
              <div className="conversation-load-state" role="status">
                <p>
                  {conversationLoadErrors[activeConversation.id]
                    ? t('chat.conversationLoadFailed')
                    : t('chat.loadingConversation')}
                </p>
                {conversationLoadErrors[activeConversation.id] ? (
                  <button
                    type="button"
                    onClick={() => void hydrateConversation(activeConversation.id)}
                  >
                    {t('chat.retryConversationLoad')}
                  </button>
                ) : null}
              </div>
            ) : (
              <ChatConversationPage
                composerDraft={activeDraft}
                conversation={activeConversation}
                contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
                contextWindowSnapshot={activeContextWindowSnapshot}
                editSelectedModelAvailable={Boolean(activeDraftSelectedModel)}
                editSelectedModelSupportsImage={Boolean(activeDraftSelectedModel?.supportsImage)}
                initialScrollTop={activeConversationInitialScrollTop}
                modelTransitionConfirmation={activeProviderTransitionConfirmation}
                modelTransitionOperations={activeProviderTransitionOperations}
                permissionModeAvailability={permissionModeAvailability}
                skillCatalogRefreshToken={activeSkillCatalogRefreshToken}
                scrollToBottomSignal={conversationScrollToBottomSignal}
                scrollTargetMessageId={scrollTargetMessageId}
                showTokenUsageDetails={uiPreferences.showTokenUsageDetails}
                onApproveAgentAction={handleApproveAgentAction}
                onCancelAgentAction={handleCancelAgentAction}
                onComposerDraftChange={(draft) => updateDraft(activeConversation.id, draft)}
                onComposerDraftMessageChange={(draft) =>
                  persistDraftMessageOnly(activeConversation.id, draft)
                }
                onGuideQueuedMessage={guideQueuedMessage}
                onModelChangeRequested={requestConversationModelChange}
                onModelTransitionCancel={cancelActiveProviderTransition}
                onModelTransitionConfirm={confirmActiveProviderTransition}
                onModelTransitionRetry={retryActiveProviderTransition}
                onEditLastUserMessage={submitEditedLastUserMessage}
                onContinueInNewTask={(messageId) =>
                  continueInNewTask(activeConversation.id, messageId)
                }
                onOpenContinuationOrigin={openContinuationOrigin}
                onMessageUiStateChange={(messageId, uiState: ChatMessageUiState | undefined) => {
                  setConversationsWithRef((currentConversations) =>
                    currentConversations.map((conversation) =>
                      conversation.id === activeConversation.id
                        ? {
                            ...conversation,
                            messages: conversation.messages.map((message) =>
                              message.id === messageId ? { ...message, uiState } : message
                            )
                          }
                        : conversation
                    )
                  )
                  enqueueChatMessageUiStateSave(activeConversation.id, messageId, uiState)
                }}
                onRejectAgentAction={handleRejectAgentAction}
                onReviewLastTurn={openLastTurnReview}
                onScrollPositionChange={rememberConversationScrollPosition}
                onStopGenerating={stopActiveGeneration}
                onSubmitMessage={submitMessage}
              />
            )
          ) : (
            <NewConversationPage
              contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
              contextWindowSnapshot={activeContextWindowSnapshot}
              draft={activeDraft}
              permissionModeAvailability={permissionModeAvailability}
              skillCatalogRefreshToken={activeSkillCatalogRefreshToken}
              onDraftChange={(draft) => updateDraft(NEW_CONVERSATION_DRAFT_ID, draft)}
              onDraftMessageChange={(draft) =>
                persistDraftMessageOnly(NEW_CONVERSATION_DRAFT_ID, draft)
              }
              onSubmitMessage={submitMessage}
            />
          )}
        </div>
      </main>

      {rightOpen && !rightMaximized && (
        <ResizeHandle side="right" onResize={(deltaX) => resizeSide('right', deltaX)} />
      )}

      <aside className="side-panel side-panel--right">
        <RightSidebar
          activeConversationId={activeConversation?.id}
          capabilities={rightSidebarCapabilities}
          isMaximized={rightMaximized}
          isOpen={rightOpen}
          isWorkspaceVisible={!settingsOpen}
          workspaceKey={rightSidebarWorkspaceProject?.id}
          workspaceKeys={rightSidebarWorkspaceKeys}
          workspaceName={rightSidebarWorkspaceProject?.name}
          workspacePath={rightSidebarWorkspacePath}
          onToggleMaximized={toggleRightSidebarMaximized}
          reviewNavigationRequest={rightSidebarReviewNavigationRequest}
          maximizedToolbarControls={rightSidebarMaximizedToolbarControls}
        />
      </aside>
      {settingsOpen &&
        createPortal(
          <AppShellSettingsView
            conversations={conversations}
            initialPage={settingsInitialPage}
            onBack={closeSettings}
            onConversationPatch={patchConversation}
            onConversationsChange={setConversationsWithRef}
            onRemoveProject={removeProject}
            onUiPreferencesChange={updateUiPreferences}
            projects={projects}
            uiPreferences={uiPreferences}
          />,
          document.body
        )}
    </AppShellWorkspace>
  )
}
