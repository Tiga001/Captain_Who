import type {
  McpChangedNotification,
  McpServerDetailsOutput,
  McpServerDetailsView,
  McpServerListOutput
} from '@mycopilot/protocol'
import { HostInvocationError } from '@mycopilot/host-api'
import { StrictMode, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { McpServerDraft } from '../mcpManagementInputs'

const service = vi.hoisted(() => ({
  addServer: vi.fn(),
  deleteServer: vi.fn(),
  disableServer: vi.fn(),
  enableServer: vi.fn(),
  getServer: vi.fn(),
  listServers: vi.fn(),
  listTools: vi.fn(),
  onChanged: vi.fn(),
  refreshCatalog: vi.fn(),
  requestLaunchAuthorization: vi.fn(),
  restartServer: vi.fn(),
  selectExecutable: vi.fn(),
  selectWorkingDirectory: vi.fn(),
  startServer: vi.fn(),
  stopServer: vi.fn(),
  unsubscribe: vi.fn(),
  updateServer: vi.fn()
}))

let changedHandler: ((event: McpChangedNotification) => void) | undefined

vi.mock('../mcpManagementClient', () => ({
  addMcpServer: service.addServer,
  deleteMcpServer: service.deleteServer,
  disableMcpServer: service.disableServer,
  enableMcpServer: service.enableServer,
  getMcpServer: service.getServer,
  listMcpServers: service.listServers,
  listMcpTools: service.listTools,
  onMcpChanged: service.onChanged,
  refreshMcpCatalog: service.refreshCatalog,
  requestMcpLaunchAuthorization: service.requestLaunchAuthorization,
  restartMcpServer: service.restartServer,
  selectMcpExecutable: service.selectExecutable,
  selectMcpWorkingDirectory: service.selectWorkingDirectory,
  startMcpServer: service.startServer,
  stopMcpServer: service.stopServer,
  updateMcpServer: service.updateServer
}))

const { useMcpManagement } = await import('../useMcpManagement')

const SERVER_A = '11111111-1111-4111-8111-111111111111'
const SERVER_B = '22222222-2222-4222-8222-222222222222'
const EPOCH_A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
const EPOCH_B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
const SOURCE_EPOCH = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'
const SOURCE_EPOCH_B = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd'
const DIGEST_A = 'a'.repeat(64)
const DIGEST_B = 'b'.repeat(64)

function details(
  serverId = SERVER_A,
  overrides: Partial<McpServerDetailsView> = {}
): McpServerDetailsView {
  const second = serverId === SERVER_B
  return {
    schemaVersion: 1,
    serverId,
    displayName: second ? 'fixture-b' : 'fixture-a',
    scope: 'user',
    source: 'userManual',
    transport: 'stdio',
    trust: 'userApproved',
    approvalMode: 'prompt',
    registryRevision: 5,
    configEpoch: second ? EPOCH_B : EPOCH_A,
    configDigest: second ? DIGEST_B : DIGEST_A,
    state: 'ready',
    enabled: true,
    launchAuthorizationState: 'authorized',
    catalogGeneration: 2,
    catalogCompleteness: 'complete',
    toolCount: 1,
    activeCallCount: 0,
    updatedAtMs: 20,
    executable: '/usr/bin/fixture',
    arguments: ['', '--safe'],
    cwd: '/tmp',
    createdAtMs: 1,
    ...overrides
  }
}

function listOutput(
  servers: McpServerDetailsView[] = [details()],
  registryRevision = 5
): McpServerListOutput {
  return {
    schemaVersion: 1,
    registryRevision,
    servers: servers.map((server) => ({
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
    }))
  }
}

function detailsOutput(server: McpServerDetailsView): McpServerDetailsOutput {
  return {
    schemaVersion: 1,
    registryRevision: server.registryRevision,
    server
  }
}

function Harness() {
  const management = useMcpManagement()
  const [lastAction, setLastAction] = useState('none')
  const servers = management.state.output?.servers ?? []
  const first = servers[0]
  const second = servers[1]
  const draft: McpServerDraft = {
    approvalMode: 'prompt',
    arguments: ['', '$(opaque)'],
    cwd: '/tmp',
    displayName: 'added',
    executable: '/usr/bin/fixture'
  }
  const catalog = first ? management.catalogsById.get(first.serverId) : undefined
  const loadedDetails = first ? management.detailsById.get(first.serverId) : undefined

  return (
    <div>
      <output data-testid="status">{management.state.status}</output>
      <output data-testid="server-state">{first?.state ?? 'none'}</output>
      <output data-testid="detail-state">
        {loadedDetails?.state ?? 'none'}:{loadedDetails?.activeCallCount ?? 0}
      </output>
      <output data-testid="catalog">
        {catalog?.status ?? 'none'}:{catalog?.tools.length ?? 0}:
        {catalog?.catalogGeneration ?? 'none'}:{catalog?.catalogCompleteness ?? 'none'}
      </output>
      <output data-testid="pending">{management.pendingOperations.size}</output>
      <output data-testid="last-action">{lastAction}</output>
      <button
        onClick={() => {
          void management.addServer(draft)
          void management.addServer(draft)
        }}
        type="button"
      >
        double add
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.loadDetails(first)
        }}
        type="button"
      >
        load details
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.loadDetails(first)
          void management.loadDetails(first)
        }}
        type="button"
      >
        load details twice
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.loadTools(first)
        }}
        type="button"
      >
        load tools
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.loadTools(first, true)
        }}
        type="button"
      >
        load more tools
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.refreshCatalog(first)
        }}
        type="button"
      >
        refresh catalog
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management
            .updateServer(first, draft)
            .then(() => setLastAction('updated'))
            .catch(() => setLastAction('update-failed'))
        }}
        type="button"
      >
        update
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management
            .authorizeLaunch(first)
            .then((result) => setLastAction(result ? 'authorized' : 'authorization-ignored'))
            .catch(() => setLastAction('authorization-failed'))
        }}
        type="button"
      >
        authorize
      </button>
      <button
        disabled={!first || !second}
        onClick={() => {
          if (!first || !second) return
          void management.restartServer(first)
          void management.disableServer(second)
        }}
        type="button"
      >
        two servers
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.restartServer(first)
          void management.restartServer(first)
        }}
        type="button"
      >
        double restart
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.enableServer(first)
          void management.enableServer(first)
        }}
        type="button"
      >
        double enable
      </button>
      <button
        disabled={!first}
        onClick={() => {
          if (!first) return
          void management.deleteServer(first)
          void management.deleteServer(first)
        }}
        type="button"
      >
        double delete
      </button>
    </div>
  )
}

