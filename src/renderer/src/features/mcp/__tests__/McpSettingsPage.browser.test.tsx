import {
  BROWSER_DATA_SCHEMA_VERSION,
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  type McpBuiltinCapabilityListItem,
  type McpBuiltinCapabilityListOutput,
  type McpServerDetailsView,
  type McpServerListItem,
  type McpServerListOutput
} from '@mycopilot/protocol'
import { HostInvocationError } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  addServer: vi.fn(),
  authorizeLaunch: vi.fn(),
  deleteServer: vi.fn(),
  disableServer: vi.fn(),
  enableServer: vi.fn(),
  hook: vi.fn(),
  builtinHook: vi.fn(),
  builtinRefresh: vi.fn(),
  builtinSetAllowed: vi.fn(),
  chooseBrowserDownloadDirectory: vi.fn(),
  clearBrowserData: vi.fn(),
  clearBrowserDownloadHistory: vi.fn(),
  deleteBrowserHistory: vi.fn(),
  getBrowserDataSummary: vi.fn(),
  getBrowserDownloadSettings: vi.fn(),
  getBrowserPreferences: vi.fn(),
  listBrowserHistory: vi.fn(),
  listBrowserDownloadHistory: vi.fn(),
  loadDetails: vi.fn(),
  loadTools: vi.fn(),
  onBrowserDownloadHistoryChanged: vi.fn(),
  onBrowserHistoryChanged: vi.fn(),
  openBrowserHistoryEntry: vi.fn(),
  refresh: vi.fn(),
  refreshCatalog: vi.fn(),
  resetBrowserDownloadDirectory: vi.fn(),
  setBrowserDownloadAskWhereToSave: vi.fn(),
  revealBrowserDownload: vi.fn(),
  restartServer: vi.fn(),
  selectExecutable: vi.fn(),
  selectWorkingDirectory: vi.fn(),
  setServerEnabled: vi.fn(),
  showToast: vi.fn(),
  startServer: vi.fn(),
  stopServer: vi.fn(),
  updateBrowserPreferences: vi.fn(),
  updateServer: vi.fn()
}))

vi.mock('../useMcpManagement', () => ({
  useMcpManagement: service.hook
}))

vi.mock('../useBuiltinMcpCapabilities', () => ({
  useBuiltinMcpCapabilities: service.builtinHook
}))

vi.mock('../browserDownloadClient', () => ({
  chooseBrowserDownloadDirectory: service.chooseBrowserDownloadDirectory,
  clearBrowserDownloadHistory: service.clearBrowserDownloadHistory,
  getBrowserDownloadSettings: service.getBrowserDownloadSettings,
  listBrowserDownloadHistory: service.listBrowserDownloadHistory,
  onBrowserDownloadHistoryChanged: service.onBrowserDownloadHistoryChanged,
  resetBrowserDownloadDirectory: service.resetBrowserDownloadDirectory,
  setBrowserDownloadAskWhereToSave: service.setBrowserDownloadAskWhereToSave,
  revealBrowserDownload: service.revealBrowserDownload
}))

vi.mock('../../browser/browserDataClient', () => ({
  clearBrowserData: service.clearBrowserData,
  deleteBrowserHistory: service.deleteBrowserHistory,
  getBrowserDataSummary: service.getBrowserDataSummary,
  getBrowserPreferences: service.getBrowserPreferences,
  listBrowserHistory: service.listBrowserHistory,
  onBrowserHistoryChanged: service.onBrowserHistoryChanged,
  openBrowserHistoryEntry: service.openBrowserHistoryEntry,
  updateBrowserPreferences: service.updateBrowserPreferences
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: service.showToast })
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    resources: { resolveFavicon: vi.fn(async () => ({ url: null })) }
  }
}))

const { McpSettingsPage } = await import('../McpSettingsPage')

const SERVER_ID = '11111111-1111-4111-8111-111111111111'

