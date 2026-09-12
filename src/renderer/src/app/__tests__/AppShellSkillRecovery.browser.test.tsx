import { HostInvocationError } from '@mycopilot/host-api'
import type {
  AgentCommandSessionGetOutput,
  AgentCommandSessionSnapshot,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  AgentEvent,
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation,
  AutomationEvent,
  AutomationOpenRequest,
  AutomationResync,
  NotificationOpenRequest,
  PendingAgentActionSnapshot,
  AgentSteerRunOutput,
  SkillSelection,
  StorageConversationForkPoint
} from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Activity } from 'react'
import { render } from 'vitest-browser-react'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin,
  ChatMessage,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../../features/chat/chatTypes'
import { CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS } from '../chatMessagePersistence'
import type { ComposerCommand } from '../../features/chat/components/ComposerCommands'
import type { RightSidebarModuleNavigationRequest } from '../../features/rightSidebar/rightSidebarTypes'

const testState = vi.hoisted(() => ({
  canOpenBottomPanel: true,
  gitStatus: 'unavailable' as 'available' | 'checking' | 'unavailable',
  openBottomPanel: vi.fn(),
  openRightSidebar: vi.fn(),
  automationEventListeners: new Set<(event: AutomationEvent) => void>(),
  automationOpenRequestListeners: new Set<(request: AutomationOpenRequest) => void>(),
  automationResyncListeners: new Set<(event: AutomationResync) => void>(),
  notificationOpenRequestListeners: new Set<(request: NotificationOpenRequest) => void>(),
  markNotificationsSeen: vi.fn(async () => undefined),
  cancelAgentRun: vi.fn(),
  collaborationRootIds: [] as Array<string | null>,
  deleteChatMessages: vi.fn(),
  forkConversation: vi.fn(),
  getContextWindowSnapshot: vi.fn(),
  getManualContextCompactionStatus: vi.fn(),
  getProviderTransitionStatus: vi.fn(),
  getAgentCommandSession: vi.fn(),
  listAgentCommandSessions: vi.fn(),
  listPendingAgentActions: vi.fn(),
  loadComposerDrafts: vi.fn(),
  loadConversation: vi.fn(),
  loadConversationMetas: vi.fn(),
  loadInputAttachments: vi.fn(),
  loadUiPreferences: vi.fn(),
  onAgentEvent: vi.fn(),
  onProviderTransition: vi.fn(),
  persistedConversations: new Map<string, ChatConversation>(),
  agentEventListeners: new Set<(event: AgentEvent) => void>(),
  providerTransitionListeners: new Set<(event: AgentProviderTransitionNotification) => void>(),
  providerTransitionSequence: 0,
  preflightProviderTransition: vi.fn(),
  saveChatMessageState: vi.fn(),
  saveComposerDraft: vi.fn(),
  saveComposerDraftMessage: vi.fn(),
  saveConversationMeta: vi.fn(),
  showToast: vi.fn(),
  rewriteConversationTurn: vi.fn(),
  openEditProjectDialog: vi.fn(),
  deleteProject: vi.fn(),
  startConversationTurn: vi.fn(),
  startProviderTransition: vi.fn(),
  steerAgentRun: vi.fn(),
  upsertChatMessages: vi.fn(),
  enabledModels: [
    {
      id: 'model-1',
      displayName: 'Model One',
      supportsImage: true,
      inputPrice: '0',
      outputPrice: '0',
      enabled: true
    },
    {
      id: 'model-2',
      displayName: 'Model Two',
      supportsImage: true,
      inputPrice: '0',
      outputPrice: '0',
      enabled: true
    }
  ],
  projects: [
    {
      id: 'project-a',
      name: 'Project A',
      folders: [
        {
          id: 'project-a-primary',
          path: '/workspace/a',
          alias: 'a',
          role: 'primary' as const,
          sortOrder: 0,
          createdAt: 1
        }
      ],
      createdAt: 1
    }
  ]
}))

const frontendConfig = vi.hoisted(() => ({ t: (key: string) => key }))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => frontendConfig
}))

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: testState.enabledModels,
    models: testState.enabledModels
  })
}))

vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({
    projects: testState.projects,
    deleteProject: testState.deleteProject,
    openCreateProjectDialog: vi.fn(async () => null),
    openEditProjectDialog: testState.openEditProjectDialog,
    showProjectInFolder: vi.fn(),
    showProjectFolder: vi.fn(),
    togglePinProject: vi.fn()
  })
}))

vi.mock('../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: testState.showToast })
}))

vi.mock('../useShellLayout', () => ({
  useShellLayout: () => ({
    canOpenBottomPanel: testState.canOpenBottomPanel,
    openBottomPanel: testState.openBottomPanel,
    openRightSidebar: testState.openRightSidebar,
    commitSidebarResize: vi.fn(),
    leftResizeMetrics: { maximum: 420, minimum: 220, width: 0 },
    leftOpen: false,
    leftWidth: 0,
    rightMaximized: false,
    rightOpen: false,
    rightResizeMetrics: { maximum: 1200, minimum: 280, width: 0 },
    rightWidth: 0,
    shellRef: { current: null },
    toggleLeftSidebar: vi.fn(),
    toggleRightSidebar: vi.fn(),
    toggleRightSidebarMaximized: vi.fn()
  })
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onPromptPreferencesChanged: vi.fn(() => () => undefined),
      onCollaborationSettingsChanged: vi.fn(() => () => undefined)
    },
    app: {
      getWindowState: vi.fn().mockResolvedValue({ isFullScreen: false, isMaximized: false }),
      onWindowStateChange: vi.fn(() => () => undefined)
    },
    automations: {
      onEvent: vi.fn((listener: (event: AutomationEvent) => void) => {
        testState.automationEventListeners.add(listener)
        return () => testState.automationEventListeners.delete(listener)
      }),
      onOpenRequested: vi.fn((listener: (request: AutomationOpenRequest) => void) => {
        testState.automationOpenRequestListeners.add(listener)
        return () => testState.automationOpenRequestListeners.delete(listener)
      }),
      onResync: vi.fn((listener: (event: AutomationResync) => void) => {
        testState.automationResyncListeners.add(listener)
        return () => testState.automationResyncListeners.delete(listener)
      })
    }
  }
}))

vi.mock('../../features/gitReview/useGitRepositoryCapability', () => ({
  useGitRepositoryCapability: (id?: string, path?: string) => ({
    contextKey: JSON.stringify([id ?? null, path ?? null]),
    status: testState.gitStatus
  })
}))

// This suite is the legacy/no-child AppShell baseline. Collaboration is covered by its own
// root-scoped integration tests and must not alter the old single-Agent fixture.
vi.mock('../../features/agentCollaboration/useCollaborationStore', () => ({
  useOptionalCollaborationStore: (rootConversationId: string | null) => {
    testState.collaborationRootIds.push(rootConversationId)
    return null
  }
}))

vi.mock('../../features/agent/agentClient', () => ({
  approveAgentAction: vi.fn(),
  cancelAgentAction: vi.fn(),
  cancelAgentRun: testState.cancelAgentRun,
  getContextWindowSnapshot: testState.getContextWindowSnapshot,
  getManualContextCompactionStatus: testState.getManualContextCompactionStatus,
  onManualContextCompaction: vi.fn(() => () => undefined),
  startManualContextCompaction: vi.fn(),
  cancelManualContextCompaction: vi.fn(),
  getProviderTransitionStatus: testState.getProviderTransitionStatus,
  getAgentCommandSession: testState.getAgentCommandSession,
  getAgentFileChangeDiff: vi.fn(),
  getAgentFileChangeHistoryDiff: vi.fn(),
  listAgentCommandSessions: testState.listAgentCommandSessions,
  listPendingAgentActions: testState.listPendingAgentActions,
  onAgentEvent: testState.onAgentEvent,
  onProviderTransition: testState.onProviderTransition,
  preflightProviderTransition: testState.preflightProviderTransition,
  rejectAgentAction: vi.fn(),
  readAgentFileChange: vi.fn(),
  rewriteConversationTurn: testState.rewriteConversationTurn,
  startConversationTurn: testState.startConversationTurn,
  startProviderTransition: testState.startProviderTransition,
  steerAgentRun: testState.steerAgentRun
}))

vi.mock('../../features/storage/storageClient', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../features/storage/storageClient')>()
  return {
    ...original,
    deleteChatMessages: testState.deleteChatMessages,
    forkConversation: testState.forkConversation,
    loadComposerDrafts: testState.loadComposerDrafts,
    loadConversation: testState.loadConversation,
    loadConversationMetas: testState.loadConversationMetas,
    loadInputAttachments: testState.loadInputAttachments,
    loadUiPreferences: testState.loadUiPreferences,
    saveChatMessageState: testState.saveChatMessageState,
    saveComposerDraft: testState.saveComposerDraft,
    saveComposerDraftMessage: testState.saveComposerDraftMessage,
    saveConversationMeta: testState.saveConversationMeta,
    saveUiPreferences: vi.fn(),
    upsertChatMessages: testState.upsertChatMessages
  }
})

vi.mock('../../components/layout/ResizeHandle', () => ({ ResizeHandle: () => null }))
vi.mock('../../features/automations/useAutomationAttention', () => ({
  useAutomationAttention: () => ({ unreadCount: 4 })
}))
vi.mock('../../features/notifications/notificationClient', () => ({
  markNotificationsSeen: testState.markNotificationsSeen,
  onNotificationOpenRequested: (listener: (request: NotificationOpenRequest) => void) => {
    testState.notificationOpenRequestListeners.add(listener)
    return () => testState.notificationOpenRequestListeners.delete(listener)
  }
}))
vi.mock('../../features/automations/ScheduledPageLayer', () => ({
  ScheduledPageLayer: ({
    externalNavigationRequest,
    initialPreferredDrawerWidth,
    onPreferredDrawerWidthChange,
    openRequest
  }: {
    externalNavigationRequest?: { proceed: () => void; requestKey: number }
    initialPreferredDrawerWidth?: number
    onPreferredDrawerWidthChange?: (width: number) => void
    openRequest?: { automationId: string; runId: string | null }
  }) => (
    <section data-testid="scheduled-page-layer">
      <output data-testid="scheduled-open-automation">{openRequest?.automationId ?? 'none'}</output>
      <output data-testid="scheduled-open-run">{openRequest?.runId ?? 'none'}</output>
      <output data-testid="scheduled-drawer-preferred-width">
        {initialPreferredDrawerWidth ?? 'none'}
      </output>
      <button type="button" onClick={() => onPreferredDrawerWidthChange?.(560)}>
        resize-scheduled-drawer
      </button>
      <output data-testid="external-navigation-request-key">
        {externalNavigationRequest?.requestKey ?? 'none'}
      </output>
      <button type="button" onClick={() => externalNavigationRequest?.proceed()}>
        confirm-external-navigation
      </button>
    </section>
  )
}))
vi.mock('../shell/sidebar/LeftSidebar', () => ({
  LeftSidebar: ({
    activeConversationId,
    conversations,
    onArchiveConversation,
    onNewConversation,
    onOpenSettings,
    onOpenScheduled,
    onRenameConversation,
    onSelectConversation,
    scheduledAttentionCount,
    scheduledSelected,
    uiPreferences
  }: {
    activeConversationId: string | null
    conversations: ChatConversation[]
    onArchiveConversation: (conversationId: string) => void
    onNewConversation: (projectId?: string | null) => void
    onOpenSettings: () => void
    onOpenScheduled: () => void
    onRenameConversation: (conversationId: string, title: string) => void
    onSelectConversation: (conversationId: string) => void
    scheduledAttentionCount: number
    scheduledSelected: boolean
    uiPreferences: { translucentSidebar: boolean }
  }) => (
    <div>
      <output data-testid="sidebar-translucent">{String(uiPreferences.translucentSidebar)}</output>
      <output data-testid="sidebar-active-conversation">{activeConversationId ?? 'none'}</output>
      <output data-testid="scheduled-selected">{String(scheduledSelected)}</output>
      <output data-testid="scheduled-attention">{scheduledAttentionCount}</output>
      <button type="button" onClick={onOpenScheduled}>
        open-scheduled
      </button>
      <button type="button" onClick={onOpenSettings}>
        open-settings
      </button>
      <button type="button" onClick={() => onNewConversation(null)}>
        new-conversation
      </button>
      <button type="button" onClick={() => onNewConversation('project-a')}>
        new-conversation-project-a
      </button>
      {conversations.map((conversation: ChatConversation) => (
        <div key={conversation.id}>
          <button type="button" onClick={() => onSelectConversation(conversation.id)}>
            select-{conversation.id}
          </button>
          <button type="button" onClick={() => onArchiveConversation(conversation.id)}>
            archive-{conversation.id}
          </button>
          <button
            type="button"
            onClick={() => onRenameConversation(conversation.id, `renamed-${conversation.id}`)}
          >
            rename-{conversation.id}
          </button>
          <output data-testid={`title-${conversation.id}`}>{conversation.title}</output>
          <output data-testid={`archived-${conversation.id}`}>
            {conversation.archivedAt ? 'true' : 'false'}
          </output>
          <output data-testid={`archive-pending-${conversation.id}`}>
            {conversation.pendingArchivedAt === undefined ? 'false' : 'true'}
          </output>
        </div>
      ))}
    </div>
  )
}))
vi.mock('../../features/rightSidebar/RightSidebar', () => ({
  RightSidebar: ({
    activeConversationId,
    moduleNavigationRequest
  }: {
    activeConversationId?: string | null
    moduleNavigationRequest?: RightSidebarModuleNavigationRequest | null
  }) => (
    <>
      <output data-testid="right-sidebar-conversation-id">{activeConversationId ?? 'none'}</output>
      <output data-testid="right-module-request">{JSON.stringify(moduleNavigationRequest)}</output>
    </>
  )
}))
vi.mock('../../features/bottomPanel/BottomPanel', () => ({
  BottomPanel: ({
    moduleNavigationRequest
  }: {
    moduleNavigationRequest?: RightSidebarModuleNavigationRequest | null
  }) => (
    <output data-testid="bottom-module-request">{JSON.stringify(moduleNavigationRequest)}</output>
  )
}))
vi.mock('../AppShellSettingsView', () => ({
  AppShellSettingsView: () => <section data-testid="settings-view" />
}))
vi.mock('../../features/chat/NewConversationPage', () => ({
  NewConversationPage: ({
    draft,
    promptIndex
  }: {
    draft: ChatComposerDraft
    promptIndex: number
  }) => (
    <div>
      <output data-testid="new-conversation-draft">{draft.message}</output>
      <output data-testid="new-conversation-project">{draft.projectId ?? 'none'}</output>
      <output data-testid="new-conversation-prompt-index">{promptIndex}</output>
      <output data-testid="new-conversation-payload">
        {JSON.stringify({
          attachments: draft.attachments,
          message: draft.message,
          modelId: draft.modelId,
          permissionMode: draft.permissionMode,
          projectId: draft.projectId,
          skills: draft.skills
        })}
      </output>
    </div>
  )
}))
vi.mock('../../features/chat/ChatConversationPage', () => ({
  ChatConversationPage: ({
    commands,
    composerDraft,
    conversation,
    onComposerDraftChange,
    onContinueInNewTask,
    onEditLastUserMessage,
    onGuideQueuedMessage,
    onToggleQueueAutoSend,
    queueAutoSendEnabled,
    onModelTransitionCancel,
    onModelTransitionConfirm,
    onModelTransitionRetry,
    onOpenContinuationOrigin,
    onStopGenerating,
    onSubmitMessage,
    modelTransitionConfirmation,
    modelTransitionOperations,
    scrollTargetMessageId,
    skillCatalogRefreshToken
  }: {
    commands?: readonly ComposerCommand[]
    composerDraft: ChatComposerDraft
    conversation: ChatConversation
    onComposerDraftChange: (draft: ChatComposerDraft) => void
    onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void
    onEditLastUserMessage: (messageId: string, content: string) => Promise<void>
    onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
    onToggleQueueAutoSend?: () => void
    queueAutoSendEnabled?: boolean
    onModelTransitionCancel?: () => void
    onModelTransitionConfirm?: () => void
    onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void
    onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void
    onStopGenerating: () => void
    onSubmitMessage: (message: string, options: ChatSubmitOptions) => void | Promise<boolean | void>
    modelTransitionConfirmation?: { reason: string }
    modelTransitionOperations?: AgentProviderTransitionOperation[]
    scrollTargetMessageId?: string | null
    skillCatalogRefreshToken?: number
  }) => (
    <div>
      {commands?.map((command) => (
        <button
          key={command.id}
          type="button"
          disabled={Boolean(command.disabledReason)}
          title={command.disabledReason}
          onClick={() => {
            if (command.id !== 'capabilities' && command.id !== 'model') void command.execute()
          }}
        >
          command-{command.id}
        </button>
      ))}
      <output data-testid="draft-skills">
        {composerDraft.skills.map((selection) => `${selection.id}@${selection.revision}`).join(',')}
      </output>
      <output data-testid="skill-catalog-refresh-token">{skillCatalogRefreshToken ?? 0}</output>
      <output data-testid="last-assistant-status">
        {conversation.messages.at(-1)?.status ?? ''}
      </output>
      <output data-testid="agent-run-status">
        {conversation.messages.at(-1)?.agentRun?.status ?? ''}
      </output>
      <output data-testid="conversation-message-ids">
        {conversation.messages.map((message) => message.id).join(',')}
      </output>
      <output data-testid="conversation-message-contents">
        {conversation.messages.map((message) => message.content).join('|')}
      </output>
      <output data-testid="approval-count">
        {conversation.messages.at(-1)?.agentRun?.approvals.length ?? 0}
      </output>
      <output data-testid="active-conversation-id">{conversation.id}</output>
      <output data-testid="scroll-target-message-id">{scrollTargetMessageId ?? 'none'}</output>
      <output data-testid="conversation-updated-at">{conversation.updatedAt}</output>
      <output data-testid="draft-model-id">{composerDraft.modelId}</output>
      <output data-testid="draft-updated-at">{composerDraft.updatedAt}</output>
      <output data-testid="draft-message">{composerDraft.message}</output>
      <output data-testid="draft-payload">
        {JSON.stringify({
          attachments: composerDraft.attachments,
          message: composerDraft.message,
          modelId: composerDraft.modelId,
          permissionMode: composerDraft.permissionMode,
          projectId: composerDraft.projectId,
          skills: composerDraft.skills
        })}
      </output>
      <output data-testid="model-transition-confirmation">
        {modelTransitionConfirmation?.reason ?? ''}
      </output>
      <output data-testid="model-transition-operations">
        {(modelTransitionOperations ?? [])
          .map((operation) => `${operation.operationId}:${operation.status}`)
          .join(',')}
      </output>
      <output data-testid="activated-skill-ids">
        {conversation.messages
          .at(-1)
          ?.agentRun?.activatedSkills?.map((skill) => skill.id)
          .join(',') ?? ''}
      </output>
      <output data-testid="queue-auto-send-enabled">{String(queueAutoSendEnabled ?? false)}</output>
      <button type="button" onClick={onToggleQueueAutoSend}>
        toggle-queue-auto-send
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            queuedMessages: [
              ...composerDraft.queuedMessages,
              queuedMessage('queue-late', 'message queued after completion', 40)
            ]
          })
        }
      >
        append-queued-message
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            queuedMessages: [...composerDraft.queuedMessages].reverse()
          })
        }
      >
        reverse-queued-messages
      </button>
      <output data-testid="queued-message-ids">
        {composerDraft.queuedMessages.map((message) => message.id).join(',')}
      </output>
      <output data-testid="queued-message-payloads">
        {JSON.stringify(composerDraft.queuedMessages)}
      </output>
      <output data-testid="guidance-timeline">
        {conversation.messages
          .at(-1)
          ?.agentRun?.timeline.filter((item) => item.type === 'user_guidance')
          .map((item) => `${item.clientMessageId}:${item.status}`)
          .join(',') ?? ''}
      </output>
      <output data-testid="command-sessions">
        {JSON.stringify(conversation.messages.at(-1)?.agentRun?.commandSessions ?? {})}
      </output>
      <output data-testid="llm-retry">
        {JSON.stringify(conversation.messages.at(-1)?.agentRun?.llmRetry ?? {})}
      </output>
      <output data-testid="command-output">
        {conversation.messages
          .at(-1)
          ?.agentRun?.commandOutputPreviews?.['command-call']?.chunks.map((chunk) => chunk.output)
          .join('') ?? ''}
      </output>
      <button
        type="button"
        onClick={() =>
          void onEditLastUserMessage(conversation.messages.at(-2)?.id ?? '', 'edited').catch(
            () => undefined
          )
        }
      >
        edit-last-message
      </button>
      <button type="button" onClick={onStopGenerating}>
        stop-generating
      </button>
      <button
        type="button"
        onClick={() =>
          onContinueInNewTask?.({
            kind: 'assistant_reply',
            assistantMessageId: conversation.messages.at(-1)?.id ?? ''
          })
        }
      >
        continue-in-new-task
      </button>
      {conversation.continuationOrigin && (
        <button
          type="button"
          onClick={() => onOpenContinuationOrigin?.(conversation.continuationOrigin!)}
        >
          open-continuation-origin
        </button>
      )}
      <button
        type="button"
        onClick={() => {
          const queuedMessage = composerDraft.queuedMessages[0]
          if (queuedMessage) onGuideQueuedMessage?.(queuedMessage)
        }}
      >
        guide-first-message
      </button>
      <button
        type="button"
        onClick={() => {
          const queuedMessage = composerDraft.queuedMessages[1]
          if (queuedMessage) onGuideQueuedMessage?.(queuedMessage)
        }}
      >
        guide-second-message
      </button>
      <button
        type="button"
        onClick={() =>
          onSubmitMessage('follow up', {
            modelId: composerDraft.modelId,
            permissionMode: 'full',
            projectId: 'project-a',
            skills: [skillSelection]
          })
        }
      >
        submit-with-skill
      </button>
      <button
        type="button"
        onClick={() =>
          onSubmitMessage('follow up', {
            modelId: composerDraft.modelId,
            permissionMode: 'full',
            projectId: 'project-a',
            skills: []
          })
        }
      >
        submit-without-skill
      </button>
      <button
        type="button"
        onClick={() =>
          onSubmitMessage(composerDraft.message, {
            attachments: composerDraft.attachments,
            modelId: composerDraft.modelId,
            permissionMode: composerDraft.permissionMode,
            projectId: composerDraft.projectId,
            skills: composerDraft.skills
          })
        }
      >
        submit-draft-content
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            modelId: 'model-2',
            updatedAt: Math.max(Date.now(), composerDraft.updatedAt + 1)
          })
        }
      >
        select-model-2
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            permissionMode: 'custom',
            updatedAt: Math.max(Date.now(), composerDraft.updatedAt + 1)
          })
        }
      >
        select-custom-permissions
      </button>
      <button type="button" onClick={onModelTransitionCancel}>
        cancel-model-transition
      </button>
      <button type="button" onClick={onModelTransitionConfirm}>
        confirm-model-transition
      </button>
      <button
        type="button"
        onClick={() => {
          const operation = modelTransitionOperations?.find(
            (candidate) => candidate.status === 'failed'
          )
          if (operation) onModelTransitionRetry?.(operation)
        }}
      >
        retry-model-transition
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            skills: [latestSkillSelection],
            updatedAt: Date.now()
          })
        }
      >
        select-latest-skill
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            message: 'new draft after failure',
            updatedAt: Date.now()
          })
        }
      >
        edit-composer-after-transition-failure
      </button>
    </div>
  )
}))

