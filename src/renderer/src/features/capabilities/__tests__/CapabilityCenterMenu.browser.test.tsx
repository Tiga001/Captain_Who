import { useState } from 'react'
import { page, userEvent } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ImageGenerationConfiguration, McpServerDetailsView } from '@mycopilot/protocol'
import { IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION } from '@mycopilot/protocol'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import '../../../styles/global.css'
import '../../settings/SettingsPage.css'

const service = vi.hoisted(() => ({
  imageGet: vi.fn(),
  imageSet: vi.fn(),
  imageChanged: vi.fn(),
  humanGet: vi.fn(),
  humanSet: vi.fn(),
  humanChanged: vi.fn(),
  humanResync: vi.fn(),
  collaborationGet: vi.fn(),
  collaborationSet: vi.fn(),
  collaborationChanged: vi.fn(),
  browserList: vi.fn(),
  browserSet: vi.fn(),
  browserChanged: vi.fn(),
  serverList: vi.fn(),
  serverEnable: vi.fn(),
  serverStart: vi.fn(),
  serverDisable: vi.fn(),
  serverChanged: vi.fn(),
  authorize: vi.fn(),
  searchSave: vi.fn(),
  model: { searchMode: 'disabled', tavilyApiKeyStatus: 'configured' },
  submit: vi.fn(),
  stop: vi.fn(),
  back: vi.fn(),
  dialogChanged: vi.fn()
}))
vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    imageGeneration: {
      getConfiguration: service.imageGet,
      setEnabled: service.imageSet,
      onChanged: service.imageChanged
    },
    humanInteraction: {
      getSettings: service.humanGet,
      updateSettings: service.humanSet,
      onSettingsChanged: service.humanChanged,
      onResync: service.humanResync
    },
    agent: {
      getCollaborationSettings: service.collaborationGet,
      updateCollaborationSettings: service.collaborationSet,
      onCollaborationSettingsChanged: service.collaborationChanged
    },
    mcp: {
      listBuiltinCapabilities: service.browserList,
      setBuiltinCapabilityAllowed: service.browserSet,
      onBuiltinCapabilitiesChanged: service.browserChanged,
      listServers: service.serverList,
      enableServer: service.serverEnable,
      startServer: service.serverStart,
      disableServer: service.serverDisable,
      onChanged: service.serverChanged,
      requestLaunchAuthorization: service.authorize
    }
  }
}))
vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/frontendTranslations')
  const translate = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
  return {
    useFrontendConfig: () => ({
      t: translate
    })
  }
})
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    ...service.model,
    saveSearchMode: service.searchSave,
    enabledModels: [{ id: 'model-1', displayName: 'Model', supportsImage: true, enabled: true }]
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], selectProjectDirectory: vi.fn() })
}))
vi.mock('../../chat/components/ImagePreview', () => ({ useImagePreview: () => vi.fn() }))
vi.mock('../../skills/skillsClient', () => ({
  listSkills: async () => ({
    schemaVersion: 1,
    catalogRevision: 'fixture',
    skills: [],
    diagnostics: [],
    truncated: false
  })
}))
vi.mock('../../chat/chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  createComposerAttachmentsFromFiles: async () => [],
  createAttachmentSummary: () => '',
  selectComposerAttachments: async () => [],
  stripAttachmentSummary: (text: string) => text,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment
}))

import { CapabilityCenterMenu } from '../CapabilityCenterMenu'
import { ChatComposer } from '../../chat/components/ChatComposer'
import type { ChatComposerDraft } from '../../chat/chatTypes'

const ok = <T,>(value: T) => ({ ok: true as const, value })
let image: ImageGenerationConfiguration
let human: { enabled: boolean; revision: number; updatedAt: number }
let collaboration: typeof human
let browser: {
  schemaVersion: 1
  kind: 'builtinCapability'
  capabilityId: 'browser_automation'
  displayName: string
  description: string
  userAllowed: boolean
  policyVersion: number
  policyRevision: number
}
let server: McpServerDetailsView
const previousCss = new Map<string, string>()
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function notify(mock: typeof service.imageChanged) {
  ;(mock.mock.calls.at(-1)![0] as () => void)()
}
function Menu() {
  return (
    <div style={{ ...getFrontendCssVariables(), position: 'relative', marginTop: 450, width: 680 }}>
      <CapabilityCenterMenu onBack={service.back} onDialogOpenChange={service.dialogChanged} />
    </div>
  )
}
function Composer() {
  const [draft, setDraft] = useState<ChatComposerDraft>({
    modelId: 'model-1',
    message: '',
    permissionMode: 'default',
    projectId: null,
    attachments: [],
    skills: [],
    queuedMessages: [],
    updatedAt: 0
  })
  return (
    <div style={{ ...getFrontendCssVariables(), marginTop: 450, width: 680 }}>
      <ChatComposer
        commands={[
          { id: 'capabilities', label: '能力中心', description: '快捷开关软件能力与 MCP' }
        ]}
        draft={draft}
        onDraftChange={setDraft}
        onDraftMessageChange={setDraft}
        onSubmitMessage={service.submit}
        onStopGenerating={service.stop}
        portalMenus
        isGenerating
      />
    </div>
  )
}