beforeEach(() => {
  changedHandler = undefined
  for (const mock of Object.values(service)) mock.mockReset()
  service.unsubscribe.mockReset()
  service.onChanged.mockImplementation((handler: (event: McpChangedNotification) => void) => {
    changedHandler = handler
    return service.unsubscribe
  })
  service.listServers.mockResolvedValue(listOutput())
  service.getServer.mockResolvedValue(detailsOutput(details()))
  service.selectExecutable.mockResolvedValue(null)
  service.selectWorkingDirectory.mockResolvedValue(null)
})

describe('useMcpManagement concurrency', () => {
  it('cleans every change subscription across StrictMode remount and unmount', async () => {
    const screen = await render(
      <StrictMode>
        <Harness />
      </StrictMode>
    )
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    expect(service.onChanged.mock.calls.length).toBeGreaterThan(0)
    screen.unmount()
    expect(service.unsubscribe).toHaveBeenCalledTimes(service.onChanged.mock.calls.length)
  })

  it('coalesces a notification storm into one follow-up refresh', async () => {
    const first = deferred<McpServerListOutput>()
    const second = deferred<McpServerListOutput>()
    service.listServers.mockReset()
    service.listServers.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)

    await render(<Harness />)
    await expect.poll(() => service.listServers.mock.calls.length).toBe(1)
    await expect.poll(() => Boolean(changedHandler)).toBe(true)
    for (let index = 1; index <= 40; index += 1) {
      changedHandler?.({
        schemaVersion: 1,
        sourceEpoch: SOURCE_EPOCH,
        sequence: index,
        registryRevision: 5,
        kind: 'stateChanged',
        serverId: SERVER_A,
        state: 'ready'
      })
    }
    first.resolve(listOutput())
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
    second.resolve(listOutput())
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
  })

  it('admits only one add operation in the same tick', async () => {
    const add = deferred<McpServerDetailsOutput>()
    service.addServer.mockReturnValue(add.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'double add' }).click()
    expect(service.addServer).toHaveBeenCalledTimes(1)
    add.resolve(detailsOutput(details(SERVER_B)))
  })

  it('allows independent operations on two Servers', async () => {
    service.listServers.mockResolvedValue(listOutput([details(), details(SERVER_B)]))
    const restart = deferred<McpServerDetailsOutput>()
    const disable = deferred<McpServerDetailsOutput>()
    service.restartServer.mockReturnValue(restart.promise)
    service.disableServer.mockReturnValue(disable.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'two servers' }).click()
    expect(service.restartServer).toHaveBeenCalledTimes(1)
    expect(service.disableServer).toHaveBeenCalledTimes(1)
    await expect.element(screen.getByTestId('pending')).toHaveTextContent('2')
    restart.resolve(detailsOutput(details()))
    disable.resolve(detailsOutput(details(SERVER_B, { enabled: false, state: 'disabled' })))
  })

  it.each([
    ['restart', 'double restart', service.restartServer],
    ['enable', 'double enable', service.enableServer],
    ['delete', 'double delete', service.deleteServer]
  ] as const)('admits only one %s operation per Server', async (_operation, button, method) => {
    const response = deferred<McpServerDetailsOutput>()
    method.mockReturnValue(response.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')

    await screen.getByRole('button', { name: button }).click()
    expect(method).toHaveBeenCalledTimes(1)
    response.resolve(detailsOutput(details()))
  })

  it('does not regress Ready to an older Starting snapshot', async () => {
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('server-state')).toHaveTextContent('ready')
    service.listServers.mockResolvedValueOnce(
      listOutput([details(SERVER_A, { state: 'starting', updatedAtMs: 10 })])
    )
    changedHandler?.({
      schemaVersion: 1,
      sourceEpoch: SOURCE_EPOCH,
      sequence: 1,
      registryRevision: 5,
      kind: 'stateChanged',
      serverId: SERVER_A,
      state: 'starting'
    })
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('server-state')).toHaveTextContent('ready')
  })

  it('updates loaded details from the latest authoritative list summary', async () => {
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('server-state')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load details', exact: true }).click()
    await expect.element(screen.getByTestId('detail-state')).toHaveTextContent('ready:0')
    service.listServers.mockResolvedValueOnce(
      listOutput([
        details(SERVER_A, {
          state: 'degraded',
          activeCallCount: 2,
          updatedAtMs: 30
        })
      ])
    )
    changedHandler?.({
      schemaVersion: 1,
      sourceEpoch: SOURCE_EPOCH,
      sequence: 1,
      registryRevision: 5,
      kind: 'stateChanged',
      serverId: SERVER_A,
      state: 'degraded'
    })
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('detail-state')).toHaveTextContent('degraded:2')
  })

  it('ignores an older per-Server details request and an identity-mismatched response', async () => {
    const first = deferred<McpServerDetailsOutput>()
    const second = deferred<McpServerDetailsOutput>()
    service.getServer.mockReset()
    service.getServer.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load details twice' }).click()
    second.resolve(
      detailsOutput(details(SERVER_A, { state: 'degraded', activeCallCount: 2, updatedAtMs: 40 }))
    )
    await expect.element(screen.getByTestId('detail-state')).toHaveTextContent('degraded:2')
    first.resolve(detailsOutput(details(SERVER_A, { state: 'starting', updatedAtMs: 30 })))
    await expect.element(screen.getByTestId('detail-state')).toHaveTextContent('degraded:2')

    service.getServer.mockResolvedValueOnce(
      detailsOutput(
        details(SERVER_A, {
          configEpoch: SOURCE_EPOCH_B,
          configDigest: 'd'.repeat(64),
          updatedAtMs: 50
        })
      )
    )
    await screen.getByRole('button', { name: 'load details', exact: true }).click()
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
    await expect.element(screen.getByTestId('detail-state')).toHaveTextContent('ready:0')
  })

  it('does not let a delayed mutation response regress a newer Ready summary', async () => {
    const update = deferred<McpServerDetailsOutput>()
    service.updateServer.mockReturnValue(update.promise)
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'update' }).click()
    service.listServers.mockResolvedValueOnce(
      listOutput([details(SERVER_A, { registryRevision: 6, updatedAtMs: 40 })], 6)
    )
    changedHandler?.({
      schemaVersion: 1,
      sourceEpoch: SOURCE_EPOCH,
      sequence: 1,
      registryRevision: 6,
      kind: 'stateChanged',
      serverId: SERVER_A,
      state: 'ready'
    })
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
    update.resolve(
      detailsOutput(
        details(SERVER_A, {
          registryRevision: 6,
          state: 'starting',
          updatedAtMs: 30
        })
      )
    )
    await expect.element(screen.getByTestId('server-state')).toHaveTextContent('ready')
    await expect.element(screen.getByTestId('detail-state')).toHaveTextContent('ready:0')
  })

  it('rejects an authorization response that changes frozen config identity', async () => {
    service.requestLaunchAuthorization.mockResolvedValue({
      schemaVersion: 1,
      authorized: true,
      server: details(SERVER_A, {
        registryRevision: 6,
        configEpoch: SOURCE_EPOCH_B,
        configDigest: 'd'.repeat(64),
        updatedAtMs: 30
      })
    })
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'authorize' }).click()
    await expect
      .element(screen.getByTestId('last-action'))
      .toHaveTextContent('authorization-ignored')
    expect(service.listServers).toHaveBeenCalledTimes(2)
    await expect.element(screen.getByTestId('server-state')).toHaveTextContent('ready')
  })

  it('tracks notification sequence independently per bounded source epoch', async () => {
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    const event = (sourceEpoch: string, sequence: number): McpChangedNotification => ({
      schemaVersion: 1,
      sourceEpoch,
      sequence,
      registryRevision: 5,
      kind: 'stateChanged',
      serverId: SERVER_A,
      state: 'ready'
    })
    changedHandler?.(event(SOURCE_EPOCH, 1))
    await expect.poll(() => service.listServers.mock.calls.length).toBe(2)
    changedHandler?.(event(SOURCE_EPOCH_B, 1))
    await expect.poll(() => service.listServers.mock.calls.length).toBe(3)
    changedHandler?.(event(SOURCE_EPOCH, 2))
    await expect.poll(() => service.listServers.mock.calls.length).toBe(4)
    changedHandler?.(event(SOURCE_EPOCH_B, 2))
    await expect.poll(() => service.listServers.mock.calls.length).toBe(5)
    changedHandler?.(event(SOURCE_EPOCH_B, 2))
    await expect.poll(() => service.listServers.mock.calls.length).toBe(5)
    screen.unmount()
  })

  it('clears a Catalog when an update changes configuration identity', async () => {
    service.listTools.mockResolvedValue({
      schemaVersion: 1,
      serverId: SERVER_A,
      catalogGeneration: 2,
      catalogCompleteness: 'complete',
      tools: [
        {
          serverId: SERVER_A,
          rawName: 'echo',
          modelName: 'mcp__fixture__echo',
          routable: true,
          disabled: false,
          schemaDigestPrefix: 'abcdefabcdef',
          description: 'safe',
          descriptionTruncated: false,
          diagnosticCodes: [],
          catalogGeneration: 2,
          catalogCompleteness: 'complete'
        }
      ]
    })
    const changed = details(SERVER_A, {
      configEpoch: 'dddddddd-dddd-4ddd-8ddd-dddddddddddd',
      configDigest: 'd'.repeat(64),
      registryRevision: 6,
      catalogGeneration: 0,
      toolCount: 0
    })
    service.updateServer.mockResolvedValue(detailsOutput(changed))
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load tools' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('ready:1')
    await screen.getByRole('button', { name: 'update' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('none:0')
  })

  it('rejects a Catalog page older than the current generation', async () => {
    service.listTools.mockResolvedValue({
      schemaVersion: 1,
      serverId: SERVER_A,
      catalogGeneration: 1,
      catalogCompleteness: 'stale',
      tools: []
    })
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load tools' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('idle:0')
  })

  it('does not replace a current Catalog with an older refresh generation', async () => {
    service.listTools.mockResolvedValue({
      schemaVersion: 1,
      serverId: SERVER_A,
      catalogGeneration: 2,
      catalogCompleteness: 'complete',
      tools: []
    })
    service.refreshCatalog.mockResolvedValue({
      schemaVersion: 1,
      serverId: SERVER_A,
      catalogGeneration: 1,
      catalogCompleteness: 'stale',
      tools: []
    })
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load tools' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('ready:0')
    await screen.getByRole('button', { name: 'refresh catalog' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('ready:0')
  })

  it('fails closed when load-more changes generation', async () => {
    service.listTools
      .mockResolvedValueOnce({
        schemaVersion: 1,
        serverId: SERVER_A,
        catalogGeneration: 2,
        catalogCompleteness: 'complete',
        tools: [],
        nextCursor: 'opaque'
      })
      .mockResolvedValueOnce({
        schemaVersion: 1,
        serverId: SERVER_A,
        catalogGeneration: 3,
        catalogCompleteness: 'complete',
        tools: []
      })
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load tools' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('ready:0:2:complete')
    await screen.getByRole('button', { name: 'load more tools' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('idle:0:none:none')
  })

  it('rejects mismatched safe Tool page provenance and completeness', async () => {
    service.listTools.mockResolvedValue({
      schemaVersion: 1,
      serverId: SERVER_A,
      catalogGeneration: 2,
      catalogCompleteness: 'complete',
      tools: [
        {
          serverId: SERVER_B,
          rawName: 'echo',
          modelName: 'mcp__fixture__echo',
          routable: true,
          disabled: false,
          schemaDigestPrefix: 'abcdefabcdef',
          description: 'safe',
          descriptionTruncated: false,
          diagnosticCodes: [],
          catalogGeneration: 2,
          catalogCompleteness: 'partial'
        }
      ]
    })
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'load tools' }).click()
    await expect.element(screen.getByTestId('catalog')).toHaveTextContent('idle:0:none:none')
  })

  it('authoritatively refreshes before surfacing cleanupIncomplete mutation errors', async () => {
    service.updateServer.mockRejectedValue(cleanupIncompleteError('update'))
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'update' }).click()
    await expect.element(screen.getByTestId('last-action')).toHaveTextContent('update-failed')
    expect(service.listServers).toHaveBeenCalledTimes(2)
  })

  it('refreshes authoritative state after a CAS conflict without retrying the mutation', async () => {
    service.updateServer.mockRejectedValue(conflictError('update'))
    const screen = await render(<Harness />)
    await expect.element(screen.getByTestId('status')).toHaveTextContent('ready')
    await screen.getByRole('button', { name: 'update' }).click()
    await expect.element(screen.getByTestId('last-action')).toHaveTextContent('update-failed')
    expect(service.updateServer).toHaveBeenCalledOnce()
    expect(service.listServers).toHaveBeenCalledTimes(2)
  })
})

function deferred<T>() {
  let resolve = (value: T) => {
    void value
  }
  let reject = (error: unknown) => {
    void error
  }
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}

function cleanupIncompleteError(operation: 'update') {
  return new HostInvocationError({
    message: 'safe outer',
    data: {
      schemaVersion: 1,
      type: 'mcpManagement',
      operation,
      code: 'cleanupIncomplete',
      recovery: 'doNotRetry',
      message: 'Cleanup requires authoritative confirmation.',
      serverId: SERVER_A,
      currentRegistryRevision: 5
    }
  })
}

function conflictError(operation: 'update') {
  return new HostInvocationError({
    message: 'safe outer',
    data: {
      schemaVersion: 1,
      type: 'mcpManagement',
      operation,
      code: 'conflict',
      recovery: 'refresh',
      message: 'Configuration changed.',
      serverId: SERVER_A,
      currentRegistryRevision: 6
    }
  })
}
