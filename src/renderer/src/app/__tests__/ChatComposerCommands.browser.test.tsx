import { useEffect, useRef, useState, type ComponentProps } from 'react'
import { page, userEvent } from 'vitest/browser'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ModelConfig } from '../../config/modelConfig'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getFrontendTheme } from '../../config/frontendTheme'
import '../../styles/global.css'

const { execute, fork, submit, stop, changed, translate, workspaceActions } = vi.hoisted(() => ({
  execute: vi.fn(),
  fork: vi.fn(),
  submit: vi.fn(),
  stop: vi.fn(),
  changed: vi.fn(),
  translate: vi.fn((key: string) => key),
  workspaceActions: {
    terminal: vi.fn(),
    browser: vi.fn(),
    review: vi.fn(),
    agents: vi.fn()
  }
}))
const modelState = vi.hoisted(() => ({ enabledModels: [] as ModelConfig[] }))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
const attachmentMocks = vi.hoisted(() => ({ build: vi.fn() }))
vi.mock('../../features/agentRun/useProviderTransition', () => ({
  useProviderTransition: () => ({
    cancelConfirmation: vi.fn(),
    confirm: vi.fn(),
    loadStatus: vi.fn(),
    retry: vi.fn(),
    store: {},
    request: async () => ({ status: 'completed', operation: { modelId: 'model-1' } })
  })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: translate })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => modelState
}))
vi.mock('../../config/ProjectSettingsProvider', async () => {
  const { singleFolderProject } = await import('../../features/projects/__tests__/projectFixtures')
  return {
    useProjectSettings: () => ({
      projects: [
        singleFolderProject({ id: 'keep-project', name: 'Keep project', path: '/repo/keep' })
      ],
      openCreateProjectDialog: vi.fn(async () => null)
    })
  }
})
vi.mock('../../features/chat/components/ImagePreview', () => ({ useImagePreview: () => vi.fn() }))
vi.mock('../../features/skills/skillsClient', () => ({
  listSkills: vi.fn(async () => ({
    schemaVersion: 1,
    catalogRevision: 'fixture',
    skills: [],
    diagnostics: [],
    truncated: false
  }))
}))
// Exercise Composer navigation and portal ownership independently of capability persistence,
// which is covered by CapabilityCenterMenu's Host-client contract tests.
vi.mock('../../features/capabilities/CapabilityCenterMenu', () => ({
  CapabilityCenterMenu: CapabilityMenuFixture
}))
vi.mock('../../features/chat/chatAttachments', () => ({
  buildAgentInputAttachments: attachmentMocks.build,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  loadComposerAttachmentImage: async () => undefined,
  loadComposerAttachmentPreview: async () => undefined,
  createAttachmentSummary: () => '',
  getComposerDroppedFilePath: () => undefined,
  loadComposerFoldersFromPaths: async () => [],
  selectComposerFolders: async () => [],
  selectComposerAttachments: async () => [],
  stripAttachmentSummary: (content: string) => content
}))
import { ChatComposer } from '../../features/chat/components/ChatComposer'
import { createComposerDraft } from '../chatMessageFactory'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { LicenseContext } from '../../features/license/LicenseContext'
import { useAppShellMessageSubmission } from '../useAppShellMessageSubmission'

const WORKSPACE_COMMANDS = [
  { id: 'terminal', label: '打开终端', query: '终端', description: '在底部栏新建终端' },
  { id: 'browser', label: '内置浏览器', query: '浏览器', description: '打开空白浏览器标签页' },
  { id: 'review', label: '查看修改', query: '修改', description: '打开当前项目的审阅页面' },
  { id: 'agents', label: '子智能体', query: '子智能体', description: '打开当前聊天的子智能体列表' }
] as const

function modelFixture(overrides: Partial<ModelConfig> = {}): ModelConfig {
  return {
    id: 'model-1',
    providerModelId: 'generic-api-model',
    displayName: 'Model',
    apiTokenOverrideStatus: 'missing',
    apiTokenOverrideMutation: { type: 'keep' },
    supportsImage: true,
    providerProfileUpdate: { kind: 'unchanged' },
    providerProfileConfig: {
      schemaVersion: 2,
      vendorId: 'generic',
      profile: { id: 'generic_openai_chat', version: 1 },
      settings: { kind: 'generic' }
    },
    inputPrice: '0',
    cachedInputPrice: '',
    outputPrice: '0',
    enabled: true,
    execution: { status: 'available' },
    ...overrides
  }
}

const MODEL_OPTIONS = [
  modelFixture({ contextWindowTokens: 128_000 }),
  modelFixture({
    id: 'model-deepseek',
    providerModelId: 'deepseek-chat-api',
    displayName: 'Reasoning work model',
    supportsImage: false,
    contextWindowTokens: 256_000,
    providerProfileConfig: {
      schemaVersion: 2,
      vendorId: 'deepseek',
      profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'provider_default', effort: 'provider_default' }
      }
    }
  }),
  modelFixture({
    id: 'model-moonshot',
    providerModelId: 'kimi-latest-api',
    displayName: 'Vision work model',
    contextWindowTokens: 1_000_000,
    providerProfileConfig: {
      schemaVersion: 2,
      vendorId: 'moonshot',
      profile: { id: 'moonshot_k3_chat', version: 1 },
      settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
    }
  })
]