function builtinCapability(
  overrides: Partial<McpBuiltinCapabilityListItem> = {}
): McpBuiltinCapabilityListItem {
  return {
    schemaVersion: 1,
    kind: 'builtinCapability',
    capabilityId: 'browser_automation',
    displayName: 'Host Browser automation',
    description: 'Host description',
    userAllowed: false,
    policyVersion: 1,
    policyRevision: 0,
    ...overrides
  }
}

function builtinOutput(
  capabilities: McpBuiltinCapabilityListItem[] = [builtinCapability()]
): McpBuiltinCapabilityListOutput {
  return { schemaVersion: 1, revision: 0, capabilities }
}

function builtinManagement(
  state: {
    status: 'loading' | 'ready' | 'error'
    output: McpBuiltinCapabilityListOutput | null
    errorMessage: string | null
    isRefreshing: boolean
  } = {
    status: 'ready',
    output: builtinOutput(),
    errorMessage: null,
    isRefreshing: false
  }
) {
  return {
    pendingCapabilities: new Set(),
    refresh: service.builtinRefresh,
    setAllowed: service.builtinSetAllowed,
    state
  }
}

function details(overrides: Partial<McpServerDetailsView> = {}): McpServerDetailsView {
  return {
    schemaVersion: 1,
    serverId: SERVER_ID,
    displayName: 'fixture',
    scope: 'user',
    source: 'userManual',
    transport: 'stdio',
    trust: 'untrusted',
    approvalMode: 'prompt',
    registryRevision: 2,
    configEpoch: '22222222-2222-4222-8222-222222222222',
    configDigest: 'a'.repeat(64),
    state: 'disabled',
    enabled: false,
    launchAuthorizationState: 'required',
    catalogGeneration: 0,
    catalogCompleteness: 'failed',
    toolCount: 0,
    activeCallCount: 0,
    updatedAtMs: 2,
    executable: '/usr/bin/fixture',
    arguments: [],
    cwd: '/tmp',
    createdAtMs: 1,
    ...overrides
  }
}

function toListItem(server: McpServerDetailsView): McpServerListItem {
  return {
    schemaVersion: 1,
    serverId: server.serverId,
    displayName: server.displayName,
    scope: server.scope,
    source: server.source,
    transport: server.transport,
    trust: server.trust,
    approvalMode: server.approvalMode,
    registryRevision: server.registryRevision,
    configEpoch: server.configEpoch,
    configDigest: server.configDigest,
    state: server.state,
    enabled: server.enabled,
    launchAuthorizationState: server.launchAuthorizationState,
    catalogGeneration: server.catalogGeneration,
    catalogCompleteness: server.catalogCompleteness,
    toolCount: server.toolCount,
    activeCallCount: server.activeCallCount,
    updatedAtMs: server.updatedAtMs
  }
}

function output(servers: McpServerDetailsView[]): McpServerListOutput {
  return {
    schemaVersion: 1,
    registryRevision: 2,
    servers: servers.map(toListItem)
  }
}

function management(
  state:
    | { status: 'loading'; output: null; errorMessage: null; isRefreshing: false }
    | { status: 'error'; output: null; errorMessage: string; isRefreshing: false }
    | {
        status: 'ready'
        output: McpServerListOutput
        errorMessage: string | null
        isRefreshing: boolean
      },
  serverDetails: readonly McpServerDetailsView[] = []
) {
  return {
    addServer: service.addServer,
    authorizeLaunch: service.authorizeLaunch,
    catalogsById: new Map(),
    deleteServer: service.deleteServer,
    detailsById: new Map(serverDetails.map((server) => [server.serverId, server])),
    disableServer: service.disableServer,
    enableServer: service.enableServer,
    isAdding: false,
    loadDetails: service.loadDetails,
    loadTools: service.loadTools,
    pendingOperations: new Map(),
    refresh: service.refresh,
    refreshCatalog: service.refreshCatalog,
    restartServer: service.restartServer,
    selectExecutable: service.selectExecutable,
    selectWorkingDirectory: service.selectWorkingDirectory,
    setServerEnabled: service.setServerEnabled,
    startServer: service.startServer,
    state,
    stopServer: service.stopServer,
    updateServer: service.updateServer
  }
}