beforeEach(() => {
  for (const [property, value] of Object.entries(getFrontendCssVariables())) {
    previousCss.set(property, document.documentElement.style.getPropertyValue(property))
    document.documentElement.style.setProperty(property, String(value))
  }
  vi.clearAllMocks()
  for (const subscribe of [
    service.imageChanged,
    service.humanChanged,
    service.humanResync,
    service.collaborationChanged,
    service.browserChanged,
    service.serverChanged
  ])
    subscribe.mockReturnValue(() => {})
  image = {
    adapterId: 'smartmlSeedream',
    endpointUrl: 'https://images.example/v1',
    modelId: 'seedream',
    capabilities: { textToImage: true, imageToImage: false },
    defaults: { sizePreset: '2K', watermark: false },
    credentialStatus: 'configured',
    enabled: false,
    readiness: 'disabled',
    revision: 'image-1'
  }
  human = { enabled: true, revision: 1, updatedAt: 1 }
  collaboration = { enabled: true, revision: 1, updatedAt: 1 }
  browser = {
    schemaVersion: 1,
    kind: 'builtinCapability',
    capabilityId: 'browser_automation',
    displayName: 'Browser automation',
    description: '',
    userAllowed: false,
    policyVersion: 1,
    policyRevision: 1
  }
  server = {
    schemaVersion: 1,
    serverId: 'server-1',
    displayName: 'Local files',
    scope: 'user',
    source: 'userManual',
    transport: 'stdio',
    trust: 'userApproved',
    approvalMode: 'prompt',
    registryRevision: 1,
    configEpoch: 'epoch',
    configDigest: 'digest',
    state: 'disabled',
    enabled: false,
    launchAuthorizationState: 'authorized',
    catalogGeneration: 1,
    catalogCompleteness: 'complete',
    toolCount: 2,
    activeCallCount: 0,
    updatedAtMs: 1,
    executable: '/usr/bin/test-mcp',
    arguments: [],
    cwd: '/tmp',
    createdAtMs: 1
  }
  service.model.searchMode = 'disabled'
  service.model.tavilyApiKeyStatus = 'configured'
  service.imageGet.mockImplementation(async () =>
    ok({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      configuration: { ...image }
    })
  )
  service.imageSet.mockImplementation(async (input) => {
    image = {
      ...image,
      enabled: input.enabled,
      readiness: input.enabled ? 'readyUnverified' : 'disabled',
      revision: 'image-2'
    }
    return ok({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      outcome: 'updated',
      configuration: image
    })
  })
  service.humanGet.mockImplementation(async () => ok({ ...human }))
  service.humanSet.mockImplementation(async (input) => {
    human = { ...human, enabled: input.enabled, revision: human.revision + 1 }
    return ok(human)
  })
  service.collaborationGet.mockImplementation(async () => ok({ ...collaboration }))
  service.collaborationSet.mockImplementation(async (input) => {
    collaboration = {
      ...collaboration,
      enabled: input.enabled,
      revision: collaboration.revision + 1
    }
    return ok(collaboration)
  })
  service.browserList.mockImplementation(async () =>
    ok({ schemaVersion: 1, revision: browser.policyRevision, capabilities: [{ ...browser }] })
  )
  service.browserSet.mockImplementation(async (input) => {
    browser = { ...browser, userAllowed: input.allowed, policyRevision: browser.policyRevision + 1 }
    return ok({ schemaVersion: 1, revision: browser.policyRevision, capability: browser })
  })
  service.serverList.mockImplementation(async () =>
    ok({ schemaVersion: 1, registryRevision: server.registryRevision, servers: [{ ...server }] })
  )
  service.serverEnable.mockImplementation(async () => {
    server = { ...server, enabled: true, registryRevision: 2, updatedAtMs: 2 }
    return ok({ schemaVersion: 1, registryRevision: 2, server: { ...server } })
  })
  service.serverStart.mockImplementation(async () => {
    server = { ...server, state: 'ready', updatedAtMs: 3 }
    return ok({ schemaVersion: 1, registryRevision: 2, server: { ...server } })
  })
  service.serverDisable.mockImplementation(async () => {
    server = { ...server, enabled: false, state: 'disabled', registryRevision: 3, updatedAtMs: 4 }
    return ok({ schemaVersion: 1, registryRevision: 3, server: { ...server } })
  })
  service.searchSave.mockImplementation(async (value) => {
    service.model.searchMode = value
  })
})