function CapabilityMenuFixture({
  onBack,
  onDialogOpenChange
}: {
  onBack: () => void
  onDialogOpenChange?: (open: boolean) => void
}) {
  const [enabled, setEnabled] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const menuRef = useRef<HTMLElement>(null)
  useEffect(() => {
    menuRef.current?.focus({ preventScroll: true })
  }, [])
  useEffect(() => {
    onDialogOpenChange?.(dialogOpen)
    return () => onDialogOpenChange?.(false)
  }, [dialogOpen, onDialogOpenChange])
  return (
    <section aria-label="capability menu" ref={menuRef} tabIndex={-1}>
      <button type="button" onClick={onBack}>
        Back to commands
      </button>
      <input aria-label="Search capabilities" />
      <button
        type="button"
        role="switch"
        aria-label="Search capability"
        aria-checked={enabled}
        onClick={() => setEnabled((current) => !current)}
      >
        Search
      </button>
      <button type="button" onClick={() => setDialogOpen(true)}>
        Show configuration error
      </button>
      {dialogOpen && (
        <ConfirmationDialog
          cancelLabel="Cancel error"
          confirmLabel="Dismiss error"
          title="Capability error"
          onCancel={() => setDialogOpen(false)}
          onConfirm={() => setDialogOpen(false)}
        />
      )}
    </section>
  )
}

function TestComposer({
  message = '',
  disabled = false,
  running = false,
  maintenance = false,
  transition = false,
  portalMenus = false,
  showWorkspaceCommands = false,
  workspaceChecking = false,
  initialDraft,
  onSubmitMessage = submit
}: {
  message?: string
  disabled?: boolean
  running?: boolean
  maintenance?: boolean
  transition?: boolean
  portalMenus?: boolean
  showWorkspaceCommands?: boolean
  workspaceChecking?: boolean
  initialDraft?: ComponentProps<typeof ChatComposer>['draft']
  onSubmitMessage?: ComponentProps<typeof ChatComposer>['onSubmitMessage']
}) {
  const [draft, setDraft] = useState(
    initialDraft ?? createComposerDraft({ modelId: 'model-1', message })
  )
  const update: ComponentProps<typeof ChatComposer>['onDraftChange'] = (next) => {
    changed(next)
    setDraft(next)
  }
  return (
    <div style={{ marginTop: 300, width: 650 }}>
      <ChatComposer
        commands={[
          { id: 'model', label: '模型', description: '查看并切换模型' },
          {
            id: 'compact',
            label: '压缩上下文',
            description: 'Compact',
            disabledReason: disabled ? '聊天空闲时可用' : undefined,
            execute
          },
          { id: 'new', label: '新聊天', description: 'New', execute },
          {
            id: 'fork',
            label: '创建聊天分支',
            description: '从当前最新可用位置创建聊天分支',
            disabledReason: disabled || running || maintenance ? '聊天空闲时可用' : undefined,
            execute: fork
          },
          { id: 'capabilities', label: '能力中心', description: 'Capabilities' },
          ...(showWorkspaceCommands
            ? WORKSPACE_COMMANDS.map(({ id, label, description }) => ({
                id,
                label,
                description,
                disabledReason: workspaceChecking ? '正在检查可用性' : undefined,
                execute: workspaceActions[id]
              }))
            : [])
        ]}
        draft={draft}
        onDraftChange={update}
        onDraftMessageChange={update}
        onSubmitMessage={onSubmitMessage}
        onStopGenerating={stop}
        portalMenus={portalMenus}
        isGenerating={running}
        isManualCompactionRunning={maintenance}
        isModelTransitionRunning={transition}
      />
    </div>
  )
}
let previousRootStyle: string | null
let previousViewport: { width: number; height: number }
beforeEach(async () => {
  vi.clearAllMocks()
  attachmentMocks.build.mockReset().mockImplementation((attachments: unknown[]) => attachments)
  translate.mockImplementation((key: string) => key)
  modelState.enabledModels = [modelFixture()]
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens))) {
    document.documentElement.style.setProperty(key, value)
  }
  previousViewport = { width: window.innerWidth, height: window.innerHeight }
  await page.viewport(1280, 720)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(previousViewport.width, previousViewport.height)
})

it('omits the signed-out login prompt while still requiring login to send a new turn', async () => {
  const requestLogin = vi.fn()
  const view = await render(
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 0,
          status: 'signedOut',
          profile: null,
          error: null,
          remembered: false
        },
        loginRequested: false,
        requestLogin,
        dismissLogin: vi.fn(),
        canStartTurn: () => false,
        logout: async () => ({ ok: true })
      }}
    >
      <TestComposer message="Keep this unsent message" />
    </AccountAuthContext.Provider>
  )
  const input = view.getByRole('textbox', { name: 'chat.inputAria' })
  expect(document.querySelector('.account-login-prompt')).toBeNull()
  expect(document.body.textContent).not.toContain('登录后才能启动新回合，正在运行的任务不受影响。')
  await view.getByRole('button', { name: 'chat.send', exact: true }).click()
  expect(requestLogin).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  await expect.element(input).toHaveValue('Keep this unsent message')

  await input.click()
  await userEvent.keyboard('{Enter}')
  expect(requestLogin).toHaveBeenCalledTimes(2)
  expect(submit).not.toHaveBeenCalled()
  expect(stop).not.toHaveBeenCalled()
  await expect.element(input).toHaveValue('Keep this unsent message')
})