const [{ AppShell }, { createComposerDraft, createConversationTitle }, { defaultUiPreferences }] =
  await Promise.all([
    import('../AppShell'),
    import('../chatMessageFactory'),
    import('../../features/storage/storageClient')
  ])

const skillSelection: SkillSelection = {
  id: 'workspace:project-a:repository-auditor',
  revision: 'skill-sha256-v1:auditor'
}

const latestSkillSelection: SkillSelection = {
  ...skillSelection,
  revision: 'skill-sha256-v1:auditor-latest'
}

const explicitSkillSummary = {
  ...skillSelection,
  name: 'Repository auditor',
  source: { id: 'project-a', kind: 'workspace' as const }
}

const modelSkillSummary = {
  id: 'bundled:application:documents',
  name: 'documents',
  revision: 'skill-package-sha256-v2:documents',
  source: { id: 'bundled:application', kind: 'bundled' as const }
}

function successfulTurnOutput(
  input: AgentConversationTurnInput,
  runSequence: number
): AgentConversationTurnOutput {
  const conversationId = input.conversationId ?? 'conversation-a'
  const userMessageId = input.userMessageId ?? `user-${runSequence}`
  const assistantMessageId = input.assistantMessageId ?? `assistant-${runSequence}`
  return {
    runId: `run-${runSequence}`,
    eventName: 'agent.event',
    conversationId,
    userMessageId,
    assistantMessageId,
    userMessage: {
      id: userMessageId,
      role: 'user',
      content: input.content,
      createdAt: 10 + runSequence,
      status: 'sent'
    },
    assistantMessage: {
      id: assistantMessageId,
      role: 'assistant',
      content: '',
      createdAt: 20 + runSequence,
      status: 'pending'
    },
    activatedSkills: input.skills?.length ? [explicitSkillSummary] : [],
    skillActivationRevision: input.skills?.length ? 'activation-sha256-v1:explicit' : undefined
  }
}

function commitSuccessfulRewrite(
  input: AgentConversationTurnRewriteInput,
  runSequence = 100
): AgentConversationTurnOutput {
  const output = successfulTurnOutput(input.turn, runSequence)
  const conversationId = input.turn.conversationId!
  const current = testState.persistedConversations.get(conversationId)
  if (!current) throw new Error('missing rewrite conversation fixture')
  const sourceUserIndex = current.messages.findIndex(
    (message) => message.id === input.sourceUserMessageId
  )
  const sourceAssistantIndex = current.messages.findIndex(
    (message) => message.id === input.sourceAssistantMessageId
  )
  if (sourceUserIndex < 0 || sourceAssistantIndex !== sourceUserIndex + 1) {
    throw new Error('invalid rewrite source fixture')
  }
  testState.persistedConversations.set(conversationId, {
    ...current,
    modelId: input.turn.modelId,
    title: input.turn.title ?? current.title,
    messages: [
      ...current.messages.slice(0, sourceUserIndex),
      {
        id: output.userMessage.id,
        role: 'user',
        content: output.userMessage.content,
        createdAt: output.userMessage.createdAt,
        status: 'sent'
      },
      {
        id: output.assistantMessage.id,
        role: 'assistant',
        content: output.assistantMessage.content,
        createdAt: output.assistantMessage.createdAt,
        status: 'pending',
        agentRun: {
          runId: output.runId,
          status: 'running',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: []
        }
      },
      ...current.messages.slice(sourceAssistantIndex + 1)
    ],
    updatedAt: Math.max(
      current.updatedAt + 1,
      output.userMessage.createdAt,
      output.assistantMessage.createdAt
    )
  })
  return output
}

function mockSuccessfulTurnStarts() {
  let runSequence = 0
  testState.startConversationTurn.mockImplementation(
    async (input: AgentConversationTurnInput): Promise<AgentConversationTurnOutput> => {
      runSequence += 1
      return successfulTurnOutput(input, runSequence)
    }
  )
}

function emitAgentEvent(event: AgentEvent) {
  for (const listener of testState.agentEventListeners) listener(event)
}

function emitProviderTransition(event: AgentProviderTransitionNotification) {
  for (const listener of testState.providerTransitionListeners) listener(event)
}

function completedProviderTransition(
  targetModelId: string,
  overrides: Partial<
    Omit<Extract<AgentProviderTransitionOperation, { status: 'completed' }>, 'status'>
  > = {}
): Extract<AgentProviderTransitionOperation, { status: 'completed' }> {
  testState.providerTransitionSequence += 1
  return {
    schemaVersion: 1,
    operationId: `provider-transition-${testState.providerTransitionSequence}`,
    conversationId: 'conversation-a',
    targetModelId,
    startedAt: 10 + testState.providerTransitionSequence,
    completedAt: 20 + testState.providerTransitionSequence,
    conversationUpdatedAt: 30 + testState.providerTransitionSequence,
    ...overrides,
    status: 'completed',
    modelId: targetModelId
  }
}

function runningProviderTransition(
  targetModelId: string,
  overrides: Partial<
    Omit<Extract<AgentProviderTransitionOperation, { status: 'running' }>, 'status'>
  > = {}
): Extract<AgentProviderTransitionOperation, { status: 'running' }> {
  testState.providerTransitionSequence += 1
  return {
    schemaVersion: 1,
    operationId: `provider-transition-${testState.providerTransitionSequence}`,
    conversationId: 'conversation-a',
    targetModelId,
    coveredThroughMessageId: 'assistant-old',
    startedAt: 10 + testState.providerTransitionSequence,
    ...overrides,
    status: 'running'
  }
}

function failedProviderTransition(
  running: Extract<AgentProviderTransitionOperation, { status: 'running' }>
): Extract<AgentProviderTransitionOperation, { status: 'failed' }> {
  return {
    ...running,
    status: 'failed',
    completedAt: running.startedAt + 10,
    error: {
      code: 'provider_transition_generation_failed',
      message: 'The transition could not be completed.',
      recovery: 'retry'
    }
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}

function storedConversation(): ChatConversation {
  return {
    id: 'conversation-a',
    projectId: 'project-a',
    modelId: 'model-1',
    title: 'Audit repository',
    messages: [
      {
        id: 'user-old',
        role: 'user',
        content: 'original',
        createdAt: 1,
        status: 'sent'
      },
      {
        id: 'assistant-old',
        role: 'assistant',
        content: 'done',
        createdAt: 2,
        status: 'sent',
        agentRun: {
          runId: 'run-old',
          status: 'completed',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: [],
          activatedSkills: [
            {
              ...skillSelection,
              name: 'Repository auditor',
              source: { id: 'project-a', kind: 'workspace' }
            }
          ]
        }
      }
    ],
    createdAt: 1,
    updatedAt: 2,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null
  }
}

function storedConversationWithRun(
  assistantMessageId: string,
  runId: string,
  status: 'running' | 'completed' | 'failed' | 'cancelled'
): ChatConversation {
  const stored = storedConversation()
  return {
    ...stored,
    messages: [
      ...stored.messages,
      {
        id: assistantMessageId,
        role: 'assistant',
        content: status === 'completed' ? 'authoritative answer' : '',
        createdAt: 3,
        status: status === 'running' ? 'pending' : status === 'failed' ? 'error' : 'sent',
        agentRun: {
          runId,
          status,
          completedAt: status === 'running' ? undefined : 4,
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: []
        }
      }
    ]
  }
}

function storedConversationWithCommandRun(): ChatConversation {
  const stored = storedConversationWithRun('assistant-command', 'run-command', 'completed')
  const assistant = stored.messages.at(-1)
  if (!assistant?.agentRun) throw new Error('missing command run fixture')
  assistant.agentRun.toolCalls = [
    {
      id: 'command-call',
      tool: 'run_command',
      args: { command: 'python3 snake_game/main.py' },
      approvalStatus: 'approved',
      reason: null
    }
  ]
  assistant.agentRun.timeline = [
    { id: 'tool-call-command-call', type: 'tool_call', callId: 'command-call' }
  ]
  return stored
}

function commandSessionSnapshot(
  overrides: Partial<AgentCommandSessionSnapshot> = {}
): AgentCommandSessionSnapshot {
  return {
    schemaVersion: 2,
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    conversationId: 'conversation-a',
    assistantMessageId: 'assistant-command',
    originRunId: 'run-command',
    callId: 'command-call',
    projectId: 'project-a',
    command: 'python3 snake_game/main.py',
    cwd: '/workspace/a',
    commandDigest: `sha256:${'a'.repeat(64)}`,
    status: 'running',
    startedAt: 10,
    latestSequence: 0,
    outputTruncated: false,
    ...overrides
  }
}

function commandSessionGetOutput(
  snapshot: AgentCommandSessionSnapshot,
  output = ''
): AgentCommandSessionGetOutput {
  return {
    session: snapshot,
    transcript: {
      requestedAfterSequence: 0,
      firstAvailableSequence: output ? 1 : undefined,
      latestSequence: output ? 1 : 0,
      truncatedBefore: false,
      outputCaptureTruncated: false,
      chunks: output ? [{ sequence: 1, stream: 'stdout', output }] : []
    }
  }
}

function queuedMessage(id: string, content: string, createdAt: number): ChatQueuedMessage {
  return {
    id,
    clientMessageId: `client-${id}`,
    content,
    attachments: [],
    modelId: 'model-1',
    permissionMode: 'full',
    projectId: 'project-a',
    skills: [],
    status: 'pending',
    createdAt
  }
}

beforeEach(() => {
  testState.openEditProjectDialog.mockReset().mockResolvedValue('cancelled')
  testState.deleteProject.mockReset().mockResolvedValue(undefined)
  testState.canOpenBottomPanel = true
  testState.gitStatus = 'unavailable'
  testState.openBottomPanel.mockReset()
  testState.openRightSidebar.mockReset()
  testState.automationEventListeners.clear()
  testState.automationOpenRequestListeners.clear()
  testState.automationResyncListeners.clear()
  testState.notificationOpenRequestListeners.clear()
  testState.markNotificationsSeen.mockReset().mockResolvedValue(undefined)
  testState.cancelAgentRun.mockReset().mockResolvedValue(true)
  testState.collaborationRootIds.length = 0
  testState.deleteChatMessages.mockReset().mockResolvedValue(undefined)
  testState.forkConversation.mockReset()
  testState.getManualContextCompactionStatus.mockReset().mockResolvedValue({ operations: [] })
  testState.getContextWindowSnapshot.mockReset().mockResolvedValue({ modelConfigId: 'model-1' })
  testState.getProviderTransitionStatus.mockReset().mockResolvedValue({ operations: [] })
  testState.getAgentCommandSession.mockReset()
  testState.listAgentCommandSessions.mockReset().mockResolvedValue({ sessions: [] })
  testState.listPendingAgentActions.mockReset().mockResolvedValue([])
  testState.loadComposerDrafts.mockReset().mockResolvedValue({
    'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' })
  })
  const stored = storedConversation()
  testState.persistedConversations.clear()
  testState.persistedConversations.set(stored.id, stored)
  testState.loadConversation
    .mockReset()
    .mockImplementation(async (conversationId: string) =>
      testState.persistedConversations.get(conversationId)
    )
  testState.loadConversationMetas.mockReset().mockImplementation(async () =>
    [...testState.persistedConversations.values()].map((conversation) => ({
      ...conversation,
      messages: [],
      messagesLoaded: false
    }))
  )
  testState.loadInputAttachments.mockReset().mockResolvedValue([])
  testState.loadUiPreferences.mockReset().mockResolvedValue({
    ...defaultUiPreferences(),
    showContextWindowUsage: false
  })
  testState.saveChatMessageState.mockReset().mockResolvedValue(undefined)
  testState.agentEventListeners.clear()
  testState.providerTransitionListeners.clear()
  testState.providerTransitionSequence = 0
  testState.onAgentEvent.mockReset().mockImplementation((listener: (event: AgentEvent) => void) => {
    testState.agentEventListeners.add(listener)
    return () => testState.agentEventListeners.delete(listener)
  })
  testState.onProviderTransition
    .mockReset()
    .mockImplementation((listener: (event: AgentProviderTransitionNotification) => void) => {
      testState.providerTransitionListeners.add(listener)
      return () => testState.providerTransitionListeners.delete(listener)
    })
  testState.preflightProviderTransition
    .mockReset()
    .mockImplementation(
      async ({
        conversationId,
        targetModelId
      }: {
        conversationId: string
        targetModelId: string
      }) => ({
        conversationId,
        targetModelId,
        decision: 'compatible' as const,
        reason: 'same_protocol' as const,
        operationId: `provider-transition-${testState.providerTransitionSequence + 1}`,
        transitionToken: `token-${targetModelId}`
      })
    )
  testState.saveComposerDraft.mockReset().mockResolvedValue(undefined)
  testState.saveComposerDraftMessage.mockReset().mockResolvedValue(true)
  testState.saveConversationMeta
    .mockReset()
    .mockImplementation(async (conversation: ChatConversation) => {
      const storedConversation = testState.persistedConversations.get(conversation.id)
      if (storedConversation && conversation.updatedAt < storedConversation.updatedAt) {
        return
      }
      testState.persistedConversations.set(conversation.id, {
        ...storedConversation,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt ?? conversation.archivedAt,
        pendingArchivedAt: undefined,
        unreadAt: conversation.pendingArchivedAt === undefined ? conversation.unreadAt : null
      })
    })
  testState.showToast.mockReset()
  testState.rewriteConversationTurn
    .mockReset()
    .mockImplementation(async (input: AgentConversationTurnRewriteInput) =>
      commitSuccessfulRewrite(input)
    )
  testState.startConversationTurn.mockReset()
  testState.startProviderTransition
    .mockReset()
    .mockImplementation(async ({ targetModelId }: { targetModelId: string }) =>
      completedProviderTransition(targetModelId)
    )
  testState.steerAgentRun.mockReset().mockResolvedValue({
    guidanceId: 'guidance-1',
    status: 'queued'
  })
  testState.upsertChatMessages.mockReset().mockResolvedValue(undefined)
})

