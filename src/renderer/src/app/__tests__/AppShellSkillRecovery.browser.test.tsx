import { HostInvocationError } from '@mycopilot/host-api'
import type {
  AgentCommandSessionGetOutput,
  AgentCommandSessionSnapshot,
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentEvent,
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation,
  PendingAgentActionSnapshot,
  AgentSteerRunOutput,
  SkillSelection,
  StorageConversationForkPoint
} from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../../features/chat/chatTypes'

const testState = vi.hoisted(() => ({
  cancelAgentRun: vi.fn(),
  deleteChatMessages: vi.fn(),
  forkConversation: vi.fn(),
  getContextWindowSnapshot: vi.fn(),
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
  agentEventListeners: new Set<(event: AgentEvent) => void>(),
  providerTransitionListeners: new Set<(event: AgentProviderTransitionNotification) => void>(),
  providerTransitionSequence: 0,
  preflightProviderTransition: vi.fn(),
  saveChatMessageState: vi.fn(),
  saveComposerDraft: vi.fn(),
  saveConversationMeta: vi.fn(),
  showToast: vi.fn(),
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
  projects: [{ id: 'project-a', name: 'Project A', path: '/workspace/a', createdAt: 1 }]
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
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
    deleteProject: vi.fn(),
    renameProject: vi.fn(),
    showProjectInFolder: vi.fn(),
    togglePinProject: vi.fn()
  })
}))

vi.mock('../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: testState.showToast })
}))

vi.mock('../useShellLayout', () => ({
  useShellLayout: () => ({
    leftOpen: false,
    leftWidth: 0,
    resizeSide: vi.fn(),
    rightMaximized: false,
    rightOpen: false,
    rightWidth: 0,
    shellRef: { current: null },
    toggleLeftSidebar: vi.fn(),
    toggleRightSidebar: vi.fn(),
    toggleRightSidebarMaximized: vi.fn()
  })
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    app: {
      getWindowState: vi.fn().mockResolvedValue({ isFullScreen: false, isMaximized: false }),
      onWindowStateChange: vi.fn(() => () => undefined)
    }
  }
}))

vi.mock('../../features/gitReview/useGitRepositoryCapability', () => ({
  useGitRepositoryCapability: () => ({ status: 'unavailable' })
}))

vi.mock('../../features/agent/agentClient', () => ({
  approveAgentAction: vi.fn(),
  cancelAgentAction: vi.fn(),
  cancelAgentRun: testState.cancelAgentRun,
  getContextWindowSnapshot: testState.getContextWindowSnapshot,
  getProviderTransitionStatus: testState.getProviderTransitionStatus,
  getAgentCommandSession: testState.getAgentCommandSession,
  listAgentCommandSessions: testState.listAgentCommandSessions,
  listPendingAgentActions: testState.listPendingAgentActions,
  onAgentEvent: testState.onAgentEvent,
  onProviderTransition: testState.onProviderTransition,
  preflightProviderTransition: testState.preflightProviderTransition,
  rejectAgentAction: vi.fn(),
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
    saveConversationMeta: testState.saveConversationMeta,
    saveUiPreferences: vi.fn(),
    upsertChatMessages: testState.upsertChatMessages
  }
})