it.each([
  { target: 'new', change: 'logout' },
  { target: 'new', change: 'license' },
  { target: 'existing', change: 'logout' },
  { target: 'existing', change: 'license' }
])(
  'preserves the $target chat draft when $change occurs during attachment preparation',
  async ({ target, change }) => {
    let signedIn = true
    let licensed = true
    let finishAttachments: (attachments: never[]) => void = () => undefined
    attachmentMocks.build.mockImplementation(
      () =>
        new Promise<never[]>((resolve) => {
          finishAttachments = resolve
        })
    )
    const draft = createComposerDraft({ modelId: 'model-1', message: 'Do not lose this message' })
    type SubmissionOptions = Parameters<typeof useAppShellMessageSubmission>[0]
    const conversations: SubmissionOptions['conversations'] =
      target === 'existing'
        ? [
            {
              id: 'existing-chat',
              title: 'Existing chat',
              modelId: 'model-1',
              projectId: null,
              messages: [],
              messagesLoaded: true,
              createdAt: 1,
              updatedAt: 1,
              pinnedAt: null,
              archivedAt: null,
              unreadAt: null
            }
          ]
        : []
    const options: SubmissionOptions = {
      activeConversationIdRef: { current: target === 'existing' ? 'existing-chat' : null },
      activeDraft: draft,
      activeDraftSelectedModel: modelFixture(),
      autoSubmitQueuedMessageRef: { current: vi.fn() },
      conversations,
      conversationsRef: { current: conversations },
      drafts: {},
      draftsRef: { current: { 'existing-chat': draft } },
      editRewriteAttemptsRef: { current: new Map() },
      editRewriteInFlightRef: { current: new Set() },
      editSubmissionSeqRef: { current: 0 },
      enabledModels: [modelFixture()],
      enqueueChatMessagesUpsert: vi.fn(),
      enqueueConversationMetaSave: vi.fn(),
      mutateDraft: (_scope, updater) => updater(draft),
      pendingProviderTransitionSubmissionsRef: { current: new Map() },
      requestAssistantResponse: vi.fn(async () => true),
      restoreSubmittedSkills: vi.fn(),
      setActiveConversationId: vi.fn(),
      setActiveConversationInitialScrollTop: vi.fn(),
      setConversationScrollToBottomSignal: vi.fn(),
      setConversationsWithRef: vi.fn(),
      setScrollTargetMessageId: vi.fn(),
      showToast: vi.fn(),
      t: translate,
      updateDraft: vi.fn(),
      waitForConversationSaves: vi.fn(async () => undefined),
      waitForMessageUpserts: vi.fn(async () => undefined),
      waitForMessageStateSaves: vi.fn(async () => undefined),
      waitForRunSettlement: vi.fn(async () => undefined)
    }
    const accepted = vi.fn()
    function SubmissionComposer() {
      const submission = useAppShellMessageSubmission(options)
      return (
        <TestComposer
          initialDraft={draft}
          onSubmitMessage={async (message, submitOptions) => {
            const result = await submission.submitMessage(message, submitOptions)
            accepted(result)
            return result
          }}
        />
      )
    }
    const screen = await render(
      <AccountAuthContext.Provider
        value={{
          state: { revision: 1, status: 'signedIn', profile: null, remembered: true, error: null },
          loginRequested: false,
          requestLogin: vi.fn(),
          dismissLogin: vi.fn(),
          canStartTurn: () => signedIn,
          logout: vi.fn()
        }}
      >
        <LicenseContext.Provider
          value={{
            state: {
              revision: 1,
              status: 'allowed',
              reason: 'active',
              expiresAt: null,
              verifiedAt: null,
              cacheValidUntil: null,
              error: null
            },
            canStartTurn: () => licensed,
            requestAccess: vi.fn(),
            refresh: vi.fn()
          }}
        >
          <SubmissionComposer />
        </LicenseContext.Provider>
      </AccountAuthContext.Provider>
    )
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })
    await screen.getByRole('button', { name: 'chat.send', exact: true }).click()
    expect(attachmentMocks.build).toHaveBeenCalledTimes(1)
    if (change === 'logout') signedIn = false
    else licensed = false
    finishAttachments([])
    await expect.poll(() => accepted.mock.calls.length).toBe(1)
    expect(accepted).toHaveBeenCalledWith(false)
    await expect.element(input).toHaveValue('Do not lose this message')
    expect(options.requestAssistantResponse).not.toHaveBeenCalled()
    expect(options.setConversationsWithRef).not.toHaveBeenCalled()
    expect(options.updateDraft).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
  }
)

it.each([false, true])(
  'requires a license for new turns but leaves running-turn input alone (running=%s)',
  async (running) => {
    const requestAccess = vi.fn()
    const view = await render(
      <LicenseContext.Provider
        value={{
          state: {
            revision: 1,
            status: 'denied',
            reason: 'expired',
            expiresAt: null,
            verifiedAt: null,
            cacheValidUntil: null,
            error: null
          },
          canStartTurn: () => false,
          requestAccess,
          refresh: vi.fn()
        }}
      >
        <TestComposer message="Keep this message" running={running} />
      </LicenseContext.Provider>
    )
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard('{Enter}')
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
    if (running) {
      expect(requestAccess).not.toHaveBeenCalled()
      expect(changed.mock.lastCall?.[0].queuedMessages).toHaveLength(1)
    } else {
      expect(requestAccess).toHaveBeenCalledTimes(1)
      await expect.element(input).toHaveValue('Keep this message')
    }
  }
)