beforeEach(() => {
  for (const mock of Object.values(service)) mock.mockReset()
  service.builtinHook.mockReturnValue(builtinManagement())
  service.builtinRefresh.mockResolvedValue(builtinOutput())
  service.builtinSetAllowed.mockResolvedValue(null)
  service.chooseBrowserDownloadDirectory.mockResolvedValue(null)
  service.clearBrowserDownloadHistory.mockResolvedValue(0)
  service.clearBrowserData.mockResolvedValue({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    deletedHistoryCount: 0,
    deletedDownloadCount: 0,
    clearedCookiesAndSiteData: false,
    clearedCache: false
  })
  service.deleteBrowserHistory.mockResolvedValue({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    deletedCount: 0
  })
  service.getBrowserDataSummary.mockResolvedValue({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    historyCount: 0,
    historySiteCount: 0,
    downloadCount: 0,
    cookieSiteCount: 0,
    cacheBytes: 0
  })
  service.getBrowserDownloadSettings.mockResolvedValue({
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: 'system',
    displayPath: '~/Downloads',
    askWhereToSave: false,
    revision: 0,
    updatedAt: 0
  })
  service.getBrowserPreferences.mockResolvedValue({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    linkOpenTarget: 'system',
    revision: 0,
    updatedAt: 0
  })
  service.listBrowserHistory.mockResolvedValue({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    entries: [],
    truncated: false
  })
  service.listBrowserDownloadHistory.mockResolvedValue({
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloads: [],
    truncated: false
  })
  service.onBrowserDownloadHistoryChanged.mockReturnValue(() => undefined)
  service.onBrowserHistoryChanged.mockReturnValue(() => undefined)
  service.openBrowserHistoryEntry.mockResolvedValue(undefined)
  service.loadTools.mockResolvedValue(null)
  service.loadDetails.mockResolvedValue(null)
  service.refresh.mockResolvedValue(null)
  service.selectExecutable.mockResolvedValue(null)
  service.selectWorkingDirectory.mockResolvedValue(null)
  service.resetBrowserDownloadDirectory.mockResolvedValue({
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: 'system',
    displayPath: '~/Downloads',
    askWhereToSave: false,
    revision: 1,
    updatedAt: 1
  })
  service.setBrowserDownloadAskWhereToSave.mockResolvedValue({
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: 'system',
    displayPath: '~/Downloads',
    askWhereToSave: true,
    revision: 1,
    updatedAt: 1
  })
  service.revealBrowserDownload.mockResolvedValue({
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    status: 'shown'
  })
  service.updateBrowserPreferences.mockResolvedValue({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    linkOpenTarget: 'builtin',
    revision: 1,
    updatedAt: 1
  })
})

