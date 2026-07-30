import type {
  McpServerDetailsView,
  McpServerListItem,
  McpServerListOutput
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
  loadDetails: vi.fn(),
  loadTools: vi.fn(),
  refresh: vi.fn(),
  refreshCatalog: vi.fn(),
  restartServer: vi.fn(),
  selectExecutable: vi.fn(),
  selectWorkingDirectory: vi.fn(),
  setServerEnabled: vi.fn(),
  showToast: vi.fn(),
  startServer: vi.fn(),
  stopServer: vi.fn(),
  updateServer: vi.fn()
}))

vi.mock('../useMcpManagement', () => ({
  useMcpManagement: service.hook
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: service.showToast })
}))

const { McpSettingsPage } = await import('../McpSettingsPage')

const SERVER_ID = '11111111-1111-4111-8111-111111111111'

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
  service.loadTools.mockResolvedValue(null)
  service.loadDetails.mockResolvedValue(null)
  service.refresh.mockResolvedValue(null)
  service.selectExecutable.mockResolvedValue(null)
  service.selectWorkingDirectory.mockResolvedValue(null)
})

describe('MCP Settings page', () => {
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
    expect(screen.getByRole('dialog').query()).toBeNull()
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