it.each([false, true])(
  'opens models as the first command, lists API metadata without search, and shares draft selection with the footer (portal=%s)',
  async (portalMenus) => {
    modelState.enabledModels = MODEL_OPTIONS
    const initialDraft = createComposerDraft({
      modelId: 'model-1',
      permissionMode: 'custom',
      projectId: 'keep-project',
      attachments: [
        {
          id: 'attachment',
          kind: 'file',
          name: 'keep-model-context.txt',
          mimeType: 'text/plain',
          sizeBytes: 4,
          encoding: 'managed',
          data: 'managed-test-a2VlcA=='
        }
      ],
      skills: [{ id: 'bundled:application:documents', revision: 'keep-revision' }]
    })
    const view = await render(
      <TestComposer portalMenus={portalMenus} initialDraft={initialDraft} />
    )
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    const footer = view.getByRole('button', { name: 'chat.selectModel' })
    await footer.click()
    const footerOptions = view.getByRole('listbox', { name: 'chat.selectModel' })
    expect(footerOptions.element().querySelectorAll('[role="option"]')).toHaveLength(3)
    for (const model of MODEL_OPTIONS) {
      await expect
        .element(footerOptions.getByRole('option', { name: new RegExp(model.displayName) }))
        .toBeVisible()
    }
    await footerOptions.getByRole('option', { name: /Reasoning work model/ }).click()
    expect(changed.mock.lastCall?.[0]).toMatchObject({ modelId: 'model-deepseek' })

    await input.click()
    await userEvent.keyboard('/')
    expect(document.querySelector('.composer-commands [role="option"]')?.textContent).toContain(
      '模型'
    )
    await userEvent.keyboard('{Enter}')
    const menu = view.getByRole('listbox', { name: 'chat.selectModel' })
    await expect.element(menu).toBeVisible()
    expect(menu.element().querySelectorAll('[role="option"]')).toHaveLength(3)
    expect(menu.element().querySelector('input, textarea, [role="searchbox"]')).toBeNull()
    const generic = menu.getByRole('option', { name: /generic-api-model/ })
    const deepseek = menu.getByRole('option', { name: /deepseek-chat-api/ })
    const moonshot = menu.getByRole('option', { name: /kimi-latest-api/ })
    await expect.element(generic).toHaveTextContent('chat.models.providerGeneric')
    await expect.element(generic).toHaveTextContent('128K')
    await expect.element(generic).toHaveTextContent('configuration.image')
    await expect.element(deepseek).toHaveTextContent('chat.models.providerDeepseek')
    await expect.element(deepseek).toHaveTextContent('256K')
    await expect.element(deepseek).toHaveTextContent('configuration.text')
    await expect.element(moonshot).toHaveTextContent('chat.models.providerMoonshot')
    await expect.element(moonshot).toHaveTextContent('1000K')
    await expect.element(moonshot).toHaveTextContent('configuration.image')
    expect(menu.element().textContent).not.toContain('Reasoning work model')
    await expect.element(deepseek).toHaveAttribute('aria-selected', 'true')
    expect(deepseek.element().querySelector('.lucide-check')).not.toBeNull()

    const writes = changed.mock.calls.length
    await moonshot.click()
    expect(changed).toHaveBeenCalledTimes(writes + 1)
    expect(changed.mock.lastCall?.[0]).toMatchObject({
      message: '',
      modelId: 'model-moonshot',
      attachments: initialDraft.attachments,
      skills: initialDraft.skills,
      permissionMode: initialDraft.permissionMode,
      projectId: initialDraft.projectId,
      queuedMessages: []
    })
    await expect.element(input).toHaveValue('')
    await expect.element(footer).toHaveTextContent('Vision work model')
    await expect.element(view.getByText('keep-model-context.txt')).toBeVisible()
    await expect.element(menu).not.toBeInTheDocument()

    await input.click()
    await userEvent.keyboard('/模型{Enter}')
    await expect
      .element(menu.getByRole('option', { name: /kimi-latest-api/ }))
      .toHaveAttribute('aria-selected', 'true')
    await menu.getByRole('option', { name: /kimi-latest-api/ }).click()
    await expect.element(input).toHaveValue('')
    expect(changed.mock.lastCall?.[0]).toMatchObject({ modelId: 'model-moonshot', message: '' })
    expect(execute).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
  }
)

it.each([false, true])(
  'returns from the model page to the same slash query without selecting or submitting (portal=%s)',
  async (portalMenus) => {
    modelState.enabledModels = MODEL_OPTIONS
    const view = await render(<TestComposer portalMenus={portalMenus} />)
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard('/model{Enter}')
    const writes = changed.mock.calls.length
    await view.getByRole('button', { name: 'capabilityCenter.back' }).click()
    await expect.element(input).toHaveFocus()
    await expect.element(input).toHaveValue('/model')
    await expect.element(view.getByRole('option')).toHaveTextContent('模型')
    for (const key of ['Escape', 'Backspace', 'Delete']) {
      await userEvent.keyboard('{Enter}')
      await expect.element(view.getByRole('listbox', { name: 'chat.selectModel' })).toBeVisible()
      await userEvent.keyboard(`{${key}}`)
      await expect.element(input).toHaveFocus()
      await expect.element(input).toHaveValue('/model')
      await expect.element(view.getByRole('option')).toHaveTextContent('模型')
    }
    expect(changed).toHaveBeenCalledTimes(writes)
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
  }
)

it('keeps both model menus available when a run starts and changes only the next-turn draft', async () => {
  modelState.enabledModels = MODEL_OPTIONS
  const view = await render(<TestComposer portalMenus />)
  const input = view.getByRole('textbox', { name: 'chat.inputAria' })
  const footer = view.getByRole('button', { name: 'chat.selectModel' })
  await expect.element(footer).not.toHaveAttribute('title')
  await input.click()
  await userEvent.keyboard('/model{Enter}')
  await view.rerender(<TestComposer portalMenus running />)
  const menu = view.getByRole('listbox', { name: 'chat.selectModel' })
  await expect.element(footer).toBeEnabled()
  await expect.element(footer).toHaveAttribute('title', 'chat.nextTurnConfigurationHint')
  await menu.getByRole('option', { name: /deepseek-chat-api/ }).click()
  expect(changed.mock.lastCall?.[0]).toMatchObject({ modelId: 'model-deepseek', message: '' })
  await expect.element(input).toHaveValue('')
  await footer.click()
  await view.getByRole('option', { name: /Vision work model/ }).click()
  expect(changed.mock.lastCall?.[0]).toMatchObject({ modelId: 'model-moonshot', message: '' })
  expect(changed.mock.lastCall?.[0].queuedMessages).toEqual([])
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  expect(stop).not.toHaveBeenCalled()
})

