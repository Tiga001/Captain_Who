import type { IpcMainInvokeEvent } from 'electron'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { McpChangedNotification, McpServerDetailsOutput } from '@mycopilot/protocol'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const sent = vi.hoisted(() => vi.fn())
const getAllWindows = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({
  BrowserWindow: {
    fromWebContents: vi.fn(() => undefined),
    getAllWindows
  },
  dialog: {
    showOpenDialog: vi.fn()
  }
}))

import { createMcpNativeDialogs, registerMcpIpc, type McpIpcNativeDialogs } from '../ipc/mcpIpc'

const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const configEpoch = '41818332-0842-4d2e-808f-175b70eb4628'
const authorizationId = '8e9a3118-88f4-4a53-9dba-82f89232d07e'
const sourceEpoch = 'd8346f56-c0f2-4e9d-b394-74dfd9e959e1'
const restartedSourceEpoch = 'b53b963f-dca3-4740-8d7a-eeb8d27536d6'
const configDigest = 'a'.repeat(64)
const precondition = {
  expectedRegistryRevision: 7,
  expectedConfigEpoch: configEpoch,
  expectedConfigDigest: configDigest
} as const
const mutation = { schemaVersion: 1, serverId, precondition } as const
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
  arguments: ['--mode', 'fixture'] as string[],
  cwd: '/owned',
  createdAtMs: 1_753_843_100_000
} as const
const detailsOutput = {
  schemaVersion: 1,
  registryRevision: 7,
  server: details
} satisfies McpServerDetailsOutput

function createTrustedIpc(): {
  handlers: Map<string, (...args: unknown[]) => unknown>
  ipc: TrustedIpcMain
} {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  return {
    handlers,
    ipc: {
      handle: (channel, handler) => handlers.set(channel, handler as never),
      on: vi.fn()
    }
  }
}

function createCore(overrides: Record<string, unknown> = {}) {
  return {
    onMcpChanged: vi.fn(() => vi.fn()),
    listMcpServers: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      registryRevision: 7,
      servers: [details]
    }),
    getMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    addMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    updateMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    deleteMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    prepareMcpLaunchAuthorization: vi.fn().mockResolvedValue({
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
    }),
    commitMcpLaunchAuthorization: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      authorized: true,
      server: { ...details, trust: 'userApproved', launchAuthorizationState: 'authorized' }
    }),
    enableMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    disableMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    startMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    stopMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    restartMcpServer: vi.fn().mockResolvedValue(detailsOutput),
    getMcpServerStatus: vi.fn().mockResolvedValue(detailsOutput),
    listMcpTools: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      serverId,
      catalogGeneration: 0,
      catalogCompleteness: 'failed',
      tools: []
    }),
    refreshMcpCatalog: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      serverId,
      catalogGeneration: 0,
      catalogCompleteness: 'failed',
      tools: []
    }),
    ...overrides
  }
}

const event = { sender: {} } as IpcMainInvokeEvent

function createNativeDialogs(overrides: Partial<McpIpcNativeDialogs> = {}): McpIpcNativeDialogs {
  return {
    selectExecutable: vi.fn(),
    selectWorkingDirectory: vi.fn(),
    ...overrides
  }
}

