import type {
  McpChangedNotification,
  McpServerCreateInput,
  McpServerDetailsOutput
} from '@mycopilot/protocol'
import {
  MCP_CATALOG_REFRESH_METHOD,
  MCP_CATALOG_TOOLS_METHOD,
  MCP_CHANGED_NOTIFICATION_METHOD,
  MCP_MANAGEMENT_ERROR_CODE,
  MCP_SERVER_ADD_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD,
  MCP_SERVER_DELETE_METHOD,
  MCP_SERVER_DISABLE_METHOD,
  MCP_SERVER_ENABLE_METHOD,
  MCP_SERVER_GET_METHOD,
  MCP_SERVER_LIST_METHOD,
  MCP_SERVER_RESTART_METHOD,
  MCP_SERVER_START_METHOD,
  MCP_SERVER_STATUS_METHOD,
  MCP_SERVER_STOP_METHOD,
  MCP_SERVER_UPDATE_METHOD
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())
const rpcOnNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
    readonly onNotification = rpcOnNotification
  }
}))

import { CoreServer } from './coreServer'

const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const configEpoch = '41818332-0842-4d2e-808f-175b70eb4628'
const authorizationId = '8e9a3118-88f4-4a53-9dba-82f89232d07e'
const sourceEpoch = 'd8346f56-c0f2-4e9d-b394-74dfd9e959e1'
const configDigest = 'a'.repeat(64)
const precondition = {
  expectedRegistryRevision: 7,
  expectedConfigEpoch: configEpoch,
  expectedConfigDigest: configDigest
} as const
const mutation = { schemaVersion: 1, serverId, precondition } as const
const create = {
  schemaVersion: 1,
  displayName: 'Owned fixture',
  transport: 'stdio',
  executable: '/owned/fixture',
  arguments: ['--mode', '', 'value with spaces;$(not-a-shell)'],
  cwd: '/owned',
  approvalMode: 'prompt'
} satisfies McpServerCreateInput
const details = {
  schemaVersion: 1,
  serverId,
  displayName: 'Owned fixture',
  scope: 'user',
  source: 'userManual',
  transport: 'stdio',
  enabled: false,
  trust: 'untrusted',
  approvalMode: 'prompt',
  launchAuthorizationState: 'required',
  state: 'disabled',
  registryRevision: 7,
  configEpoch,
  configDigest,
  catalogGeneration: 0,
  catalogCompleteness: 'failed',
  toolCount: 0,
  activeCallCount: 0,
  updatedAtMs: 1_753_843_200_000,
  executable: '/owned/fixture',
  arguments: create.arguments,
  cwd: '/owned',
  createdAtMs: 1_753_843_100_000
} as const
const summary: Record<string, unknown> = { ...details }
delete summary.executable
delete summary.arguments
delete summary.cwd
delete summary.createdAtMs
const detailsOutput = {
  schemaVersion: 1,
  registryRevision: 7,
  server: details
} satisfies McpServerDetailsOutput