it.each(['model', 'capabilities'])(
  'returns from the %s submenu with Delete or Backspace after refocusing the composer without deleting its slash query',
  async (command) => {
    modelState.enabledModels = MODEL_OPTIONS
    const view = await render(<TestComposer portalMenus />)
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard(`/${command}{Enter}`)
    for (const key of ['Backspace', 'Delete']) {
      await expect.element(view.getByRole('region')).toBeVisible()
      await input.click()
      const writes = changed.mock.calls.length
      await userEvent.keyboard(`{${key}}`)
      await expect.element(input).toHaveFocus()
      await expect.element(input).toHaveValue(`/${command}`)
      await expect.element(view.getByRole('region')).not.toBeInTheDocument()
      await expect.element(view.getByRole('option')).toBeVisible()
      expect(changed).toHaveBeenCalledTimes(writes)
      if (key === 'Backspace') await userEvent.keyboard('{Enter}')
    }
    expect(execute).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
  }
)

it.each([{ maintenance: true }, { transition: true }])(
  'shares footer busy protection when state changes while the model page is open: %j',
  async (busy) => {
    modelState.enabledModels = MODEL_OPTIONS
    const view = await render(<TestComposer portalMenus />)
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard('/model{Enter}')
    const menu = view.getByRole('listbox', { name: 'chat.selectModel' })
    const option = menu.getByRole('option', { name: /deepseek-chat-api/ })
    await expect.element(option).toBeEnabled()
    await view.rerender(<TestComposer portalMenus {...busy} />)
    await expect.element(menu).toBeVisible()
    await expect.element(view.getByRole('button', { name: 'chat.selectModel' })).toBeDisabled()
    await expect.element(option).toBeDisabled()
    await expect.element(option).toHaveAttribute('aria-disabled', 'true')
    const writes = changed.mock.calls.length
    option.element().dispatchEvent(new MouseEvent('click', { bubbles: true }))
    option
      .element()
      .dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
      )
    view.container.querySelector('form')!.requestSubmit()
    expect(changed).toHaveBeenCalledTimes(writes)
    await expect.element(input).toHaveValue('/model')
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()

    await view.getByRole('button', { name: 'capabilityCenter.back' }).click()
    await view.getByRole('option', { name: '模型 查看并切换模型' }).click()
    await expect.element(option).toBeDisabled()
    await view.rerender(<TestComposer portalMenus />)
    await expect.element(option).toBeEnabled()
    await option.click()
    expect(changed.mock.lastCall?.[0]).toMatchObject({ modelId: 'model-deepseek', message: '' })
  }
)

it('shows an empty model page and reflects the current enabled model list after configuration changes', async () => {
  modelState.enabledModels = []
  const view = await render(<TestComposer portalMenus />)
  const input = view.getByRole('textbox', { name: 'chat.inputAria' })
  await expect.element(view.getByRole('button', { name: 'chat.selectModel' })).toBeDisabled()
  await input.click()
  await userEvent.keyboard('/model{Enter}')
  const menu = view.getByRole('listbox', { name: 'chat.selectModel' })
  await expect.element(menu).toHaveTextContent('chat.noEnabledModels')
  expect(menu.element().querySelector('[role="option"]')).toBeNull()

  modelState.enabledModels = MODEL_OPTIONS
  await view.rerender(<TestComposer portalMenus />)
  await expect.element(menu.getByRole('option', { name: /deepseek-chat-api/ })).toBeVisible()
  modelState.enabledModels = [
    modelFixture({
      id: 'model-replacement',
      displayName: 'Replacement model',
      providerModelId: 'replacement-api',
      contextWindowTokens: 32_000,
      supportsImage: false
    })
  ]
  await view.rerender(<TestComposer portalMenus />)
  await expect
    .element(menu.getByRole('option', { name: /deepseek-chat-api/ }))
    .not.toBeInTheDocument()
  expect(menu.element().querySelectorAll('[role="option"]')).toHaveLength(1)
  const replacement = menu.getByRole('option', { name: /replacement-api/ })
  await expect.element(replacement).toHaveTextContent('32K')
  await replacement.click()
  expect(changed.mock.lastCall?.[0]).toMatchObject({
    modelId: 'model-replacement',
    message: ''
  })
  await view.getByRole('button', { name: 'chat.selectModel' }).click()
  await expect.element(menu.getByRole('option')).toHaveTextContent('Replacement model')
  expect(menu.element().querySelectorAll('[role="option"]')).toHaveLength(1)
  expect(submit).not.toHaveBeenCalled()
})

it('does not select a model or leave its menu on IME confirmation and composition Escape', async () => {
  modelState.enabledModels = MODEL_OPTIONS
  const view = await render(<TestComposer portalMenus />)
  const input = view.getByRole('textbox', { name: 'chat.inputAria' })
  await input.click()
  await userEvent.keyboard('/model')
  const textarea = input.element()
  textarea.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))
  textarea.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, keyCode: 229, bubbles: true })
  )
  await expect
    .element(view.getByRole('listbox', { name: 'chat.selectModel' }))
    .not.toBeInTheDocument()
  textarea.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))
  await new Promise((resolve) => setTimeout(resolve, 130))
  await userEvent.keyboard('{Enter}')
  const menu = view.getByRole('listbox', { name: 'chat.selectModel' })
  const option = menu.getByRole('option', { name: /deepseek-chat-api/ })
  const writes = changed.mock.calls.length
  for (const key of ['Enter', 'Escape', 'Backspace', 'Delete']) {
    option.element().dispatchEvent(
      new KeyboardEvent('keydown', {
        key,
        isComposing: true,
        keyCode: 229,
        bubbles: true,
        cancelable: true
      })
    )
  }
  await expect.element(menu).toBeVisible()
  expect(changed).toHaveBeenCalledTimes(writes)
  expect(submit).not.toHaveBeenCalled()
  expect(stop).not.toHaveBeenCalled()
})