describe('MCP trusted IPC', () => {
  beforeEach(() => {
    sent.mockReset()
    getAllWindows.mockReturnValue([
      {
        isDestroyed: () => false,
        webContents: { isDestroyed: () => false, send: sent }
      }
    ])
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('registers only explicit management methods and rejects forbidden input before Core', async () => {
    const trusted = createTrustedIpc()
    const core = createCore()
    const cleanup = registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())

    expect([...trusted.handlers.keys()]).toEqual(
      expect.arrayContaining([
        'host:mcp.listServers',
        'host:mcp.getServer',
        'host:mcp.addServer',
        'host:mcp.updateServer',
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
    )
    expect([...trusted.handlers.keys()]).not.toContain('host:mcp.callTool')
    expect([...trusted.handlers.keys()]).not.toContain('host:mcp.request')

    const add = trusted.handlers.get('host:mcp.addServer')
    await expect(
      add?.(event, {
        schemaVersion: 1,
        displayName: 'Owned fixture',
        transport: 'stdio',
        executable: '/owned/fixture',
        arguments: [],
        cwd: '/owned',
        approvalMode: 'prompt',
        environment: { TOKEN: 'fixed-canary-must-not-cross' }
      })
    ).resolves.toEqual({
      ok: false,
      error: {
        message: 'The MCP management request is invalid.',
        code: -32030,
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'add',
          code: 'invalidInput',
          recovery: 'fixInput',
          message: 'The MCP management request is invalid.'
        }
      }
    })
    expect(core.addMcpServer).not.toHaveBeenCalled()
    cleanup()
  })

  it('commits only the authorization id and frozen precondition after the trusted request', async () => {
    const trusted = createTrustedIpc()
    const core = createCore()
    registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())

    const authorize = trusted.handlers.get('host:mcp.requestLaunchAuthorization')
    await expect(authorize?.(event, mutation)).resolves.toMatchObject({
      ok: true,
      value: { schemaVersion: 1, authorized: true }
    })
    expect(core.commitMcpLaunchAuthorization).toHaveBeenCalledWith({
      schemaVersion: 1,
      authorizationId,
      precondition
    })
  })

  it('fails closed if the prepared identity drifts before commit', async () => {
    const core = createCore({
      prepareMcpLaunchAuthorization: vi.fn().mockResolvedValue({
        schemaVersion: 1,
        authorizationId,
        expiresAtMs: 1_753_843_260_000,
        serverId: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
        displayName: 'Changed fixture',
        executable: '/owned/fixture',
        arguments: [],
        cwd: '/owned',
        launchSpecDigest: 'b'.repeat(64),
        precondition
      })
    })
    const trusted = createTrustedIpc()
    registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())

    const authorize = trusted.handlers.get('host:mcp.requestLaunchAuthorization')
    await expect(authorize?.(event, mutation)).resolves.toEqual({
      ok: false,
      error: {
        message: 'MCP management operation failed.',
        code: -32030,
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'prepareLaunchAuthorization',
          code: 'internalSafeError',
          recovery: 'doNotRetry',
          message: 'MCP management operation failed.'
        }
      }
    })
    expect(core.commitMcpLaunchAuthorization).not.toHaveBeenCalled()
  })

  it('preserves validated MCP errors and replaces all other errors with a fixed projection', async () => {
    const safeData = {
      schemaVersion: 1,
      type: 'mcpManagement',
      operation: 'enable',
      code: 'authorizationRequired',
      recovery: 'requestLaunchAuthorization',
      message: 'Authorize the current launch configuration.',
      serverId,
      currentRegistryRevision: 7
    }
    const trusted = createTrustedIpc()
    const core = createCore({
      enableMcpServer: vi.fn().mockRejectedValue(
        Object.assign(new Error('unsafe process detail fixed-canary-must-not-cross'), {
          code: -32030,
          data: safeData
        })
      ),
      stopMcpServer: vi.fn().mockRejectedValue(new Error('stderr fixed-canary-must-not-cross'))
    })
    registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())

    await expect(trusted.handlers.get('host:mcp.enableServer')?.(event, mutation)).resolves.toEqual(
      {
        ok: false,
        error: { message: safeData.message, code: -32030, data: safeData }
      }
    )
    await expect(trusted.handlers.get('host:mcp.stopServer')?.(event, mutation)).resolves.toEqual({
      ok: false,
      error: {
        message: 'MCP management operation failed.',
        code: -32030,
        data: {
          schemaVersion: 1,
          type: 'mcpManagement',
          operation: 'stop',
          code: 'internalSafeError',
          recovery: 'doNotRetry',
          message: 'MCP management operation failed.'
        }
      }
    })
  })

  it('coalesces notification storms per Server and forwards only the latest safe event', async () => {
    vi.useFakeTimers()
    let onChanged: ((event: McpChangedNotification) => void) | undefined
    const unsubscribe = vi.fn()
    const core = createCore({
      onMcpChanged: vi.fn((handler) => {
        onChanged = handler
        return unsubscribe
      })
    })
    const trusted = createTrustedIpc()
    const cleanup = registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())
    onChanged?.({
      schemaVersion: 1,
      sourceEpoch,
      sequence: 8,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'starting'
    })
    onChanged?.({
      schemaVersion: 1,
      sourceEpoch,
      sequence: 9,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'ready'
    })

    expect(sent).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(25)
    expect(sent).toHaveBeenCalledOnce()
    expect(sent).toHaveBeenCalledWith(
      'host:mcp.changed',
      expect.objectContaining({ sequence: 9, state: 'ready' })
    )
    onChanged?.({
      schemaVersion: 1,
      sourceEpoch,
      sequence: 8,
      registryRevision: 6,
      kind: 'stateChanged',
      serverId,
      state: 'starting'
    })
    await vi.advanceTimersByTimeAsync(25)
    expect(sent).toHaveBeenCalledOnce()

    onChanged?.({
      schemaVersion: 1,
      sourceEpoch: restartedSourceEpoch,
      sequence: 1,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'discovering'
    })
    await vi.advanceTimersByTimeAsync(25)
    expect(sent).toHaveBeenCalledTimes(2)
    expect(sent).toHaveBeenLastCalledWith(
      'host:mcp.changed',
      expect.objectContaining({
        sourceEpoch: restartedSourceEpoch,
        sequence: 1,
        state: 'discovering'
      })
    )
    cleanup()
    expect(unsubscribe).toHaveBeenCalledOnce()
  })

  it('clears pending invalidations and resets the sequence watermark when Core epoch changes', async () => {
    vi.useFakeTimers()
    let onChanged: ((event: McpChangedNotification) => void) | undefined
    const core = createCore({
      onMcpChanged: vi.fn((handler) => {
        onChanged = handler
        return vi.fn()
      })
    })
    const trusted = createTrustedIpc()
    const cleanup = registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())

    onChanged?.({
      schemaVersion: 1,
      sourceEpoch,
      sequence: 900,
      registryRevision: 6,
      kind: 'stateChanged',
      serverId,
      state: 'starting'
    })
    onChanged?.({
      schemaVersion: 1,
      sourceEpoch: restartedSourceEpoch,
      sequence: 1,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'ready'
    })

    await vi.advanceTimersByTimeAsync(25)
    expect(sent).toHaveBeenCalledOnce()
    expect(sent).toHaveBeenCalledWith(
      'host:mcp.changed',
      expect.objectContaining({
        sourceEpoch: restartedSourceEpoch,
        sequence: 1,
        registryRevision: 7,
        state: 'ready'
      })
    )
    cleanup()
  })

  it('silently drops invalid notifications and caps pending unique Server invalidations', async () => {
    vi.useFakeTimers()
    let onChanged: ((event: unknown) => void) | undefined
    const core = createCore({
      onMcpChanged: vi.fn((handler) => {
        onChanged = handler
        return vi.fn()
      })
    })
    const trusted = createTrustedIpc()
    const cleanup = registerMcpIpc(trusted.ipc, core as never, createNativeDialogs())

    expect(() =>
      onChanged?.({
        schemaVersion: 1,
        sourceEpoch,
        sequence: 1,
        registryRevision: 7,
        kind: 'stateChanged',
        serverId,
        state: 'ready',
        rawArguments: 'fixed-canary-must-not-cross'
      })
    ).not.toThrow()
    for (let index = 1; index <= 1025; index += 1) {
      onChanged?.({
        schemaVersion: 1,
        sourceEpoch,
        sequence: index,
        registryRevision: 7,
        kind: 'stateChanged',
        serverId: `00000000-0000-4000-8000-${index.toString().padStart(12, '0')}`,
        state: 'ready'
      })
    }

    await vi.advanceTimersByTimeAsync(25)
    expect(sent).toHaveBeenCalledOnce()
    expect(sent).toHaveBeenCalledWith('host:mcp.changed', {
      schemaVersion: 1,
      sourceEpoch,
      sequence: 1025,
      registryRevision: 7,
      kind: 'resyncRequired'
    })
    expect(JSON.stringify(sent.mock.calls)).not.toContain('fixed-canary')
    cleanup()
  })

  it('selects paths without reading or executing them and treats cancellation as null', async () => {
    const showOpenDialog = vi
      .fn()
      .mockResolvedValueOnce({ canceled: false, filePaths: ['fixtures/owned-server'] })
      .mockResolvedValueOnce({ canceled: true, filePaths: ['/ignored'] })
    const dialogs = createMcpNativeDialogs(showOpenDialog)

    await expect(dialogs.selectExecutable(event)).resolves.toMatch(/\/fixtures\/owned-server$/)
    await expect(dialogs.selectWorkingDirectory(event)).resolves.toBeNull()
    expect(showOpenDialog).toHaveBeenNthCalledWith(1, event, {
      title: 'Select MCP server executable',
      properties: ['openFile']
    })
    expect(showOpenDialog).toHaveBeenNthCalledWith(2, event, {
      title: 'Select MCP server working directory',
      properties: ['openDirectory']
    })
  })
})
