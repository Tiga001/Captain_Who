import { randomUUID } from 'node:crypto'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it, vi } from 'vitest'

import {
  MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
  type AgentEvent,
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
  private agentEvent?: (input: AgentEvent) => void
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

  onAgentEvent(handler: (input: AgentEvent) => void): () => void {
    this.agentEvent = handler
    return () => {
      if (this.agentEvent === handler) this.agentEvent = undefined
    }
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

  emitAgentEvent(input: AgentEvent): void {
    this.agentEvent?.(input)
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

  it('reports a queued timeout without dispatch phases or cancelling the active browser call', async () => {
    const core = new FakeCore()
    let release!: (value: unknown) => void
    let markStarted!: () => void
    const started = new Promise<void>((resolve) => (markStarted = resolve))
    const callTool = vi.fn<ManagedMcpClient['callTool']>(async (request) => {
      if (request.name === 'browser_tabs') return { content: [], isError: false }
      markStarted()
      return await new Promise((resolve) => (release = resolve))
    })
    const detachAutomation = vi.fn(async () => undefined)
    const managed = hostWith({ callTool, detachAutomation, queueTimeoutMs: 40 })
    await managed.connect()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: targetBindingBroker(),
      createHost: () => managed
    })
    vi.useFakeTimers()
    try {
      const first = command({
        type: 'call_tool',
        name: 'browser_snapshot',
        arguments: { call_reason: 'Read the first page.' },
        timeoutMs: 1_000,
        authorizationContext: AUTHORIZATION_CONTEXT
      })
      core.emitCommand(first)
      await started
      const queued = command({
        type: 'call_tool',
        name: 'browser_snapshot',
        arguments: { call_reason: 'Read the next page.' },
        timeoutMs: 1_000,
        authorizationContext: { ...AUTHORIZATION_CONTEXT, callId: 'call-queued' }
      })
      core.emitCommand(queued)
      await vi.advanceTimersByTimeAsync(41)
      expect(core.completions).toEqual([
        {
          schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
          requestId: queued.requestId,
          outcome: {
            type: 'error',
            code: 'queue_timeout',
            dispatchCertainty: 'definitely_not_dispatched'
          }
        }
      ])
      expect(core.dispatchPhases.filter((phase) => phase.requestId === queued.requestId)).toEqual(
        []
      )
      expect(detachAutomation).not.toHaveBeenCalled()
      expect(
        callTool.mock.calls.filter(([request]) => request.name === 'browser_snapshot')
      ).toHaveLength(1)
      release({ content: [], isError: false })
      await vi.advanceTimersByTimeAsync(1)
      expect(
        core.completions.find((completion) => completion.requestId === first.requestId)?.outcome
          .type
      ).toBe('tool_called')
    } finally {
      vi.useRealTimers()
      await bridge.close()
    }
  })

  it('cancels a finished run while queued without cancelling another run or dispatching late', async () => {
    const core = new FakeCore()
    let release!: (value: unknown) => void
    let markStarted!: () => void
    const started = new Promise<void>((resolve) => {
      markStarted = resolve
    })
    const callTool = vi.fn<ManagedMcpClient['callTool']>(async (request) => {
      if (request.name === 'browser_tabs') return { content: [], isError: false }
      markStarted()
      return await new Promise((resolve) => {
        release = resolve
      })
    })
    const releaseRunTarget = vi.fn()
    const group = singleSurfaceGroupAdapter()
    group.prepareRunTarget = vi.fn()
    const managed = hostWith({ callTool, surfaceGroup: group })
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      sensitiveTargetBindings: targetBindingBroker(),
      releaseRunTarget,
      createHost: () => managed
    })
    try {
      const first = command({
        type: 'call_tool',
        name: 'browser_snapshot',
        arguments: { call_reason: 'Read the first run page.' },
        timeoutMs: 5_000,
        authorizationContext: AUTHORIZATION_CONTEXT
      })
      core.emitCommand(first)
      await started
      const queued = command({
        type: 'call_tool',
        name: 'browser_snapshot',
        arguments: { call_reason: 'Read the second run page.' },
        timeoutMs: 5_000,
        authorizationContext: {
          ...AUTHORIZATION_CONTEXT,
          runId: 'run-finished',
          callId: 'call-queued'
        }
      })
      core.emitCommand(queued)
      await vi.waitFor(() => expect(group.prepareRunTarget).toHaveBeenCalledTimes(2))
      core.emitAgentEvent({
        type: 'done',
        runId: 'run-finished',
        success: false,
        status: 'cancelled'
      })
      await vi.waitFor(() => expect(core.completions).toHaveLength(1))
      expect(core.completions[0]).toMatchObject({
        requestId: queued.requestId,
        outcome: {
          type: 'error',
          code: 'cancelled',
          dispatchCertainty: 'definitely_not_dispatched'
        }
      })
      expect(releaseRunTarget).toHaveBeenCalledExactlyOnceWith('run-finished')
      expect(core.dispatchPhases.filter((phase) => phase.requestId === queued.requestId)).toEqual(
        []
      )
      release({ content: [], isError: false })
      await vi.waitFor(() => expect(core.completions).toHaveLength(2))
      expect(core.completions[1]).toMatchObject({
        requestId: first.requestId,
        outcome: { type: 'tool_called' }
      })
      expect(
        callTool.mock.calls.filter(([request]) => request.name === 'browser_snapshot')
      ).toHaveLength(1)
    } finally {
      await bridge.close()
    }
  })

  it('releases target markers on terminal events before an MCP Host has been constructed', async () => {
    const core = new FakeCore()
    const createHost = vi.fn(() => hostWith({}))
    const releaseRunTarget = vi.fn()
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      createHost,
      releaseRunTarget,
      sensitiveTargetBindings: targetBindingBroker()
    })
    core.emitAgentEvent({
      type: 'done',
      runId: 'run-preflight',
      success: false,
      status: 'waiting_for_approval'
    })
    expect(releaseRunTarget).not.toHaveBeenCalled()
    core.emitAgentEvent({
      type: 'done',
      runId: 'run-preflight',
      success: false,
      status: 'cancelled'
    })
    expect(releaseRunTarget).toHaveBeenCalledExactlyOnceWith('run-preflight')
    expect(createHost).not.toHaveBeenCalled()
    await bridge.close()
  })

  it('does not apply the bridge envelope deadline to an admitted call_tool', async () => {
    vi.useFakeTimers()
    try {
      const core = new FakeCore()
      let release!: () => void
      let markStarted!: () => void
      const started = new Promise<void>((resolve) => {
        markStarted = resolve
      })
      const managedHost = {
        callTool: vi.fn(async () => {
          markStarted()
          await new Promise<void>((resolve) => {
            release = resolve
          })
          return { content: [{ type: 'text', text: 'ok' }], isError: false }
        }),
        close: vi.fn(async () => undefined),
        releaseRun: vi.fn(async () => undefined)
      } as unknown as ManagedPlaywrightMcpHost
      const bridge = new ManagedPlaywrightBridgeHost({
        core,
        sensitiveTargetBindings: targetBindingBroker(),
        createHost: () => managedHost
      })
      const request = {
        ...command({
          type: 'call_tool',
          name: 'browser_snapshot',
          arguments: { call_reason: 'Wait at the Host-owned Tool boundary.' },
          timeoutMs: 1_000,
          authorizationContext: AUTHORIZATION_CONTEXT
        }),
        deadlineMs: Date.now() + 20
      }

      core.emitCommand(request)
      await started
      await vi.advanceTimersByTimeAsync(60)
      expect(core.completions).toHaveLength(0)

      release()
      await vi.waitFor(() => expect(core.completions).toHaveLength(1))
      expect(core.completions[0]).toMatchObject({
        requestId: request.requestId,
        outcome: { type: 'tool_called' }
      })
      await bridge.close()
    } finally {
      vi.useRealTimers()
    }
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

  it.each([
    ['completed', 'managed_surface', 'browser_evaluate'],
    ['failed', 'managed_browser_profile', 'browser_cookie_list'],
    ['cancelled', 'managed_surface', 'browser_evaluate']
  ] as const)(
    'keeps a prepared sensitive binding while approval waits and releases it on %s',
    async (terminalStatus, bindingScope, toolName) => {
      const root = await mkdtemp(join(tmpdir(), 'mycopilot-bridge-run-release-'))
      const core = new FakeCore()
      const bindings = targetBindingBroker()
      const fileBroker = new BrowserFileBroker({
        rootDirectory: join(root, 'browser-automation-files'),
        selectionProvider: { selectFiles: vi.fn(async () => null) }
      })
      const managedHost = hostWith({})
      const releaseFileRun = vi.spyOn(fileBroker, 'releaseRun')
      const releaseHostRun = vi.spyOn(managedHost, 'releaseRun')
      const bridge = new ManagedPlaywrightBridgeHost({
        core,
        fileBroker,
        sensitiveTargetBindings: bindings,
        createHost: () => managedHost
      })
      const now = Date.now()
      const prepareRequest = command({
        type: 'prepare_sensitive_tool',
        input: {
          bindingRequestId: randomUUID(),
          bindingScope,
          runId: 'run-approval-lifecycle',
          capabilityId: 'browser_automation',
          activationId: AUTHORIZATION_CONTEXT.activationId,
          manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
          policyRevision: 1,
          grantExpiresAtMs: now + 120_000,
          callId: `call-${terminalStatus}`,
          toolName,
          argumentsDigest: `sha256:${'d'.repeat(64)}`,
          createdAtMs: now,
          expiresAtMs: now + 60_000,
          filePreparation: null
        }
      })

      try {
        core.emitCommand(command({ type: 'connect' }))
        await vi.waitFor(() => expect(core.completions).toHaveLength(1))
        core.emitCommand(prepareRequest)
        await vi.waitFor(() => expect(core.completions).toHaveLength(2))
        expect(
          core.completions.find((completion) => completion.requestId === prepareRequest.requestId)
            ?.outcome
        ).toMatchObject({
          type: 'sensitive_tool_prepared',
          origin: bindingScope === 'managed_browser_profile' ? null : 'http://127.0.0.1'
        })
        expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })

        core.emitAgentEvent({
          type: 'done',
          runId: 'run-approval-lifecycle',
          success: false,
          status: 'waiting_for_approval'
        })
        expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })
        expect(releaseFileRun).not.toHaveBeenCalled()
        expect(releaseHostRun).not.toHaveBeenCalled()

        core.emitAgentEvent({
          type: 'done',
          runId: 'run-approval-lifecycle',
          success: terminalStatus === 'completed',
          status: terminalStatus
        })
        expect(bindings.snapshot()).toEqual({ bindings: 0, requests: 0 })
        expect(releaseFileRun).toHaveBeenCalledOnce()
        expect(releaseFileRun).toHaveBeenCalledWith('run-approval-lifecycle')
        expect(releaseHostRun).toHaveBeenCalledOnce()
        expect(releaseHostRun).toHaveBeenCalledWith('run-approval-lifecycle')
      } finally {
        await bridge.close()
        await fileBroker.shutdown()
        await rm(root, { recursive: true, force: true })
      }
    }
  )

  it('keeps a prepared sensitive binding when a Done event has no status', async () => {
    const core = new FakeCore()
    const bindings = targetBindingBroker()
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
          bindingScope: 'managed_surface',
          runId: 'run-status-missing',
          capabilityId: 'browser_automation',
          activationId: AUTHORIZATION_CONTEXT.activationId,
          manifestDigest: AUTHORIZATION_CONTEXT.manifestDigest,
          policyRevision: 1,
          grantExpiresAtMs: now + 120_000,
          callId: 'call-status-missing',
          toolName: 'browser_evaluate',
          argumentsDigest: `sha256:${'e'.repeat(64)}`,
          createdAtMs: now,
          expiresAtMs: now + 60_000,
          filePreparation: null
        }
      })
    )

    await vi.waitFor(() => expect(core.completions).toHaveLength(1))
    expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })
    core.emitAgentEvent({
      type: 'done',
      runId: 'run-status-missing',
      success: true
    })
    expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })

    core.emitAgentEvent({
      type: 'done',
      runId: 'run-status-missing',
      success: false,
      status: 'cancelled'
    })
    expect(bindings.snapshot()).toEqual({ bindings: 0, requests: 0 })
    await bridge.close()
  })

  it('delegates tab creation to the fixed official group transport', async () => {
    const core = new FakeCore()
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroupAdapter(),
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
    const managedHost = hostWith({})
    const releaseFileRun = vi.spyOn(fileBroker, 'releaseRun')
    const releaseHostRun = vi.spyOn(managedHost, 'releaseRun')
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      fileBroker,
      sensitiveTargetBindings: bindings,
      createHost: () => managedHost
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
    try {
      core.emitCommand(command({ type: 'connect' }))
      await vi.waitFor(() => expect(core.completions).toHaveLength(1))
      core.emitCommand(request)
      await vi.waitFor(() => expect(core.completions).toHaveLength(2))
      const outcome = core.completions.find(
        (completion) => completion.requestId === request.requestId
      )?.outcome
      expect(outcome).toMatchObject({
        type: 'sensitive_tool_prepared',
        fileBasenames: ['浙江大学2026年招生资料汇编.pptx'],
        fileRevisionDigest: expect.stringMatching(/^sha256:[0-9a-f]{64}$/)
      })
      expect(JSON.stringify(outcome)).not.toContain(source)
      expect(JSON.stringify(outcome)).not.toContain('browser-file:')
      expect(fileBroker.snapshot().handles).toBe(1)
      expect(selectFiles).not.toHaveBeenCalled()
      core.emitAgentEvent({
        type: 'done',
        runId: 'run-1',
        success: false,
        status: 'waiting_for_approval'
      })
      expect(bindings.snapshot()).toEqual({ bindings: 1, requests: 1 })
      expect(fileBroker.snapshot().handles).toBe(1)
      expect(releaseFileRun).not.toHaveBeenCalled()
      expect(releaseHostRun).not.toHaveBeenCalled()

      core.emitAgentEvent({
        type: 'done',
        runId: 'run-1',
        success: true,
        status: 'completed'
      })
      expect(bindings.snapshot()).toEqual({ bindings: 0, requests: 0 })
      await vi.waitFor(() => expect(fileBroker.snapshot().handles).toBe(0))
      expect(releaseFileRun).toHaveBeenCalledOnce()
      expect(releaseFileRun).toHaveBeenCalledWith('run-1')
      expect(releaseHostRun).toHaveBeenCalledOnce()
      expect(releaseHostRun).toHaveBeenCalledWith('run-1')
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
  queueTimeoutMs?: number
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
    queueTimeoutMs: options.queueTimeoutMs,
    surfaceGroup: options.surfaceGroup ?? singleSurfaceGroupAdapter(),
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
  const lease = () => ({
    closeSurface: vi.fn(async () => undefined),
    finish: vi.fn(),
    generation: surface.generation,
    index: surface.index,
    resolveIndex: vi.fn(async () => surface.index),
    resizeSurface: vi.fn(async (input: { height: number; width: number }) => input),
    selectionRevision: 1,
    surfaceId: surface.surfaceId
  })
  return {
    beginExistingToolSurfaceLease: vi.fn(async () => lease()),
    beginTargetCreationIntent: vi.fn(() => vi.fn()),
    beginToolSurfaceLease: vi.fn(async () => lease()),
    beginToolSurfaceLeaseByIndex: vi.fn(async () => lease()),
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