afterEach(() => {
  vi.useRealTimers()
})

async function renderSelectedConversation() {
  const screen = await render(<AppShell />)
  const selectConversation = screen.getByRole('button', {
    name: 'select-conversation-a',
    exact: true
  })
  await expect.element(selectConversation).toBeVisible()
  await selectConversation.click()
  await expect.element(screen.getByRole('button', { name: 'submit-with-skill' })).toBeVisible()
  return screen
}

describe('title project editor removal', () => {
  it('routes remove-requested through confirmation before removing the project', async () => {
    testState.openEditProjectDialog.mockResolvedValue('remove-requested')
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'project.openDetails' }).click()
    await screen.getByRole('button', { name: 'project.editProject' }).click()
    expect(testState.openEditProjectDialog).toHaveBeenCalledWith('project-a')
    await expect.element(screen.getByRole('heading', { name: 'project.removeTitle' })).toBeVisible()
    expect(testState.deleteProject).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'project.cancel', exact: true }).last().click()
    expect(testState.deleteProject).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'project.openDetails' }).click()
    await screen.getByRole('button', { name: 'project.editProject' }).click()
    await screen.getByRole('button', { name: 'project.confirmRemove' }).click()
    await expect.poll(() => testState.deleteProject.mock.calls).toEqual([['project-a']])
    await expect
      .element(screen.getByRole('heading', { name: 'project.removeTitle' }))
      .not.toBeInTheDocument()
  })
})

describe('scheduled workspace isolation', () => {
  it('keeps the selected conversation, composer draft, and right workspace mounted while covered', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')

    const mainPanel = screen.container.querySelector<HTMLElement>('.main-panel')
    const rightPanel = screen.container.querySelector<HTMLElement>('.side-panel--right')
    const chatSurface = screen.getByTestId('active-conversation-id').element()
    const draftSurface = screen.getByTestId('draft-model-id').element()
    const rightSurface = screen.getByTestId('right-sidebar-conversation-id').element()
    expect(mainPanel).not.toBeNull()
    expect(rightPanel).not.toBeNull()
    expect(chatSurface.textContent).toBe('conversation-a')
    expect(rightSurface.textContent).toBe('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('conversation-a')

    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'app.expandLeftSidebar' }))
      .toBeVisible()

    expect(screen.container.querySelector('.main-panel')).toBe(mainPanel)
    expect(screen.container.querySelector('.side-panel--right')).toBe(rightPanel)
    expect(screen.getByTestId('active-conversation-id').element()).toBe(chatSurface)
    expect(screen.getByTestId('draft-model-id').element()).toBe(draftSurface)
    expect(screen.getByTestId('right-sidebar-conversation-id').element()).toBe(rightSurface)
    expect(mainPanel?.inert).toBe(true)
    expect(rightPanel?.inert).toBe(true)
    expect(mainPanel?.getAttribute('aria-hidden')).toBe('true')
    expect(rightPanel?.getAttribute('aria-hidden')).toBe('true')
    expect(chatSurface.textContent).toBe('conversation-a')
    expect(draftSurface.textContent).toBe('model-2')
    expect(rightSurface.textContent).toBe('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
    await expect.element(screen.getByTestId('scheduled-selected')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('scheduled-attention')).toHaveTextContent('4')

    await screen.getByRole('button', { name: 'select-conversation-a' }).click()
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await expect
      .poll(() => screen.container.querySelector('[data-testid="scheduled-page-layer"]'))
      .toBeNull()

    expect(mainPanel?.inert).toBe(false)
    expect(rightPanel?.inert).toBe(false)
    expect(mainPanel?.hasAttribute('aria-hidden')).toBe(false)
    expect(rightPanel?.hasAttribute('aria-hidden')).toBe(false)
    expect(chatSurface.textContent).toBe('conversation-a')
    expect(draftSurface.textContent).toBe('model-2')
    expect(rightSurface.textContent).toBe('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('conversation-a')
  })

  it('defers sidebar navigation until the scheduled page accepts the external request', async () => {
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    await expect
      .element(screen.getByTestId('external-navigation-request-key'))
      .not.toHaveTextContent('none')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')

    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()

    await expect
      .poll(() => screen.container.querySelector('[data-testid="scheduled-page-layer"]'))
      .toBeNull()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('none')
  })

  it('keeps the automation drawer preference for the current app session only', async () => {
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect
      .element(screen.getByTestId('scheduled-drawer-preferred-width'))
      .toHaveTextContent('440')
    await screen.getByRole('button', { name: 'resize-scheduled-drawer' }).click()
    await expect
      .element(screen.getByTestId('scheduled-drawer-preferred-width'))
      .toHaveTextContent('560')

    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect
      .element(screen.getByTestId('scheduled-drawer-preferred-width'))
      .toHaveTextContent('560')
  })
})

describe('native notification navigation', () => {
  it('opens the exact conversation message without an in-app notification surface', async () => {
    const screen = await renderSelectedConversation()
    await expect.poll(() => testState.notificationOpenRequestListeners.size).toBe(1)

    const request: NotificationOpenRequest = {
      schemaVersion: 1,
      batchId: 'batch-conversation',
      eventIds: ['notification-conversation'],
      destination: {
        kind: 'conversation',
        conversationId: 'conversation-a',
        messageId: 'assistant-a',
        approvalActionId: null
      }
    }
    for (const listener of testState.notificationOpenRequestListeners) listener(request)

    expect(testState.markNotificationsSeen).toHaveBeenCalledWith({
      kind: 'events',
      eventIds: ['notification-conversation']
    })
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('scroll-target-message-id'))
      .toHaveTextContent('assistant-a')
    expect(screen.container.querySelector('.notification-center')).toBeNull()
  })

  it('opens the selected automation run directly', async () => {
    const screen = await renderSelectedConversation()
    await expect.poll(() => testState.notificationOpenRequestListeners.size).toBe(1)

    const request: NotificationOpenRequest = {
      schemaVersion: 1,
      batchId: 'batch-automation',
      eventIds: ['notification-automation'],
      destination: {
        kind: 'automation',
        automationId: 'automation-notification',
        runId: 'run-notification'
      }
    }
    for (const listener of testState.notificationOpenRequestListeners) listener(request)

    await expect
      .element(screen.getByTestId('scheduled-open-automation'))
      .toHaveTextContent('automation-notification')
    await expect
      .element(screen.getByTestId('scheduled-open-run'))
      .toHaveTextContent('run-notification')
  })

  it('leaves the current workspace unchanged for an application-only fallback', async () => {
    const screen = await renderSelectedConversation()
    const chatSurface = screen.getByTestId('active-conversation-id').element()
    const rightSurface = screen.getByTestId('right-sidebar-conversation-id').element()
    await expect.poll(() => testState.notificationOpenRequestListeners.size).toBe(1)
    await screen.getByRole('button', { name: 'open-settings' }).click()
    await expect.element(screen.getByTestId('settings-view')).toBeInTheDocument()

    const request: NotificationOpenRequest = {
      schemaVersion: 1,
      batchId: 'batch-application',
      eventIds: ['notification-application'],
      destination: { kind: 'application' }
    }
    for (const listener of testState.notificationOpenRequestListeners) listener(request)

    expect(screen.getByTestId('active-conversation-id').element()).toBe(chatSurface)
    expect(screen.getByTestId('right-sidebar-conversation-id').element()).toBe(rightSurface)
    await expect.element(screen.getByTestId('settings-view')).toBeInTheDocument()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
  })
})

describe('automation conversation navigation', () => {
  it('refreshes conversation metadata when an Automation run creates a new chat', async () => {
    const screen = await render(<AppShell />)
    await expect
      .element(screen.getByRole('button', { name: 'select-conversation-a' }))
      .toBeVisible()
    await expect.poll(() => testState.automationEventListeners.size).toBe(1)
    const initialMetaLoadCount = testState.loadConversationMetas.mock.calls.length
    const automationConversation: ChatConversation = {
      ...storedConversation(),
      id: 'automation-conversation',
      title: 'Automation output',
      updatedAt: 20
    }
    testState.persistedConversations.set(automationConversation.id, automationConversation)

    const event: AutomationEvent = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'automation-event-1',
      kind: 'run_updated',
      automationId: 'automation-1',
      runId: 'automation-run-1',
      resourceRevision: 1,
      occurredAt: 20
    }
    for (const listener of testState.automationEventListeners) listener(event)

    await expect
      .poll(() => testState.loadConversationMetas.mock.calls.length)
      .toBe(initialMetaLoadCount + 1)
    await expect
      .element(screen.getByRole('button', { name: 'select-automation-conversation' }))
      .toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith('automation-conversation')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
  })

  it('reloads an open Automation chat and binds events that arrived before its run projection', async () => {
    const screen = await renderSelectedConversation()
    await expect.poll(() => testState.automationEventListeners.size).toBe(1)
    const initialPendingActionLoads = testState.listPendingAgentActions.mock.calls.length
    const stored = storedConversation()
    const automationAssistant: ChatMessage = {
      id: 'assistant-automation',
      role: 'assistant',
      content: '',
      createdAt: 20,
      status: 'pending',
      agentRun: {
        runId: 'automation-agent-run',
        status: 'running',
        toolDefinitions: [],
        toolCalls: [],
        toolResults: [],
        approvals: [],
        fileChangeProposals: [],
        timeline: []
      }
    }
    testState.persistedConversations.set(stored.id, {
      ...stored,
      messages: [...stored.messages, automationAssistant],
      updatedAt: 20
    })

    const agentEvent: AgentEvent = {
      type: 'message',
      runId: 'automation-agent-run',
      content: 'live automation answer'
    }
    for (const listener of testState.agentEventListeners) listener(agentEvent)
    const automationEvent: AutomationEvent = {
      schemaVersion: 1,
      sequence: 2,
      eventId: 'automation-event-2',
      kind: 'run_updated',
      automationId: 'automation-1',
      runId: 'automation-run-1',
      resourceRevision: 2,
      occurredAt: 20
    }
    for (const listener of testState.automationEventListeners) listener(automationEvent)

    await expect
      .poll(() => testState.loadConversation.mock.calls.filter(([id]) => id === stored.id).length)
      .toBeGreaterThan(1)
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('assistant-automation')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('live automation answer')
    await expect
      .poll(() => testState.listPendingAgentActions.mock.calls.length)
      .toBeGreaterThan(initialPendingActionLoads)
  })

  it('hydrates a conversation missing from the local catalog before opening its target message', async () => {
    const screen = await render(<AppShell />)
    await expect
      .element(screen.getByRole('button', { name: 'select-conversation-a' }))
      .toBeVisible()
    await expect.poll(() => testState.automationOpenRequestListeners.size).toBe(1)
    const automationConversation: ChatConversation = {
      ...storedConversation(),
      id: 'automation-conversation',
      title: 'Automation output',
      updatedAt: 20
    }
    testState.persistedConversations.set(automationConversation.id, automationConversation)

    const openRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'automation-run-1',
      destination: {
        kind: 'conversation',
        conversationId: automationConversation.id,
        messageId: 'assistant-old'
      }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(openRequest)

    await expect
      .poll(() => testState.loadConversation.mock.calls)
      .toContainEqual([automationConversation.id])
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent(automationConversation.id)
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent(automationConversation.id)
    await expect
      .element(screen.getByTestId('scroll-target-message-id'))
      .toHaveTextContent('assistant-old')
    await expect
      .element(screen.getByRole('button', { name: 'select-automation-conversation' }))
      .toBeVisible()
  })

  it('keeps the scheduled page mounted while a native chat request is guarded and hydrated', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect.poll(() => testState.automationOpenRequestListeners.size).toBe(1)
    const automationConversation: ChatConversation = {
      ...storedConversation(),
      id: 'automation-conversation',
      title: 'Automation output',
      updatedAt: 20
    }
    const detail = deferred<ChatConversation | undefined>()
    testState.loadConversation.mockImplementationOnce(() => detail.promise)

    const openRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'automation-run-1',
      destination: {
        kind: 'conversation',
        conversationId: automationConversation.id,
        messageId: 'assistant-old'
      }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(openRequest)

    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith(automationConversation.id)
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await expect
      .poll(() => testState.loadConversation.mock.calls)
      .toContainEqual([automationConversation.id])
    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()

    detail.resolve(automationConversation)
    await expect
      .poll(() => screen.container.querySelector('[data-testid="scheduled-page-layer"]'))
      .toBeNull()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent(automationConversation.id)
    await expect
      .element(screen.getByTestId('scroll-target-message-id'))
      .toHaveTextContent('assistant-old')
  })

  it('does not let a stale native chat load override a newer scheduled task intent', async () => {
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-scheduled' }).click()
    await expect.poll(() => testState.automationOpenRequestListeners.size).toBe(1)
    const detail = deferred<ChatConversation | undefined>()
    testState.loadConversation.mockImplementationOnce(() => detail.promise)
    const conversationRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'automation-run-1',
      destination: {
        kind: 'conversation',
        conversationId: 'automation-conversation',
        messageId: 'assistant-old'
      }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(conversationRequest)
    await screen.getByRole('button', { name: 'confirm-external-navigation' }).click()
    await expect
      .poll(() => testState.loadConversation.mock.calls)
      .toContainEqual(['automation-conversation'])

    const taskRequest: AutomationOpenRequest = {
      schemaVersion: 1,
      automationId: 'automation-2',
      runId: 'automation-run-2',
      destination: { kind: 'task' }
    }
    for (const listener of testState.automationOpenRequestListeners) listener(taskRequest)
    detail.resolve({
      ...storedConversation(),
      id: 'automation-conversation',
      updatedAt: 20
    })

    await expect
      .element(screen.getByRole('button', { name: 'select-automation-conversation' }))
      .toBeVisible()
    await expect.element(screen.getByTestId('scheduled-page-layer')).toBeVisible()
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('sidebar-active-conversation'))
      .toHaveTextContent('none')
  })
})