afterEach(() => {
  for (const [property, value] of previousCss) {
    if (value) document.documentElement.style.setProperty(property, value)
    else document.documentElement.style.removeProperty(property)
  }
  previousCss.clear()
})

describe('CapabilityCenterMenu domain Host contracts', () => {
  it('lists software and MCP only, and persists all software switches with existing CAS contracts', async () => {
    const screen = await render(<Menu />)
    await expect.element(screen.getByRole('switch', { name: 'Local files' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="switch"]')).toHaveLength(6)
    expect(screen.container.textContent).not.toContain('Skill')
    for (const name of ['图片生成', '人机交互', '多智能体', '浏览器自动化', '联网搜索'])
      await screen.getByRole('switch', { name }).click()
    await expect
      .element(screen.getByRole('switch', { name: '图片生成' }))
      .toHaveAttribute('aria-checked', 'true')
    expect(service.imageSet).toHaveBeenCalledWith({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      expectedRevision: 'image-1',
      enabled: true
    })
    expect(service.humanSet).toHaveBeenCalledWith({ expectedRevision: 1, enabled: false })
    expect(service.collaborationSet).toHaveBeenCalledWith({ expectedRevision: 1, enabled: false })
    expect(service.browserSet).toHaveBeenCalledWith({
      schemaVersion: 1,
      capabilityId: 'browser_automation',
      expectedPolicyRevision: 1,
      allowed: true
    })
    expect(service.searchSave).toHaveBeenCalledWith('auto')
    expect(screen.container.textContent).not.toContain('正在保存')
    expect(service.back).not.toHaveBeenCalled()
  })

  it('uses enable then start for an authorized MCP and the existing stop/disable operation when switched off', async () => {
    const screen = await render(<Menu />)
    const row = screen.getByRole('switch', { name: 'Local files' })
    await row.click()
    await expect.element(row).toHaveAttribute('aria-checked', 'true')
    await expect.poll(() => service.serverStart.mock.calls.length).toBe(1)
    expect(service.serverEnable).toHaveBeenCalledWith({
      schemaVersion: 1,
      serverId: 'server-1',
      precondition: {
        expectedRegistryRevision: 1,
        expectedConfigEpoch: 'epoch',
        expectedConfigDigest: 'digest'
      }
    })
    await expect.element(row).toHaveAttribute('aria-disabled', 'false')
    await row.click()
    await expect.element(row).toHaveAttribute('aria-checked', 'false')
    expect(service.serverDisable).toHaveBeenCalledTimes(1)
    expect(service.authorize).not.toHaveBeenCalled()
  })

  it('shows an acknowledgement-only dialog for missing configuration or MCP authorization without changing state', async () => {
    image.credentialStatus = 'missing'
    service.model.tavilyApiKeyStatus = 'missing'
    service.model.searchMode = 'auto'
    server.launchAuthorizationState = 'required'
    const screen = await render(<Menu />)
    for (const name of ['图片生成', '联网搜索', 'Local files']) {
      await screen.getByRole('switch', { name }).click()
      await expect.element(page.getByRole('dialog')).toBeVisible()
      expect(document.querySelectorAll('.app-confirm-dialog__actions button')).toHaveLength(1)
      await page.getByRole('button', { name: '知道了', exact: true }).click()
    }
    expect(service.imageSet).not.toHaveBeenCalled()
    expect(service.searchSave).not.toHaveBeenCalled()
    expect(service.serverEnable).not.toHaveBeenCalled()
    expect(service.authorize).not.toHaveBeenCalled()
    expect(service.dialogChanged).toHaveBeenCalledWith(true)
    expect(service.back).not.toHaveBeenCalled()
  })

  it('supports filtering, hover Enter, and IME confirmation without an accidental toggle', async () => {
    const screen = await render(<Menu />)
    const search = screen.getByRole('searchbox')
    await expect.element(search).not.toHaveFocus()
    await expect.element(screen.getByRole('region', { name: '能力中心' })).toHaveFocus()
    await search.click()
    await expect.element(search).toHaveFocus()
    await search.fill('人机')
    await expect.element(screen.getByRole('switch', { name: '人机交互' })).toBeVisible()
    expect(screen.container.querySelectorAll('[role="switch"]')).toHaveLength(1)
    const input = search.element()
    input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))
    input.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true })
    )
    input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    expect(service.humanSet).not.toHaveBeenCalled()
    expect(service.back).not.toHaveBeenCalled()
    await search.fill('')
    await screen.getByRole('switch', { name: '多智能体' }).hover()
    // Normal Enter after the composition tail operates on the hovered row.
    await expect.poll(() => performance.now()).toBeGreaterThan(performance.now() + 130)
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => service.collaborationSet.mock.calls.length).toBe(1)
    expect(service.humanSet).not.toHaveBeenCalled()
  })

  it('blocks repeat toggles and reconciles a change notification that arrives during the post-save read', async () => {
    const screen = await render(<Menu />)
    const row = screen.getByRole('switch', { name: '人机交互' })
    await expect.element(row).toHaveAttribute('aria-disabled', 'false')
    const postSaveRead = deferred<ReturnType<typeof ok<typeof human>>>()
    service.humanGet.mockReturnValueOnce(postSaveRead.promise)
    await row.click()
    await expect.element(row).toHaveAttribute('aria-disabled', 'true')
    row.element().dispatchEvent(new MouseEvent('click', { bubbles: true }))
    expect(service.humanSet).toHaveBeenCalledTimes(1)
    human = { enabled: true, revision: 3, updatedAt: 3 }
    notify(service.humanChanged)
    postSaveRead.resolve(ok({ enabled: false, revision: 2, updatedAt: 2 }))
    await expect.element(row).toHaveAttribute('aria-disabled', 'false')
    await expect.element(row).toHaveAttribute('aria-checked', 'true')
    expect(service.humanGet.mock.calls.length).toBeGreaterThanOrEqual(3)
  })

  it('re-reads an indeterminate mutation and retains the actual committed value while showing its error', async () => {
    service.imageSet.mockImplementation(async () => {
      image = { ...image, enabled: true, readiness: 'readyUnverified', revision: 'image-committed' }
      return { ok: false, error: { message: 'transport lost' } }
    })
    const screen = await render(<Menu />)
    await screen.getByRole('switch', { name: '图片生成' }).click()
    await expect.element(page.getByRole('dialog')).toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: '图片生成' }))
      .toHaveAttribute('aria-checked', 'true')
    expect(service.imageGet.mock.calls.length).toBeGreaterThanOrEqual(2)
    expect(page.getByRole('dialog').element().textContent).not.toContain('transport lost')
  })

  it('applies external setting notices and rejects a late older revision', async () => {
    const screen = await render(<Menu />)
    const row = screen.getByRole('switch', { name: '人机交互' })
    await expect.element(row).toHaveAttribute('aria-disabled', 'false')
    human = { enabled: false, revision: 4, updatedAt: 4 }
    notify(service.humanChanged)
    await expect.element(row).toHaveAttribute('aria-checked', 'false')
    service.humanGet.mockResolvedValueOnce(ok({ enabled: true, revision: 2, updatedAt: 2 }))
    notify(service.humanChanged)
    await expect.poll(() => service.humanGet.mock.calls.length).toBe(3)
    await expect.element(row).toHaveAttribute('aria-checked', 'false')
    image = { ...image, enabled: true, readiness: 'readyUnverified', revision: 'image-external' }
    notify(service.imageChanged)
    await expect
      .element(screen.getByRole('switch', { name: '图片生成' }))
      .toHaveAttribute('aria-checked', 'true')
    browser = { ...browser, userAllowed: true, policyRevision: 4 }
    notify(service.browserChanged)
    await expect
      .element(screen.getByRole('switch', { name: '浏览器自动化' }))
      .toHaveAttribute('aria-checked', 'true')
  })

  it('keeps the real Composer capability submenu through a portal error dialog and never sends or stops', async () => {
    image.credentialStatus = 'missing'
    const screen = await render(<Composer />)
    await screen.getByRole('textbox').click()
    await userEvent.keyboard('/')
    await page.getByRole('option', { name: /能力中心/ }).click()
    await expect.element(page.getByRole('switch', { name: 'Local files' })).toBeVisible()
    await page.getByRole('switch', { name: '图片生成' }).click()
    await expect.element(page.getByRole('dialog')).toBeVisible()
    page.getByRole('button', { name: '知道了', exact: true }).element().focus()
    await userEvent.keyboard('{Tab}')
    expect(page.getByRole('dialog').element().contains(document.activeElement)).toBe(true)
    await userEvent.keyboard('{Shift>}{Tab}{/Shift}')
    expect(page.getByRole('dialog').element().contains(document.activeElement)).toBe(true)
    await userEvent.keyboard('{Escape}')
    await expect.element(page.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(page.getByRole('searchbox')).toBeVisible()
    await expect.element(page.getByRole('searchbox')).not.toHaveFocus()
    await page.getByRole('searchbox').click()
    await page.getByRole('searchbox').fill('人机')
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => service.humanSet.mock.calls.length).toBe(1)
    await userEvent.keyboard('{Escape}')
    await expect.element(page.getByRole('option', { name: /能力中心/ })).toBeVisible()
    expect(service.submit).not.toHaveBeenCalled()
    expect(service.stop).not.toHaveBeenCalled()
    await expect.element(screen.getByRole('textbox')).toHaveValue('/')
  })
})
