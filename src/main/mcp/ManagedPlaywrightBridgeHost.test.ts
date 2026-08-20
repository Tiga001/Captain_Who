import { randomUUID } from 'node:crypto'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it, vi } from 'vitest'

import {
  MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
  type ManagedPlaywrightCancelNotification,
  type ManagedPlaywrightCommandNotification,
  type ManagedPlaywrightCompletionInput,
  type ManagedPlaywrightDispatchPhaseInput
} from '@mycopilot/protocol'
import { ManagedPlaywrightBridgeHost } from './ManagedPlaywrightBridgeHost'
import { ManagedPlaywrightSensitiveTargetBindingBroker } from './ManagedPlaywrightSensitiveTargetBindingBroker'
import {
  ManagedPlaywrightMcpHost,
  type ManagedMcpClient,
  type ManagedPlaywrightConnectionFactory,
  type ManagedPlaywrightSurfaceGroupAdapter
} from './ManagedPlaywrightMcpHost'
import { MANAGED_PLAYWRIGHT_CATALOG_LOCK } from './managedPlaywrightCatalog'
import { MANAGED_PLAYWRIGHT_SERVER_ID } from './managedPlaywrightManifest'
import { BrowserFileBroker } from '../browser/BrowserFileBroker'

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
  readonly dispatchPhases: ManagedPlaywrightDispatchPhaseInput[] = []
  private cancel?: (input: ManagedPlaywrightCancelNotification) => void
  private command?: (input: ManagedPlaywrightCommandNotification) => void

  constructor(
    private readonly completeResult: (
      input: ManagedPlaywrightCompletionInput
    ) => Promise<boolean> = async () => true
  ) {}

  async completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<boolean> {
    this.completions.push(input)
    return await this.completeResult(input)
  }

  async acknowledgeManagedPlaywrightDispatchPhase(
    input: ManagedPlaywrightDispatchPhaseInput
  ): Promise<boolean> {
    this.dispatchPhases.push(input)
    return true
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
      sensitiveTargetBindings: targetBindingBroker(),
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
      sensitiveTargetBindings: targetBindingBroker(),
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
    expect(core.dispatchPhases.filter((phase) => phase.requestId === invalid.requestId)).toEqual([])
    expect(core.dispatchPhases.filter((phase) => phase.requestId === oversized.requestId)).toEqual([
      expect.objectContaining({ phase: 'possibly_dispatched' }),
      expect.objectContaining({ phase: 'response_received' })
    ])
    expect(core.dispatchPhases.filter((phase) => phase.requestId === protocol.requestId)).toEqual([
      expect.objectContaining({ phase: 'possibly_dispatched' })
    ])
    await host.close()
  })

  it('keeps a raw Host-construction failure definitely undispatched', async () => {
    const core = new FakeCore()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: targetBindingBroker(),
      createHost: () => {
        throw new Error('untrusted fixture construction failure')
      }
    })
    const request = command({
      type: 'call_tool',
      name: 'browser_snapshot',
      arguments: { call_reason: 'Inspect the local fixture.' },
      timeoutMs: 1_000,
      authorizationContext: AUTHORIZATION_CONTEXT
    })

    core.emitCommand(request)
    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.dispatchPhases).toEqual([])
    expect(core.completions[0]).toMatchObject({
      requestId: request.requestId,
      outcome: {
        type: 'error',
        code: 'internal_safe_error',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    })
    await bridge.close()
  })

  it('preserves a sensitive Host rejection as definitely not dispatched', async () => {
    const core = new FakeCore()
    const callTool = vi.fn<ManagedMcpClient['callTool']>()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: targetBindingBroker(),
      createHost: () =>
        hostWith({
          callTool,
          surfaceGroup: singleSurfaceGroupAdapter()
        })
    })
    const request = command({
      type: 'call_tool',
      name: 'browser_evaluate',
      arguments: {
        function: '() => document.title',
        call_reason: 'Exercise a missing sensitive grant.'
      },
      timeoutMs: 1_000,
      authorizationContext: {
        ...AUTHORIZATION_CONTEXT,
        triggerToolName: 'browser_evaluate'
      }
    })
    core.emitCommand(request)

    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.completions[0]).toMatchObject({
      requestId: request.requestId,
      outcome: {
        type: 'error',
        code: 'invalid_arguments',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    })
    expect(callTool).not.toHaveBeenCalled()
    await bridge.close()
  })

  it('delegates tab creation to the fixed official group transport', async () => {
    const core = new FakeCore()
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      getSensitiveTargetIdentity: () => ({
        surfaceId: 'surface-1',
        generation: 1,
        navigationEpoch: 1,
        origin: 'http://127.0.0.1'
      }),
      ensureActiveSurface: vi.fn(async () => surfaceView()),
      listSurfaces: vi.fn(() => [surfaceView()]),
      createSurface: vi.fn(async () => {
        throw Object.assign(new Error('untrusted capacity detail'), {
          code: 'browser.surface_capacity_exceeded'
        })
      }),
      selectSurface: vi.fn(async () => surfaceView()),
      closeSurfaceByIndex: vi.fn(async () => undefined)
    }
    const callTool = vi.fn<ManagedMcpClient['callTool']>().mockResolvedValue({
      content: [{ type: 'text', text: 'official managed tab created' }],
      isError: false
    })
    const host = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: targetBindingBroker(),
      createHost: () => hostWith({ callTool, surfaceGroup })
    })
    const request = command({
      type: 'call_tool',
      name: 'browser_tabs',
      arguments: { action: 'new', call_reason: 'Open another local fixture tab.' },
      timeoutMs: 1_000,
      authorizationContext: {
        ...AUTHORIZATION_CONTEXT,
        triggerToolName: 'browser_tabs',
        callReason: 'Open another local fixture tab.'
      }
    })
    core.emitCommand(request)

    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.completions[0].outcome).toMatchObject({
      type: 'tool_called',
      result: { isError: false }
    })
    expect(callTool).toHaveBeenCalledWith(
      { name: 'browser_tabs', arguments: { action: 'new' } },
      undefined,
      expect.any(Object)
    )
    expect(surfaceGroup.createSurface).not.toHaveBeenCalled()
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
      sensitiveTargetBindings: targetBindingBroker(),
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

  it('releases a prepared binding when Core no longer accepts the late completion', async () => {
    let releaseCompletion!: () => void
    const completionBarrier = new Promise<void>((resolve) => {
      releaseCompletion = resolve
    })
    const core = new FakeCore(async () => {
      await completionBarrier
      return false
    })
    const bindings = targetBindingBroker()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: bindings,
      createHost: () => hostWith({})
    })
    const now = Date.now()
    const request = command({
      type: 'prepare_sensitive_tool',
      input: {
        bindingRequestId: randomUUID(),
        bindingScope: 'managed_surface',
        runId: 'run-1',
        capabilityId: 'browser_automation',
        activationId: AUTHORIZATION_CONTEXT.activationId,
        manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
        policyRevision: 1,
        grantExpiresAtMs: now + 120_000,
        callId: 'call-sensitive-late',
        toolName: 'browser_evaluate',
        argumentsDigest: `sha256:${'b'.repeat(64)}`,
        createdAtMs: now,
        expiresAtMs: now + 60_000,
        filePreparation: null
      }
    })
    core.emitCommand(request)
    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.completions[0].outcome.type).toBe('sensitive_tool_prepared')
    expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })
    core.emitCancel({
      schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
      requestId: request.requestId,
      reason: 'cancelled'
    })
    releaseCompletion()
    await vi.waitFor(() => expect(bindings.snapshot()).toEqual({ bindings: 0, requests: 0 }))
    await bridge.close()
  })

  it('releases a prepared binding when the Core completion never settles', async () => {
    const core = new FakeCore(async () => await new Promise<boolean>(() => undefined))
    const bindings = targetBindingBroker()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      completionSettleMs: 10,
      sensitiveTargetBindings: bindings,
      createHost: () => hostWith({})
    })
    const now = Date.now()
    core.emitCommand(
      command({
        type: 'prepare_sensitive_tool',
        input: {
          bindingRequestId: randomUUID(),
          bindingScope: 'managed_surface',
          runId: 'run-1',
          capabilityId: 'browser_automation',
          activationId: AUTHORIZATION_CONTEXT.activationId,
          manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
          policyRevision: 1,
          grantExpiresAtMs: now + 120_000,
          callId: 'call-sensitive-completion-timeout',
          toolName: 'browser_evaluate',
          argumentsDigest: `sha256:${'c'.repeat(64)}`,
          createdAtMs: now,
          expiresAtMs: now + 60_000,
          filePreparation: null
        }
      })
    )
    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })
    await vi.waitFor(() => expect(bindings.snapshot()).toEqual({ bindings: 0, requests: 0 }))
    await bridge.close()
  })

  it('prepares a managed-profile approval with no active browser surface', async () => {
    const core = new FakeCore()
    const beginDispatchFence = vi.fn(() => ({ finish: () => undefined }))
    const bindings = new ManagedPlaywrightSensitiveTargetBindingBroker({
      beginDispatchFence,
      getActiveTarget: () => null
    })
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: bindings,
      createHost: () => hostWith({})
    })
    const now = Date.now()
    core.emitCommand(
      command({
        type: 'prepare_sensitive_tool',
        input: {
          bindingRequestId: randomUUID(),
          bindingScope: 'managed_browser_profile',
          runId: 'run-1',
          capabilityId: 'browser_automation',
          activationId: AUTHORIZATION_CONTEXT.activationId,
          manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
          policyRevision: 1,
          grantExpiresAtMs: now + 120_000,
          callId: 'call-zero-tab-cookie-list',
          toolName: 'browser_cookie_list',
          argumentsDigest: `sha256:${'f'.repeat(64)}`,
          createdAtMs: now,
          expiresAtMs: now + 60_000,
          filePreparation: null
        }
      })
    )

    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(core.completions[0].outcome).toMatchObject({
      type: 'sensitive_tool_prepared',
      origin: null
    })
    expect(beginDispatchFence).not.toHaveBeenCalled()
    await bridge.close()
  })

  it('rejects static binding-scope drift while allowing both reviewed cookie-set shapes', async () => {
    const core = new FakeCore()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: targetBindingBroker(),
      createHost: () => hostWith({})
    })
    const now = Date.now()
    const prepare = (
      toolName: string,
      bindingScope: 'managed_surface' | 'managed_browser_profile',
      suffix: string
    ): ManagedPlaywrightCommandNotification =>
      command({
        type: 'prepare_sensitive_tool',
        input: {
          bindingRequestId: randomUUID(),
          bindingScope,
          runId: 'run-1',
          capabilityId: 'browser_automation',
          activationId: AUTHORIZATION_CONTEXT.activationId,
          manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
          policyRevision: 1,
          grantExpiresAtMs: now + 120_000,
          callId: `call-binding-${suffix}`,
          toolName,
          argumentsDigest: `sha256:${suffix.repeat(64).slice(0, 64)}`,
          createdAtMs: now,
          expiresAtMs: now + 60_000,
          filePreparation: null
        }
      })
    const requests = [
      prepare('browser_cookie_list', 'managed_surface', 'a'),
      prepare('browser_evaluate', 'managed_browser_profile', 'b'),
      prepare('browser_cookie_set', 'managed_surface', 'c'),
      prepare('browser_cookie_set', 'managed_browser_profile', 'd')
    ]
    for (const request of requests) core.emitCommand(request)

    await vi.waitFor(() => expect(core.completions).toHaveLength(requests.length))
    const outcomes = new Map(
      core.completions.map((completion) => [completion.requestId, completion.outcome])
    )
    expect(outcomes.get(requests[0].requestId)).toEqual({
      type: 'error',
      code: 'tool_not_reviewed',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(outcomes.get(requests[1].requestId)).toEqual({
      type: 'error',
      code: 'tool_not_reviewed',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(outcomes.get(requests[2].requestId)).toMatchObject({
      type: 'sensitive_tool_prepared',
      origin: 'http://127.0.0.1'
    })
    expect(outcomes.get(requests[3].requestId)).toMatchObject({
      type: 'sensitive_tool_prepared',
      origin: null
    })
    await bridge.close()
  })

  it('freezes an authorized workspace path before approval and publishes only a safe basename', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mycopilot-bridge-file-preflight-'))
    const source = join(root, '浙江大学2026年招生资料汇编.pptx')
    await writeFile(source, 'fixture-presentation')
    const selectFiles = vi.fn(async () => [source])
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(root, 'browser-automation-files'),
      selectionProvider: { selectFiles }
    })
    const bindings = targetBindingBroker(({ runId, callId }) => {
      void fileBroker.releaseToolCall({ runId, toolCallId: callId })
    })
    const core = new FakeCore()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      fileBroker,
      sensitiveTargetBindings: bindings,
      createHost: () => hostWith({})
    })
    const now = Date.now()
    const request = command({
      type: 'prepare_sensitive_tool',
      input: {
        bindingRequestId: randomUUID(),
        bindingScope: 'managed_surface',
        runId: 'run-1',
        capabilityId: 'browser_automation',
        activationId: AUTHORIZATION_CONTEXT.activationId,
        manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
        policyRevision: 1,
        grantExpiresAtMs: now + 120_000,
        callId: 'call-file-preflight',
        toolName: 'browser_file_upload',
        argumentsDigest: `sha256:${'e'.repeat(64)}`,
        createdAtMs: now,
        expiresAtMs: now + 60_000,
        filePreparation: { mode: 'resolved_paths', paths: [source] }
      }
    })
    if (request.command.type !== 'prepare_sensitive_tool') {
      throw new Error('invalid prepare fixture')
    }
    const preparedInput = request.command.input

    try {
      core.emitCommand(request)
      await vi.waitFor(() => expect(core.completions).toHaveLength(1))
      const outcome = core.completions[0].outcome
      expect(outcome).toMatchObject({
        type: 'sensitive_tool_prepared',
        fileBasenames: ['浙江大学2026年招生资料汇编.pptx'],
        fileRevisionDigest: expect.stringMatching(/^sha256:[0-9a-f]{64}$/)
      })
      expect(JSON.stringify(outcome)).not.toContain(source)
      expect(JSON.stringify(outcome)).not.toContain('browser-file:')
      expect(fileBroker.snapshot().handles).toBe(1)
      expect(selectFiles).not.toHaveBeenCalled()
      if (outcome.type !== 'sensitive_tool_prepared') throw new Error('invalid fixture outcome')
      core.emitCommand(
        command({
          type: 'release_sensitive_tool_binding',
          bindingId: outcome.bindingId,
          runId: preparedInput.runId,
          activationId: preparedInput.activationId,
          callId: preparedInput.callId,
          reason: 'rejected'
        })
      )
      await vi.waitFor(() => expect(fileBroker.snapshot().handles).toBe(0))
    } finally {
      await bridge.close()
      await fileBroker.shutdown()
      await rm(root, { recursive: true, force: true })
    }
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

