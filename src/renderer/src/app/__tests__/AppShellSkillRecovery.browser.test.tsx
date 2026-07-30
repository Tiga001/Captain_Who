import { HostInvocationError } from '@mycopilot/host-api'
import type {
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentEvent,
  AgentSteerRunOutput,
  SkillSelection
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
  loadComposerDrafts: vi.fn(),
  loadConversation: vi.fn(),
  loadConversationMetas: vi.fn(),
  loadInputAttachments: vi.fn(),
  loadUiPreferences: vi.fn(),
  onAgentEvent: vi.fn(),
  agentEventListeners: new Set<(event: AgentEvent) => void>(),
  saveChatMessageState: vi.fn(),
  saveComposerDraft: vi.fn(),
  saveConversationMeta: vi.fn(),
  showToast: vi.fn(),
  startConversationTurn: vi.fn(),
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
  listPendingAgentActions: vi.fn().mockResolvedValue([]),
  onAgentEvent: testState.onAgentEvent,
  rejectAgentAction: vi.fn(),
  startConversationTurn: testState.startConversationTurn,
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
    onOpenContinuationOrigin,
    onStopGenerating,
    onSubmitMessage,
    skillCatalogRefreshToken
  }: {
    composerDraft: ChatComposerDraft
    conversation: ChatConversation
    onComposerDraftChange: (draft: ChatComposerDraft) => void
    onContinueInNewTask?: (messageId: string) => void
    onEditLastUserMessage: (messageId: string, content: string) => Promise<void>
    onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
    onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void
    onStopGenerating: () => void
    onSubmitMessage: (message: string, options: ChatSubmitOptions) => void
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
      <output data-testid="active-conversation-id">{conversation.id}</output>
      <output data-testid="activated-skill-ids">
        {conversation.messages
          .at(-1)
          ?.agentRun?.activatedSkills?.map((skill) => skill.id)
          .join(',') ?? ''}
      </output>
      <output data-testid="queued-message-ids">
        {composerDraft.queuedMessages.map((message) => message.id).join(',')}
      </output>
      <output data-testid="guidance-timeline">
        {conversation.messages
          .at(-1)
          ?.agentRun?.timeline.filter((item) => item.type === 'user_guidance')
          .map((item) => `${item.clientMessageId}:${item.status}`)
          .join(',') ?? ''}
      </output>
      <button
        type="button"
        onClick={() => void onEditLastUserMessage(conversation.messages.at(-2)?.id ?? '', 'edited')}
      >
        edit-last-message
      </button>
      <button type="button" onClick={onStopGenerating}>
        stop-generating
      </button>
      <button
        type="button"
        onClick={() => onContinueInNewTask?.(conversation.messages.at(-1)?.id ?? '')}
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
            modelId: 'model-1',
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
            modelId: 'model-1',
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
            skills: [latestSkillSelection],
            updatedAt: Date.now()
          })
        }
      >
        select-latest-skill
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
  testState.onAgentEvent.mockReset().mockImplementation((listener: (event: AgentEvent) => void) => {
    testState.agentEventListeners.add(listener)
    return () => testState.agentEventListeners.delete(listener)
  })
  testState.saveComposerDraft.mockReset().mockResolvedValue(undefined)
  testState.saveConversationMeta.mockReset().mockResolvedValue(undefined)
  testState.showToast.mockReset()
  testState.startConversationTurn.mockReset()
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

describe('authoritative run cancellation and conversation forking', () => {
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

  it('shows the backend fork rejection instead of replacing it with a generic toast', async () => {
    testState.forkConversation.mockRejectedValueOnce(
      new Error('这条回复仍在生成，结束后才能在新任务中继续。')
    )
    const screen = await renderSelectedConversation()

    await screen.getByRole('button', { name: 'continue-in-new-task' }).click()

    await expect.poll(() => testState.forkConversation.mock.calls.length).toBe(1)
    expect(testState.showToast).toHaveBeenCalledWith('这条回复仍在生成，结束后才能在新任务中继续。')
  })
})
