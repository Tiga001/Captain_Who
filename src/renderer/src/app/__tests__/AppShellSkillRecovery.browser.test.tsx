import { HostInvocationError } from '@mycopilot/host-api'
import type { SkillSelection } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatSubmitOptions
} from '../../features/chat/chatTypes'

const testState = vi.hoisted(() => ({
  deleteChatMessages: vi.fn(),
  getContextWindowSnapshot: vi.fn(),
  loadComposerDrafts: vi.fn(),
  loadConversations: vi.fn(),
  loadInputAttachments: vi.fn(),
  loadUiPreferences: vi.fn(),
  saveChatMessageState: vi.fn(),
  saveComposerDraft: vi.fn(),
  saveConversationMeta: vi.fn(),
  startConversationTurn: vi.fn(),
  upsertChatMessages: vi.fn(),
  enabledModels: [
    {
      id: 'model-1',
      displayName: 'Model One',
      shortName: 'Model 1',
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
  useToast: () => ({ showToast: vi.fn() })
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
  cancelAgentRun: vi.fn(),
  getContextWindowSnapshot: testState.getContextWindowSnapshot,
  listPendingAgentActions: vi.fn().mockResolvedValue([]),
  onAgentEvent: vi.fn(() => () => undefined),
  rejectAgentAction: vi.fn(),
  startConversationTurn: testState.startConversationTurn
}))

vi.mock('../../features/storage/storageClient', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../features/storage/storageClient')>()
  return {
    ...original,
    deleteChatMessages: testState.deleteChatMessages,
    forkConversation: vi.fn(),
    loadComposerDrafts: testState.loadComposerDrafts,
    loadConversations: testState.loadConversations,
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
vi.mock('../../components/sidebar/LeftSidebar', () => ({ LeftSidebar: () => null }))
vi.mock('../../components/sidebar/RightSidebar', () => ({ RightSidebar: () => null }))
vi.mock('../AppShellSettingsView', () => ({ AppShellSettingsView: () => null }))
vi.mock('../../features/chat/NewConversationPage', () => ({ NewConversationPage: () => null }))
vi.mock('../../features/chat/ChatConversationPage', () => ({
  ChatConversationPage: ({
    composerDraft,
    conversation,
    onComposerDraftChange,
    onEditLastUserMessage,
    onSubmitMessage,
    skillCatalogRefreshToken
  }: {
    composerDraft: ChatComposerDraft
    conversation: ChatConversation
    onComposerDraftChange: (draft: ChatComposerDraft) => void
    onEditLastUserMessage: (messageId: string, content: string) => Promise<void>
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
      <button
        type="button"
        onClick={() => void onEditLastUserMessage(conversation.messages[0]?.id ?? '', 'edited')}
      >
        edit-last-message
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

beforeEach(() => {
  testState.deleteChatMessages.mockReset().mockResolvedValue(undefined)
  testState.getContextWindowSnapshot.mockReset().mockResolvedValue({ snapshot: null })
  testState.loadComposerDrafts.mockReset().mockResolvedValue({
    'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' })
  })
  testState.loadConversations.mockReset().mockResolvedValue([storedConversation()])
  testState.loadInputAttachments.mockReset().mockResolvedValue([])
  testState.loadUiPreferences.mockReset().mockResolvedValue({
    ...defaultUiPreferences(),
    showContextWindowUsage: false
  })
  testState.saveChatMessageState.mockReset().mockResolvedValue(undefined)
  testState.saveComposerDraft.mockReset().mockResolvedValue(undefined)
  testState.saveConversationMeta.mockReset().mockResolvedValue(undefined)
  testState.startConversationTurn.mockReset()
  testState.upsertChatMessages.mockReset().mockResolvedValue(undefined)
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

    const screen = await render(<AppShell />)
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

    const screen = await render(<AppShell />)
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('error')
    await expect.element(screen.getByTestId('draft-skills')).toHaveTextContent(skillSelection.id)
    await expect.element(screen.getByTestId('skill-catalog-refresh-token')).toHaveTextContent('1')
  })

  it('does not replace a newer live revision when a deferred request reports stale input', async () => {
    const start = deferred<never>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)

    const screen = await render(<AppShell />)
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

    const screen = await render(<AppShell />)
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
      const screen = await render(<AppShell />)
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