vi.mock('../../components/layout/ResizeHandle', () => ({ ResizeHandle: () => null }))
vi.mock('../shell/sidebar/LeftSidebar', () => ({
  LeftSidebar: ({
    conversations,
    onSelectConversation,
    uiPreferences
  }: {
    conversations: ChatConversation[]
    onSelectConversation: (conversationId: string) => void
    uiPreferences: { translucentSidebar: boolean }
  }) => (
    <div>
      <output data-testid="sidebar-translucent">{String(uiPreferences.translucentSidebar)}</output>
      {conversations.map((conversation: ChatConversation) => (
        <button
          key={conversation.id}
          type="button"
          onClick={() => onSelectConversation(conversation.id)}
        >
          select-{conversation.id}
        </button>
      ))}
    </div>
  )
}))
vi.mock('../../features/rightSidebar/RightSidebar', () => ({ RightSidebar: () => null }))
vi.mock('../AppShellSettingsView', () => ({ AppShellSettingsView: () => null }))
vi.mock('../../features/chat/NewConversationPage', () => ({
  NewConversationPage: ({ draft }: { draft: ChatComposerDraft }) => (
    <output data-testid="new-conversation-draft">{draft.message}</output>
  )
}))
vi.mock('../../features/chat/ChatConversationPage', () => ({
  ChatConversationPage: ({
    composerDraft,
    conversation,
    onComposerDraftChange,
    onContinueInNewTask,
    onEditLastUserMessage,
    onGuideQueuedMessage,
    onModelTransitionCancel,
    onModelTransitionConfirm,
    onModelTransitionRetry,
    onOpenContinuationOrigin,
    onStopGenerating,
    onSubmitMessage,
    modelTransitionConfirmation,
    modelTransitionOperations,
    skillCatalogRefreshToken
  }: {
    composerDraft: ChatComposerDraft
    conversation: ChatConversation
    onComposerDraftChange: (draft: ChatComposerDraft) => void
    onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void
    onEditLastUserMessage: (messageId: string, content: string) => Promise<void>
    onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
    onModelTransitionCancel?: () => void
    onModelTransitionConfirm?: () => void
    onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void
    onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void
    onStopGenerating: () => void
    onSubmitMessage: (message: string, options: ChatSubmitOptions) => void | Promise<boolean | void>
    modelTransitionConfirmation?: { reason: string }
    modelTransitionOperations?: AgentProviderTransitionOperation[]
    skillCatalogRefreshToken?: number
  }) => (
    <div>
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
      <output data-testid="approval-count">
        {conversation.messages.at(-1)?.agentRun?.approvals.length ?? 0}
      </output>
      <output data-testid="active-conversation-id">{conversation.id}</output>
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
          onComposerDraftChange({
            ...composerDraft,
            modelId: 'model-2',
            updatedAt: Math.max(Date.now(), composerDraft.updatedAt + 1)
          })
        }
      >
        select-model-2
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

const [{ AppShell }, { createComposerDraft }, { defaultUiPreferences }] = await Promise.all([
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
          diffs: [],
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
          diffs: [],
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
      approvalStatus: 'approved'
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
    schemaVersion: 1,
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
  testState.cancelAgentRun.mockReset().mockResolvedValue(true)
  testState.deleteChatMessages.mockReset().mockResolvedValue(undefined)
  testState.forkConversation.mockReset()
  testState.getContextWindowSnapshot.mockReset().mockResolvedValue({ snapshot: null })
  testState.getProviderTransitionStatus.mockReset().mockResolvedValue({ operations: [] })
  testState.getAgentCommandSession.mockReset()
  testState.listAgentCommandSessions.mockReset().mockResolvedValue({ sessions: [] })
  testState.listPendingAgentActions.mockReset().mockResolvedValue([])
  testState.loadComposerDrafts.mockReset().mockResolvedValue({
    'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' })
  })
  const stored = storedConversation()
  testState.loadConversation.mockReset().mockResolvedValue(stored)
  testState.loadConversationMetas
    .mockReset()
    .mockResolvedValue([{ ...stored, messages: [], messagesLoaded: false }])
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
  testState.saveConversationMeta.mockReset().mockResolvedValue(undefined)
  testState.showToast.mockReset()
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

describe('provider transition guard', () => {
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

  it('preflights edited-turn resubmission before deleting or replacing history', async () => {
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
    await expect.poll(() => testState.preflightProviderTransition.mock.calls.length).toBe(1)
    expect(testState.deleteChatMessages).not.toHaveBeenCalled()
    expect(testState.upsertChatMessages).not.toHaveBeenCalled()
    await expect
      .element(screen.getByTestId('model-transition-confirmation'))
      .toHaveTextContent('provider_protocol_changed')
  })
})

describe('conversation startup loading', () => {
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
})

describe('managed command Session lifecycle routing', () => {
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
      call: {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'python3 snake_game/main.py' },
        approvalStatus: 'approved'
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
        approvalStatus: 'required'
      }
    ]
    assistant.agentRun.approvals = [
      {
        type: 'command',
        command: {
          id: 'command-call',
          command: 'python3 snake_game/main.py',
          approvalStatus: 'required'
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
            approvalStatus: 'required'
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

    await screen.getByRole('button', { name: 'guide-second-message' }).click()
    await expect.poll(() => testState.steerAgentRun.mock.calls.length).toBe(1)
    expect(testState.steerAgentRun).toHaveBeenCalledWith({
      conversationId: 'conversation-a',
      expectedRunId: 'run-1',
      clientMessageId: 'client-queue-second',
      content: 'guide this run',
      attachments: []
    })
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
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)

    expect(
      (testState.startConversationTurn.mock.calls[1]?.[0] as AgentConversationTurnInput).skills
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
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(2)

    expect(
      (testState.startConversationTurn.mock.calls[1]?.[0] as AgentConversationTurnInput).skills
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

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
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

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
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

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
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

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .not.toHaveTextContent(skillSelection.id)
  })
})

describe('edited turn Skill recovery', () => {
  it.each(['saveConversationMeta', 'deleteChatMessages', 'upsertChatMessages'] as const)(
    'restores the prior Skill selection when %s fails',
    async (failureStage) => {
      const screen = await renderSelectedConversation()
      await expect.element(screen.getByRole('button', { name: 'edit-last-message' })).toBeVisible()

      testState[failureStage].mockRejectedValueOnce(new Error(`${failureStage} failed`))
      await screen.getByRole('button', { name: 'edit-last-message' }).click()

      await expect
        .poll(() => {
          const latestDraft = testState.saveComposerDraft.mock.calls.at(-1)?.[1] as
            ChatComposerDraft | undefined
          return latestDraft?.skills
        })
        .toEqual([skillSelection])
      await expect.element(screen.getByTestId('draft-skills')).toHaveTextContent(skillSelection.id)
    }
  )
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
      call: {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'python3 snake_game/main.py' },
        approvalStatus: 'approved'
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
    expect(testState.loadConversation).toHaveBeenCalledTimes(initialLoadCount + 1)
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