describe('provider transition guard', () => {
  it('keeps newer next-turn choices and input when an earlier confirmed transition completes', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-older-composer'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'summary-older-composer',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(
      expect.objectContaining({ content: 'follow up', modelId: 'model-1' })
    )
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({
        modelId: 'model-2',
        permissionMode: 'custom',
        message: 'new draft after failure'
      })
    )
  })

  it('preserves next-turn choices made while a compatible transition reply is pending', async () => {
    mockSuccessfulTurnStarts()
    const transition =
      deferred<Extract<AgentProviderTransitionOperation, { status: 'completed' }>>()
    testState.startProviderTransition.mockReturnValueOnce(transition.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()

    transition.resolve(completedProviderTransition('model-1'))
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].modelId).toBe('model-1')
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({ modelId: 'model-2', permissionMode: 'custom' })
    )
  })

  it('uses the completed transition model when the submitted composer selection is unchanged', async () => {
    mockSuccessfulTurnStarts()
    testState.startProviderTransition.mockImplementationOnce(async () => ({
      ...completedProviderTransition('model-1'),
      modelId: 'model-2'
    }))
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].modelId).toBe('model-2')
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
  })

  it('keeps model selection in the draft and preflights only when the user sends', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-2')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      decision: 'requires_compaction',
      reason: 'api_provider_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-model-2'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
    expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.saveConversationMeta).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect.poll(() => testState.saveComposerDraft.mock.calls.length).toBeGreaterThan(0)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('api_provider_changed')
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:running`)
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')

    const conversationUpdatedAt = running.startedAt + 11
    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-2',
      summaryId: 'summary-model-2',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt
    })
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
    await expect
      .poll(() => Number(screen.getByTestId('conversation-updated-at').element().textContent))
      .toBeGreaterThanOrEqual(conversationUpdatedAt)
    await expect
      .poll(() => Number(screen.getByTestId('draft-updated-at').element().textContent))
      .toBeGreaterThanOrEqual(conversationUpdatedAt)
    await expect.poll(() => screen.getByTestId('draft-message').element().textContent).toBe('')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(
      (testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput).modelId
    ).toBe('model-2')
  })

  it('preflights a same-model submit before creating messages and resumes it after confirmation', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-same-model'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')

    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'summary-same-model',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(
      (testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput).content
    ).toBe('follow up')
  })

  it('keeps the full composer payload and target model when transition confirmation is cancelled', async () => {
    const retainedDraft = createComposerDraft({
      attachments: [
        {
          id: 'attachment-retained',
          kind: 'file',
          name: 'retained.txt',
          mimeType: 'text/plain',
          sizeBytes: 8,
          encoding: 'base64',
          data: 'cmV0YWluZWQ='
        }
      ],
      message: 'keep this draft',
      modelId: 'model-1',
      permissionMode: 'custom',
      projectId: 'project-a',
      skills: [skillSelection]
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': retainedDraft
    })
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: 'provider-transition-cancelled',
      transitionToken: 'transition-token-cancelled'
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await screen.getByRole('button', { name: 'cancel-model-transition' }).click()

    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual({
      attachments: retainedDraft.attachments,
      message: retainedDraft.message,
      modelId: 'model-2',
      permissionMode: retainedDraft.permissionMode,
      projectId: retainedDraft.projectId,
      skills: retainedDraft.skills
    })
  })

  it('keeps the original model after failure and switches only after an explicit retry succeeds', async () => {
    const running = runningProviderTransition('model-2')
    const completedRetry = completedProviderTransition('model-2', {
      coveredThroughMessageId: 'assistant-old',
      summaryId: 'summary-retry'
    })
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-2',
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId: running.operationId,
        transitionToken: 'transition-token-model-2'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-2',
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId: completedRetry.operationId,
        transitionToken: 'transition-token-model-2-retry'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(failedProviderTransition(running))
      .mockResolvedValueOnce(completedRetry)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:failed`)
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')

    await screen.getByRole('button', { name: 'retry-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
  })

  it('does not replay a failed composer intent after the user edits the draft and retries', async () => {
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    const completedRetry = completedProviderTransition('model-1', {
      coveredThroughMessageId: 'assistant-old',
      summaryId: 'summary-retry'
    })
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: running.operationId,
        transitionToken: 'transition-token-submit'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: completedRetry.operationId,
        transitionToken: 'transition-token-retry'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(failedProviderTransition(running))
      .mockResolvedValueOnce(completedRetry)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:failed`)

    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')

    await screen.getByRole('button', { name: 'retry-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')
  })

  it('restores historical dividers without replaying an old completed switch on reload', async () => {
    const older = completedProviderTransition('model-1', {
      operationId: 'operation-old',
      coveredThroughMessageId: 'assistant-before-old',
      summaryId: 'summary-old',
      startedAt: 10,
      completedAt: 20
    })
    const newer = completedProviderTransition('model-2', {
      operationId: 'operation-new',
      coveredThroughMessageId: 'assistant-before-new',
      summaryId: 'summary-new',
      startedAt: 30,
      completedAt: 40
    })
    testState.getProviderTransitionStatus.mockResolvedValueOnce({ operations: [newer, older] })
    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent('operation-old:completed,operation-new:completed')
    // The persisted conversation/draft already contains the authoritative current model. A later
    // compatible transition may not have a compaction receipt, so loading old receipts must not
    // roll it back to the target of the latest historical compaction.
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-1')
  })

  it('rewrites on the current model without compacting or replacing source history first', async () => {
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: 'provider-transition-edit',
      transitionToken: 'transition-token-edit'
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .not.toHaveTextContent('provider_protocol_changed')
  })
})

describe('conversation startup loading', () => {
  it('keeps the prompt stable when the global new-conversation action is repeated on the root page', async () => {
    const screen = await render(<AppShell />)
    const promptIndex = screen.getByTestId('new-conversation-prompt-index')
    await expect.element(promptIndex).toBeVisible()
    const initialPromptIndex = promptIndex.element().textContent

    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    await expect.element(promptIndex).toHaveTextContent(initialPromptIndex ?? '')
    await expect.element(screen.getByTestId('new-conversation-project')).toHaveTextContent('none')
  })

  it('selects a different prompt when returning from an existing conversation', async () => {
    const screen = await render(<AppShell />)
    const promptIndex = screen.getByTestId('new-conversation-prompt-index')
    await expect.element(promptIndex).toBeVisible()
    const initialPromptIndex = promptIndex.element().textContent

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    await expect.element(screen.getByTestId('new-conversation-prompt-index')).toBeVisible()
    expect(screen.getByTestId('new-conversation-prompt-index').element().textContent).not.toBe(
      initialPromptIndex
    )
  })

  it('starts on a new conversation and hydrates a stored conversation only after selection', async () => {
    const active = storedConversation()
    const archived = {
      ...storedConversation(),
      id: 'conversation-archived',
      title: 'Archived',
      messages: [],
      messagesLoaded: false,
      archivedAt: 10
    }
    const activeDetail = deferred<ChatConversation>()
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...active, messages: [], messagesLoaded: false },
      archived
    ])
    testState.loadConversation.mockReturnValueOnce(activeDetail.promise)

    const screen = await render(<AppShell />)

    await expect.poll(() => testState.loadConversationMetas.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.loadConversation).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect.poll(() => testState.loadConversation.mock.calls).toEqual([['conversation-a']])
    await expect.element(screen.getByText('chat.loadingConversation')).toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith('conversation-archived')

    activeDetail.resolve(active)
    await expect.element(screen.getByRole('button', { name: 'submit-with-skill' })).toBeVisible()
    expect(testState.loadConversation).not.toHaveBeenCalledWith('conversation-archived')
  })

  it('commits preferences and drafts without waiting for the conversation catalog', async () => {
    const conversationMetas = deferred<ChatConversation[]>()
    testState.loadConversationMetas.mockReturnValueOnce(conversationMetas.promise)
    testState.loadUiPreferences.mockResolvedValueOnce({
      ...defaultUiPreferences(),
      translucentSidebar: true,
      showContextWindowUsage: false
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'new-conversation': createComposerDraft({ message: 'independent draft' })
    })

    const screen = await render(<AppShell />)

    await expect.element(screen.getByTestId('sidebar-translucent')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('new-conversation-draft'))
      .toHaveTextContent('independent draft')

    conversationMetas.resolve([])
  })

  it('restores the complete new-conversation draft after visiting an existing conversation', async () => {
    const attachment = {
      id: 'new-task-attachment',
      kind: 'file' as const,
      name: 'requirements.txt',
      mimeType: 'text/plain',
      sizeBytes: 12,
      encoding: 'base64' as const,
      data: 'cmVxdWlyZW1lbnRz'
    }
    const bundledSkill: SkillSelection = {
      id: 'bundled:application:documents',
      revision: 'skill-package-sha256-v2:documents'
    }
    const newConversationDraft = createComposerDraft({
      attachments: [attachment],
      message: 'unfinished new task',
      modelId: 'model-2',
      permissionMode: 'custom',
      projectId: 'project-a',
      skills: [bundledSkill]
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
      'new-conversation': newConversationDraft
    })

    const screen = await render(<AppShell />)
    await expect
      .element(screen.getByTestId('new-conversation-draft'))
      .toHaveTextContent(newConversationDraft.message)

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await screen.getByRole('button', { name: 'new-conversation', exact: true }).click()

    expect(
      JSON.parse(screen.getByTestId('new-conversation-payload').element().textContent ?? '{}')
    ).toEqual({
      attachments: newConversationDraft.attachments,
      message: newConversationDraft.message,
      modelId: newConversationDraft.modelId,
      permissionMode: newConversationDraft.permissionMode,
      projectId: newConversationDraft.projectId,
      skills: newConversationDraft.skills
    })
  })

  it('changes only the workspace when opening a project-specific new conversation', async () => {
    const bundledSkill: SkillSelection = {
      id: 'bundled:application:spreadsheets',
      revision: 'skill-package-sha256-v2:spreadsheets'
    }
    const workspaceSkill: SkillSelection = {
      id: 'workspace:old-project:local-skill',
      revision: 'skill-package-sha256-v2:local-skill'
    }
    const newConversationDraft = createComposerDraft({
      attachments: [
        {
          id: 'new-task-image',
          kind: 'image',
          name: 'reference.png',
          mimeType: 'image/png',
          sizeBytes: 8,
          encoding: 'base64',
          data: 'aW1hZ2U='
        }
      ],
      message: 'continue this new task',
      modelId: 'model-2',
      permissionMode: 'full',
      projectId: null,
      skills: [bundledSkill, workspaceSkill]
    })
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
      'new-conversation': newConversationDraft
    })

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'new-conversation-project-a', exact: true }).click()

    expect(
      JSON.parse(screen.getByTestId('new-conversation-payload').element().textContent ?? '{}')
    ).toEqual({
      attachments: newConversationDraft.attachments,
      message: newConversationDraft.message,
      modelId: newConversationDraft.modelId,
      permissionMode: newConversationDraft.permissionMode,
      projectId: 'project-a',
      skills: [bundledSkill]
    })
    await expect.poll(() => testState.saveComposerDraft.mock.calls.length).toBe(1)
    expect(testState.saveComposerDraft).toHaveBeenLastCalledWith(
      'new-conversation',
      expect.objectContaining({
        attachments: newConversationDraft.attachments,
        message: newConversationDraft.message,
        modelId: newConversationDraft.modelId,
        projectId: 'project-a',
        skills: [bundledSkill]
      })
    )
  })
})

describe('conversation archive navigation', () => {
  it('leaves the active conversation only after its archive metadata is committed', async () => {
    const archiveSave = deferred<void>()
    testState.saveConversationMeta.mockReturnValueOnce(archiveSave.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.poll(() => testState.saveConversationMeta.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('false')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')

    const savedConversation = testState.saveConversationMeta.mock.calls[0]?.[0]
    if (!savedConversation) throw new Error('missing archive metadata write')
    expect(savedConversation).toMatchObject({
      id: 'conversation-a',
      archivedAt: null,
      pendingArchivedAt: expect.any(Number),
      unreadAt: null
    })
    testState.persistedConversations.set('conversation-a', {
      ...savedConversation,
      archivedAt: savedConversation.pendingArchivedAt,
      pendingArchivedAt: undefined
    })
    archiveSave.resolve(undefined)

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect.element(screen.getByTestId('new-conversation-project')).toHaveTextContent('none')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('none')
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('false')
    await expect.poll(() => testState.collaborationRootIds.at(-1)).toBe(null)

    await screen.unmount()
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...storedConversation(), messages: [], messagesLoaded: false, archivedAt: 100 }
    ])
    const reloaded = await render(<AppShell />)
    await expect.element(reloaded.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.loadConversation).toHaveBeenCalledTimes(1)
  })

  it('archives a non-active conversation without interrupting the current one', async () => {
    const conversationA = storedConversation()
    const conversationB = {
      ...storedConversation(),
      id: 'conversation-b',
      title: 'Second conversation'
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...conversationA, messages: [], messagesLoaded: false },
      { ...conversationB, messages: [], messagesLoaded: false }
    ])
    testState.persistedConversations.set(conversationB.id, conversationB)

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')

    await screen.getByRole('button', { name: 'archive-conversation-b' }).click()

    await expect.element(screen.getByTestId('archived-conversation-b')).toHaveTextContent('true')
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('right-sidebar-conversation-id'))
      .toHaveTextContent('conversation-a')
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('keeps the active conversation selected when archive persistence fails', async () => {
    const persistenceError = new Error('simulated archive failure')
    const failedSave = deferred<void>()
    let firstAttempt = true
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    testState.saveConversationMeta.mockImplementation(async () => {
      if (firstAttempt) {
        firstAttempt = false
        await failedSave.promise
      }
      throw persistenceError
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('false')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')

    failedSave.reject(persistenceError)

    await expect.poll(() => testState.showToast.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('conversation.archiveFailed')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('false')
    expect(consoleError).toHaveBeenCalledWith(
      'Failed to archive conversation',
      expect.objectContaining({ message: persistenceError.message })
    )
    consoleError.mockRestore()
  })

  it('confirms a committed archive when the save response is lost', async () => {
    const responseLost = new Error('simulated response loss')
    testState.saveConversationMeta.mockImplementationOnce(
      async (conversation: ChatConversation) => {
        testState.persistedConversations.set(conversation.id, {
          ...conversation,
          archivedAt: conversation.pendingArchivedAt,
          pendingArchivedAt: undefined,
          unreadAt: null
        })
        throw responseLost
      }
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect.element(screen.getByTestId('archived-conversation-a')).toHaveTextContent('true')
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('keeps an unknown archive token retryable after a later commit loses readback', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const screen = await renderSelectedConversation()
    let saveAttempt = 0
    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      saveAttempt += 1
      if (saveAttempt === 2) {
        testState.persistedConversations.set(conversation.id, {
          ...conversation,
          archivedAt: conversation.pendingArchivedAt,
          pendingArchivedAt: undefined,
          unreadAt: null
        })
      }
      if (saveAttempt > 1) throw new Error('response lost')
    })
    let readAttempt = 0
    testState.loadConversationMetas.mockImplementation(async () => {
      readAttempt += 1
      if (readAttempt === 1) {
        return [{ ...storedConversation(), messages: [], messagesLoaded: false }]
      }
      throw new Error('readback unavailable')
    })

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.poll(() => testState.showToast.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')

    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      const current = testState.persistedConversations.get(conversation.id)
      testState.persistedConversations.set(conversation.id, {
        ...current,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt,
        pendingArchivedAt: undefined,
        unreadAt: null
      })
    })
    testState.loadConversationMetas.mockImplementation(async () =>
      [...testState.persistedConversations.values()].map((conversation) => ({
        ...conversation,
        messages: [],
        messagesLoaded: false
      }))
    )
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('false')
    expect(testState.persistedConversations.get('conversation-a')?.archivedAt).toEqual(
      expect.any(Number)
    )
    consoleError.mockRestore()
  })

  it('fences a delayed pre-archive metadata save after authoritative readback', async () => {
    const delayedStaleWrite = deferred<void>()
    let delayFirstWrite = true
    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      if (delayFirstWrite) {
        delayFirstWrite = false
        await delayedStaleWrite.promise
      }
      const current = testState.persistedConversations.get(conversation.id)
      if (current && conversation.updatedAt < current.updatedAt) return
      testState.persistedConversations.set(conversation.id, {
        ...current,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt ?? conversation.archivedAt,
        pendingArchivedAt: undefined,
        unreadAt: conversation.pendingArchivedAt === undefined ? conversation.unreadAt : null
      })
    })
    const staleSave = testState.saveConversationMeta({
      ...storedConversation(),
      title: 'stale pre-archive title'
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    const archivedBeforeStaleCommit = testState.persistedConversations.get('conversation-a')
    expect(archivedBeforeStaleCommit?.archivedAt).toEqual(expect.any(Number))

    delayedStaleWrite.resolve(undefined)
    await staleSave

    const archivedAfterStaleCommit = testState.persistedConversations.get('conversation-a')
    expect(archivedAfterStaleCommit?.archivedAt).toBe(archivedBeforeStaleCommit?.archivedAt)
    expect(archivedAfterStaleCommit?.title).not.toBe('stale pre-archive title')
  })

  it('preserves a concurrent rename when archive and metadata responses settle out of order', async () => {
    const delayedArchiveWrite = deferred<void>()
    let delayFirstWrite = true
    testState.saveConversationMeta.mockImplementation(async (conversation: ChatConversation) => {
      if (delayFirstWrite) {
        delayFirstWrite = false
        await delayedArchiveWrite.promise
      }
      const current = testState.persistedConversations.get(conversation.id)
      if (current && conversation.updatedAt < current.updatedAt) return
      testState.persistedConversations.set(conversation.id, {
        ...current,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt ?? conversation.archivedAt,
        pendingArchivedAt: undefined,
        unreadAt: conversation.pendingArchivedAt === undefined ? conversation.unreadAt : null
      })
    })
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect
      .element(screen.getByTestId('archive-pending-conversation-a'))
      .toHaveTextContent('true')
    await screen.getByRole('button', { name: 'rename-conversation-a' }).click()
    await expect
      .poll(() => testState.persistedConversations.get('conversation-a')?.title)
      .toBe('renamed-conversation-a')

    delayedArchiveWrite.resolve(undefined)

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.persistedConversations.get('conversation-a')).toMatchObject({
      archivedAt: expect.any(Number),
      title: 'renamed-conversation-a'
    })
    await expect
      .element(screen.getByTestId('title-conversation-a'))
      .toHaveTextContent('renamed-conversation-a')
  })

  it('does not cancel or retire an active Run when navigating away after archive', async () => {
    mockSuccessfulTurnStarts()
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': {
        ...createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
        queuedMessages: [queuedMessage('queued-after-archive', 'do not start hidden', 20)]
      }
    })
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')

    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'Finished after archive.'
    })
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queued-after-archive')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()
  })

  it('does not submit a message when its deferred model transition completes after archive', async () => {
    const transition =
      deferred<Extract<AgentProviderTransitionOperation, { status: 'completed' }>>()
    testState.startProviderTransition.mockReturnValueOnce(transition.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()

    transition.resolve(completedProviderTransition('model-1'))
    await new Promise((resolve) => window.setTimeout(resolve, 0))

    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })

  it('does not reopen history when an atomic edit response arrives after archive', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 99))
    await new Promise((resolve) => window.setTimeout(resolve, 0))

    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.rewriteConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
  })
})

describe('managed command Session lifecycle routing', () => {
  it('keeps copied command history immutable when a root fork lists matching terminal Sessions', async () => {
    const stored = storedConversationWithCommandRun()
    stored.continuationOrigin = {
      sourceConversationId: 'source-conversation',
      sourceMessageId: 'source-assistant',
      boundaryMessageId: 'assistant-command'
    }
    const snapshot = commandSessionSnapshot({ status: 'exited', endedAt: 30, exitCode: 0 })
    stored.messages.at(-1)!.agentRun!.commandSessions = { 'command-call': snapshot }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [snapshot] })
    testState.getAgentCommandSession.mockResolvedValue(commandSessionGetOutput(snapshot))
    testState.saveChatMessageState.mockRejectedValue(
      new Error('Child context snapshot message is immutable')
    )
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(0)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"exited"')
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(consoleError).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('refreshes new fork-owned Sessions without reconciling missing copied-history receipts', async () => {
    const stored = storedConversationWithCommandRun()
    const copiedAssistant = stored.messages.at(-1)!
    copiedAssistant.agentRun!.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          output: 'copied receipt'
        }
      }
    ]
    stored.continuationOrigin = {
      sourceConversationId: 'source-conversation',
      sourceMessageId: 'source-assistant',
      boundaryMessageId: copiedAssistant.id
    }
    const currentAssistant = structuredClone(copiedAssistant)
    currentAssistant.id = 'assistant-after-fork'
    currentAssistant.agentRun!.runId = 'run-after-fork'
    currentAssistant.agentRun!.toolResults = []
    stored.messages.push(currentAssistant)
    const snapshot = commandSessionSnapshot({
      sessionId: 'cmd_abcdef1234567890abcdef1234567890',
      assistantMessageId: currentAssistant.id,
      originRunId: 'run-after-fork',
      status: 'exited',
      endedAt: 30,
      exitCode: 0
    })
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [snapshot] })
    testState.getAgentCommandSession.mockResolvedValue(
      commandSessionGetOutput(snapshot, 'after the fork\n')
    )
    testState.saveChatMessageState.mockImplementation(async (_conversationId, message) => {
      if ((message as ChatMessage).id === copiedAssistant.id) {
        throw new Error('Child context snapshot message is immutable')
      }
    })
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('after the fork')
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    expect(
      testState.saveChatMessageState.mock.calls.every(
        ([, message]) => (message as ChatMessage).id === currentAssistant.id
      )
    ).toBe(true)
    expect(testState.getAgentCommandSession).toHaveBeenCalledTimes(1)
    expect(consoleError).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('refreshes Host-owned Sessions whenever an already loaded conversation is reopened', async () => {
    const conversationA = storedConversation()
    const conversationB = {
      ...storedConversation(),
      id: 'conversation-b',
      title: 'Second conversation'
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...conversationA, messages: [], messagesLoaded: false },
      { ...conversationB, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation.mockImplementation(async (conversationId: string) =>
      conversationId === 'conversation-a' ? conversationA : conversationB
    )

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .poll(
        () =>
          testState.listAgentCommandSessions.mock.calls.filter(
            ([input]) => input.conversationId === 'conversation-a'
          ).length
      )
      .toBe(1)

    await screen.getByRole('button', { name: 'select-conversation-b', exact: true }).click()
    await expect
      .poll(
        () =>
          testState.listAgentCommandSessions.mock.calls.filter(
            ([input]) => input.conversationId === 'conversation-b'
          ).length
      )
      .toBe(1)

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .poll(
        () =>
          testState.listAgentCommandSessions.mock.calls.filter(
            ([input]) => input.conversationId === 'conversation-a'
          ).length
      )
      .toBe(2)
  })

  it('keeps routing background output and exit to the original completed assistant message', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const turnInput = testState.startConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnInput
    const assistantMessageId = turnInput.assistantMessageId!
    const sessionId = 'cmd_1234567890abcdef1234567890abcdef'

    emitAgentEvent({
      type: 'tool_call',
      runId: 'run-1',
      traceSequence: 0,
      identity: { type: 'builtin', toolName: 'run_command' },
      call: {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'python3 snake_game/main.py' },
        approvalStatus: 'approved',
        reason: null
      }
    })
    emitAgentEvent({
      type: 'command_started',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      startedAt: 10
    })
    emitAgentEvent({
      type: 'tool_result',
      runId: 'run-1',
      result: {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: { status: 'running', sessionId, output: '', startedAt: 10 }
      }
    })
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'The game is running.'
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')

    emitAgentEvent({
      type: 'command_output',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      sequence: 1,
      stream: 'stdout',
      output: 'window closed\n'
    })
    emitAgentEvent({
      type: 'command_exited',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      status: 'exited',
      exitCode: 0,
      endedAt: 20,
      latestSequence: 1,
      outputTruncated: false
    })

    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('window closed')
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"exited"')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('restores a terminal Session and bounded transcript into its existing command item', async () => {
    const stored = storedConversationWithCommandRun()
    const snapshot = commandSessionSnapshot({
      status: 'interrupted',
      endedAt: 30,
      latestSequence: 1
    })
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [snapshot] })
    testState.getAgentCommandSession.mockResolvedValueOnce(
      commandSessionGetOutput(snapshot, 'recovered output\n')
    )

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"interrupted"')
    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('recovered output')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.listAgentCommandSessions).toHaveBeenCalledWith({
      conversationId: 'conversation-a'
    })
    expect(testState.getAgentCommandSession).toHaveBeenCalledWith({
      conversationId: 'conversation-a',
      sessionId: snapshot.sessionId,
      afterSequence: 0,
      maxBytes: 256 * 1024
    })
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
  })

  it('does not let a stale pending-action snapshot restore approval after Session authority', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    assistant.status = 'pending'
    assistant.agentRun.status = 'waiting_for_approval'
    assistant.agentRun.toolCalls = [
      {
        ...assistant.agentRun.toolCalls[0],
        approvalStatus: 'required',
        reason: null
      }
    ]
    assistant.agentRun.approvals = [
      {
        type: 'command',
        command: {
          id: 'command-call',
          command: 'python3 snake_game/main.py',
          cwd: null,
          timeoutMs: null,
          approvalStatus: 'required',
          riskLevel: null,
          reason: null,
          observe: null
        }
      }
    ]
    const session = commandSessionSnapshot()
    const pendingActions = deferred<PendingAgentActionSnapshot[]>()
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [session] })
    testState.getAgentCommandSession.mockResolvedValueOnce(commandSessionGetOutput(session))
    testState.listPendingAgentActions.mockReturnValueOnce(pendingActions.promise)

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')

    pendingActions.resolve([
      {
        actionId: 'command-call',
        actionType: 'command',
        toolName: 'run_command',
        toolCallId: 'command-call',
        runId: 'run-command',
        conversationId: 'conversation-a',
        assistantMessageId: 'assistant-command',
        action: {
          type: 'command',
          command: {
            id: 'command-call',
            command: 'python3 snake_game/main.py',
            cwd: null,
            timeoutMs: null,
            approvalStatus: 'required',
            riskLevel: null,
            reason: null,
            observe: null
          }
        },
        createdAt: 5,
        status: 'pending'
      }
    ])

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('approval-count')).toHaveTextContent('0')
  })

  it('retries a rejected transcript read without rolling back the applied Session status', async () => {
    const stored = storedConversationWithCommandRun()
    const session = commandSessionSnapshot()
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [session] })
    testState.getAgentCommandSession
      .mockRejectedValueOnce(new Error('temporary transcript failure'))
      .mockResolvedValue(commandSessionGetOutput(session, 'recovered after retry\n'))
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.getAgentCommandSession.mock.calls.length).toBeGreaterThan(0)
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(1)
    await expect.poll(() => testState.getAgentCommandSession.mock.calls.length).toBeGreaterThan(1)
    await expect
      .element(screen.getByTestId('command-output'))
      .toHaveTextContent('recovered after retry')
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    consoleError.mockRestore()
  })

  it('keeps the current running projection when Host Session refresh fails', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    const snapshot = commandSessionSnapshot()
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: snapshot.sessionId,
          output: 'application ready\n',
          startedAt: snapshot.startedAt,
          latestSequence: 1
        }
      }
    ]
    assistant.agentRun.commandSessions = {
      'command-call': {
        callId: 'command-call',
        sessionId: snapshot.sessionId,
        status: 'running',
        startedAt: snapshot.startedAt,
        latestSequence: 1,
        outputTruncated: false
      }
    }
    assistant.agentRun.commandOutputPreviews = {
      'command-call': {
        callId: 'command-call',
        chunks: [{ sequence: 1, stream: 'stdout', output: 'application ready\n' }]
      }
    }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockRejectedValue(new Error('temporary Host failure'))
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(0)
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect
      .element(screen.getByTestId('command-output'))
      .toHaveTextContent('application ready')
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    consoleError.mockRestore()
  })

  it('settles a pre-refresh active projection missing from a successful Host list', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    const snapshot = commandSessionSnapshot()
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: snapshot.sessionId,
          output: '',
          startedAt: snapshot.startedAt
        }
      }
    ]
    assistant.agentRun.commandSessions = {
      'command-call': {
        callId: 'command-call',
        sessionId: snapshot.sessionId,
        status: 'running',
        startedAt: snapshot.startedAt,
        latestSequence: 0,
        outputTruncated: false
      }
    }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [] })

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('command-sessions'))
      .toHaveTextContent('"outcome_unknown"')
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
  })

  it('settles a pre-refresh running receipt missing from a successful Host list', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    const snapshot = commandSessionSnapshot()
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: snapshot.sessionId,
          output: '',
          startedAt: snapshot.startedAt
        }
      }
    ]
    delete assistant.agentRun.commandSessions
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [] })

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('command-sessions'))
      .toHaveTextContent('"outcome_unknown"')
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
  })

  it('keeps durable terminal metadata when the Host retention window no longer lists the Session', async () => {
    const stored = storedConversationWithCommandRun()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing command run fixture')
    assistant.agentRun.toolResults = [
      {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          status: 'running',
          sessionId: 'cmd_1234567890abcdef1234567890abcdef',
          output: '',
          startedAt: 10
        }
      }
    ]
    assistant.agentRun.commandSessions = {
      'command-call': {
        callId: 'command-call',
        status: 'exited',
        startedAt: 10,
        endedAt: 30,
        exitCode: 0,
        latestSequence: 0,
        outputTruncated: false
      }
    }
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [] })

    const screen = await renderSelectedConversation()

    await expect.poll(() => testState.listAgentCommandSessions.mock.calls.length).toBeGreaterThan(0)
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"exited"')
    expect(testState.getAgentCommandSession).not.toHaveBeenCalled()
  })

  it('rejects foreign list entries and mismatched transcript responses during hydration', async () => {
    const stored = storedConversationWithCommandRun()
    const listed = commandSessionSnapshot()
    const foreign = commandSessionSnapshot({
      conversationId: 'conversation-foreign',
      assistantMessageId: 'assistant-foreign',
      originRunId: 'run-foreign'
    })
    testState.loadConversation.mockResolvedValueOnce(stored)
    testState.listAgentCommandSessions.mockResolvedValueOnce({ sessions: [foreign, listed] })
    testState.getAgentCommandSession.mockResolvedValueOnce(
      commandSessionGetOutput(
        commandSessionSnapshot({
          sessionId: listed.sessionId,
          callId: 'different-call',
          status: 'exited',
          endedAt: 40,
          exitCode: 0
        }),
        'must not be merged\n'
      )
    )

    const screen = await renderSelectedConversation()

    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')
    await expect.element(screen.getByTestId('command-output')).toHaveTextContent('')
    expect(testState.getAgentCommandSession).toHaveBeenCalledTimes(1)
  })
})

describe('running conversation guidance queue', () => {
  function loadQueue(messages: ChatQueuedMessage[]) {
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: messages
      })
    })
    mockSuccessfulTurnStarts()
  }

  function finishRun(runId: string, status: 'completed' | 'failed' | 'cancelled' = 'completed') {
    emitAgentEvent({
      type: 'done',
      runId,
      success: status === 'completed',
      status,
      content: status === 'completed' ? 'finished current run' : ''
    })
  }

  it('keeps a running turn on its settings and applies composer choices to the next turn', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const firstTurn = structuredClone(testState.startConversationTurn.mock.calls[0]?.[0])
    const preflightCalls = testState.preflightProviderTransition.mock.calls.length
    const transitionCalls = testState.startProviderTransition.mock.calls.length

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(firstTurn)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(preflightCalls)
    expect(testState.startProviderTransition).toHaveBeenCalledTimes(transitionCalls)
    expect(testState.steerAgentRun).not.toHaveBeenCalled()
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()

    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({
        content: 'new draft after failure',
        modelId: 'model-2',
        permissions: defaultUiPreferences().customPermissions
      })
    )
  })

  it('auto-sends the frozen queue settings without replacing newer composer choices', async () => {
    const queued = queuedMessage('queue-older-settings', 'queued on the first model', 10)
    loadQueue([queued])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const firstTurn = testState.startConversationTurn.mock.calls[0]?.[0]
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()

    finishRun('run-1')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({
        content: queued.content,
        modelId: queued.modelId,
        permissions: firstTurn.permissions
      })
    )
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({
        modelId: 'model-2',
        permissionMode: 'custom',
        message: 'new draft after failure'
      })
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')

    finishRun('run-2')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(3)
    expect(testState.startConversationTurn.mock.calls[2]?.[0]).toEqual(
      expect.objectContaining({
        content: 'new draft after failure',
        modelId: 'model-2',
        permissions: defaultUiPreferences().customPermissions
      })
    )
  })

  it('keeps newer composer choices when a queued message finishes its confirmed model transition', async () => {
    loadQueue([queuedMessage('queue-transition-settings', 'queued before model change', 10)])
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'requires_compaction',
      reason: 'provider_protocol_changed',
      operationId: running.operationId,
      transitionToken: 'transition-token-queue-settings'
    })
    testState.startProviderTransition.mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect.poll(() => testState.startProviderTransition.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      summaryId: 'summary-queue-settings',
      completedAt: running.startedAt + 10,
      conversationUpdatedAt: running.startedAt + 11
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(
      expect.objectContaining({ content: 'queued before model change', modelId: 'model-1' })
    )
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({ modelId: 'model-2', permissionMode: 'custom' })
    )
  })

  it('leaves the queue untouched by default when a reply completes', async () => {
    loadQueue([queuedMessage('queue-first', 'keep for later', 10)])
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('starts an idle queue immediately and sends exactly one row per completed reply', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first', 30),
      queuedMessage('queue-second', 'second', 20),
      queuedMessage('queue-third', 'third', 10)
    ])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].content).toBe('first')
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-second,queue-third')

    finishRun('run-1')
    finishRun('run-1')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe('second')
    finishRun('run-1')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-third')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(2)

    finishRun('run-2')
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(3)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[2]?.[0].content).toBe('third')
    finishRun('run-3')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(3)
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
  })

  it('waits for the terminal read and state save before preflighting the next queued message', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first', 10),
      queuedMessage('queue-second', 'after durable settlement', 20)
    ])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    const terminal = storedConversationWithRun(input.assistantMessageId!, 'run-1', 'completed')
    const terminalRead = deferred<ChatConversation>()
    const terminalSave = deferred<void>()
    testState.loadConversation.mockReturnValueOnce(terminalRead.promise)
    testState.saveChatMessageState.mockImplementation(async (_conversationId, message) => {
      const saved = message as ChatMessage
      if (saved.id === input.assistantMessageId && saved.agentRun?.status === 'completed') {
        await terminalSave.promise
      }
    })
    const readsBeforeDone = testState.loadConversation.mock.calls.length

    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.poll(() => testState.loadConversation.mock.calls.length).toBe(readsBeforeDone + 1)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')

    terminalRead.resolve(terminal)
    await expect
      .poll(() =>
        testState.saveChatMessageState.mock.calls.some(([, message]) => {
          const saved = message as ChatMessage
          return saved.id === input.assistantMessageId && saved.agentRun?.status === 'completed'
        })
      )
      .toBe(true)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    terminalSave.resolve()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'after durable settlement'
    )
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('preserves enabled queue execution across effect teardown and restoration', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first before effect teardown', 10),
      queuedMessage('queue-second', 'continue after effect restoration', 20)
    ])
    // Effect restoration rehydrates drafts; reflect the writes instead of returning the
    // suite's empty default draft on that second storage read.
    testState.loadComposerDrafts.mockImplementation(async () => ({
      'conversation-a': testState.saveComposerDraft.mock.calls.findLast(
        ([scopeId]) => scopeId === 'conversation-a'
      )?.[1]
    }))
    const shell = (mode: 'visible' | 'hidden') => (
      <Activity mode={mode}>
        <AppShell />
      </Activity>
    )
    const screen = await render(shell('visible'))
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    await screen.rerender(shell('hidden'))
    await expect.poll(() => testState.agentEventListeners.size).toBe(0)
    await screen.rerender(shell('visible'))
    await expect.poll(() => testState.agentEventListeners.size).toBe(1)
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    finishRun('run-1')
    await expect
      .poll(() => ({
        starts: testState.startConversationTurn.mock.calls.length,
        preflights: testState.preflightProviderTransition.mock.calls.length,
        status: screen.getByTestId('agent-run-status').element().textContent,
        enabled: screen.getByTestId('queue-auto-send-enabled').element().textContent
      }))
      .toEqual({ starts: 2, preflights: 2, status: 'running', enabled: 'true' })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'continue after effect restoration'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('honors the current drag order while retaining each queued message payload', async () => {
    const next = {
      ...queuedMessage('queue-second', 'send with attached context', 20),
      modelId: 'model-2',
      permissionMode: 'custom' as const,
      skills: [skillSelection],
      attachments: [
        {
          id: 'attachment-queue-order',
          kind: 'file' as const,
          name: 'context.txt',
          mimeType: 'text/plain',
          sizeBytes: 6,
          encoding: 'base64' as const,
          data: 'cXVldWVk'
        }
      ]
    }
    loadQueue([queuedMessage('queue-first', 'send last', 10), next])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'reverse-queued-messages' }).click()
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      decision: 'compatible',
      reason: 'no_incompatible_history',
      operationId: 'queued-context-model-transition',
      transitionToken: 'queued-context-model-token'
    })
    testState.startProviderTransition.mockResolvedValueOnce(
      completedProviderTransition('model-2', { operationId: 'queued-context-model-transition' })
    )
    finishRun('run-1')

    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(testState.startConversationTurn.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({
        content: next.content,
        modelId: next.modelId,
        permissions: defaultUiPreferences().customPermissions,
        projectId: next.projectId,
        skills: next.skills,
        attachments: next.attachments
      })
    )
    expect(testState.startProviderTransition).toHaveBeenLastCalledWith({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      transitionToken: 'queued-context-model-token'
    })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('sends a message added after completion already observed an empty enabled queue', async () => {
    loadQueue([queuedMessage('queue-first', 'first', 10)])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'append-queued-message' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'message queued after completion'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('waits for completed Done after an active-run block instead of polling preflight', async () => {
    loadQueue([queuedMessage('queue-next', 'send after the ready signal', 10)])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'state',
      runId: 'run-1',
      state: { status: 'completed', activeRunId: null, lastError: null, updatedAt: 30 }
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    testState.preflightProviderTransition.mockResolvedValueOnce({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'blocked',
      reason: 'active_run',
      message: 'The turn has not published its ready signal yet.'
    })
    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await vi.advanceTimersByTimeAsync(0)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(10_000)
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    finishRun('run-1')
    await vi.advanceTimersByTimeAsync(0)
    vi.useRealTimers()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
      'send after the ready signal'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('resumes an enabled queue when authoritative storage refresh restores a completed reply', async () => {
    const running = storedConversationWithRun('assistant-restored', 'run-restored', 'running')
    testState.persistedConversations.set(running.id, running)
    loadQueue([queuedMessage('queue-next', 'send after completed state is restored', 10)])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    const completed = {
      ...storedConversationWithRun('assistant-restored', 'run-restored', 'completed'),
      updatedAt: running.updatedAt + 100
    }
    const refresh = deferred<ChatConversation>()
    testState.persistedConversations.set(completed.id, completed)
    testState.loadConversation.mockReturnValueOnce(refresh.promise)
    const loadsBeforeRefresh = testState.loadConversation.mock.calls.length
    const event: AutomationEvent = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'completed-state-refresh',
      kind: 'run_updated',
      automationId: 'automation-refresh',
      runId: 'automation-refresh-run',
      resourceRevision: 1,
      occurredAt: completed.updatedAt
    }
    for (const listener of testState.automationEventListeners) listener(event)
    await expect
      .poll(() => testState.loadConversation.mock.calls.length)
      .toBe(loadsBeforeRefresh + 1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    refresh.resolve(completed)
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.startConversationTurn.mock.calls[0]?.[0].content).toBe(
      'send after completed state is restored'
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('does not retry past a pending-approval preflight block', async () => {
    loadQueue([queuedMessage('queue-first', 'wait for approval', 10)])
    testState.preflightProviderTransition.mockResolvedValue({
      conversationId: 'conversation-a',
      targetModelId: 'model-1',
      decision: 'blocked',
      reason: 'pending_approval',
      message: 'A user approval is still pending.'
    })
    const screen = await renderSelectedConversation()
    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await vi.advanceTimersByTimeAsync(10_000)
    vi.useRealTimers()

    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(1)
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('disabling auto-send preserves the remaining queue and does not cancel the active reply', async () => {
    loadQueue([
      queuedMessage('queue-first', 'start now', 10),
      queuedMessage('queue-second', 'keep for later', 20)
    ])
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()
    finishRun('run-1')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')
  })

  it.each(['waiting_for_approval', 'waiting_for_user_input'] as const)(
    'keeps auto-send enabled but waits for the same run to finish while %s',
    async (status) => {
      loadQueue([
        queuedMessage('queue-first', 'needs approval', 10),
        queuedMessage('queue-second', 'after approval and reply', 20)
      ])
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      emitAgentEvent({ type: 'done', runId: 'run-1', success: true, status })
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent(status)
      await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
      expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
      await expect
        .element(screen.getByTestId('queued-message-ids'))
        .toHaveTextContent('queue-second')

      emitAgentEvent({ type: 'started', runId: 'run-1', toolDefinitions: [] })
      finishRun('run-1')
      await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
      expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe(
        'after approval and reply'
      )
    }
  )

  it.each(['failed', 'cancelled'] as const)(
    'pauses the queue after a %s run until explicitly re-enabled',
    async (status) => {
      loadQueue([
        queuedMessage('queue-first', 'first', 10),
        queuedMessage('queue-second', 'retry manually', 20)
      ])
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      finishRun('run-1', status)
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent(status)
      await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
      await expect
        .element(screen.getByTestId('queued-message-ids'))
        .toHaveTextContent('queue-second')
      expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
      expect(testState.startConversationTurn.mock.calls[1]?.[0].content).toBe('retry manually')
    }
  )

  it('pauses auto-send immediately when Stop is clicked before cancellation is acknowledged', async () => {
    loadQueue([
      queuedMessage('queue-first', 'first', 10),
      queuedMessage('queue-second', 'keep after stop', 20)
    ])
    const cancellation = deferred<boolean>()
    testState.cancelAgentRun.mockReturnValueOnce(cancellation.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')
    finishRun('run-1')
    cancellation.resolve(true)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-second')
  })

  it.each([false, true])(
    'discards a stale preflight after disabling auto-send (re-enable: %s)',
    async (reEnable) => {
      loadQueue([queuedMessage('queue-first', 'send only from the current switch session', 10)])
      const preflight = deferred<{
        conversationId: string
        targetModelId: string
        decision: 'compatible'
        reason: 'same_protocol'
        operationId: string
        transitionToken: string
      }>()
      testState.preflightProviderTransition.mockReturnValueOnce(preflight.promise)
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
      await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      if (reEnable) await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
      preflight.resolve({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'compatible',
        reason: 'same_protocol',
        operationId: 'stale-queue-preflight',
        transitionToken: 'stale-queue-token'
      })

      if (reEnable) {
        await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
        await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
        expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(2)
      } else {
        await new Promise((resolve) => window.setTimeout(resolve, 0))
        await expect
          .element(screen.getByTestId('queue-auto-send-enabled'))
          .toHaveTextContent('false')
        expect(testState.startConversationTurn).not.toHaveBeenCalled()
        await expect
          .element(screen.getByTestId('queued-message-ids'))
          .toHaveTextContent('queue-first')
      }
      expect(testState.startProviderTransition).not.toHaveBeenCalledWith(
        expect.objectContaining({ transitionToken: 'stale-queue-token' })
      )
    }
  )

  it('keeps the switch scoped to its conversation and never sends a different conversation queue', async () => {
    const other = { ...storedConversation(), id: 'conversation-b' }
    testState.persistedConversations.set(other.id, other)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queuedMessage('queue-a', 'send A', 10)]
      }),
      'conversation-b': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queuedMessage('queue-b', 'keep B', 10)]
      })
    })
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'select-conversation-b', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-b')
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    finishRun('run-1')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-b')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('true')
  })

  it('uses the latest queue head after reordering during a running Provider transition', async () => {
    loadQueue([
      queuedMessage('queue-first', 'send last', 10),
      { ...queuedMessage('queue-second', 'send first after transition', 20), modelId: 'model-2' }
    ])
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: running.operationId,
        transitionToken: 'reordered-queue-transition'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-2',
        decision: 'compatible',
        reason: 'no_incompatible_history',
        operationId: 'reordered-next-model-transition',
        transitionToken: 'reordered-next-model-token'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(running)
      .mockResolvedValueOnce(
        completedProviderTransition('model-2', { operationId: 'reordered-next-model-transition' })
      )
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await screen.getByRole('button', { name: 'confirm-model-transition' }).click()
    await expect
      .element(screen.getByTestId('model-transition-operations'))
      .toHaveTextContent(`${running.operationId}:running`)
    await screen.getByRole('button', { name: 'reverse-queued-messages' }).click()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()

    emitProviderTransition({
      ...running,
      status: 'completed',
      modelId: 'model-1',
      completedAt: 40,
      conversationUpdatedAt: 50
    })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.startConversationTurn.mock.calls[0]?.[0]).toEqual(
      expect.objectContaining({ modelId: 'model-2', content: 'send first after transition' })
    )
    expect(testState.startProviderTransition).toHaveBeenCalledTimes(2)
    expect(testState.startProviderTransition).toHaveBeenLastCalledWith({
      conversationId: 'conversation-a',
      targetModelId: 'model-2',
      transitionToken: 'reordered-next-model-token'
    })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
  })

  it('restores acknowledged guidance abandoned by a core restart into the editable queue', async () => {
    const interrupted = storedConversation()
    const assistant = interrupted.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing assistant run fixture')
    assistant.agentRun.timeline.push({
      id: 'user-guidance-client-interrupted',
      type: 'user_guidance',
      guidanceId: 'guidance-interrupted',
      clientMessageId: 'client-interrupted',
      content: 'Recover after restart.\n\nAttachments: recovery.txt',
      attachments: [
        {
          id: 'attachment-interrupted',
          kind: 'file',
          name: 'recovery.txt',
          mimeType: 'text/plain',
          sizeBytes: 7
        }
      ],
      status: 'rejected',
      rejectionCode: 'run_interrupted',
      error: 'Core process restarted.',
      recoverable: true,
      createdAt: 10
    })
    testState.loadConversation.mockResolvedValueOnce(interrupted)
    testState.loadInputAttachments.mockResolvedValueOnce([
      {
        id: 'attachment-interrupted',
        kind: 'file',
        name: 'recovery.txt',
        mimeType: 'text/plain',
        sizeBytes: 7,
        encoding: 'base64',
        data: 'cmVjb3Zlcg=='
      }
    ])

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('recovered-guidance-guidance-interrupted')
    expect(testState.loadInputAttachments).toHaveBeenCalledWith(['attachment-interrupted'])
    await expect
      .poll(() => {
        const draft = testState.saveComposerDraft.mock.calls.at(-1)?.[1] as
          ChatComposerDraft | undefined
        return draft?.queuedMessages[0]
      })
      .toEqual(
        expect.objectContaining({
          clientMessageId: 'client-interrupted',
          content: 'Recover after restart.\n\nAttachments: recovery.txt',
          status: 'error',
          attachments: [expect.objectContaining({ id: 'attachment-interrupted' })]
        })
      )
  })

  it('guides any selected item in the active run and auto-sends the remaining head item', async () => {
    const first = queuedMessage('queue-first', 'send me next', 10)
    const second = queuedMessage('queue-second', 'guide this run', 20)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [first, second]
      })
    })
    mockSuccessfulTurnStarts()
    const steering = deferred<AgentSteerRunOutput>()
    testState.steerAgentRun.mockReturnValueOnce(steering.promise)
    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-first,queue-second')
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    const preflightsBeforeGuidance = testState.preflightProviderTransition.mock.calls.length
    await screen.getByRole('button', { name: 'guide-second-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    expect(testState.steerAgentRun).toHaveBeenCalledWith({
      conversationId: 'conversation-a',
      expectedRunId: 'run-1',
      clientMessageId: 'client-queue-second',
      content: 'guide this run',
      attachments: []
    })
    expect(testState.preflightProviderTransition).toHaveBeenCalledTimes(preflightsBeforeGuidance)
    expect(testState.startConversationTurn.mock.calls[0]?.[0].modelId).toBe('model-1')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:submitting')

    steering.resolve({ guidanceId: 'guidance-1', status: 'queued' })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-first')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:queued')

    emitAgentEvent({
      type: 'guidance_applied',
      runId: 'run-1',
      guidanceId: 'guidance-1',
      clientMessageId: 'client-queue-second',
      content: 'guide this run',
      attachments: [],
      createdAt: 20,
      sequence: 1
    })
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent('client-queue-second:applied')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'finished current run'
    })

    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(
      (testState.startConversationTurn.mock.calls[1]?.[0] as AgentConversationTurnInput).content
    ).toBe('send me next')
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
  })

  it('keeps an auto-send queue item until its required Provider transition succeeds', async () => {
    const queued: ChatQueuedMessage = {
      ...queuedMessage('queue-preserved', 'send after compaction', 10),
      attachments: [
        {
          id: 'attachment-queued',
          kind: 'file',
          name: 'queued.txt',
          mimeType: 'text/plain',
          sizeBytes: 6,
          encoding: 'base64',
          data: 'cXVldWVk'
        }
      ],
      permissionMode: 'custom',
      skills: [skillSelection]
    }
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [queued]
      })
    })
    mockSuccessfulTurnStarts()
    const running = runningProviderTransition('model-1')
    testState.preflightProviderTransition
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'compatible',
        reason: 'same_protocol',
        operationId: 'provider-transition-2',
        transitionToken: 'token-initial-run'
      })
      .mockResolvedValueOnce({
        conversationId: 'conversation-a',
        targetModelId: 'model-1',
        decision: 'requires_compaction',
        reason: 'provider_protocol_changed',
        operationId: running.operationId,
        transitionToken: 'token-queued-run'
      })
    testState.startProviderTransition
      .mockResolvedValueOnce(completedProviderTransition('model-1'))
      .mockResolvedValueOnce(running)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'finished current run'
    })

    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-preserved')
    expect(
      JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent ?? '[]')
    ).toEqual([queued])
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'cancel-model-transition' }).click()
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('queue-preserved')
    expect(
      JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent ?? '[]')
    ).toEqual([queued])
    expect(testState.startProviderTransition).toHaveBeenCalledTimes(1)
  })

  it('accepts guidance when approval arrives before the queued message is submitted', async () => {
    const queued = queuedMessage('approval-race', 'keep this instruction', 10)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued] })
    })
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'approval_required',
      runId: 'run-1',
      action: {
        type: 'command',
        command: {
          id: 'approval-command',
          command: 'sleep 5',
          cwd: null,
          timeoutMs: null,
          approvalStatus: 'required',
          riskLevel: null,
          reason: 'check approval',
          observe: null
        }
      }
    })
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_approval')
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent(`${queued.clientMessageId}:queued`)
    expect(testState.steerAgentRun.mock.calls[0][0].expectedRunId).toBe('run-1')
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_approval')
  })

  it('manually resends a confirmed rejected guidance identity once and ignores its late notifications', async () => {
    const queued = {
      ...queuedMessage('rejected-row', 'retry this instruction', 10),
      attachments: [
        {
          id: 'guidance-attachment',
          kind: 'file' as const,
          name: 'notes.txt',
          mimeType: 'text/plain',
          sizeBytes: 5,
          encoding: 'base64' as const,
          data: 'aGVsbG8='
        }
      ],
      status: 'error' as const,
      error: 'The previous run stopped accepting guidance.'
    }
    const next = queuedMessage('next-row', 'preserve this position', 20)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued, next] })
    })
    mockSuccessfulTurnStarts()
    const original = deferred<AgentSteerRunOutput>()
    const replacement = deferred<AgentSteerRunOutput>()
    testState.steerAgentRun
      .mockReturnValueOnce(original.promise)
      .mockReturnValueOnce(replacement.promise)
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    const rejection: AgentEvent = {
      type: 'guidance_rejected',
      runId: 'run-1',
      guidanceId: 'old-guidance',
      clientMessageId: queued.clientMessageId,
      content: queued.content,
      rejectionCode: 'run_not_steerable',
      message: queued.error,
      createdAt: queued.createdAt
    }
    // Notifications may restore the error row before the original RPC has acknowledged it.
    emitAgentEvent(rejection)
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    expect(testState.steerAgentRun).toHaveBeenCalledTimes(1)
    original.resolve({
      guidanceId: 'old-guidance',
      status: 'rejected',
      rejectionCode: 'run_not_steerable'
    })
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(2)
    const replacementInput = testState.steerAgentRun.mock.calls[1][0]
    expect(replacementInput.clientMessageId).not.toBe(queued.clientMessageId)
    expect(replacementInput).toEqual({
      conversationId: 'conversation-a',
      expectedRunId: 'run-1',
      clientMessageId: expect.any(String),
      content: queued.content,
      attachments: queued.attachments
    })
    emitAgentEvent(rejection)
    await expect
      .element(screen.getByTestId('queued-message-ids'))
      .toHaveTextContent('rejected-row,next-row')
    replacement.resolve({ guidanceId: 'new-guidance', status: 'queued' })
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('next-row')
    emitAgentEvent(rejection)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('next-row')
    await expect
      .element(screen.getByTestId('guidance-timeline'))
      .toHaveTextContent(`${replacementInput.clientMessageId}:queued`)
    expect(screen.getByTestId('guidance-timeline').element().textContent).not.toContain(
      queued.clientMessageId
    )
    expect(testState.steerAgentRun).toHaveBeenCalledTimes(2)
  })

  it.each(['queued', 'applied', 'identity_conflict', 'transport_error'] as const)(
    'preserves the original guidance identity after an error when Host reports %s',
    async (outcome) => {
      const queued = {
        ...queuedMessage('uncertain-row', 'do not duplicate this message', 10),
        status: 'error' as const,
        error: 'No acknowledgement received'
      }
      testState.loadComposerDrafts.mockResolvedValueOnce({
        'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued] })
      })
      mockSuccessfulTurnStarts()
      if (outcome === 'transport_error') {
        testState.steerAgentRun.mockRejectedValueOnce(new Error('Transport unavailable'))
      } else {
        testState.steerAgentRun.mockResolvedValueOnce({
          guidanceId: 'original-guidance',
          status: outcome === 'identity_conflict' ? 'rejected' : outcome,
          ...(outcome === 'identity_conflict' ? { rejectionCode: 'identity_conflict' } : {})
        })
      }
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      await screen.getByRole('button', { name: 'guide-first-message' }).click()
      if (outcome === 'queued' || outcome === 'applied') {
        await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
        await expect
          .element(screen.getByTestId('guidance-timeline'))
          .toHaveTextContent(`${queued.clientMessageId}:${outcome}`)
      } else {
        await expect
          .poll(
            () =>
              JSON.parse(
                screen.getByTestId('queued-message-payloads').element().textContent ?? '[]'
              )[0]?.status
          )
          .toBe('error')
        expect(
          JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent ?? '[]')[0]
            .clientMessageId
        ).toBe(queued.clientMessageId)
      }
      expect(testState.steerAgentRun).toHaveBeenCalledExactlyOnceWith({
        conversationId: 'conversation-a',
        expectedRunId: 'run-1',
        clientMessageId: queued.clientMessageId,
        content: queued.content,
        attachments: []
      })
    }
  )

  it.each(['guidance_queued', 'guidance_applied'] as const)(
    'keeps the authoritative %s receipt when the RPC acknowledgement is lost',
    async (type) => {
      const queued = queuedMessage('lost-reply', 'accepted once', 10)
      testState.loadComposerDrafts.mockResolvedValueOnce({
        'conversation-a': createComposerDraft({ modelId: 'model-1', queuedMessages: [queued] })
      })
      mockSuccessfulTurnStarts()
      const reply = deferred<AgentSteerRunOutput>()
      testState.steerAgentRun.mockReturnValueOnce(reply.promise)
      const screen = await renderSelectedConversation()
      await screen.getByRole('button', { name: 'submit-without-skill' }).click()
      await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
      await screen.getByRole('button', { name: 'guide-first-message' }).click()
      await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
      emitAgentEvent({
        type,
        runId: 'run-1',
        guidanceId: 'accepted-guidance',
        clientMessageId: queued.clientMessageId,
        content: queued.content,
        attachments: [],
        createdAt: queued.createdAt,
        ...(type === 'guidance_applied' ? { sequence: 1 } : {})
      } as AgentEvent)
      await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
      reply.reject(new Error('Acknowledgement lost'))
      await new Promise((resolve) => window.setTimeout(resolve, 0))
      await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('')
      await expect
        .element(screen.getByTestId('guidance-timeline'))
        .toHaveTextContent(
          `${queued.clientMessageId}:${type === 'guidance_queued' ? 'queued' : 'applied'}`
        )
      expect(testState.steerAgentRun).toHaveBeenCalledTimes(1)
    }
  )

  it('auto-sends the next row when the steer acknowledgement arrives after run completion', async () => {
    const first = queuedMessage('queue-first', 'late guidance', 10)
    const second = queuedMessage('queue-second', 'send after the race', 20)
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        modelId: 'model-1',
        projectId: 'project-a',
        queuedMessages: [first, second]
      })
    })
    mockSuccessfulTurnStarts()
    const steering = deferred<AgentSteerRunOutput>()
    testState.steerAgentRun.mockReturnValueOnce(steering.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await screen.getByRole('button', { name: 'guide-first-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'finished before steering acknowledgement'
    })
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)

    steering.resolve({ guidanceId: 'guidance-late', status: 'queued' })
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)
    expect(
      (testState.startConversationTurn.mock.calls[1]?.[0] as AgentConversationTurnInput).content
    ).toBe('send after the race')
  })
})

describe('unified activated Skill inventory', () => {
  it('restores the unified inventory from persisted message state', async () => {
    const restored = storedConversation()
    const assistantMessage = restored.messages.at(-1)
    if (!assistantMessage?.agentRun) throw new Error('missing assistant run fixture')
    assistantMessage.agentRun.activatedSkills = [explicitSkillSummary, modelSkillSummary]
    assistantMessage.agentRun.explicitSkillSelections = [skillSelection]
    assistantMessage.agentRun.skillActivationRevision = 'activation-sha256-v1:explicit-and-model'
    testState.loadConversation.mockResolvedValueOnce(restored)

    const screen = await renderSelectedConversation()

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(`${explicitSkillSummary.id},${modelSkillSummary.id}`)
  })

  it('merges and persists model activation without turning it into an explicit edit selection', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(explicitSkillSummary.id)

    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:explicit-and-model',
      activatedBy: 'model',
      skill: modelSkillSummary
    })

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(`${explicitSkillSummary.id},${modelSkillSummary.id}`)
    await expect
      .poll(() =>
        testState.saveChatMessageState.mock.calls.some(
          (call) =>
            (call[1] as ChatConversation['messages'][number]).agentRun?.activatedSkills?.length ===
            2
        )
      )
      .toBe(true)
    const persistedMessage = testState.saveChatMessageState.mock.calls
      .map((call) => call[1] as ChatConversation['messages'][number])
      .find((candidate) => candidate.agentRun?.activatedSkills?.length === 2)

    expect(persistedMessage?.agentRun?.skillActivationRevision).toBe(
      'activation-sha256-v1:explicit-and-model'
    )
    expect(persistedMessage?.agentRun?.explicitSkillSelections).toEqual([skillSelection])

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'done'
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    expect(
      (testState.rewriteConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnRewriteInput)
        .turn.skills
    ).toEqual([skillSelection])
  })

  it('keeps a model-only activation out of an edited turn explicit selections', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:model-only',
      activatedBy: 'model',
      skill: modelSkillSummary
    })
    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(modelSkillSummary.id)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'done'
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    expect(
      (testState.rewriteConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnRewriteInput)
        .turn.skills
    ).toBeUndefined()
  })

  it('replays a model activation buffered before the turn start response', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:explicit-and-model',
      activatedBy: 'model',
      skill: modelSkillSummary
    })
    start.resolve(successfulTurnOutput(input, 1))

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(`${explicitSkillSummary.id},${modelSkillSummary.id}`)
    await expect
      .poll(() =>
        testState.saveChatMessageState.mock.calls.some(
          (call) =>
            (call[1] as ChatConversation['messages'][number]).agentRun?.skillActivationRevision ===
            'activation-sha256-v1:explicit-and-model'
        )
      )
      .toBe(true)
  })

  it('restores early workspace Skill resource activity from the authoritative terminal message', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    const assistantMessageId = input.assistantMessageId!
    const resourceUri = `skill://package/${encodeURIComponent(explicitSkillSummary.id)}/${encodeURIComponent(explicitSkillSummary.revision)}/references/workflows.md`

    emitAgentEvent({
      type: 'skill_activated',
      runId: 'run-1',
      activationRevision: 'activation-sha256-v1:workspace-resource',
      activatedBy: 'model',
      skill: explicitSkillSummary
    })
    emitAgentEvent({
      type: 'tool_call',
      runId: 'run-1',
      traceSequence: 1,
      identity: { type: 'builtin', toolName: 'skills_read_resource' },
      call: {
        id: 'read-workspace-resource',
        tool: 'skills_read_resource',
        args: { uri: resourceUri },
        approvalStatus: 'not_required',
        reason: null
      }
    })
    emitAgentEvent({
      type: 'tool_result',
      runId: 'run-1',
      result: {
        callId: 'read-workspace-resource',
        tool: 'skills_read_resource',
        ok: true,
        result: { uri: resourceUri }
      }
    })
    for (let index = 0; index < 128; index += 1) {
      emitAgentEvent({ type: 'message', runId: 'run-1', content: `buffered-${index}` })
    }

    const saveCountBeforeBinding = testState.saveChatMessageState.mock.calls.length
    start.resolve(successfulTurnOutput(input, 1))
    await expect
      .poll(() => testState.saveChatMessageState.mock.calls.length)
      .toBeGreaterThan(saveCountBeforeBinding)
    await expect.element(screen.getByTestId('activated-skill-ids')).toHaveTextContent('')

    const authoritative = storedConversationWithRun(assistantMessageId, 'run-1', 'completed')
    const authoritativeAssistant = authoritative.messages.at(-1)
    if (!authoritativeAssistant?.agentRun) throw new Error('missing authoritative run fixture')
    authoritativeAssistant.agentRun.activatedSkills = [explicitSkillSummary]
    authoritativeAssistant.agentRun.skillActivationRevision =
      'activation-sha256-v1:workspace-resource'
    authoritativeAssistant.agentRun.toolCalls = [
      {
        id: 'read-workspace-resource',
        tool: 'skills_read_resource',
        args: { uri: resourceUri },
        approvalStatus: 'not_required',
        reason: null
      }
    ]
    authoritativeAssistant.agentRun.toolResults = [
      {
        callId: 'read-workspace-resource',
        tool: 'skills_read_resource',
        ok: true,
        result: { uri: resourceUri }
      }
    ]
    authoritativeAssistant.agentRun.timeline = [
      {
        id: 'tool-call-read-workspace-resource',
        type: 'tool_call',
        callId: 'read-workspace-resource'
      }
    ]
    testState.persistedConversations.set('conversation-a', authoritative)
    testState.loadConversation.mockResolvedValueOnce(authoritative)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'authoritative answer'
    })

    await expect
      .element(screen.getByTestId('activated-skill-ids'))
      .toHaveTextContent(explicitSkillSummary.id)
    await expect
      .poll(() =>
        testState.saveChatMessageState.mock.calls.some((call) =>
          (call[1] as ChatMessage).agentRun?.toolCalls.some(
            (toolCall) =>
              toolCall.id === 'read-workspace-resource' && toolCall.tool === 'skills_read_resource'
          )
        )
      )
      .toBe(true)
  })
})

