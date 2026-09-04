import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type {
  AgentEvent,
  AgentProviderTransitionOperation,
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
  RightSidebarAgentNavigationRequest,
  RightSidebarCapabilities,
  RightSidebarReviewNavigationRequest
} from '../features/rightSidebar/rightSidebarTypes'
import { ChatConversationPage } from '../features/chat/ChatConversationPage'
import { NewConversationPage } from '../features/chat/NewConversationPage'
import {
  getNextRandomNewConversationPromptIndex,
  getRandomNewConversationPromptIndex
} from '../features/chat/newConversationPrompts'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatMessage,
  ChatMessageUiState,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../features/chat/chatTypes'
import { retainGlobalSkillSelections } from '../features/skills/skillSelection'
import { defaultUiPreferences } from '../features/storage/storageClient'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { NEW_CONVERSATION_DRAFT_ID } from './appConstants'
import type { ActiveRunBinding } from './appTypes'
import { createComposerDraft } from './chatMessageFactory'
import { getActiveRunModelId, getActiveRunSkillSelections } from './appShellConversationUtils'
import { useConversationPersistence } from './useConversationPersistence'
import { useComposerDraftPersistence } from './useComposerDraftPersistence'
import { useShellLayout } from './useShellLayout'
import { useConversationNavigation } from './useConversationNavigation'
import { useProjectRemoval } from './useProjectRemoval'
import { useAppWindowSettings } from './useAppWindowSettings'
import { AppShellSettingsView } from './AppShellSettingsView'
import {
  getAppShellPanelStyle,
  getPermissionModeAvailability,
  HAS_MACOS_WINDOW_CONTROLS,
  MainPanelToolbar,
  MaximizedSidebarControls,
  PanelToggleButton,
  SUPPORTS_NATIVE_FONT_SMOOTHING
} from './AppShellSupport'
import type { PendingMessageDelta } from './AppShellSupport'
import { useAgentActionDecisionHandlers } from '../features/agentRun/useAgentActionDecisionHandlers'
import { useContextWindowSnapshots } from '../features/agentRun/useContextWindowSnapshots'
import { useAppShellMessageSubmission } from './useAppShellMessageSubmission'
import { useAppShellRuntime } from './useAppShellRuntime'
import { useAppShellRunControls } from './useAppShellRunControls'
import { selectRenderableModelTransitionOperations } from '../features/chat/modelTransitionUiState'
import { useOptionalCollaborationStore } from '../features/agentCollaboration/useCollaborationStore'
import { CollaborationApprovalPanel } from '../features/agentCollaboration/CollaborationApprovalPanel'
import { useCollaborationApprovals } from '../features/agentCollaboration/useCollaborationApprovals'
import { AgentObserverConversationSurface } from '../features/agentCollaboration/AgentObserverConversationSurface'
import { useBrowserSurfaceCommand } from '../features/browser/browserSurface'
import { hostClient } from '../host/hostClient'
import { ScheduledPageLayer } from '../features/automations/ScheduledPageLayer'
import { AUTOMATION_DRAWER_DEFAULT_WIDTH } from '../features/automations/automationLayout'
import { useAutomationAttention } from '../features/automations/useAutomationAttention'
import {
  markNotificationsSeen,
  onNotificationOpenRequested
} from '../features/notifications/notificationClient'
import {
  AppShellCoveredRegion,
  AppShellWorkspace,
  getVisibleActiveConversationId,
  type PrimaryView
} from './AppShellWorkspace'

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

interface ScheduledOpenRequest {
  automationId: string
  requestKey: number
  runId: string | null
}