it('keeps long portalled model lists within their scroll area without moving the page', async () => {
  modelState.enabledModels = Array.from({ length: 22 }, (_, index) =>
    modelFixture({
      id: `scroll-model-${index}`,
      providerModelId: `scroll-api-${index}`,
      displayName: `Scroll model ${index}`,
      contextWindowTokens: 128_000
    })
  )
  const view = await render(
    <div style={{ height: 1300, paddingTop: 140 }}>
      <TestComposer
        portalMenus
        initialDraft={createComposerDraft({ modelId: 'scroll-model-21' })}
      />
    </div>
  )
  try {
    window.scrollTo(0, 80)
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    const originalScroll = window.scrollY
    await userEvent.keyboard('/model{Enter}')
    const menu = view.getByRole('listbox', { name: 'chat.selectModel' })
    await expect.element(menu).toBeVisible()
    await expect.element(view.getByRole('button', { name: 'capabilityCenter.back' })).toBeVisible()
    expect(menu.element().querySelector('input, textarea')).toBeNull()
    expect(menu.element().scrollHeight).toBeGreaterThan(menu.element().clientHeight)
    await expect.poll(() => menu.element().scrollTop).toBeGreaterThan(0)
    expect(Math.abs(window.scrollY - originalScroll)).toBeLessThanOrEqual(1)
    await userEvent.keyboard('{Home}')
    await expect.poll(() => menu.element().scrollTop).toBeLessThanOrEqual(8)
    await userEvent.keyboard('{End}')
    await expect.poll(() => menu.element().scrollTop).toBeGreaterThan(0)
    expect(Math.abs(window.scrollY - originalScroll)).toBeLessThanOrEqual(1)
    const rect = menu.element().getBoundingClientRect()
    expect(rect.top).toBeGreaterThanOrEqual(0)
    expect(rect.bottom).toBeLessThanOrEqual(window.innerHeight)
    expect(submit).not.toHaveBeenCalled()
  } finally {
    window.scrollTo(0, 0)
  }
})

it('captures the compact model command page with readable API metadata', async () => {
  const labels: Record<string, string> = {
    'chat.commands.model': '模型',
    'chat.selectModel': '选择模型',
    'chat.models.providerGeneric': '通用兼容',
    'chat.models.providerDeepseek': '深度求索',
    'chat.models.providerMoonshot': '月之暗面',
    'configuration.text': '文本',
    'configuration.image': '图片',
    'capabilityCenter.back': '返回命令列表'
  }
  translate.mockImplementation((key: string) => labels[key] ?? key)
  modelState.enabledModels = MODEL_OPTIONS
  const view = await render(<TestComposer portalMenus />)
  await view.getByRole('textbox', { name: 'chat.inputAria' }).click()
  await userEvent.keyboard('/model{Enter}')
  const menu = view.getByRole('region', { name: '模型' })
  await expect.element(menu).toBeVisible()
  await document.fonts.ready
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
  const panel = menu.element()
  const toolbarModelButton = view.getByRole('button', { name: '选择模型' }).element()
  expect(getComputedStyle(panel).fontSize).toBe(getComputedStyle(toolbarModelButton).fontSize)
  expect(getComputedStyle(panel).fontFamily).not.toContain('Times')
  expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth)
  const anchorWidth = view
    .getByRole('textbox', { name: 'chat.inputAria' })
    .element()
    .getBoundingClientRect().width
  expect(Math.abs(panel.getBoundingClientRect().width - anchorWidth)).toBeLessThanOrEqual(2)
  await page.screenshot({
    element: panel,
    path: '.vitest-attachments/slash-model-menu.png'
  })
})

it.each(WORKSPACE_COMMANDS)(
  'dispatches $id through Chinese and English filtering and hovered Enter, preserves attachments, and permits repeated invocation',
  async ({ id, label, query, description }) => {
    const initialDraft = createComposerDraft({
      modelId: 'model-1',
      permissionMode: 'custom',
      attachments: [
        {
          id: 'workspace-attachment',
          kind: 'file',
          name: 'keep-context.txt',
          mimeType: 'text/plain',
          sizeBytes: 4,
          encoding: 'managed',
          data: 'managed-test-a2VlcA=='
        }
      ],
      skills: [{ id: 'bundled:application:documents', revision: 'keep-revision' }]
    })
    const view = await render(
      <TestComposer running showWorkspaceCommands initialDraft={initialDraft} />
    )
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard(`/${query}`)
    await expect.element(view.getByRole('option')).toHaveTextContent(label)
    await userEvent.keyboard('{Enter}')
    expect(workspaceActions[id]).toHaveBeenCalledTimes(1)
    await expect.element(input).toHaveValue('')

    // Reopening an empty Composer starts another local command session.
    await input.click()
    await userEvent.keyboard(`/${id}`)
    await expect.element(view.getByRole('option')).toHaveTextContent(label)
    await userEvent.keyboard('{Enter}')
    expect(workspaceActions[id]).toHaveBeenCalledTimes(2)
    await expect.element(input).toHaveValue('')

    await input.click()
    await userEvent.keyboard('/')
    const hovered = view.getByRole('option', { name: `${label} ${description}` })
    await hovered.hover()
    await expect.element(hovered).toHaveAttribute('aria-selected', 'true')
    await userEvent.keyboard('{Enter}')
    expect(workspaceActions[id]).toHaveBeenCalledTimes(3)
    for (const command of WORKSPACE_COMMANDS) {
      if (command.id !== id) expect(workspaceActions[command.id]).not.toHaveBeenCalled()
    }
    await expect.element(view.getByText('keep-context.txt')).toBeVisible()
    expect(changed.mock.lastCall?.[0]).toMatchObject({
      message: '',
      attachments: initialDraft.attachments,
      skills: initialDraft.skills,
      modelId: initialDraft.modelId,
      permissionMode: initialDraft.permissionMode,
      queuedMessages: []
    })
    expect(execute).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
  }
)

