import { randomUUID } from 'node:crypto'
import { describe, expect, it, vi } from 'vitest'

import {
  MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
  type ManagedPlaywrightCancelNotification,
  type ManagedPlaywrightCommandNotification,
  type ManagedPlaywrightCompletionInput
} from '@mycopilot/protocol'
import { ManagedPlaywrightBridgeHost } from './ManagedPlaywrightBridgeHost'
import {
  ManagedPlaywrightMcpHost,
  type ManagedMcpClient,
  type ManagedPlaywrightConnectionFactory
} from './ManagedPlaywrightMcpHost'
import { MANAGED_PLAYWRIGHT_CATALOG_LOCK } from './managedPlaywrightCatalog'
import { MANAGED_PLAYWRIGHT_SERVER_ID } from './managedPlaywrightManifest'

const AUTHORIZATION_CONTEXT = {
  runId: 'run-1',
  capabilityId: 'browser_automation' as const,
  activationId: '4af35bbd-cb1e-4b20-a021-92cd6b160829',
  manifestDigest: `sha256:${'a'.repeat(64)}`,
  policyRevision: 1,
  grantExpiresAtMs: 2_000_000_000_000,
  invocationId: 'c12f8536-faf9-439e-8e5a-a76049074e73',
  callId: 'call-1',
  triggerToolName: 'browser_snapshot',
  callReason: 'Inspect the local fixture.'
}

class FakeCore {
  readonly completions: ManagedPlaywrightCompletionInput[] = []
  private cancel?: (input: ManagedPlaywrightCancelNotification) => void
  private command?: (input: ManagedPlaywrightCommandNotification) => void

  async completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<void> {
    this.completions.push(input)
  }

  onManagedPlaywrightCancel(
    handler: (input: ManagedPlaywrightCancelNotification) => void
  ): () => void {
    this.cancel = handler
    return () => {
      if (this.cancel === handler) this.cancel = undefined
    }
  }

  onManagedPlaywrightCommand(
    handler: (input: ManagedPlaywrightCommandNotification) => void
  ): () => void {
    this.command = handler
    return () => {
      if (this.command === handler) this.command = undefined
    }
  }

  emitCommand(input: ManagedPlaywrightCommandNotification): void {
    this.command?.(input)
  }

  emitCancel(input: ManagedPlaywrightCancelNotification): void {
    this.cancel?.(input)
  }
}

describe('ManagedPlaywrightBridgeHost', () => {
  it('ignores the wrong managed Server identity and duplicate active request IDs', async () => {
    const core = new FakeCore()
    let resolveConnect: (() => void) | undefined
    const host = new ManagedPlaywrightBridgeHost({
      core,
      createHost: () =>
        hostWith({
          createOfficialConnection: async () => ({
            connect: () => new Promise<void>((resolve) => (resolveConnect = resolve)),
            close: vi.fn(async () => undefined)
          })
        })
    })
    const request = command({ type: 'connect' })
    core.emitCommand({ ...request, serverId: randomUUID() })
    core.emitCommand(request)
    core.emitCommand(request)
    await vi.waitFor(() => expect(resolveConnect).toBeTypeOf('function'))
    resolveConnect?.()

    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.completions[0]).toMatchObject({
      requestId: request.requestId,
      outcome: { type: 'connected' }
    })
    await host.close()
  })

  it('reports phase-accurate dispatch certainty for reviewed call failures', async () => {
    const core = new FakeCore()
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'x'.repeat(256 * 1024 + 1) }],
        isError: false
      })
      .mockRejectedValueOnce(new Error('untrusted upstream failure'))
    const host = new ManagedPlaywrightBridgeHost({
      core,
      createHost: () => hostWith({ callTool })
    })

    const invalid = command({
      type: 'call_tool',
      name: 'browser_navigate',
      arguments: { url: 'http://127.0.0.1/fixture' },
      timeoutMs: 1_000,
      authorizationContext: { ...AUTHORIZATION_CONTEXT, triggerToolName: 'browser_navigate' }
    })
    const oversized = command({
      type: 'call_tool',
      name: 'browser_snapshot',
      arguments: { call_reason: 'Inspect fixture.' },
      timeoutMs: 1_000,
      authorizationContext: AUTHORIZATION_CONTEXT
    })
    const protocol = command({
      type: 'call_tool',
      name: 'browser_snapshot',
      arguments: { call_reason: 'Inspect fixture.' },
      timeoutMs: 1_000,
      authorizationContext: AUTHORIZATION_CONTEXT
    })
    core.emitCommand(invalid)
    core.emitCommand(oversized)
    core.emitCommand(protocol)

    await vi.waitFor(() => expect(core.completions).toHaveLength(3))
    const byRequest = new Map(core.completions.map((value) => [value.requestId, value.outcome]))
    expect(byRequest.get(invalid.requestId)).toEqual({
      type: 'error',
      code: 'invalid_arguments',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(byRequest.get(oversized.requestId)).toEqual({
      type: 'error',
      code: 'output_too_large',
      dispatchCertainty: 'response_received'
    })
    expect(byRequest.get(protocol.requestId)).toEqual({
      type: 'error',
      code: 'protocol_error',
      dispatchCertainty: 'possibly_dispatched'
    })
    await host.close()
  })

  it('cancels and retires an in-progress connect so late completion cannot revive it', async () => {
    const core = new FakeCore()
    let resolveConnection:
      ((connection: Awaited<ReturnType<ManagedPlaywrightConnectionFactory>>) => void) | undefined
    const closeOfficial = vi.fn(async () => undefined)
    const detachAutomation = vi.fn(async () => undefined)
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      createHost: () =>
        hostWith({
          detachAutomation,
          createOfficialConnection: () =>
            new Promise((resolve) => {
              resolveConnection = resolve
            })
        })
    })
    const request = command({ type: 'connect' })
    core.emitCommand(request)
    await vi.waitFor(() => expect(resolveConnection).toBeTypeOf('function'))
    core.emitCancel({
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: request.requestId,
      reason: 'shutdown'
    })
    resolveConnection?.({ connect: vi.fn(async () => undefined), close: closeOfficial })

    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.completions[0].outcome).toEqual({
      type: 'error',
      code: 'cancelled',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    await vi.waitFor(() => expect(closeOfficial).toHaveBeenCalledOnce())
    expect(detachAutomation).toHaveBeenCalled()
    await bridge.close()
  })
})

function command(
  commandValue: ManagedPlaywrightCommandNotification['command']
): ManagedPlaywrightCommandNotification {
  return {
    schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
    requestId: randomUUID(),
    serverId: MANAGED_PLAYWRIGHT_SERVER_ID,
    deadlineMs: Date.now() + 5_000,
    command: commandValue
  }
}

function hostWith(options: {
  callTool?: ManagedMcpClient['callTool']
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  detachAutomation?: () => Promise<void>
}): ManagedPlaywrightMcpHost {
  const upstreamTools = MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: structuredClone(tool.inputSchema),
    annotations: structuredClone(tool.annotations ?? {})
  }))
  return new ManagedPlaywrightMcpHost({
    getBrowserContext: async () => {
      throw new Error('not used by this bridge fixture')
    },
    closeSurface: vi.fn(async () => undefined),
    detachAutomation: options.detachAutomation ?? vi.fn(async () => undefined),
    createOfficialConnection:
      options.createOfficialConnection ??
      (async () => ({
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined)
      })),
    createClient: () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined),
      listTools: vi.fn(async () => ({ tools: upstreamTools })),
      callTool:
        options.callTool ??
        vi.fn(async () => ({ content: [{ type: 'text', text: 'ok' }], isError: false }))
    })
  })
}
