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
    await screen.getByLabelText('mcp.form.name').fill('fixture')
    await screen.getByLabelText('mcp.form.executable').fill('/usr/bin/fixture')
    await screen.getByLabelText('mcp.form.cwd').fill('/tmp')
    await screen.getByRole('button', { name: 'mcp.actions.saveServer' }).click()
    await expect.poll(() => service.addServer.mock.calls.length).toBe(1)
    expect(service.authorizeLaunch).not.toHaveBeenCalled()
    expect(service.enableServer).not.toHaveBeenCalled()
  })

  it('keeps native launch authorization cancellation separate from enable', async () => {
    const server = details()
    service.authorizeLaunch.mockResolvedValue(null)
    service.enableServer.mockResolvedValue(server)
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
    await screen.getByRole('button', { name: 'mcp.actions.openDetails' }).click()
    await screen.getByRole('button', { name: 'mcp.actions.authorizeLaunch' }).click()
    await expect.poll(() => service.authorizeLaunch.mock.calls.length).toBe(1)
    expect(service.enableServer).not.toHaveBeenCalled()
    await screen.getByRole('switch', { name: 'mcp.actions.enable' }).click()
    expect(service.enableServer).toHaveBeenCalledTimes(1)
  })

  it('requires confirmation before deleting the authoritative Server row', async () => {
    const server = details()
    service.deleteServer.mockResolvedValue(server)
    service.hook.mockReturnValue(
      management({
        status: 'ready',
        output: output([server]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)
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
      management({
        status: 'ready',
        output: output([server]),
        errorMessage: null,
        isRefreshing: false
      })
    )
    const screen = await render(<McpSettingsPage />)
    await screen.getByRole('button', { name: 'mcp.actions.delete' }).click()
    await screen.getByRole('button', { name: 'mcp.actions.confirmDelete' }).click()
    await expect.poll(() => service.deleteServer.mock.calls.length).toBe(1)
    await expect
      .element(screen.getByRole('button', { name: 'mcp.actions.confirmDelete' }))
      .not.toBeInTheDocument()
    expect(service.deleteServer).toHaveBeenCalledTimes(1)
  })
})