it.each(WORKSPACE_COMMANDS)(
  'keeps checking /$id out of message and queue paths for keyboard and form submission',
  async ({ id, label }) => {
    const view = await render(<TestComposer running showWorkspaceCommands workspaceChecking />)
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard(`/${id}`)
    const row = view.getByRole('option', { name: `${label} 正在检查可用性` })
    await expect.element(row).toHaveAttribute('aria-disabled', 'true')
    await userEvent.keyboard('{Enter}')
    view.container.querySelector('form')!.requestSubmit()
    await userEvent.keyboard('{Escape}{Enter}')
    for (const command of WORKSPACE_COMMANDS)
      expect(workspaceActions[command.id]).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
    expect(changed.mock.calls.every(([draft]) => draft.queuedMessages.length === 0)).toBe(true)
    await expect.element(input).toHaveValue(`/${id}`)
  }
)

it('opens only for a typed slash and executes a local command without a message', async () => {
  const view = await render(<TestComposer />)
  const input = view.getByRole('textbox')
  await input.click()
  await userEvent.keyboard('/compact')
  await expect.element(view.getByRole('option', { name: '压缩上下文 Compact' })).toBeVisible()
  await userEvent.keyboard('{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  await expect.element(input).toHaveValue('')
  expect(changed.mock.lastCall?.[0]).toMatchObject({
    modelId: 'model-1',
    permissionMode: 'default'
  })
})

it('never queues a disabled command through Enter or form submission during a run', async () => {
  const view = await render(<TestComposer disabled running />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/compact{Enter}')
  document.querySelector('form')!.requestSubmit()
  await userEvent.keyboard('{Escape}{Enter}')
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  expect(changed.mock.calls.every(([draft]) => draft.queuedMessages.length === 0)).toBe(true)
  await expect.element(view.getByRole('textbox')).toHaveValue('/compact')
})

it('does not interpret restored slash drafts or pasted paths as commands', async () => {
  const view = await render(<TestComposer message="/compact" />)
  expect(view.container.querySelector('[role="listbox"]')).toBeNull()
  await view.getByRole('textbox').click()
  await userEvent.keyboard('{Enter}')
  expect(submit).toHaveBeenCalledWith('/compact', expect.anything())
  await view.getByRole('textbox').fill('')
  const input = view.container.querySelector('textarea')!
  const clipboardData = new DataTransfer()
  clipboardData.setData('text/plain', '/Users/example/project')
  input.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, clipboardData }))
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(
    input,
    '/Users/example/project'
  )
  input.dispatchEvent(
    new InputEvent('input', {
      bubbles: true,
      inputType: 'insertFromPaste',
      data: '/Users/example/project'
    })
  )
  expect(view.container.querySelector('[role="listbox"]')).toBeNull()
  await userEvent.keyboard('{Enter}')
  expect(submit).toHaveBeenLastCalledWith('/Users/example/project', expect.anything())
  expect(execute).not.toHaveBeenCalled()
})

it('leaves IME confirmation alone, then supports Chinese search and keyboard selection', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/')
  const input = view.container.querySelector('textarea')!
  input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))
  input.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, keyCode: 229, bubbles: true })
  )
  input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  await new Promise((resolve) => setTimeout(resolve, 130))
  await userEvent.keyboard('压缩{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
})

it('allows navigation commands during maintenance and prevents duplicate execution', async () => {
  let resolve!: () => void
  execute.mockImplementationOnce(
    () =>
      new Promise<void>((done) => {
        resolve = done
      })
  )
  const view = await render(<TestComposer disabled maintenance />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/new{Enter}{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  resolve()
})

it('supports mouse execution and sends unknown slash text normally', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/new')
  await view.getByRole('option').click()
  expect(execute).toHaveBeenCalledTimes(1)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/unknown{Enter}')
  expect(submit).toHaveBeenCalledWith('/unknown', expect.anything())
})

it('uses the hovered row for Enter and highlights matching Chinese text', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/')
  await userEvent.hover(view.getByRole('option', { name: '新聊天 New' }))
  await expect
    .element(view.getByRole('option', { name: '新聊天 New' }))
    .toHaveAttribute('aria-selected', 'true')
  await userEvent.keyboard('{Enter}')
  expect(execute).toHaveBeenCalledTimes(1)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/压缩')
  expect(view.container.querySelector('.composer-commands__label strong')?.textContent).toBe('压缩')
})

