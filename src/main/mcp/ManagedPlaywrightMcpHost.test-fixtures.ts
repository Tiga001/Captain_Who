import { vi } from 'vitest'
import { randomUUID } from 'node:crypto'
import type { BrowserContext } from 'playwright'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import { BrowserFileBroker } from '../browser/BrowserFileBroker'
import type {
  BrowserRiskAuthorizationContext,
  BrowserRiskFailure
} from '../browser/BrowserRiskCoordinator'
import {
  ManagedPlaywrightMcpHost,
  ManagedPlaywrightMcpHostError,
  type ManagedPlaywrightConnectionFactory,
  type ManagedPlaywrightMcpHostOptions,
  type ManagedPlaywrightSurfaceView,
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedPlaywrightToolSurfaceLease,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import { MANAGED_PLAYWRIGHT_CATALOG_LOCK } from './managedPlaywrightCatalog'
import {
  canonicalSha256,
  sensitiveBindingScopeForInvocation,
  sensitivePolicyForTool
} from './managedPlaywrightSensitivePolicy'
import {
  ManagedPlaywrightSensitiveTargetBindingError,
  type ManagedPlaywrightSensitiveTargetBindingStore
} from './ManagedPlaywrightSensitiveTargetBindingBroker'

/** Each test file owns its hosts, authorization context, and mock state. */
export function createManagedPlaywrightHostTestFixture() {
  const trackedHosts = new Set<ManagedPlaywrightMcpHost>()
  const RISK_CONTEXT: BrowserRiskAuthorizationContext = {
    runId: 'run-1',
    capabilityId: 'browser_automation',
    activationId: '123e4567-e89b-42d3-a456-426614174000',
    manifestDigest: `sha256:${'a'.repeat(64)}`,
    policyRevision: 1,
    grantExpiresAtMs: 2_000_000,
    invocationId: '123e4567-e89b-42d3-a456-426614174001',
    callId: 'call-1',
    triggerToolName: 'browser_navigate',
    callReason: 'Open the local fixture.'
  }
  const PARENT_REQUEST_ID = '123e4567-e89b-42d3-a456-426614174002'

  async function closeTrackedHosts(): Promise<void> {
    const hosts = [...trackedHosts]
    trackedHosts.clear()
    const results = await Promise.allSettled(hosts.map(async (host) => host.close()))
    const failures = results.flatMap((result) =>
      result.status === 'rejected' ? [result.reason] : []
    )
    if (failures.length > 0) {
      throw new AggregateError(failures, 'Failed to close ManagedPlaywrightMcpHost test fixtures')
    }
  }

  function surfaceView(index: number, isActive: boolean): ManagedPlaywrightSurfaceView {
    return {
      surfaceId: `surface-${index}`,
      index,
      title: `Tab ${index}`,
      url: `http://127.0.0.1/tab-${index}`,
      isActive,
      generation: index + 1
    }
  }

  function singleSurfaceGroup(
    surfaces: ReturnType<typeof surfaceView>[] = [surfaceView(0, true)]
  ): ManagedPlaywrightSurfaceGroupAdapter {
    return {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => {
        const active = surfaces.find((surface) => surface.isActive)
        if (!active) throw new Error('no active surface')
        return active
      }),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        const created = surfaceView(surfaces.length, true)
        for (const surface of surfaces) surface.isActive = false
        surfaces.push(created)
        return created
      }),
      selectSurface: vi.fn(async ({ index }) => {
        for (const surface of surfaces) surface.isActive = surface.index === index
        return surfaces[index]
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined)
    }
  }

  function testSurfaceLeaseApi(
    getSurfaces: () => ReturnType<typeof surfaceView>[]
  ): Pick<
    ManagedPlaywrightSurfaceGroupAdapter,
    | 'beginExistingToolSurfaceLease'
    | 'beginTargetCreationIntent'
    | 'beginToolSurfaceLease'
    | 'beginToolSurfaceLeaseByIndex'
  > {
    const leaseAt = (requestedIndex: number): ManagedPlaywrightToolSurfaceLease => {
      const surface = getSurfaces()[requestedIndex]
      if (!surface) throw new ManagedPlaywrightMcpHostError('browser.target_closed')
      return {
        closeSurface: vi.fn(async () => undefined),
        finish: vi.fn(),
        generation: surface.generation,
        index: requestedIndex,
        resolveIndex: vi.fn(async () => {
          const currentIndex = getSurfaces().findIndex(
            (candidate) =>
              candidate.surfaceId === surface.surfaceId &&
              candidate.generation === surface.generation
          )
          if (currentIndex < 0) throw new ManagedPlaywrightMcpHostError('browser.target_closed')
          return currentIndex
        }),
        resizeSurface: vi.fn(async (input) => input),
        selectionRevision: 1,
        surfaceId: surface.surfaceId
      }
    }
    const activeIndex = (): number => getSurfaces().findIndex((surface) => surface.isActive)
    return {
      beginExistingToolSurfaceLease: vi.fn(async () => {
        const index = activeIndex()
        return index < 0 ? null : leaseAt(index)
      }),
      beginTargetCreationIntent: vi.fn(() => vi.fn()),
      beginToolSurfaceLease: vi.fn(async () => {
        let index = activeIndex()
        if (index < 0 && getSurfaces().length === 0) {
          getSurfaces().push(surfaceView(0, true))
          index = 0
        }
        return leaseAt(index)
      }),
      beginToolSurfaceLeaseByIndex: vi.fn(async (index) => leaseAt(index))
    }
  }

  function sensitiveContext(
    toolName: string,
    modelArguments: Record<string, unknown>,
    overrides: Partial<BrowserRiskAuthorizationContext> = {}
  ): BrowserRiskAuthorizationContext {
    const policy = sensitivePolicyForTool(toolName)
    if (!policy) throw new Error(`Missing sensitive policy for ${toolName}`)
    const bindingScope = sensitiveBindingScopeForInvocation(toolName, modelArguments)
    const origin = bindingScope === 'managed_browser_profile' ? null : 'http://127.0.0.1'
    const argumentsDigest = canonicalSha256(modelArguments)
    const expiresAtMs = Date.now() + 60_000
    const targetBindingId = randomUUID()
    const targetBindingDigest = canonicalSha256({ targetBindingId })
    return {
      ...RISK_CONTEXT,
      callId: `call-${toolName}`,
      triggerToolName: toolName,
      grantExpiresAtMs: expiresAtMs + 60_000,
      ...overrides,
      builtinToolGrant: {
        grantId: randomUUID(),
        approvalId: randomUUID(),
        argumentsDigest,
        targetBindingId,
        targetBindingDigest,
        resourceScopeDigest: canonicalSha256({
          schemaVersion: 2,
          toolId: toolName,
          argumentsDigest,
          origin,
          riskKinds: [...policy.riskKinds],
          scope: bindingScope,
          targetBindingDigest
        }),
        origin,
        riskKinds: [...policy.riskKinds],
        expiresAtMs
      }
    }
  }

  function fakeBrowserContext(
    overrides: {
      route?: (...args: unknown[]) => Promise<void>
      setOffline?: (offline: boolean) => Promise<void>
      unroute?: (...args: unknown[]) => Promise<void>
    } = {}
  ): BrowserContext {
    return {
      route: overrides.route ?? vi.fn(async () => undefined),
      unroute: overrides.unroute ?? vi.fn(async () => undefined),
      setOffline: overrides.setOffline ?? vi.fn(async () => undefined),
      once: vi.fn(),
      off: vi.fn(),
      tracing: {
        start: vi.fn(async () => undefined),
        stop: vi.fn(async () => undefined)
      }
    } as unknown as BrowserContext
  }

  function closableBrowserContext(): { context: BrowserContext; close(): void } {
    let connected = true
    let handleClose: (() => void) | undefined
    const context = {
      route: vi.fn(async () => undefined),
      unroute: vi.fn(async () => undefined),
      setOffline: vi.fn(async () => undefined),
      once: vi.fn((event: string, handler: () => void) => {
        if (event === 'close') handleClose = handler
        return context
      }),
      off: vi.fn((event: string, handler: () => void) => {
        if (event === 'close' && handleClose === handler) handleClose = undefined
        return context
      }),
      browser: vi.fn(() => ({ isConnected: () => connected })),
      tracing: {
        start: vi.fn(async () => undefined),
        stop: vi.fn(async () => undefined)
      }
    } as unknown as BrowserContext
    return {
      context,
      close: () => {
        connected = false
        handleClose?.()
      }
    }
  }

  function fakeHost(overrides: {
    artifactBroker?: BrowserArtifactBroker
    fileBroker?: BrowserFileBroker
    beginNetworkOperation?: ManagedPlaywrightMcpHostOptions['beginNetworkOperation']
    beginTargetCreationOperation?: ManagedPlaywrightMcpHostOptions['beginTargetCreationOperation']
    callTool?: ManagedMcpClient['callTool']
    closeSurface?: () => Promise<void>
    createClient?: () => ManagedMcpClient
    createOfficialConnection?: ManagedPlaywrightConnectionFactory
    detachAutomation?: () => Promise<void>
    getAgentDownloadSnapshot?: ManagedPlaywrightMcpHostOptions['getAgentDownloadSnapshot']
    getBrowserContext?: () => Promise<BrowserContext>
    listTools?: ManagedMcpClient['listTools']
    preparedFileHandles?: readonly string[]
    surfaceGroup?: ManagedPlaywrightSurfaceGroupAdapter
    toolTimeoutMs?: number
    queueTimeoutMs?: number
  }): ManagedPlaywrightMcpHost {
    const upstreamTools = MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      inputSchema: structuredClone(tool.inputSchema),
      annotations: structuredClone(tool.annotations ?? {})
    }))
    const usesDefaultSurfaceGroup = overrides.surfaceGroup === undefined
    const surfaceGroup = overrides.surfaceGroup ?? singleSurfaceGroup()
    const getBrowserContext = overrides.getBrowserContext ?? (async () => fakeBrowserContext())
    const createOfficialConnection =
      overrides.createOfficialConnection ??
      (usesDefaultSurfaceGroup
        ? async (
            _config: Parameters<ManagedPlaywrightConnectionFactory>[0],
            contextGetter: Parameters<ManagedPlaywrightConnectionFactory>[1]
          ) => {
            await contextGetter()
            return {
              connect: vi.fn(async () => undefined),
              close: vi.fn(async () => undefined)
            }
          }
        : async () => ({
            connect: vi.fn(async () => undefined),
            close: vi.fn(async () => undefined)
          }))
    const callTool =
      overrides.callTool ??
      vi.fn(async () => ({ content: [{ type: 'text', text: 'ok' }], isError: false }))
    const host = new ManagedPlaywrightMcpHost({
      artifactBroker: overrides.artifactBroker,
      fileBroker: overrides.fileBroker,
      beginNetworkOperation: overrides.beginNetworkOperation,
      beginTargetCreationOperation: overrides.beginTargetCreationOperation,
      getAgentDownloadSnapshot: overrides.getAgentDownloadSnapshot,
      getBrowserContext,
      surfaceGroup,
      sensitiveTargetBindings: fakeSensitiveTargetBindings(
        surfaceGroup,
        overrides.preparedFileHandles
      ),
      toolTimeoutMs: overrides.toolTimeoutMs,
      queueTimeoutMs: overrides.queueTimeoutMs,
      closeSurface: overrides.closeSurface ?? vi.fn(async () => undefined),
      detachAutomation: overrides.detachAutomation ?? vi.fn(async () => undefined),
      createOfficialConnection,
      createClient:
        overrides.createClient ??
        (() => ({
          connect: vi.fn(async () => undefined),
          close: vi.fn(async () => undefined),
          listTools: overrides.listTools ?? vi.fn(async () => ({ tools: upstreamTools })),
          callTool: async (request, resultSchema, options) => {
            if (
              usesDefaultSurfaceGroup &&
              request.name === 'browser_tabs' &&
              (request.arguments.action === 'list' || request.arguments.action === 'select')
            ) {
              return {
                content: [{ type: 'text', text: 'fixture surface synchronized' }],
                isError: false
              }
            }
            return await callTool(request, resultSchema, options)
          }
        }))
    })
    trackedHosts.add(host)
    return host
  }

  function sensitiveTargetFromSurfaces(surfaces: ReturnType<typeof surfaceView>[]) {
    const surface = surfaces.find((candidate) => candidate.isActive)
    if (!surface) return null
    try {
      return {
        surfaceId: surface.surfaceId,
        generation: surface.generation,
        navigationEpoch: 1,
        origin: new URL(surface.url).origin
      }
    } catch {
      return null
    }
  }

  function fakeSensitiveTargetBindings(
    surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter | undefined,
    preparedFileHandles?: readonly string[]
  ): ManagedPlaywrightSensitiveTargetBindingStore {
    const consumed = new Set<string>()
    return {
      acquire: (authorization) => {
        const grant = authorization.builtinToolGrant
        const target = surfaceGroup?.getSensitiveTargetIdentity()
        const profileScoped = grant?.origin === null
        if (!grant || (!profileScoped && !target)) {
          throw new ManagedPlaywrightSensitiveTargetBindingError('origin_drifted')
        }
        if (consumed.has(grant.targetBindingId)) {
          throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
        }
        let dispatched = false
        return {
          ...(target && !profileScoped ? { target: Object.freeze({ ...target }) } : {}),
          ...(preparedFileHandles ? { preparedFileHandles } : {}),
          markDispatched: () => {
            if (dispatched || consumed.has(grant.targetBindingId)) {
              throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
            }
            dispatched = true
            consumed.add(grant.targetBindingId)
          },
          finish: () => undefined
        }
      },
      releaseRun: () => 0,
      releaseToolCall: () => 0
    }
  }

  function riskLease(options: {
    check?: (input: unknown) => Promise<void>
    downloadProgress?: readonly import('@mycopilot/protocol').BrowserAgentDownloadStatus[]
    failure?: BrowserRiskFailure | (() => BrowserRiskFailure | undefined)
    settle?: () => Promise<void>
  }): {
    finish: ReturnType<typeof vi.fn>
    lease: BrowserNetworkOperationLease
    markDispatched: ReturnType<typeof vi.fn>
    settle: ReturnType<typeof vi.fn>
  } {
    const finish = vi.fn()
    const markDispatched = vi.fn()
    const settle = vi.fn(options.settle ?? (async () => undefined))
    const lease = {
      preflight: options.check ?? vi.fn(async () => undefined),
      operation: {
        check: options.check ?? vi.fn(async () => undefined)
      },
      failure: () =>
        (typeof options.failure === 'function' ? options.failure() : options.failure) ?? null,
      markDispatched,
      settle,
      downloads: () => [],
      downloadProgress: () => options.downloadProgress ?? [],
      finish
    } as unknown as BrowserNetworkOperationLease
    return { finish, lease, markDispatched, settle }
  }

  return {
    closeTrackedHosts,
    trackedHosts,
    RISK_CONTEXT,
    PARENT_REQUEST_ID,
    surfaceView,
    singleSurfaceGroup,
    testSurfaceLeaseApi,
    sensitiveContext,
    sensitiveTargetFromSurfaces,
    fakeBrowserContext,
    closableBrowserContext,
    fakeHost,
    riskLease
  }
}