describe('Host-owned turn acceptance', () => {
  const submittedAttachment = {
    id: 'submitted-attachment',
    kind: 'file' as const,
    name: 'question.txt',
    mimeType: 'text/plain',
    sizeBytes: 8,
    encoding: 'base64' as const,
    data: 'cXVlc3Rpb24='
  }

  function prepareDraft() {
    testState.loadComposerDrafts.mockResolvedValueOnce({
      'conversation-a': createComposerDraft({
        message: 'read this file',
        attachments: [submittedAttachment],
        modelId: 'model-1',
        projectId: 'project-a'
      })
    })
  }

  it('keeps optimistic messages local while Host accepts the turn before a delayed metadata save', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()
    const metadata = deferred<void>()
    testState.saveConversationMeta.mockReturnValueOnce(metadata.promise)

    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('starting')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('read this file')
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    expect(input.attachments).toEqual([submittedAttachment])
    start.resolve(successfulTurnOutput(input, 1))

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    metadata.resolve()
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBe(1)
    expect(testState.saveChatMessageState.mock.calls[0]?.[1]).toMatchObject({
      id: input.assistantMessageId,
      createdAt: 21,
      agentRun: { runId: 'run-1', status: 'running' }
    })
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })

  it('persists the rejected local pair with its input and attachments without overwriting a newer draft', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    start.reject(new Error('Host rejected the turn before starting a Run'))

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect.poll(() => testState.upsertChatMessages.mock.calls.length).toBe(1)
    // A transport failure can hide a successful Host start. Never send an unbound
    // failed state over its authoritative assistant; the insert-only fallback is safe.
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages.mock.calls[0]).toEqual([
      'conversation-a',
      [
        expect.objectContaining({
          role: 'user',
          content: 'read this file',
          attachments: [expect.objectContaining({ id: submittedAttachment.id })]
        }),
        expect.objectContaining({
          role: 'assistant',
          status: 'sent',
          agentRun: expect.objectContaining({ runId: null, status: 'failed' })
        })
      ],
      2
    ])
    await expect
      .element(screen.getByTestId('draft-message'))
      .toHaveTextContent('new draft after failure')
  })

  it('keeps an unconfirmed stopped start local instead of overwriting Host state', async () => {
    prepareDraft()
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-draft-content' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    start.reject(new Error('The start response was lost'))

    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    await expect.poll(() => testState.upsertChatMessages.mock.calls.length).toBe(1)
    expect(testState.saveChatMessageState).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages.mock.calls[0]?.[1]).toEqual([
      expect.objectContaining({ role: 'user', content: 'read this file' }),
      expect.objectContaining({
        role: 'assistant',
        agentRun: expect.objectContaining({ runId: null, status: 'cancelled' })
      })
    ])
  })
})