it('routes the send button to the command while preserving attached files and permission settings', async () => {
  const initialDraft = createComposerDraft({
    modelId: 'model-1',
    permissionMode: 'custom',
    attachments: [
      {
        id: 'attachment',
        kind: 'file',
        name: 'keep.txt',
        mimeType: 'text/plain',
        sizeBytes: 4,
        encoding: 'managed',
        data: 'managed-test-a2VlcA=='
      }
    ]
  })
  const view = await render(<TestComposer initialDraft={initialDraft} />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/compact')
  await view.getByRole('button', { name: '压缩上下文', exact: true }).click()
  expect(execute).toHaveBeenCalledTimes(1)
  expect(submit).not.toHaveBeenCalled()
  expect(changed.mock.lastCall?.[0]).toMatchObject({
    message: '',
    attachments: initialDraft.attachments,
    permissionMode: 'custom',
    modelId: 'model-1'
  })
  await expect.element(view.getByText('keep.txt')).toBeVisible()
})

it('filters the fork command in Chinese and executes the hovered row with Enter', async () => {
  const view = await render(<TestComposer />)
  await view.getByRole('textbox').click()
  await userEvent.keyboard('/分支')
  await expect.element(view.getByRole('option')).toHaveTextContent('创建聊天分支')
  await userEvent.keyboard('{Escape}')
  await view.getByRole('textbox').fill('')
  await userEvent.keyboard('/')
  await userEvent.hover(
    view.getByRole('option', { name: '创建聊天分支 从当前最新可用位置创建聊天分支' })
  )
  await userEvent.keyboard('{Enter}')
  expect(fork).toHaveBeenCalledTimes(1)
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
})

it.each([{ running: true }, { maintenance: true }])(
  'keeps /fork out of message and queue paths while busy: %j',
  async (busy) => {
    const view = await render(<TestComposer {...busy} />)
    await view.getByRole('textbox').click()
    await userEvent.keyboard('/fork{Enter}')
    document.querySelector('form')!.requestSubmit()
    await userEvent.keyboard('{Escape}{Enter}')
    expect(fork).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(changed.mock.calls.every(([draft]) => draft.queuedMessages.length === 0)).toBe(true)
    await expect.element(view.getByRole('textbox')).toHaveValue('/fork')
  }
)

it.each([false, true])(
  'drills into capabilities without changing Composer context and returns to the same query (portal=%s)',
  async (portalMenus) => {
    const initialDraft = createComposerDraft({
      modelId: 'model-1',
      permissionMode: 'custom',
      attachments: [
        {
          id: 'attachment',
          kind: 'file',
          name: 'keep.txt',
          mimeType: 'text/plain',
          sizeBytes: 4,
          encoding: 'managed',
          data: 'managed-test-a2VlcA=='
        }
      ],
      skills: [{ id: 'bundled:application:documents', revision: 'keep-revision' }],
      queuedMessages: [
        {
          id: 'queued',
          clientMessageId: 'queued-client',
          content: 'Keep queued content',
          attachments: [],
          skills: [],
          modelId: 'model-1',
          permissionMode: 'custom',
          projectId: null,
          status: 'pending',
          createdAt: 1
        }
      ]
    })
    const view = await render(
      <TestComposer running portalMenus={portalMenus} initialDraft={initialDraft} />
    )
    const input = view.getByRole('textbox', { name: 'chat.inputAria' })
    await input.click()
    await userEvent.keyboard('/能力')
    await expect.element(view.getByRole('option')).toHaveTextContent('能力中心')
    const writes = changed.mock.calls.length
    await userEvent.keyboard('{Enter}')
    const search = view.getByRole('textbox', { name: 'Search capabilities' })
    await expect.element(search).not.toHaveFocus()
    await expect.element(view.getByRole('region', { name: 'capability menu' })).toHaveFocus()
    await search.click()
    await expect.element(search).toHaveFocus()
    await search.fill('搜索')
    await userEvent.keyboard('{Enter}')
    await view.getByRole('switch', { name: 'Search capability' }).click()
    await expect.element(view.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
    view.container.querySelector('form')!.requestSubmit()
    expect(changed).toHaveBeenCalledTimes(writes)
    expect(changed.mock.lastCall?.[0]).toMatchObject({
      message: '/能力',
      attachments: initialDraft.attachments,
      skills: initialDraft.skills,
      queuedMessages: initialDraft.queuedMessages,
      permissionMode: initialDraft.permissionMode,
      modelId: initialDraft.modelId
    })
    await userEvent.keyboard('{Escape}')
    await expect.element(input).toHaveFocus()
    await expect.element(input).toHaveValue('/能力')
    await expect.element(view.getByRole('option')).toHaveTextContent('能力中心')
    await userEvent.keyboard('{Enter}')
    await expect.element(search).toBeVisible()
    await view.getByRole('button', { name: 'Back to commands' }).click()
    await expect.element(input).toHaveFocus()
    expect(execute).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(stop).not.toHaveBeenCalled()
  }
)

it('keeps a portalled capability menu alive through an error dialog and resumes its focus', async () => {
  const view = await render(<TestComposer portalMenus running />)
  const input = view.getByRole('textbox', { name: 'chat.inputAria' })
  await input.click()
  await userEvent.keyboard('/capabilities{Enter}')
  await view.getByRole('button', { name: 'Show configuration error' }).click()
  await expect.element(view.getByRole('dialog', { name: 'Capability error' })).toBeVisible()
  await view.getByRole('button', { name: 'Dismiss error' }).click()
  await expect.element(view.getByRole('region', { name: 'capability menu' })).toBeVisible()
  await view.getByRole('button', { name: 'Show configuration error' }).click()
  await userEvent.keyboard('{Escape}')
  await expect
    .element(view.getByRole('dialog', { name: 'Capability error' }))
    .not.toBeInTheDocument()
  await expect.element(view.getByRole('region', { name: 'capability menu' })).toBeVisible()
  await userEvent.keyboard('{Escape}')
  await expect.element(input).toHaveFocus()
  await expect.element(view.getByRole('option')).toHaveTextContent('能力中心')
  expect(execute).not.toHaveBeenCalled()
  expect(submit).not.toHaveBeenCalled()
  expect(stop).not.toHaveBeenCalled()
})

it('does not treat capability-search IME confirmation or cancellation as a command or run control', async () => {
  const view = await render(<TestComposer portalMenus running />)
  await view.getByRole('textbox', { name: 'chat.inputAria' }).click()
  await userEvent.keyboard('/capabilities{Enter}')
  await view.getByRole('textbox', { name: 'Search capabilities' }).click()
  const search = document.querySelector<HTMLInputElement>(
    'input[aria-label="Search capabilities"]'
  )!
  for (const key of ['Enter', 'Escape']) {
    search.dispatchEvent(
      new KeyboardEvent('keydown', {
        key,
        isComposing: true,
        keyCode: 229,
        bubbles: true,
        cancelable: true
      })
    )
  }
  await expect.element(view.getByRole('region', { name: 'capability menu' })).toBeVisible()
  expect(submit).not.toHaveBeenCalled()
  expect(stop).not.toHaveBeenCalled()
})
