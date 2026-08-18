import type { IpcRenderer } from 'electron'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type {
  McpChangedNotification,
  McpServerCreateInput,
  McpServerDetailsOutput,
  McpServerMutationInput
} from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createMcpIpcBridge } from './McpIpcBridge'

type McpIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

function createIpcRenderer(): {
  invoke: ReturnType<typeof vi.fn>
  on: ReturnType<typeof vi.fn>
  removeListener: ReturnType<typeof vi.fn>
  renderer: McpIpcRenderer
} {
  const invoke = vi.fn()
  const on = vi.fn()
  const removeListener = vi.fn()
  return {
    invoke,
    on,
    removeListener,
    renderer: { invoke, on, removeListener } as unknown as McpIpcRenderer
  }
}

const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const sourceEpoch = 'd8346f56-c0f2-4e9d-b394-74dfd9e959e1'
const mutation = {
  schemaVersion: 1,
  serverId,
  precondition: {
    expectedRegistryRevision: 7,
    expectedConfigEpoch: '41818332-0842-4d2e-808f-175b70eb4628',
    expectedConfigDigest: 'a'.repeat(64)
  }
} satisfies McpServerMutationInput

describe('MCP IPC bridge', () => {
  it('routes only the explicit management surface without unwrapping results', async () => {
    const response = {
      ok: false,
      error: {
        message: 'Authorize the current launch configuration.',
        code: -32030,
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'enable',
          code: 'authorizationRequired',
          recovery: 'requestLaunchAuthorization',
          message: 'Authorize the current launch configuration.',
          serverId
        }
      }
    } satisfies HostInvocationResult<McpServerDetailsOutput>
    const ipc = createIpcRenderer()
    ipc.invoke.mockResolvedValue(response)
    const bridge = createMcpIpcBridge(ipc.renderer)

    await expect(bridge.listBuiltinCapabilities()).resolves.toBe(response)
    await expect(
      bridge.setBuiltinCapabilityAllowed({
        schemaVersion: 1,
        capabilityId: 'browser_automation',
        allowed: true,
        expectedPolicyRevision: 0
      })
    ).resolves.toBe(response)
    await expect(bridge.listServers()).resolves.toBe(response)
    await expect(bridge.getServer({ schemaVersion: 1, serverId })).resolves.toBe(response)
    await expect(bridge.deleteServer(mutation)).resolves.toBe(response)
    await expect(bridge.requestLaunchAuthorization(mutation)).resolves.toBe(response)
    await expect(bridge.enableServer(mutation)).resolves.toBe(response)
    await expect(bridge.disableServer(mutation)).resolves.toBe(response)
    await expect(bridge.startServer(mutation)).resolves.toBe(response)
    await expect(bridge.stopServer(mutation)).resolves.toBe(response)
    await expect(bridge.restartServer(mutation)).resolves.toBe(response)
    await expect(bridge.getStatus({ schemaVersion: 1, serverId })).resolves.toBe(response)
    await expect(bridge.listTools({ schemaVersion: 1, serverId, limit: 50 })).resolves.toBe(
      response
    )
    await expect(bridge.refreshCatalog(mutation)).resolves.toBe(response)
    await expect(bridge.selectExecutable()).resolves.toBe(response)
    await expect(bridge.selectWorkingDirectory()).resolves.toBe(response)

    expect(ipc.invoke.mock.calls.map(([channel]) => channel)).toEqual([
      'host:mcp.listBuiltinCapabilities',
      'host:mcp.setBuiltinCapabilityAllowed',
      'host:mcp.listServers',
      'host:mcp.getServer',
      'host:mcp.deleteServer',
      'host:mcp.requestLaunchAuthorization',
      'host:mcp.enableServer',
      'host:mcp.disableServer',
      'host:mcp.startServer',
      'host:mcp.stopServer',
      'host:mcp.restartServer',
      'host:mcp.getStatus',
      'host:mcp.listTools',
      'host:mcp.refreshCatalog',
      'host:mcp.selectExecutable',
      'host:mcp.selectWorkingDirectory'
    ])
    expect(bridge).not.toHaveProperty('callTool')
    expect(bridge).not.toHaveProperty('request')
    expect(bridge).not.toHaveProperty('invoke')
  })

  it('keeps executable and argv separate for add and update', async () => {
    const ipc = createIpcRenderer()
    const bridge = createMcpIpcBridge(ipc.renderer)
    const create = {
      schemaVersion: 1,
      displayName: 'Owned fixture',
      transport: 'stdio',
      executable: '/owned/fixture',
      arguments: ['--mode', '', 'value with spaces;$(not-a-shell)'],
      cwd: '/owned',
      approvalMode: 'prompt'
    } satisfies McpServerCreateInput

    await bridge.addServer(create)
    await bridge.updateServer({ ...create, serverId, precondition: mutation.precondition })

    expect(ipc.invoke).toHaveBeenNthCalledWith(1, 'host:mcp.addServer', create)
    expect(ipc.invoke).toHaveBeenNthCalledWith(2, 'host:mcp.updateServer', {
      ...create,
      serverId,
      precondition: mutation.precondition
    })
  })

  it('removes the exact changed listener', () => {
    const ipc = createIpcRenderer()
    const bridge = createMcpIpcBridge(ipc.renderer)
    const handler = vi.fn()
    const changed = {
      schemaVersion: 1,
      sourceEpoch,
      sequence: 9,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'ready'
    } satisfies McpChangedNotification

    const unsubscribe = bridge.onChanged(handler)
    const listener = ipc.on.mock.calls[0]?.[1] as
      ((event: unknown, payload: McpChangedNotification) => void) | undefined
    listener?.({}, changed)

    expect(handler).toHaveBeenCalledWith(changed)
    unsubscribe()
    expect(ipc.removeListener).toHaveBeenCalledWith('host:mcp.changed', listener)
  })
})