describe('activation failure recovery', () => {
  it('does not restore a selection rejected by the host', async () => {
    testState.startConversationTurn.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'Selection was rejected',
        data: {
          type: 'skillActivation',
          code: 'invalidSelection',
          recovery: 'rejectSelection',
          message: 'Selection was rejected',
          skillId: skillSelection.id
        }
      })
    )

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .not.toHaveTextContent(skillSelection.id)
    expect(
      (testState.saveComposerDraft.mock.calls.at(-1)?.[1] as ChatComposerDraft | undefined)?.skills
    ).toEqual([])
    await expect.element(screen.getByTestId('skill-catalog-refresh-token')).toHaveTextContent('0')
  })

  it('restores the selection and explicitly requests a catalog refresh', async () => {
    testState.startConversationTurn.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'Selection is stale',
        data: {
          type: 'skillActivation',
          code: 'stale',
          recovery: 'refreshCatalog',
          message: 'Selection is stale',
          skillId: skillSelection.id,
          expectedRevision: skillSelection.revision,
          actualRevision: 'skill-sha256-v1:latest'
        }
      })
    )

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect.element(screen.getByTestId('draft-skills')).toHaveTextContent(skillSelection.id)
    await expect.element(screen.getByTestId('skill-catalog-refresh-token')).toHaveTextContent('1')
  })

  it('does not replace a newer live revision when a deferred request reports stale input', async () => {
    const start = deferred<never>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-latest-skill' }).click()
    await expect
      .element(screen.getByTestId('draft-skills'))
      .toHaveTextContent(latestSkillSelection.revision)

    start.reject(
      new HostInvocationError({
        message: 'Selection is stale',
        data: {
          type: 'skillActivation',
          code: 'stale',
          recovery: 'refreshCatalog',
          message: 'Selection is stale',
          skillId: skillSelection.id,
          expectedRevision: skillSelection.revision,
          actualRevision: latestSkillSelection.revision
        }
      })
    )

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .toHaveTextContent(latestSkillSelection.revision)
  })

  it('removes a rejected Skill that was re-selected while the request was in flight', async () => {
    const start = deferred<never>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-latest-skill' }).click()
    await expect.element(screen.getByTestId('draft-skills')).toHaveTextContent(skillSelection.id)

    start.reject(
      new HostInvocationError({
        message: 'Selection was rejected',
        data: {
          type: 'skillActivation',
          code: 'invalidSelection',
          recovery: 'rejectSelection',
          message: 'Selection was rejected',
          skillId: skillSelection.id
        }
      })
    )

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .not.toHaveTextContent(skillSelection.id)
  })
})