interface ScheduledExternalNavigationRequest {
  proceed: () => void
  requestKey: number
}

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
  const {
    projects,
    deleteProject,
    renameProject,
    selectProjectDirectory,
    showProjectInFolder,
    togglePinProject
  } = useProjectSettings()
  const {
    commitSidebarResize,
    leftResizeMetrics,
    leftOpen,
    leftWidth,
    openRightSidebar,
    rightMaximized,
    rightOpen,
    rightResizeMetrics,
    rightWidth,
    shellRef,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  } = useShellLayout()
  const browserSurfaceBridge = useBrowserSurfaceCommand(openRightSidebar)
  const [rightSidebarReviewNavigationRequest, setRightSidebarReviewNavigationRequest] =
    useState<RightSidebarReviewNavigationRequest | null>(null)
  const rightSidebarReviewNavigationRequestIdRef = useRef(0)
  const [rightSidebarAgentNavigationRequest, setRightSidebarAgentNavigationRequest] =
    useState<RightSidebarAgentNavigationRequest | null>(null)
  const rightSidebarAgentNavigationRequestIdRef = useRef(0)
  const {
    appWindowMaximized,
    closeSettings,
    openSettings,
    setSettingsOpen,
    settingsInitialBrowserView,
    settingsInitialPage,
    settingsOpen
  } = useAppWindowSettings(shellRef)
  const [uiPreferences, setUiPreferences] = useState<UiPreferencesSnapshot>(() =>
    defaultUiPreferences()
  )
  const [conversations, setConversations] = useState<ChatConversation[]>([])
  const conversationsRef = useRef<ChatConversation[]>([])
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null)
  const [newConversationPromptIndex, setNewConversationPromptIndex] = useState(() =>
    getRandomNewConversationPromptIndex()
  )
  const [primaryView, setPrimaryView] = useState<PrimaryView>('conversation')
  const [scheduledDrawerPreferredWidth, setScheduledDrawerPreferredWidth] = useState(
    AUTOMATION_DRAWER_DEFAULT_WIDTH
  )
  const [scheduledOpenRequest, setScheduledOpenRequest] = useState<ScheduledOpenRequest | null>(
    null
  )
  const scheduledOpenRequestKeyRef = useRef(0)
  const [scheduledExternalNavigationRequest, setScheduledExternalNavigationRequest] =
    useState<ScheduledExternalNavigationRequest | null>(null)
  const scheduledExternalNavigationRequestKeyRef = useRef(0)
  const conversationOpenRequestKeyRef = useRef(0)
  const { unreadCount: scheduledAttentionCount } = useAutomationAttention()
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
  const automationConversationMetaRefreshEpochRef = useRef(0)
  const [conversationLoadErrors, setConversationLoadErrors] = useState<Record<string, string>>({})
  const conversationScrollPositionsRef = useRef<Map<string, number>>(new Map())
  const activeRunBindingsRef = useRef<Map<string, ActiveRunBinding>>(new Map())
  const bufferedAgentEventsRef = useRef<Map<string, AgentEvent[]>>(new Map())
  const retiredAgentRunIdsRef = useRef<Set<string>>(new Set())
  const locallyUnconfirmedStoppedRunIdsRef = useRef<Set<string>>(new Set())
  const pendingMessageDeltasRef = useRef<Map<string, PendingMessageDelta>>(new Map())
  const {
    enqueueChatMessageCheckpoint,
    enqueueChatMessagesUpsert,
    enqueueChatMessageStateSave,
    enqueueChatMessageUiStateSave,
    enqueueConversationMetaSave,
    flushChatMessageStateSave,
    flushConversationMessageStateSaves,
    pendingConversationSavesRef,
    pendingMessageSavesRef,
    pendingMessageUpsertsRef,
    sealAndFlushChatMessageStateSaves,
    waitForConversationSaves,
    waitForMessageStateSaves,
    waitForMessageUpserts
  } = useConversationPersistence()
  const { discardDraft, flushDraft, persistDraftNow, resumeDraft, scheduleMessageSave } =
    useComposerDraftPersistence()
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
  const editRewriteInFlightRef = useRef<Set<string>>(new Set())
  const editRewriteAttemptsRef = useRef<Map<string, EditRewriteAttempt>>(new Map())
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
  const collaborationSnapshot = useOptionalCollaborationStore(activeConversation?.id ?? null)
  const collaborationTreeCandidate = collaborationSnapshot?.tree ?? null
  const collaborationTree =
    collaborationTreeCandidate?.rootConversationId === activeConversation?.id
      ? collaborationTreeCandidate
      : null
  const collaborationChildren = useMemo(
    () => collaborationTree?.agents.filter((agent) => agent.parentAgentId !== null) ?? [],
    [collaborationTree]
  )
  const collaborationApprovals = useCollaborationApprovals({
    enabled: collaborationChildren.length > 0,
    invalidationSequence: collaborationTree?.lastSequence ?? 0,
    rootConversationId: collaborationTree?.rootConversationId ?? null
  })
  const activeDraftId = activeConversation?.id ?? NEW_CONVERSATION_DRAFT_ID
  const previousActiveDraftIdRef = useRef(activeDraftId)
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
      const conversationId = activeConversation?.id
      if (!projectId || !conversationId) return
      rightSidebarReviewNavigationRequestIdRef.current += 1
      setRightSidebarReviewNavigationRequest({
        kind: 'git-review',
        projectId,
        requestId: rightSidebarReviewNavigationRequestIdRef.current,
        target: { kind: 'lastTurn', conversationId },
        ...(filePath ? { filePath } : {})
      })
      openRightSidebar()
    },
    [activeConversation?.id, openRightSidebar, rightSidebarWorkspaceProject?.id]
  )
  const openAgentCenter = useCallback(
    (agentId: string) => {
      const rootConversationId = activeConversation?.id
      if (!rootConversationId) return
      rightSidebarAgentNavigationRequestIdRef.current += 1
      setRightSidebarAgentNavigationRequest({
        agentId,
        requestId: rightSidebarAgentNavigationRequestIdRef.current,
        rootConversationId
      })
      openRightSidebar()
    },
    [activeConversation?.id, openRightSidebar]
  )
  const renderAgentObserver = useCallback(
    ({ agent, agentLabelsById, invalidationVersion, rootConversationId }) => (
      <AgentObserverConversationSurface
        agent={agent}
        agentLabelsById={agentLabelsById}
        invalidationVersion={invalidationVersion}
        rootConversationId={rootConversationId}
        showTokenUsageDetails={uiPreferences.showTokenUsageDetails}
      />
    ),
    [uiPreferences.showTokenUsageDetails]
  )
  const {
    approvals: projectedCollaborationApprovals,
    decide: decideCollaborationApproval,
    error: collaborationApprovalError,
    refresh: refreshCollaborationApprovals
  } = collaborationApprovals
  const collaborationContent = useMemo(
    () =>
      collaborationChildren.length > 0 ? (
        <CollaborationApprovalPanel
          approvals={projectedCollaborationApprovals}
          loadError={collaborationApprovalError}
          mode="interactive"
          onDecision={decideCollaborationApproval}
          onOpenAgent={openAgentCenter}
          onRetryLoad={() => void refreshCollaborationApprovals()}
        />
      ) : null,
    [
      collaborationApprovalError,
      collaborationChildren,
      decideCollaborationApproval,
      openAgentCenter,
      projectedCollaborationApprovals,
      refreshCollaborationApprovals
    ]
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

  const {
    cleanupRunBinding,
    flushRunMessagePersistence,
    hydrateConversation,
    mutateDraft,
    persistDraftMessageOnly,
    removeQueuedMessageByClientId,
    requestAssistantResponse,
    restoreRejectedGuidance,
    restoreSubmittedSkills,
    scheduleStoppedRunReconciliation,
    setConversationsWithRef,
    setDraftsWithRef,
    updateAssistantMessage,
    updateDraft,
    updateUiPreferences
  } = useAppShellRuntime({
    activeConversationId,
    activeConversationIdRef,
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
  })
  const {
    cancelProviderTransitionConfirmation,
    confirmProviderTransition,
    loadProviderTransitionStatus,
    providerTransitionStore,
    retryProviderTransition,
    submitEditedLastUserMessage,
    submitMessage
  } = useAppShellMessageSubmission({
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
  })

  const handleActiveConversationArchived = useCallback(() => {
    // Archiving changes navigation only. Existing Run/provider bindings remain alive so a task
    // already in flight can durably settle in the background without being cancelled or retired.
    // The new-task composer is an independent persisted workspace and must not inherit or erase
    // state merely because another conversation was archived.
    setRightSidebarAgentNavigationRequest(null)
    activeConversationIdRef.current = null
    setActiveConversationId(null)
    setNewConversationPromptIndex((currentIndex) =>
      getNextRandomNewConversationPromptIndex(currentIndex)
    )
    setScrollTargetMessageId(null)
    setActiveConversationInitialScrollTop(null)
  }, [])

  const requestScheduledExit = useCallback(
    (proceed: () => void) => {
      if (primaryView !== 'scheduled') {
        proceed()
        return
      }

      scheduledExternalNavigationRequestKeyRef.current += 1
      setScheduledExternalNavigationRequest({
        proceed: () => {
          setScheduledExternalNavigationRequest(null)
          proceed()
        },
        requestKey: scheduledExternalNavigationRequestKeyRef.current
      })
    },
    [primaryView]
  )

  const openNewConversation = useCallback(
    (projectId: string | null = null) => {
      requestScheduledExit(() => {
        const isAlreadyShowingRootNewConversation =
          primaryView === 'conversation' &&
          activeConversationIdRef.current === null &&
          projectId === null &&
          (draftsRef.current[NEW_CONVERSATION_DRAFT_ID]?.projectId ?? null) === null

        conversationOpenRequestKeyRef.current += 1
        setPrimaryView('conversation')
        setScheduledOpenRequest(null)
        activeConversationIdRef.current = null
        setActiveConversationId(null)
        if (!isAlreadyShowingRootNewConversation) {
          setNewConversationPromptIndex((currentIndex) =>
            getNextRandomNewConversationPromptIndex(currentIndex)
          )
        }
        setScrollTargetMessageId(null)
        setActiveConversationInitialScrollTop(null)

        // Returning through the global New Conversation entry restores the existing draft
        // verbatim. A project-specific entry is an explicit scope change, so only that scope is
        // updated; the user's text, model, attachments and other draft choices remain intact.
        if (projectId !== null) {
          const currentDraft = draftsRef.current[NEW_CONVERSATION_DRAFT_ID]
          if (currentDraft?.projectId !== projectId) {
            mutateDraft(NEW_CONVERSATION_DRAFT_ID, (draft) => ({
              ...draft,
              projectId,
              skills: retainGlobalSkillSelections(draft.skills)
            }))
          }
        }
      })
    },
    [mutateDraft, primaryView, requestScheduledExit]
  )

  const {
    archiveConversation,
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
      archiveFailed: t('conversation.archiveFailed'),
      continueInNewTaskFailed: t('chat.continueInNewTaskFailed'),
      originArchived: t('chat.continuationOriginArchived'),
      originMissing: t('chat.continuationOriginMissing'),
      originOpenFailed: t('chat.continuationOriginOpenFailed')
    },
    onActiveConversationArchived: handleActiveConversationArchived,
    persistDraftNow,
    setActiveConversationId,
    setActiveConversationInitialScrollTop,
    setConversationScrollToBottomSignal,
    setConversationsWithRef,
    setDraftsWithRef,
    setScrollTargetMessageId,
    setSettingsOpen,
    showToast,
    waitForConversationSaves
  })

  const openScheduled = useCallback(() => {
    conversationOpenRequestKeyRef.current += 1
    setScheduledExternalNavigationRequest(null)
    setScheduledOpenRequest(null)
    setPrimaryView('scheduled')
  }, [])

  const commitOpenConversationFromScheduled = useCallback(
    (conversationId: string, messageId?: string | null) => {
      const requestKey = conversationOpenRequestKeyRef.current + 1
      conversationOpenRequestKeyRef.current = requestKey
      const currentConversation = conversationsRef.current.find(
        (conversation) => conversation.id === conversationId
      )
      const loadSelectedConversation =
        currentConversation && currentConversation.messagesLoaded !== false
          ? Promise.resolve(currentConversation)
          : hydrateConversation(conversationId)

      void loadSelectedConversation.then((loadedConversation) => {
        if (conversationOpenRequestKeyRef.current !== requestKey) return
        if (!loadedConversation) {
          showToast(t('chat.conversationLoadFailed'))
          return
        }

        setScheduledExternalNavigationRequest(null)
        setScheduledOpenRequest(null)
        setPrimaryView('conversation')
        selectConversation(conversationId, messageId, loadedConversation)
      })
    },
    [hydrateConversation, selectConversation, showToast, t]
  )

  const requestOpenConversationFromScheduled = useCallback(
    (conversationId: string, messageId?: string | null) => {
      if (primaryView !== 'scheduled') {
        // Ordinary sidebar navigation activates the metadata row immediately. The navigation
        // hook then hydrates its detail in place, preserving both the loading surface and a
        // single loaded+active lifecycle transition for Host-owned Session reconciliation.
        selectConversation(conversationId, messageId)
        return
      }
      requestScheduledExit(() => commitOpenConversationFromScheduled(conversationId, messageId))
    },
    [commitOpenConversationFromScheduled, primaryView, requestScheduledExit, selectConversation]
  )

  useEffect(() => {
    // Older isolated Renderer test hosts do not expose the additive Automation surface.
    if (!hostClient.automations?.onOpenRequested) return
    return hostClient.automations.onOpenRequested((request) => {
      if (request.destination.kind === 'conversation') {
        requestOpenConversationFromScheduled(
          request.destination.conversationId,
          request.destination.messageId
        )
        return
      }

      scheduledOpenRequestKeyRef.current += 1
      conversationOpenRequestKeyRef.current += 1
      setScheduledExternalNavigationRequest(null)
      setScheduledOpenRequest({
        automationId: request.automationId,
        requestKey: scheduledOpenRequestKeyRef.current,
        runId: request.runId
      })
      setPrimaryView('scheduled')
    })
  }, [requestOpenConversationFromScheduled])

  useEffect(
    () =>
      onNotificationOpenRequested((request) => {
        void markNotificationsSeen({ kind: 'events', eventIds: request.eventIds }).catch(
          () => undefined
        )
        if (request.destination.kind === 'conversation') {
          closeSettings()
          requestOpenConversationFromScheduled(
            request.destination.conversationId,
            request.destination.messageId
          )
          return
        }

        if (request.destination.kind === 'automation') {
          closeSettings()
          scheduledOpenRequestKeyRef.current += 1
          conversationOpenRequestKeyRef.current += 1
          setScheduledExternalNavigationRequest(null)
          setScheduledOpenRequest({
            automationId: request.destination.automationId,
            requestKey: scheduledOpenRequestKeyRef.current,
            runId: request.destination.runId
          })
          setPrimaryView('scheduled')
        }
      }),
    [closeSettings, requestOpenConversationFromScheduled]
  )

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
    discardDraft,
    draftsRef,
    editSubmissionSeqRef,
    enqueueChatMessageStateSave,
    pendingConversationSavesRef,
    pendingMessageSavesRef,
    pendingMessageUpsertsRef,
    persistDraftNow,
    removeFailedMessage: t('project.removeFailed'),
    resumeDraft,
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

  const { guideQueuedMessage, stopActiveGeneration } = useAppShellRunControls({
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
    scheduleStoppedRunReconciliation,
    stopRequestedPendingMessageIdsRef,
    stopRequestedRunIdsRef,
    t,
    updateAssistantMessage
  })

  const { handleApproveAgentAction, handleCancelAgentAction, handleRejectAgentAction } =
    useAgentActionDecisionHandlers({
      activeConversationId,
      conversationsRef,
      flushMessagePersistence: flushRunMessagePersistence,
      updateAssistantMessage
    })

  return (
    <AppShellWorkspace
      ref={shellRef}
      className="app-shell"
      data-left-open={leftOpen ? 'true' : 'false'}
      data-macos-window-controls={HAS_MACOS_WINDOW_CONTROLS ? 'true' : undefined}
      data-native-font-smoothing={
        SUPPORTS_NATIVE_FONT_SMOOTHING && uiPreferences.nativeFontSmoothing ? 'true' : undefined
      }
      data-primary-view={primaryView}
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
            activeConversationId={getVisibleActiveConversationId(primaryView, activeConversationId)}
            conversations={conversations}
            projects={projects}
            uiPreferences={uiPreferences}
            onArchiveAllProjectConversations={() => {
              const projectIds = new Set(projects.map((project) => project.id))
              void archiveConversations(
                (conversation) =>
                  Boolean(conversation.projectId) && projectIds.has(conversation.projectId!)
              )
            }}
            onArchiveAllRootConversations={() => {
              const projectIds = new Set(projects.map((project) => project.id))
              void archiveConversations(
                (conversation) => !conversation.projectId || !projectIds.has(conversation.projectId)
              )
            }}
            onArchiveConversation={(conversationId) => void archiveConversation(conversationId)}
            onArchiveProjectConversations={(projectId) => {
              void archiveConversations((conversation) => conversation.projectId === projectId)
            }}
            onMarkConversationUnread={(conversationId) =>
              patchConversation(conversationId, { unreadAt: Date.now() })
            }
            onNewConversation={openNewConversation}
            onNewProject={selectProjectDirectory}
            onOpenSettings={() => openSettings('general')}
            onRemoveProject={removeProject}
            onRenameConversation={(conversationId, title) =>
              patchConversation(conversationId, { title })
            }
            onRenameProject={renameProject}
            onOpenScheduled={openScheduled}
            onSelectConversation={requestOpenConversationFromScheduled}
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
            scheduledAttentionCount={scheduledAttentionCount}
            scheduledSelected={primaryView === 'scheduled'}
          />
        </div>
      </aside>

      {leftOpen && (
        <ResizeHandle
          metrics={leftResizeMetrics}
          onCollapse={toggleLeftSidebar}
          onResizeCommit={commitSidebarResize}
          resizeTargetRef={shellRef}
          side="left"
        />
      )}

      <AppShellCoveredRegion
        as="main"
        className="main-panel"
        aria-label={t('app.mainWorkspace')}
        covered={primaryView === 'scheduled'}
      >
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
                collaborationContent={collaborationContent}
                collaborationTimelineActivities={collaborationSnapshot?.activities ?? []}
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
                onModelTransitionCancel={cancelActiveProviderTransition}
                onModelTransitionConfirm={confirmActiveProviderTransition}
                onModelTransitionRetry={retryActiveProviderTransition}
                onOpenCollaborationAgent={openAgentCenter}
                onEditLastUserMessage={submitEditedLastUserMessage}
                onContinueInNewTask={(forkPoint) =>
                  continueInNewTask(activeConversation.id, forkPoint)
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
              promptIndex={newConversationPromptIndex}
              skillCatalogRefreshToken={activeSkillCatalogRefreshToken}
              onDraftChange={(draft) => updateDraft(NEW_CONVERSATION_DRAFT_ID, draft)}
              onDraftMessageChange={(draft) =>
                persistDraftMessageOnly(NEW_CONVERSATION_DRAFT_ID, draft)
              }
              onSubmitMessage={submitMessage}
            />
          )}
        </div>
      </AppShellCoveredRegion>

      {primaryView === 'conversation' && rightOpen && !rightMaximized && (
        <ResizeHandle
          metrics={rightResizeMetrics}
          onCollapse={toggleRightSidebar}
          onResizeCommit={commitSidebarResize}
          resizeTargetRef={shellRef}
          side="right"
        />
      )}

      <AppShellCoveredRegion
        as="aside"
        className="side-panel side-panel--right"
        covered={primaryView === 'scheduled'}
      >
        <RightSidebar
          activeConversationId={activeConversation?.id}
          agentNavigationRequest={rightSidebarAgentNavigationRequest}
          capabilities={rightSidebarCapabilities}
          browserSurfaceCommand={browserSurfaceBridge.command}
          collaborationSnapshot={collaborationSnapshot}
          isMaximized={rightMaximized}
          isOpen={rightOpen}
          isWorkspaceVisible={!settingsOpen}
          workspaceKey={rightSidebarWorkspaceProject?.id}
          workspaceKeys={rightSidebarWorkspaceKeys}
          workspaceName={rightSidebarWorkspaceProject?.name}
          workspacePath={rightSidebarWorkspacePath}
          onToggleMaximized={toggleRightSidebarMaximized}
          onBrowserSurfaceReady={browserSurfaceBridge.surfaceReady}
          onOpenAgentTemplates={() => openSettings('agentTemplates')}
          onOpenBrowserSettings={(destination) =>
            openSettings(
              'browser',
              destination === 'downloads'
                ? 'downloadHistory'
                : destination === 'history'
                  ? 'history'
                  : 'settings'
            )
          }
          reviewNavigationRequest={rightSidebarReviewNavigationRequest}
          renderAgentObserver={renderAgentObserver}
          maximizedToolbarControls={rightSidebarMaximizedToolbarControls}
        />
      </AppShellCoveredRegion>
      {primaryView === 'scheduled' && (
        <>
          <ScheduledPageLayer
            conversations={conversations}
            defaultModelId={activeDraftSelectedModel?.id ?? activeDraft.modelId ?? null}
            defaultPermissionMode={activeDraft.permissionMode}
            defaultProjectId={activeDraft.projectId}
            externalNavigationRequest={scheduledExternalNavigationRequest ?? undefined}
            initialPreferredDrawerWidth={scheduledDrawerPreferredWidth}
            models={enabledModels}
            openRequest={scheduledOpenRequest ?? undefined}
            onPreferredDrawerWidthChange={setScheduledDrawerPreferredWidth}
            permissionModeAvailability={permissionModeAvailability}
            projects={projects}
            onOpenConversation={commitOpenConversationFromScheduled}
            onOpenPermissionSettings={() => openSettings('general')}
          />
          {!leftOpen && (
            <PanelToggleButton
              className="panel-toggle panel-toggle--left scheduled-view__left-toggle"
              hasUnread={hasUnreadConversations}
              onClick={toggleLeftSidebar}
              open={false}
              side="left"
              t={t}
            />
          )}
        </>
      )}
      {settingsOpen &&
        createPortal(
          <AppShellSettingsView
            conversations={conversations}
            initialBrowserView={settingsInitialBrowserView}
            initialPage={settingsInitialPage}
            initialProjectId={rightSidebarWorkspaceProject?.id}
            onBack={closeSettings}
            onBeforeConversationDelete={discardDraft}
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