describe('MCP Settings page', () => {
  it('separates the built-in capability from the unchanged external Server area', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    await expect.element(screen.getByRole('heading', { name: 'mcp.section.builtin' })).toBeVisible()
    await expect
      .element(screen.getByRole('heading', { name: 'mcp.section.external' }))
      .toBeVisible()
    await expect.element(screen.getByText('mcp.builtin.browserAutomation.name')).toBeVisible()
    await expect
      .element(screen.getByText('mcp.builtin.browserAutomation.description'))
      .toBeVisible()
    expect(screen.container.querySelector('[data-mcp-icon="browser-automation"]')).not.toBeNull()
    expect(screen.getByRole('switch').elements()).toHaveLength(1)
    expect(screen.getByText('Host Browser automation').query()).toBeNull()
    expect(screen.getByText('Host description').query()).toBeNull()
    await expect.element(screen.getByText('mcp.empty.title')).toBeVisible()
  })

  it('changes only user_allowed when the built-in capability switch is used', async () => {
    const capability = builtinCapability()
    service.builtinHook.mockReturnValue(
      builtinManagement({
        status: 'ready',
        output: builtinOutput([capability]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    const toggle = screen.getByRole('switch', { name: 'mcp.builtin.toggleNamed' })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    await toggle.click()

    await expect.poll(() => service.builtinSetAllowed.mock.calls.length).toBe(1)
    expect(service.builtinSetAllowed).toHaveBeenCalledWith(capability, true)
    expect(service.setServerEnabled).not.toHaveBeenCalled()
    expect(service.startServer).not.toHaveBeenCalled()
    expect(service.authorizeLaunch).not.toHaveBeenCalled()
  })

  it('opens browser automation settings, toggles manual save prompts, and separates history', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    await screen.getByRole('button', { name: 'mcp.browserDownloads.configure' }).click()
    await expect
      .element(screen.getByRole('heading', { name: 'mcp.browserDownloads.section' }))
      .toBeVisible()
    await expect
      .element(screen.getByText('mcp.browserDownloads.history', { exact: true }))
      .toBeVisible()
    await expect.element(screen.getByText('mcp.browserDownloads.systemLocation')).toBeVisible()
    await expect
      .element(screen.getByRole('navigation', { name: 'settings.breadcrumb.label' }))
      .toBeVisible()
    expect(screen.getByText('mcp.browserDownloads.empty', { exact: true }).elements()).toHaveLength(
      0
    )
    expect(service.getBrowserDownloadSettings).toHaveBeenCalled()
    expect(service.getBrowserPreferences).toHaveBeenCalled()
    expect(service.listBrowserDownloadHistory).not.toHaveBeenCalled()
    expect(service.builtinSetAllowed).not.toHaveBeenCalled()
    expect(service.setServerEnabled).not.toHaveBeenCalled()

    const askWhereToSave = screen.getByRole('switch', {
      name: 'mcp.browserDownloads.askWhereToSave'
    })
    await expect.element(askWhereToSave).toHaveAttribute('aria-checked', 'false')
    await askWhereToSave.click()
    await expect.poll(() => service.setBrowserDownloadAskWhereToSave.mock.calls.length).toBe(1)
    expect(service.setBrowserDownloadAskWhereToSave).toHaveBeenCalledWith(true)

    const downloadHistoryRow = screen
      .getByText('mcp.browserDownloads.history', { exact: true })
      .element()
      .closest('.browser-download-preference-row')
    const manageDownloadHistory = downloadHistoryRow?.querySelector('button')
    if (!(manageDownloadHistory instanceof HTMLButtonElement)) {
      throw new Error('download history management button missing')
    }
    manageDownloadHistory.click()
    await expect
      .element(screen.getByRole('heading', { name: 'mcp.browserDownloads.history' }))
      .toBeVisible()
    await expect
      .element(screen.getByText('mcp.browserDownloads.empty', { exact: true }))
      .toBeVisible()
    expect(service.listBrowserDownloadHistory).toHaveBeenCalledWith('')

    await screen.getByRole('button', { name: 'settings.nav.mcp' }).click()
    await expect.element(screen.getByRole('heading', { name: 'mcp.section.builtin' })).toBeVisible()
  })

  it('updates the link target and opens browsing history from the General section', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    await screen.getByRole('button', { name: 'mcp.browserDownloads.configure' }).click()
    await expect
      .element(screen.getByRole('heading', { name: 'mcp.browserData.general' }))
      .toBeVisible()
    const linkTargetSelect = screen.container.querySelector('.browser-link-target-select')
    if (!(linkTargetSelect instanceof HTMLElement)) throw new Error('link target select missing')
    expect(
      Number.parseFloat(window.getComputedStyle(linkTargetSelect).width)
    ).toBeGreaterThanOrEqual(190)
    await screen
      .getByRole('button', {
        name: 'mcp.browserData.linkTarget: mcp.browserData.systemBrowser'
      })
      .click()
    await screen.getByRole('option', { name: 'mcp.browserData.builtinBrowser' }).click()
    await expect.poll(() => service.updateBrowserPreferences.mock.calls.length).toBe(1)
    expect(service.updateBrowserPreferences).toHaveBeenCalledWith('builtin')

    const historyRow = screen
      .getByText('browser.history', { exact: true })
      .element()
      .closest('.browser-download-preference-row')
    const manageHistory = historyRow?.querySelector('button')
    if (!(manageHistory instanceof HTMLButtonElement)) {
      throw new Error('browsing history management button missing')
    }
    manageHistory.click()

    await expect.element(screen.getByRole('heading', { name: 'browser.history' })).toBeVisible()
    await expect
      .element(screen.getByText('mcp.browserData.emptyHistory', { exact: true }))
      .toBeVisible()
    expect(service.listBrowserHistory).toHaveBeenCalledWith('')
    const breadcrumb = screen.getByRole('navigation', { name: 'settings.breadcrumb.label' })
    expect(breadcrumb.getByText('browser.history', { exact: true }).elements()).toHaveLength(1)
  })

  it('renders the clear-data dialog in a top-level portal and enforces all-time-only rows', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    await screen.getByRole('button', { name: 'mcp.browserDownloads.configure' }).click()
    await screen.getByRole('button', { name: 'browser.clearBrowsingData' }).click()
    const dialog = screen.getByRole('dialog', { name: 'browser.clearBrowsingData' })
    await expect.element(dialog).toBeVisible()
    expect(dialog.element().closest('.browser-data-dialog__backdrop')?.parentElement).toBe(
      document.body
    )

    await screen.getByRole('button', { name: 'mcp.browserData.range.last7Days' }).click()
    const cookies = screen.getByRole('checkbox', { name: /mcp\.browserData\.cookies/u })
    const cache = screen.getByRole('checkbox', { name: /mcp\.browserData\.cache/u })
    await expect.element(cookies).toBeDisabled()
    await expect.element(cache).toBeDisabled()
    await expect.element(cookies).not.toBeChecked()
    await expect.element(cache).not.toBeChecked()
  })

  it('groups browsing history and opens each row menu from a top-level portal', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    service.listBrowserHistory.mockResolvedValue({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      entries: [
        {
          schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
          historyId: 'browser-history:123e4567-e89b-42d3-a456-426614174000',
          url: 'https://example.test/docs',
          title: 'Example documentation',
          hostname: 'example.test',
          faviconUrl: null,
          visitedAt: Date.UTC(2026, 7, 27, 6, 15)
        },
        {
          schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
          historyId: 'browser-history:223e4567-e89b-42d3-a456-426614174000',
          url: 'https://other.test/',
          title: 'Other page',
          hostname: 'other.test',
          faviconUrl: null,
          visitedAt: Date.UTC(2026, 7, 26, 4, 0)
        }
      ],
      truncated: false
    })
    const onCloseSettings = vi.fn()
    const screen = await render(
      <McpSettingsPage initialBrowserView="history" onCloseSettings={onCloseSettings} />
    )

    await new Promise((resolve) => window.setTimeout(resolve, 50))
    expect(screen.container.textContent).toContain('Example documentation')
    expect(screen.container.textContent).toContain('example.test')
    expect(screen.container.querySelectorAll('.browser-history-group')).toHaveLength(2)
    await screen.getByText('Example documentation').click()
    await expect
      .element(screen.getByRole('checkbox', { name: 'Example documentation' }))
      .toBeChecked()
    expect(service.openBrowserHistoryEntry).not.toHaveBeenCalled()
    const moreButton = screen.container.querySelector<HTMLButtonElement>(
      'button[aria-label="browser.downloadCenter.moreActions"]'
    )
    if (!moreButton) throw new Error('history menu trigger missing')
    moreButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    const historyMenu = document.body.querySelector('.browser-history-menu') as HTMLElement | null
    expect(historyMenu).not.toBeNull()
    expect(historyMenu?.parentElement).toBe(document.body)
    const openButton = historyMenu?.querySelector<HTMLButtonElement>('button')
    if (!openButton) throw new Error('history open action missing')
    openButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await Promise.resolve()
    expect(service.openBrowserHistoryEntry).toHaveBeenCalledWith('https://example.test/docs')
    await Promise.resolve()
    expect(onCloseSettings).toHaveBeenCalledTimes(1)
  })

  it('shows built-in loading and safe retry independently from external Servers', async () => {
    service.builtinHook.mockReturnValue(
      builtinManagement({
        status: 'error',
        output: null,
        errorMessage: 'safe built-in failure',
        isRefreshing: false
      })
    )
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([details()]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    await expect.element(screen.getByText('safe built-in failure')).toBeVisible()
    await expect.element(screen.getByText('fixture')).toBeVisible()
    await screen.getByRole('button', { name: 'mcp.actions.retry' }).click()
    expect(service.builtinRefresh).toHaveBeenCalledWith(true)
    expect(service.refresh).not.toHaveBeenCalled()
  })

  it('refreshes built-in and external MCP from the shared refresh action', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)

    await screen.getByRole('button', { name: 'mcp.actions.refresh' }).click()
    expect(service.refresh).toHaveBeenCalledTimes(1)
    expect(service.builtinRefresh).toHaveBeenCalledWith(true)
  })

  it('renders the initial loading state', async () => {
    service.hook.mockReturnValue(
      management({ status: 'loading', output: null, errorMessage: null, isRefreshing: false })
    )
    const screen = await render(<McpSettingsPage />)
    await expect.element(screen.getByText('mcp.loading')).toBeVisible()
  })

  it('renders the authoritative empty state', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)
    await expect.element(screen.getByText('mcp.empty.title')).toBeVisible()
    expect(screen.getByRole('button', { name: 'mcp.actions.addServer' }).element()).toHaveClass(
      'mcp-secondary-button'
    )
    expect(screen.getByRole('button', { name: 'mcp.actions.addServer' }).element()).not.toHaveClass(
      'mcp-primary-button'
    )
  })

  it('renders a safe error and retries through the hook', async () => {
    service.hook.mockReturnValue(
      management({
        status: 'error',
        output: null,
        errorMessage: 'safe failure',
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)
    await expect.element(screen.getByText('safe failure')).toBeVisible()
    await screen.getByRole('button', { name: 'mcp.actions.retry' }).click()
    expect(service.refresh).toHaveBeenCalledTimes(1)
  })

  it('saves a new Server without authorizing or enabling it', async () => {
    const saved = details()
    service.addServer.mockResolvedValue(saved)
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('button', { name: 'mcp.actions.addServer' }).click()
    expect(screen.getByText('mcp.add.description').query()).toBeNull()
    expect(screen.getByText('mcp.form.argumentsHelp').query()).toBeNull()
    expect(screen.getByText('mcp.form.cwdHelp').query()).toBeNull()
    await screen.getByLabelText('mcp.form.name').fill('fixture')
    await screen.getByLabelText('mcp.form.executable').fill('/usr/bin/fixture')
    await screen.getByLabelText('mcp.form.cwd').fill('/tmp')
    const saveButton = screen.getByRole('button', { name: 'mcp.actions.save' })
    expect(saveButton.element()).toHaveClass('primary-settings-button')
    await saveButton.click()
    await expect.poll(() => service.addServer.mock.calls.length).toBe(1)
    expect(service.authorizeLaunch).not.toHaveBeenCalled()
    expect(service.enableServer).not.toHaveBeenCalled()
  })

  it('uses the shared Settings confirmation before the first enable flow', async () => {
    const server = details()
    service.setServerEnabled.mockResolvedValue(null)
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    expect(service.setServerEnabled).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'mcp.actions.enable' }).click()
    await expect.poll(() => service.setServerEnabled.mock.calls.length).toBe(1)
    expect(service.setServerEnabled).toHaveBeenCalledWith(toListItem(server), true)
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    expect(service.authorizeLaunch).not.toHaveBeenCalled()
    expect(service.enableServer).not.toHaveBeenCalled()
  })

  it('cancels the shared confirmation without authorizing and hides launch details', async () => {
    const server = details()
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()

    await expect.element(screen.getByRole('dialog')).toBeVisible()
    expect(screen.getByText('/usr/bin/fixture').query()).toBeNull()
    await screen.getByText('mcp.actions.cancel', { exact: true }).click()
    expect(service.setServerEnabled).not.toHaveBeenCalled()
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
  })

  it('enables an already authorized Server without another confirmation', async () => {
    const server = details({
      launchAuthorizationState: 'authorized',
      state: 'ready',
      toolCount: 14,
      trust: 'userApproved'
    })
    service.loadDetails.mockResolvedValue(server)
    service.setServerEnabled.mockResolvedValue(server)
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    const primary = screen.getByText('fixture').element().closest('.mcp-server-row__primary')
    expect(primary).not.toBeNull()
    expect(primary).toContainElement(screen.getByText('mcp.state.ready').element())
    expect(primary?.querySelector('.mcp-server-row__status-dot')).toBeNull()
    expect(screen.getByText('mcp.tools.count: 14').query()).toBeNull()
    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()

    await expect.poll(() => service.setServerEnabled.mock.calls.length).toBe(1)
    expect(service.loadDetails).toHaveBeenCalledWith(toListItem(server))
    expect(screen.getByRole('dialog').query()).toBeNull()
  })

  it('reconfirms when a live detail check finds a structurally authorized Server stale', async () => {
    const listed = details({
      launchAuthorizationState: 'authorized',
      trust: 'userApproved'
    })
    const stale = details({
      launchAuthorizationState: 'stale',
      trust: 'userApproved'
    })
    service.loadDetails.mockResolvedValue(stale)
    service.setServerEnabled.mockResolvedValue(stale)
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([listed]),
          errorMessage: null,
          isRefreshing: false
        },
        [listed]
      )
    )
    const screen = await render(<McpSettingsPage />)

    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()

    await expect.element(screen.getByRole('dialog')).toBeVisible()
    expect(service.loadDetails).toHaveBeenCalledTimes(1)
    expect(service.setServerEnabled).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'mcp.actions.enable' }).click()
    await expect.poll(() => service.setServerEnabled.mock.calls.length).toBe(1)
    expect(service.setServerEnabled).toHaveBeenCalledWith(stale, true)
  })

  it('returns an enable-time authorization race to confirmation without retrying', async () => {
    const listed = details({
      launchAuthorizationState: 'authorized',
      trust: 'userApproved'
    })
    const stale = details({
      launchAuthorizationState: 'stale',
      trust: 'userApproved'
    })
    service.loadDetails.mockResolvedValueOnce(listed).mockResolvedValueOnce(stale)
    service.setServerEnabled.mockRejectedValue(
      new HostInvocationError({
        message: 'safe outer error',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'enable',
          code: 'authorizationRequired',
          recovery: 'requestLaunchAuthorization',
          message: 'Launch authorization is required.',
          serverId: SERVER_ID,
          currentRegistryRevision: 2
        }
      })
    )
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([listed]),
          errorMessage: null,
          isRefreshing: false
        },
        [listed]
      )
    )
    const screen = await render(<McpSettingsPage />)

    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()

    await expect.element(screen.getByRole('dialog')).toBeVisible()
    expect(service.loadDetails).toHaveBeenCalledTimes(2)
    expect(service.setServerEnabled).toHaveBeenCalledTimes(1)
    expect(service.authorizeLaunch).not.toHaveBeenCalled()
  })

  it('keeps confirmation open when authorization drifts again after confirmation', async () => {
    const stale = details({
      launchAuthorizationState: 'stale',
      trust: 'userApproved'
    })
    service.loadDetails.mockResolvedValue(stale)
    service.setServerEnabled.mockRejectedValue(
      new HostInvocationError({
        message: 'safe outer error',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'enable',
          code: 'authorizationStale',
          recovery: 'requestLaunchAuthorization',
          message: 'Launch authorization changed during confirmation.',
          serverId: SERVER_ID,
          currentRegistryRevision: 2
        }
      })
    )
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([stale]),
          errorMessage: null,
          isRefreshing: false
        },
        [stale]
      )
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()

    await screen.getByRole('button', { name: 'mcp.actions.enable' }).click()

    await expect.element(screen.getByRole('dialog')).toBeVisible()
    expect(service.setServerEnabled).toHaveBeenCalledTimes(1)
    expect(service.loadDetails).toHaveBeenCalledTimes(1)
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('coalesces repeated live authorization preflight clicks', async () => {
    const server = details({
      launchAuthorizationState: 'authorized',
      trust: 'userApproved'
    })
    let resolveDetails: ((value: McpServerDetailsView) => void) | undefined
    service.loadDetails.mockReturnValue(
      new Promise<McpServerDetailsView>((resolve) => {
        resolveDetails = resolve
      })
    )
    service.setServerEnabled.mockResolvedValue(server)
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    const toggle = screen.getByRole('switch', { name: 'mcp.actions.enable' })

    await toggle.click()
    await toggle.click()

    expect(service.loadDetails).toHaveBeenCalledTimes(1)
    resolveDetails?.(server)
    await expect.poll(() => service.setServerEnabled.mock.calls.length).toBe(1)
  })

  it('opens the configuration editor directly without a status or Catalog page', async () => {
    const server = details()
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('button', { name: 'mcp.actions.edit: fixture' }).click()
    await expect.element(screen.getByLabelText('mcp.form.name')).toHaveValue('fixture')
    expect(screen.getByText('mcp.edit.description').query()).toBeNull()
    expect(screen.getByText('mcp.form.argumentsHelp').query()).toBeNull()
    expect(screen.getByText('mcp.form.cwdHelp').query()).toBeNull()
    expect(screen.getByText('mcp.catalog.title').query()).toBeNull()
    expect(screen.getByText('mcp.detail.launchConfiguration').query()).toBeNull()
    expect(screen.getByText('mcp.detail.technicalDetails').query()).toBeNull()
    expect(service.loadTools).not.toHaveBeenCalled()
    expect(service.refreshCatalog).not.toHaveBeenCalled()
  })

  it('returns to the list when configuration details cannot be loaded', async () => {
    const server = details()
    service.loadDetails.mockRejectedValue(new Error('fixed-load-details-failure'))
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([server]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('button', { name: 'mcp.actions.edit: fixture' }).click()
    await expect.poll(() => service.showToast.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByRole('button', { name: 'mcp.actions.edit: fixture' }))
      .toBeVisible()
    expect(screen.getByText('mcp.detail.loading').query()).toBeNull()
  })

  it('requires confirmation before deleting the authoritative Server row', async () => {
    const server = details()
    service.deleteServer.mockResolvedValue(server)
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('button', { name: 'mcp.actions.edit: fixture' }).click()
    await screen.getByRole('button', { name: 'mcp.actions.delete' }).click()
    expect(service.deleteServer).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: 'mcp.actions.confirmDelete' }).click()
    await expect.poll(() => service.deleteServer.mock.calls.length).toBe(1)
  })

  it('closes delete confirmation after an indeterminate cleanup result', async () => {
    const server = details()
    service.deleteServer.mockRejectedValue(
      new HostInvocationError({
        message: 'safe outer error',
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'delete',
          code: 'cleanupIncomplete',
          recovery: 'doNotRetry',
          message: 'Cleanup outcome requires confirmation.',
          serverId: SERVER_ID,
          currentRegistryRevision: 2
        }
      })
    )
    service.hook.mockReturnValue(
      management(
        {
          status: 'ready',
          output: output([server]),
          errorMessage: null,
          isRefreshing: false
        },
        [server]
      )
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('button', { name: 'mcp.actions.edit: fixture' }).click()
    await screen.getByRole('button', { name: 'mcp.actions.delete' }).click()
    await screen.getByRole('button', { name: 'mcp.actions.confirmDelete' }).click()
    await expect.poll(() => service.deleteServer.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByRole('button', { name: 'mcp.actions.confirmDelete' }))
      .not.toBeInTheDocument()
    expect(service.deleteServer).toHaveBeenCalledTimes(1)
  })
})