describe('edited turn Skill recovery', () => {
  it.each(['running', 'waiting_for_approval', 'waiting_for_user_input'] as const)(
    'rejects edit while the source Turn is %s',
    async (status) => {
      const active = storedConversation()
      const assistant = active.messages.at(-1)
      if (!assistant?.agentRun) throw new Error('missing active edit fixture')
      assistant.status = 'pending'
      assistant.agentRun.status = status
      testState.persistedConversations.set(active.id, active)
      const screen = await renderSelectedConversation()

      await screen.getByRole('button', { name: 'edit-last-message' }).click()
      await new Promise((resolve) => window.setTimeout(resolve, 0))

      expect(testState.rewriteConversationTurn).not.toHaveBeenCalled()
      expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
      await expect
        .element(screen.getByTestId('conversation-message-ids'))
        .toHaveTextContent('user-old,assistant-old')
    }
  )

  it('uses one Agent-domain rewrite without sending or copying conversation history', async () => {
    const longConversation = storedConversation()
    longConversation.messages = [
      ...Array.from({ length: 100 }, (_, index) => [
        {
          id: `history-user-${index}`,
          role: 'user' as const,
          content: `history-user-content-${index}`,
          createdAt: index * 2 + 10,
          status: 'sent' as const
        },
        {
          id: `history-assistant-${index}`,
          role: 'assistant' as const,
          content: `history-assistant-content-${index}`,
          createdAt: index * 2 + 11,
          status: 'sent' as const
        }
      ]).flat(),
      ...longConversation.messages
    ]
    testState.persistedConversations.set(longConversation.id, longConversation)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    expect(input.requestId).toMatch(/^conversation-turn-rewrite-request-/)
    expect(input.sourceUserMessageId).toBe('user-old')
    expect(input.sourceAssistantMessageId).toBe('assistant-old')
    expect(input.turn.conversationId).toBe('conversation-a')
    expect(input.turn.content).toBe('edited')
    expect(JSON.stringify(input)).not.toContain('history-user-content-0')
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .not.toHaveTextContent('user-old')
  })

  it('keeps the current model and replaces only an automatic title without a Provider transition', async () => {
    const automaticTitleConversation = storedConversation()
    automaticTitleConversation.title = createConversationTitle(
      automaticTitleConversation.messages[0]!.content,
      'chat.newConversation'
    )
    testState.persistedConversations.set(automaticTitleConversation.id, automaticTitleConversation)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await expect.element(screen.getByTestId('draft-model-id')).toHaveTextContent('model-2')
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    expect(input.turn.modelId).toBe('model-1')
    expect(input.turn.title).toBe(createConversationTitle('edited', 'chat.newConversation'))
    expect(testState.preflightProviderTransition).not.toHaveBeenCalled()
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('title-conversation-a'))
      .toHaveTextContent(createConversationTitle('edited', 'chat.newConversation'))
  })

  it('keeps the prior Timeline unchanged until the rewrite is durably accepted', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('user-old,assistant-old')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('original|done')

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 101))

    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
  })

  it('keeps the original Timeline and never creates an error pair when rewrite is rejected', async () => {
    testState.rewriteConversationTurn.mockRejectedValueOnce(new Error('rewrite rejected'))
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .toHaveTextContent('user-old,assistant-old')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('original|done')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
  })

  it('recovers a committed rewrite when the first authoritative reload fails', async () => {
    const screen = await renderSelectedConversation()
    const reload = deferred<ChatConversation | null>()
    testState.loadConversation.mockReturnValueOnce(reload.promise)

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('original|done')

    reload.reject(new Error('transient reload failure'))
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    expect(testState.loadConversation.mock.calls.length).toBeGreaterThanOrEqual(3)

    emitAgentEvent({
      type: 'done',
      runId: 'run-100',
      success: true,
      status: 'completed',
      content: 'rewritten answer'
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('rewritten answer')
  })

  it('falls back to the committed output pair and preserves event routing when reload stays down', async () => {
    const screen = await renderSelectedConversation()
    testState.loadConversation
      .mockRejectedValueOnce(new Error('reload unavailable 1'))
      .mockRejectedValueOnce(new Error('reload unavailable 2'))

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    await expect
      .element(screen.getByTestId('conversation-message-ids'))
      .not.toHaveTextContent('user-old')

    emitAgentEvent({
      type: 'done',
      runId: 'run-100',
      success: true,
      status: 'completed',
      content: 'fallback answer'
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('fallback answer')
  })

  it('keeps a terminal rewrite response settled when authoritative reload stays down', async () => {
    let replacementAssistantId = ''
    testState.rewriteConversationTurn.mockImplementationOnce(
      async (input: AgentConversationTurnRewriteInput) => {
        const output = commitSuccessfulRewrite(input, 106)
        replacementAssistantId = output.assistantMessageId
        const conversation = testState.persistedConversations.get(output.conversationId)
        const assistant = conversation?.messages.find(
          (message) => message.id === output.assistantMessageId
        )
        if (!conversation || !assistant?.agentRun) {
          throw new Error('missing terminal rewrite fixture')
        }
        assistant.content = 'rewrite launch failed'
        assistant.status = 'error'
        assistant.agentRun.status = 'failed'
        assistant.agentRun.completedAt = 30
        output.assistantMessage = structuredClone(assistant)
        return output
      }
    )
    const screen = await renderSelectedConversation()
    testState.loadConversation
      .mockRejectedValueOnce(new Error('reload unavailable 1'))
      .mockRejectedValueOnce(new Error('reload unavailable 2'))

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('rewrite launch failed')
    expect(
      testState.saveChatMessageState.mock.calls.some(
        (call) =>
          (call[1] as ChatConversation['messages'][number]).id === replacementAssistantId &&
          (call[1] as ChatConversation['messages'][number]).status === 'pending'
      )
    ).toBe(false)

    emitAgentEvent({
      type: 'done',
      runId: 'run-106',
      success: true,
      status: 'completed',
      content: 'must remain ignored'
    })
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('failed')
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .not.toHaveTextContent('must remain ignored')
  })

  it('deduplicates concurrent edit submission before it reaches the Host', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 102))
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    expect(testState.rewriteConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('keeps next-turn settings and new input queued before an edited turn is acknowledged', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    await screen.getByRole('button', { name: 'select-model-2' }).click()
    await screen.getByRole('button', { name: 'select-custom-permissions' }).click()
    await screen.getByRole('button', { name: 'edit-composer-after-transition-failure' }).click()
    await screen.getByRole('button', { name: 'select-latest-skill' }).click()
    await screen.getByRole('button', { name: 'append-queued-message' }).click()

    rewrite.resolve(commitSuccessfulRewrite(input, 107))
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
    expect(input.turn.modelId).toBe('model-1')
    expect(JSON.parse(screen.getByTestId('draft-payload').element().textContent ?? '{}')).toEqual(
      expect.objectContaining({
        modelId: 'model-2',
        permissionMode: 'custom',
        message: 'new draft after failure',
        skills: [latestSkillSelection]
      })
    )
    await expect.element(screen.getByTestId('queued-message-ids')).toHaveTextContent('queue-late')
    expect(testState.startProviderTransition).not.toHaveBeenCalled()
  })

  it('keeps navigation on another conversation when a background rewrite settles', async () => {
    const conversationB = {
      ...storedConversation(),
      id: 'conversation-b',
      title: 'Second conversation'
    }
    testState.persistedConversations.set(conversationB.id, conversationB)
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-conversation-b', exact: true }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-b')

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 103))
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-b')

    await screen.getByRole('button', { name: 'select-conversation-a', exact: true }).click()
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
  })

  it('does not reopen an archived conversation when its committed rewrite response arrives', async () => {
    const rewrite = deferred<AgentConversationTurnOutput>()
    testState.rewriteConversationTurn.mockReturnValueOnce(rewrite.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'archive-conversation-a' }).click()
    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()

    const input = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    rewrite.resolve(commitSuccessfulRewrite(input, 104))
    await new Promise((resolve) => window.setTimeout(resolve, 0))

    await expect.element(screen.getByTestId('new-conversation-draft')).toBeInTheDocument()
    expect(testState.rewriteConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('reuses the idempotency key when the same rejected edit is retried', async () => {
    let firstAttempt: AgentConversationTurnRewriteInput | null = null
    let committedOutput: AgentConversationTurnOutput | null = null
    testState.rewriteConversationTurn.mockImplementation(
      async (input: AgentConversationTurnRewriteInput) => {
        if (!firstAttempt) {
          firstAttempt = structuredClone(input)
          committedOutput = commitSuccessfulRewrite(input, 105)
          throw new Error('transport interrupted')
        }
        if (JSON.stringify(input) !== JSON.stringify(firstAttempt)) {
          throw new Error('idempotency identity conflict')
        }
        if (!committedOutput) throw new Error('missing committed rewrite output')
        return committedOutput
      }
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await screen.getByRole('button', { name: 'edit-last-message' }).click()
    await expect.poll(() => testState.rewriteConversationTurn.mock.calls.length).toBe(2)

    const first = testState.rewriteConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnRewriteInput
    const second = testState.rewriteConversationTurn.mock
      .calls[1]?.[0] as AgentConversationTurnRewriteInput
    expect(second).toEqual(first)
    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('edited')
  })
})

describe('streaming message persistence checkpoints', () => {
  it('keeps 80ms message rendering while coalescing full saves and flushing terminal state', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect.poll(() => testState.saveChatMessageState.mock.calls.length).toBeGreaterThan(0)
    await Promise.resolve()
    testState.saveChatMessageState.mockClear()

    vi.useFakeTimers()
    for (let index = 1; index <= 9; index += 1) {
      emitAgentEvent({
        type: 'message_delta',
        runId: 'run-1',
        streamId: 'stream-1',
        delta: String(index)
      })
      await vi.advanceTimersByTimeAsync(80)
    }

    await expect
      .element(screen.getByTestId('conversation-message-contents'))
      .toHaveTextContent('123456789')
    expect(testState.saveChatMessageState).toHaveBeenCalledTimes(2)
    expect(CHAT_MESSAGE_CHECKPOINT_INTERVAL_MS).toBe(300)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: '123456789'
    })
    await vi.advanceTimersByTimeAsync(0)
    vi.useRealTimers()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect
      .poll(() => {
        const lastMessage = testState.saveChatMessageState.mock.calls.at(-1)?.[1] as
          ChatMessage | undefined
        return lastMessage?.agentRun?.status
      })
      .toBe('completed')
    expect(testState.saveChatMessageState).toHaveBeenCalledTimes(3)
  })
})