function targetBindingBroker(
  releasePreparedFiles?: (owner: { runId: string; callId: string }) => void
): ManagedPlaywrightSensitiveTargetBindingBroker {
  return new ManagedPlaywrightSensitiveTargetBindingBroker({
    beginDispatchFence: () => ({ finish: () => undefined }),
    getActiveTarget: () => ({
      surfaceId: 'surface-1',
      generation: 1,
      navigationEpoch: 1,
      origin: 'http://127.0.0.1'
    }),
    releasePreparedFiles
  })
}

function hostWith(options: {
  callTool?: ManagedMcpClient['callTool']
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  detachAutomation?: () => Promise<void>
  surfaceGroup?: ManagedPlaywrightSurfaceGroupAdapter
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
    surfaceGroup: options.surfaceGroup,
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

function surfaceView(): Awaited<ReturnType<ManagedPlaywrightSurfaceGroupAdapter['createSurface']>> {
  return {
    surfaceId: 'surface-1',
    index: 0,
    title: 'Fixture',
    url: 'about:blank',
    isActive: true,
    generation: 1
  }
}

function singleSurfaceGroupAdapter(): ManagedPlaywrightSurfaceGroupAdapter {
  const surface = surfaceView()
  return {
    getSensitiveTargetIdentity: () => ({
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      navigationEpoch: 1,
      origin: 'http://127.0.0.1'
    }),
    ensureActiveSurface: vi.fn(async () => surface),
    listSurfaces: () => [surface],
    createSurface: vi.fn(async () => surface),
    selectSurface: vi.fn(async () => surface),
    closeSurfaceByIndex: vi.fn(async () => undefined)
  }
}