describe('CoreServer MCP management client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
    rpcOnNotification.mockReset()
    rpcOnNotification.mockReturnValue(vi.fn())
  })

  it('uses stable RPC methods and keeps launch argv separate', async () => {
    rpcRequest
      .mockResolvedValueOnce({ schemaVersion: 1, registryRevision: 7, servers: [summary] })
      .mockResolvedValueOnce(detailsOutput)
      .mockResolvedValueOnce(detailsOutput)
    const server = new CoreServer()

    await server.listMcpServers()
    await server.addMcpServer(create)
    await server.enableMcpServer(mutation)

    expect(rpcRequest.mock.calls).toEqual([
      [MCP_SERVER_LIST_METHOD, { schemaVersion: 1 }],
      [MCP_SERVER_ADD_METHOD, create],
      [MCP_SERVER_ENABLE_METHOD, mutation]
    ])
  })

  it('routes every Server read and CAS mutation through its dedicated RPC method', async () => {
    rpcRequest.mockResolvedValue(detailsOutput)
    const server = new CoreServer()
    const identity = { schemaVersion: 1, serverId } as const
    const update = {
      ...create,
      serverId,
      precondition,
      displayName: 'Owned fixture updated',
      approvalMode: 'deny'
    } as const

    await server.getMcpServer(identity)
    await server.updateMcpServer(update)
    await server.deleteMcpServer(mutation)
    await server.disableMcpServer(mutation)
    await server.startMcpServer(mutation)
    await server.stopMcpServer(mutation)
    await server.restartMcpServer(mutation)
    await server.getMcpServerStatus(identity)

    expect(rpcRequest.mock.calls).toEqual([
      [MCP_SERVER_GET_METHOD, identity],
      [MCP_SERVER_UPDATE_METHOD, update],
      [MCP_SERVER_DELETE_METHOD, mutation],
      [MCP_SERVER_DISABLE_METHOD, mutation],
      [MCP_SERVER_START_METHOD, mutation],
      [MCP_SERVER_STOP_METHOD, mutation],
      [MCP_SERVER_RESTART_METHOD, mutation],
      [MCP_SERVER_STATUS_METHOD, identity]
    ])
  })

  it('routes prepare/commit and Catalog operations through strict DTOs', async () => {
    const preview = {
      schemaVersion: 1,
      authorizationId,
      expiresAtMs: 1_753_843_260_000,
      serverId,
      displayName: 'Owned fixture',
      executable: details.executable,
      arguments: details.arguments,
      cwd: details.cwd,
      launchSpecDigest: 'b'.repeat(64),
      precondition
    }
    const page = {
      schemaVersion: 1,
      serverId,
      catalogGeneration: 0,
      catalogCompleteness: 'failed',
      tools: []
    }
    rpcRequest
      .mockResolvedValueOnce(preview)
      .mockResolvedValueOnce({ schemaVersion: 1, authorized: true, server: details })
      .mockResolvedValueOnce(page)
      .mockResolvedValueOnce(page)
    const server = new CoreServer()

    await server.prepareMcpLaunchAuthorization(mutation)
    await server.commitMcpLaunchAuthorization({
      schemaVersion: 1,
      authorizationId,
      precondition
    })
    await server.listMcpTools({ schemaVersion: 1, serverId, limit: 50 })
    await server.refreshMcpCatalog(mutation)

    expect(rpcRequest.mock.calls).toEqual([
      [MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD, mutation],
      [
        MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
        { schemaVersion: 1, authorizationId, precondition }
      ],
      [MCP_CATALOG_TOOLS_METHOD, { schemaVersion: 1, serverId, limit: 50 }],
      [MCP_CATALOG_REFRESH_METHOD, mutation]
    ])
  })

  it('rejects malformed Renderer input before it can cross JSON-RPC', () => {
    expect(() =>
      new CoreServer().addMcpServer({
        ...create,
        environment: { TOKEN: 'fixed-canary-must-not-cross' }
      } as never)
    ).toThrow(/unexpected field environment/)
    expect(rpcRequest).not.toHaveBeenCalled()
  })

  it('rejects successful responses carrying private or unknown fields', async () => {
    rpcRequest.mockResolvedValueOnce({
      ...detailsOutput,
      server: { ...details, stderr: 'fixed-canary-must-not-cross' }
    })
    await expect(new CoreServer().enableMcpServer(mutation)).rejects.toThrow(
      /unexpected field stderr/
    )
  })

  it('preserves only validated bounded MCP error data', async () => {
    const data = {
      schemaVersion: 1,
      type: 'mcpManagement',
      operation: 'enable',
      code: 'authorizationRequired',
      recovery: 'requestLaunchAuthorization',
      message: 'Authorize the current launch configuration.',
      serverId,
      currentRegistryRevision: 7
    } as const
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error('stderr fixed-canary-must-not-cross'), {
        code: MCP_MANAGEMENT_ERROR_CODE,
        data
      })
    )

    await expect(new CoreServer().enableMcpServer(mutation)).rejects.toMatchObject({
      message: data.message,
      code: MCP_MANAGEMENT_ERROR_CODE,
      data
    })
  })

  it('drops invalid notifications rather than forwarding rejected bodies', () => {
    let notificationHandler: ((value: unknown) => void) | undefined
    rpcOnNotification.mockImplementation((_method, handler) => {
      notificationHandler = handler
      return vi.fn()
    })
    const receiver = vi.fn()
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    new CoreServer().onMcpChanged(receiver)

    const safe = {
      schemaVersion: 1,
      sourceEpoch,
      sequence: 9,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'ready'
    } satisfies McpChangedNotification
    notificationHandler?.(safe)
    notificationHandler?.({ ...safe, rawArguments: 'fixed-canary-must-not-cross' })

    expect(rpcOnNotification).toHaveBeenCalledWith(
      MCP_CHANGED_NOTIFICATION_METHOD,
      expect.any(Function)
    )
    expect(receiver).toHaveBeenCalledOnce()
    expect(receiver).toHaveBeenCalledWith(safe)
    expect(warning).toHaveBeenCalledWith('Ignored invalid mcp.changed notification')
    warning.mockRestore()
  })
})