describe('transient LLM retry lifecycle', () => {
  it('updates live state without persisting retryAt into the assistant record', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    const savesBeforeRetry = testState.saveChatMessageState.mock.calls.length

    emitAgentEvent({
      type: 'llm_retry',
      runId: 'run-1',
      streamId: 'stream-1',
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: Date.now() + 5_000,
      attempt: 2,
      maxAttempts: 6
    })

    await expect.element(screen.getByTestId('llm-retry')).toHaveTextContent('"rate_limited"')
    expect(testState.saveChatMessageState).toHaveBeenCalledTimes(savesBeforeRetry)
  })
})

describe('authoritative run cancellation and conversation forking', () => {
  it('retains a suspended Run binding until the same Run resumes and completes', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const readsBeforeWaiting = testState.loadConversation.mock.calls.length
    await expect.element(screen.getByRole('button', { name: 'command-capabilities' })).toBeEnabled()

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'waiting_for_user_input',
      usage: { totalTokens: 12, billableRequestCount: 1 }
    })
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_user_input')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')
    await expect.element(screen.getByRole('button', { name: 'command-compact' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'command-capabilities' })).toBeEnabled()
    expect(testState.loadConversation.mock.calls.length).toBe(readsBeforeWaiting)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
    expect(testState.forkConversation).not.toHaveBeenCalled()

    emitAgentEvent({ type: 'started', runId: 'run-1', toolDefinitions: [] })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('running')
    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'Continued with the submitted answer.',
      usage: { totalTokens: 25, billableRequestCount: 2 }
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.startConversationTurn).toHaveBeenCalledTimes(1)
  })

  it('rehydrates a waiting Run binding and lets Stop settle it authoritatively', async () => {
    const stored = storedConversation()
    const assistant = stored.messages.at(-1)
    if (!assistant?.agentRun) throw new Error('missing human-input Run fixture')
    assistant.status = 'pending'
    assistant.agentRun.status = 'waiting_for_user_input'
    testState.persistedConversations.set(stored.id, stored)
    const screen = await renderSelectedConversation()

    await expect.element(screen.getByRole('button', { name: 'command-compact' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-old')
    await expect
      .element(screen.getByTestId('agent-run-status'))
      .toHaveTextContent('waiting_for_user_input')
    emitAgentEvent({
      type: 'done',
      runId: 'run-old',
      success: false,
      status: 'cancelled',
      content: ''
    })
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('keeps a stopped run command event routed to its original Timeline item', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const turnInput = testState.startConversationTurn.mock
      .calls[0]?.[0] as AgentConversationTurnInput
    const assistantMessageId = turnInput.assistantMessageId!
    const sessionId = 'cmd_1234567890abcdef1234567890abcdef'

    emitAgentEvent({
      type: 'tool_call',
      runId: 'run-1',
      traceSequence: 0,
      identity: { type: 'builtin', toolName: 'run_command' },
      call: {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'python3 snake_game/main.py' },
        approvalStatus: 'approved',
        reason: null
      }
    })
    emitAgentEvent({
      type: 'command_started',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      startedAt: 10
    })
    emitAgentEvent({
      type: 'tool_result',
      runId: 'run-1',
      result: {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: { status: 'running', sessionId, output: '', startedAt: 10 }
      }
    })
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"running"')

    await screen.getByRole('button', { name: 'stop-generating' }).click()
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    emitAgentEvent({
      type: 'command_interrupted',
      runId: 'run-1',
      conversationId: 'conversation-a',
      assistantMessageId,
      projectId: 'project-a',
      callId: 'command-call',
      sessionId,
      endedAt: 20,
      latestSequence: 0,
      outputTruncated: false
    })

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('cancelled')
    await expect.element(screen.getByTestId('command-sessions')).toHaveTextContent('"interrupted"')
  })

  it('keeps a stopped run pending until the backend terminal event is received', async () => {
    mockSuccessfulTurnStarts()
    const cancellation = deferred<boolean>()
    testState.cancelAgentRun.mockReturnValueOnce(cancellation.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')
    await screen.getByRole('button', { name: 'stop-generating' }).click()

    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    cancellation.resolve(true)

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    const persistedTerminal = testState.saveChatMessageState.mock.calls
      .map((call) => call[1] as ChatConversation['messages'][number])
      .find((message) => message.agentRun?.runId === 'run-1' && message.status === 'sent')
    expect(persistedTerminal?.agentRun?.status).toBe('cancelled')
  })

  it('binds a run before cancelling when stop is requested during the start RPC', async () => {
    const start = deferred<AgentConversationTurnOutput>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    expect(testState.cancelAgentRun).not.toHaveBeenCalled()

    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    start.resolve(successfulTurnOutput(input, 1))
    await expect.poll(() => testState.cancelAgentRun.mock.calls.length).toBe(1)
    expect(testState.cancelAgentRun).toHaveBeenCalledWith('run-1')
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('pending')

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
  })

  it('reconciles authoritative storage even when cancel reports that no active registration exists', async () => {
    mockSuccessfulTurnStarts()
    testState.cancelAgentRun.mockResolvedValueOnce(false)
    const screen = await renderSelectedConversation()
    const initialLoadCount = testState.loadConversation.mock.calls.length

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput
    testState.loadConversation.mockResolvedValueOnce(
      storedConversationWithRun(input.assistantMessageId!, 'run-1', 'cancelled')
    )

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await vi.advanceTimersByTimeAsync(400)
    vi.useRealTimers()

    await expect.poll(() => testState.loadConversation.mock.calls.length).toBe(initialLoadCount + 1)
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('ends the spinner with an explicit unknown status when cancellation hangs and storage checks fail', async () => {
    mockSuccessfulTurnStarts()
    testState.cancelAgentRun.mockReturnValueOnce(deferred<boolean>().promise)
    const screen = await renderSelectedConversation()
    const initialLoadCount = testState.loadConversation.mock.calls.length
    testState.loadConversation.mockReturnValue(deferred<ChatConversation | null>().promise)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await vi.advanceTimersByTimeAsync(20_000)
    vi.useRealTimers()

    await expect.poll(() => testState.loadConversation.mock.calls.length).toBe(initialLoadCount + 3)
    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
    expect(testState.showToast).toHaveBeenCalledWith('chat.stopStatusUnknown')
  })

  it('does not let a stale storage response overwrite a terminal event received during the read', async () => {
    mockSuccessfulTurnStarts()
    const screen = await renderSelectedConversation()
    const initialLoadCount = testState.loadConversation.mock.calls.length
    const storageRead = deferred<ChatConversation | null>()
    testState.loadConversation.mockReturnValueOnce(storageRead.promise)

    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    const input = testState.startConversationTurn.mock.calls[0]?.[0] as AgentConversationTurnInput

    vi.useFakeTimers()
    await screen.getByRole('button', { name: 'stop-generating' }).click()
    await vi.advanceTimersByTimeAsync(400)
    expect(testState.loadConversation).toHaveBeenCalledTimes(initialLoadCount + 1)

    emitAgentEvent({
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled',
      content: ''
    })
    storageRead.resolve(storedConversationWithRun(input.assistantMessageId!, 'run-1', 'running'))
    await vi.advanceTimersByTimeAsync(10_000)
    vi.useRealTimers()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    expect(testState.loadConversation).toHaveBeenCalledTimes(initialLoadCount + 2)
  })

  it('routes workspace commands to explicit scoped panel opens without sending a model message', async () => {
    testState.gitStatus = 'available'
    testState.loadComposerDrafts.mockResolvedValue({
      'conversation-a': createComposerDraft({
        message: 'keep my draft',
        modelId: 'model-1',
        projectId: 'project-a'
      })
    })
    const screen = await renderSelectedConversation()
    const request = (target: string) =>
      JSON.parse(screen.getByTestId(target).element().textContent!)
    const commandButtons = Array.from(screen.container.querySelectorAll('button')).filter(
      (button) => button.textContent?.startsWith('command-')
    )
    expect(commandButtons[0]?.textContent).toBe('command-model')
    await screen.getByRole('button', { name: 'command-model', exact: true }).click()
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'command-terminal', exact: true }).click()
    expect(testState.openBottomPanel).toHaveBeenCalledTimes(1)
    expect(request('bottom-module-request')).toEqual({
      moduleId: 'terminal',
      requestId: 1,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a',
      conversationId: 'conversation-a'
    })
    await screen.getByRole('button', { name: 'command-terminal', exact: true }).click()
    expect(request('bottom-module-request').requestId).toBe(2)
    expect(testState.openBottomPanel).toHaveBeenCalledTimes(2)
    await screen.getByRole('button', { name: 'command-browser', exact: true }).click()
    expect(request('right-module-request')).toEqual({
      moduleId: 'browser',
      requestId: 1,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a',
      conversationId: 'conversation-a'
    })
    await screen.getByRole('button', { name: 'command-review', exact: true }).click()
    expect(request('right-module-request')).toEqual({
      moduleId: 'git-review',
      requestId: 2,
      workspaceKey: 'project-a',
      workspacePath: '/workspace/a',
      conversationId: 'conversation-a'
    })
    expect(testState.openRightSidebar).toHaveBeenCalledTimes(2)
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('keep my draft')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
  })

  it('hides unavailable review and child-agent commands, and disables terminal when the bottom panel cannot fit', async () => {
    testState.canOpenBottomPanel = false
    const screen = await renderSelectedConversation()
    await expect
      .element(screen.getByRole('button', { name: 'command-review', exact: true }))
      .not.toBeInTheDocument()
    await expect
      .element(screen.getByRole('button', { name: 'command-agents', exact: true }))
      .not.toBeInTheDocument()
    await expect
      .element(screen.getByRole('button', { name: 'command-terminal', exact: true }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-browser', exact: true }))
      .toBeEnabled()
    expect(testState.openBottomPanel).not.toHaveBeenCalled()
  })

  it('keeps repository checking commands disabled without requiring the agent to be idle', async () => {
    testState.gitStatus = 'checking'
    const screen = await renderSelectedConversation()
    await expect
      .element(screen.getByRole('button', { name: 'command-review', exact: true }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-review', exact: true }))
      .toHaveAttribute('title', 'chat.commands.moduleChecking')
    mockSuccessfulTurnStarts()
    await screen.getByRole('button', { name: 'submit-without-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByRole('button', { name: 'command-browser', exact: true }))
      .toBeEnabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-terminal', exact: true }))
      .toBeEnabled()
  })

  it('routes the fork command to latest and deduplicates a concurrent timeline click before navigating', async () => {
    const request = deferred<ChatConversation>()
    testState.forkConversation.mockReturnValueOnce(request.promise)
    const screen = await renderSelectedConversation()
    const command = screen.getByRole('button', { name: 'command-fork' })
    await expect.element(command).toBeEnabled()
    await command.click()
    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()
    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.forkConversation).toHaveBeenCalledWith({
      requestId: expect.any(String),
      sourceConversationId: 'conversation-a',
      forkPoint: { kind: 'latest' }
    })
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    request.resolve({ ...storedConversation(), id: 'fork-created', messagesLoaded: true })
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('fork-created')
    await expect.element(screen.getByTestId('draft-message')).toHaveTextContent('')
    expect(testState.saveComposerDraft).toHaveBeenCalledWith(
      'fork-created',
      expect.objectContaining({
        message: '',
        attachments: [],
        skills: [],
        modelId: 'model-1',
        projectId: 'project-a'
      })
    )
  })

  it('disables the fork command during manual compaction response drain', async () => {
    testState.getManualContextCompactionStatus.mockResolvedValue({
      operations: [
        {
          schemaVersion: 1,
          operationId: 'manual',
          requestId: 'request',
          conversationId: 'conversation-a',
          status: 'cancelled',
          isBusy: true,
          phase: 'generating',
          startedAt: 10,
          updatedAt: 20,
          completedAt: 20
        }
      ]
    })
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-fork' }))
      .toHaveAttribute('title', 'chat.commands.compacting')
    expect(testState.forkConversation).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'toggle-queue-auto-send' }).click()
    await expect.element(screen.getByTestId('queue-auto-send-enabled')).toHaveTextContent('false')
    expect(testState.startConversationTurn).not.toHaveBeenCalled()
    expect(testState.showToast).toHaveBeenCalledWith('chat.commands.compacting')
  })

  it('disables latest fork when the conversation ends with an unanswered user message', async () => {
    testState.loadConversation.mockResolvedValue({
      ...storedConversation(),
      messages: [storedConversation().messages[0]]
    })
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-fork' }))
      .toHaveAttribute('title', 'chat.commands.noForkPoint')
    expect(testState.forkConversation).not.toHaveBeenCalled()
  })

  it('disables fork when a completed turn still owns a running background command', async () => {
    const stored = storedConversationWithCommandRun()
    const session = commandSessionSnapshot()
    stored.messages.at(-1)!.agentRun!.commandSessions = {
      'command-call': session
    }
    testState.loadConversation.mockResolvedValue(stored)
    testState.listAgentCommandSessions.mockResolvedValue({ sessions: [session] })
    testState.getAgentCommandSession.mockResolvedValue(commandSessionGetOutput(session))
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeDisabled()
    await expect
      .element(screen.getByRole('button', { name: 'command-fork' }))
      .toHaveAttribute('title', 'chat.continueInNewTaskActiveCommand')
    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()
    expect(testState.forkConversation).not.toHaveBeenCalled()
  })

  it('allows latest fork from a settled failed reply', async () => {
    testState.loadConversation.mockResolvedValue(
      storedConversationWithRun('failed-assistant', 'failed-run', 'failed')
    )
    testState.forkConversation.mockResolvedValue({
      ...storedConversation(),
      id: 'recovery-fork',
      messagesLoaded: true
    })
    const screen = await renderSelectedConversation()
    await expect.element(screen.getByRole('button', { name: 'command-fork' })).toBeEnabled()
    await screen.getByRole('button', { name: 'command-fork' }).click()
    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('recovery-fork')
    expect(testState.forkConversation.mock.calls[0]?.[0].forkPoint).toEqual({ kind: 'latest' })
  })

  it('opens an existing continuation source at the original reply', async () => {
    const forked = {
      ...storedConversation(),
      continuationOrigin: {
        sourceConversationId: 'conversation-source',
        sourceMessageId: 'assistant-old',
        boundaryMessageId: 'assistant-old'
      }
    }
    const source = {
      ...storedConversation(),
      id: 'conversation-source',
      title: 'Source'
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...forked, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation
      .mockReset()
      .mockResolvedValueOnce(forked)
      .mockResolvedValueOnce(source)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-continuation-origin' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-source')
    expect(testState.loadConversation.mock.calls).toEqual([
      ['conversation-a'],
      ['conversation-source']
    ])
    expect(testState.showToast).not.toHaveBeenCalled()
  })

  it('keeps the current task when the continuation source is archived', async () => {
    const forked = {
      ...storedConversation(),
      continuationOrigin: {
        sourceConversationId: 'conversation-source',
        sourceMessageId: 'assistant-old',
        boundaryMessageId: 'assistant-old'
      }
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...forked, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation
      .mockReset()
      .mockResolvedValueOnce(forked)
      .mockResolvedValueOnce({
        ...storedConversation(),
        id: 'conversation-source',
        archivedAt: 10
      })

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-continuation-origin' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    expect(testState.showToast).toHaveBeenCalledWith('chat.continuationOriginArchived')
  })

  it('keeps the current task when the continuation source was deleted', async () => {
    const forked = {
      ...storedConversation(),
      continuationOrigin: {
        sourceConversationId: 'conversation-source',
        sourceMessageId: 'assistant-old',
        boundaryMessageId: 'assistant-old'
      }
    }
    testState.loadConversationMetas.mockResolvedValueOnce([
      { ...forked, messages: [], messagesLoaded: false }
    ])
    testState.loadConversation.mockReset().mockResolvedValueOnce(forked).mockResolvedValueOnce(null)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'open-continuation-origin' }).click()

    await expect
      .element(screen.getByTestId('active-conversation-id'))
      .toHaveTextContent('conversation-a')
    expect(testState.showToast).toHaveBeenCalledWith('chat.continuationOriginMissing')
  })

  it('shows the localized active-command explanation for a typed fork rejection', async () => {
    testState.forkConversation.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'Conversation fork was rejected.',
        code: -32000,
        data: {
          type: 'conversation_fork',
          code: 'active_command_session',
          conversationId: 'conversation-a',
          activeSessionCount: 1
        }
      })
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()

    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('chat.continueInNewTaskActiveCommand')
  })

  it('does not expose an unknown Core, IPC, or database failure in the fork toast', async () => {
    testState.forkConversation.mockRejectedValueOnce(
      new Error(
        "Error invoking remote method 'host:storage.forkConversation': CoreJsonRpcError: UNIQUE constraint failed"
      )
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()

    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('chat.continueInNewTaskFailed')
  })
})
