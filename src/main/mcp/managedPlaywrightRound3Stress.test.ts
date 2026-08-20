import { randomUUID } from 'node:crypto'
import { EventEmitter, getEventListeners } from 'node:events'
import { access, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import type { Debugger, Session, WebContents } from 'electron'
import type { BrowserContext } from 'playwright'
import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
  type ManagedPlaywrightCancelNotification,
  type ManagedPlaywrightCommandNotification,
  type ManagedPlaywrightCompletionInput,
  type ManagedPlaywrightDispatchPhaseInput
} from '@mycopilot/protocol'

import {
  BrowserArtifactBroker,
  type BrowserArtifactBrokerClock
} from '../browser/BrowserArtifactBroker'
import { BrowserFileBroker, type BrowserFileOwner } from '../browser/BrowserFileBroker'
import { BrowserTargetBroker } from '../browser/BrowserTargetBroker'
import type { BrowserRiskAuthorizationContext } from '../browser/BrowserRiskCoordinator'
import { ManagedPlaywrightBridgeHost } from './ManagedPlaywrightBridgeHost'
import {
  ManagedPlaywrightSensitiveTargetBindingBroker,
  ManagedPlaywrightSensitiveTargetBindingError,
  type ManagedPlaywrightSensitiveTargetBindingStore
} from './ManagedPlaywrightSensitiveTargetBindingBroker'
import {
  ManagedPlaywrightMcpHost,
  ManagedPlaywrightMcpHostError,
  type ManagedMcpClient,
  type ManagedPlaywrightConnectionFactory,
  type ManagedPlaywrightMcpHostOptions,
  type ManagedPlaywrightProtocolSnapshot,
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedPlaywrightSurfaceView
} from './ManagedPlaywrightMcpHost'
import { MANAGED_PLAYWRIGHT_CATALOG_LOCK } from './managedPlaywrightCatalog'
import { MANAGED_PLAYWRIGHT_SERVER_ID } from './managedPlaywrightManifest'
import {
  canonicalSha256,
  sensitiveBindingScopeForInvocation,
  sensitivePolicyForTool
} from './managedPlaywrightSensitivePolicy'

const PARTITION = 'persist:mycopilot-browser'
const EXPECTED_SESSION = {} as Session
const FIXED_NOW = 1_000_000
const TEMPORARY_ROOTS = new Set<string>()

afterEach(async () => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  await Promise.allSettled(
    [...TEMPORARY_ROOTS].map((root) => rm(root, { recursive: true, force: true }))
  )
  TEMPORARY_ROOTS.clear()
})

describe('managed Playwright Round 3 deterministic stress gate', () => {
  it('settles 500 continuous Host calls and fails closed at the 32-reader admission boundary', async () => {
    const sequential = createHostHarness()
    try {
      for (let index = 0; index < 500; index += 1) {
        const result = await sequential.host.callTool('browser_console_messages', {
          level: 'warning',
          call_reason: `Round 3 continuous read ${index}.`
        })
        expect(result.isError).toBe(false)
      }
      expect(sequential.callTool).toHaveBeenCalledTimes(500)
      expect(sequential.listTools).toHaveBeenCalledOnce()
      expect(sequential.outputDirectories).toHaveLength(1)
      await expect(access(sequential.outputDirectories[0])).resolves.toBeUndefined()
    } finally {
      await sequential.host.close()
    }
    await expect(access(sequential.outputDirectories[0])).rejects.toMatchObject({ code: 'ENOENT' })
    expect(sequential.detachAutomation).toHaveBeenCalledOnce()

    let releaseCatalog!: (value: ReturnType<typeof officialCatalogPage>) => void
    let markCatalogStarted!: () => void
    const catalogStarted = new Promise<void>((resolve) => {
      markCatalogStarted = resolve
    })
    const concurrent = createHostHarness({
      listTools: vi.fn(async () => {
        markCatalogStarted()
        return await new Promise<ReturnType<typeof officialCatalogPage>>((resolve) => {
          releaseCatalog = resolve
        })
      })
    })
    const caller = new AbortController()
    const attempts = Array.from({ length: 32 }, (_, index) =>
      concurrent.host
        .callTool(
          'browser_console_messages',
          { level: 'warning', call_reason: `Round 3 concurrent read ${index}.` },
          { signal: caller.signal }
        )
        .then(
          (value) => ({ status: 'fulfilled' as const, value }),
          (reason: unknown) => ({ status: 'rejected' as const, reason })
        )
    )
    await catalogStarted
    expect(getEventListeners(caller.signal, 'abort')).toHaveLength(8)
    releaseCatalog(officialCatalogPage())
    const settled = await Promise.all(attempts)
    expect(settled.filter((result) => result.status === 'fulfilled')).toHaveLength(8)
    expect(
      settled.filter(
        (result) =>
          result.status === 'rejected' &&
          result.reason instanceof ManagedPlaywrightMcpHostError &&
          result.reason.code === 'mcp.builtin_playwright.busy'
      )
    ).toHaveLength(24)
    expect(getEventListeners(caller.signal, 'abort')).toHaveLength(0)

    let completedReaders = 8
    let readersWaitingForBackpressure = 24
    let admissionWaves = 1
    while (readersWaitingForBackpressure > 0) {
      const retryWave = await Promise.all(
        Array.from({ length: readersWaitingForBackpressure }, (_, index) =>
          concurrent.host
            .callTool(
              'browser_console_messages',
              {
                level: 'warning',
                call_reason: `Round 3 bounded retry wave ${admissionWaves} reader ${index}.`
              },
              { signal: caller.signal }
            )
            .then(
              (value) => ({ status: 'fulfilled' as const, value }),
              (reason: unknown) => ({ status: 'rejected' as const, reason })
            )
        )
      )
      const fulfilled = retryWave.filter((result) => result.status === 'fulfilled').length
      const rejected = retryWave.filter((result) => result.status === 'rejected')
      expect(
        rejected.every(
          (result) =>
            result.reason instanceof ManagedPlaywrightMcpHostError &&
            result.reason.code === 'mcp.builtin_playwright.busy'
        )
      ).toBe(true)
      completedReaders += fulfilled
      readersWaitingForBackpressure = rejected.length
      admissionWaves += 1
    }
    expect(completedReaders).toBe(32)
    expect(admissionWaves).toBe(4)
    expect(concurrent.callTool).toHaveBeenCalledTimes(32)
    expect(getEventListeners(caller.signal, 'abort')).toHaveLength(0)
    await expect(
      concurrent.host.callTool('browser_console_messages', {
        level: 'warning',
        call_reason: 'Verify the bounded reader authority returned to baseline.'
      })
    ).resolves.toMatchObject({ isError: false })
    expect(concurrent.callTool).toHaveBeenCalledTimes(33)
    await concurrent.host.close()
    expect(getEventListeners(caller.signal, 'abort')).toHaveLength(0)
    expect(concurrent.detachAutomation).toHaveBeenCalledOnce()
    await Promise.all(
      concurrent.outputDirectories.map(async (directory) => {
        await expect(access(directory)).rejects.toMatchObject({ code: 'ENOENT' })
      })
    )
  }, 30_000)

  it('returns tabs, routes, context listeners, and connection temp directories to baseline for 100 cycles', async () => {
    const context = new BrowserContextHarness()
    const surfaces = new SurfaceGroupHarness()
    const harness = createHostHarness({
      callTool: vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
        if (name === 'browser_tabs' && args.action === 'new') {
          await surfaces.createSurface({
            ...(typeof args.url === 'string' ? { url: args.url } : {})
          })
        } else if (name === 'browser_tabs' && args.action === 'close') {
          await surfaces.closeSurfaceByIndex(
            typeof args.index === 'number' ? args.index : undefined
          )
        }
        return { content: [{ type: 'text', text: 'ok' }], isError: false }
      }),
      getBrowserContext: async () => context.asBrowserContext(),
      surfaceGroup: surfaces
    })
    try {
      for (let index = 0; index < 100; index += 1) {
        await expect(
          harness.host.callTool('browser_tabs', {
            action: 'new',
            url: `http://127.0.0.1/tab-${index}`,
            call_reason: `Open bounded tab ${index}.`
          })
        ).resolves.toMatchObject({ isError: false })
        expect(surfaces.snapshot()).toEqual({ active: 1, surfaces: 2 })
        await expect(
          harness.host.callTool('browser_tabs', {
            action: 'close',
            index: 1,
            call_reason: `Close bounded tab ${index}.`
          })
        ).resolves.toMatchObject({ isError: false })
        expect(surfaces.snapshot()).toEqual({ active: 1, surfaces: 1 })
      }
      expect(surfaces.createSurface).toHaveBeenCalledTimes(100)
      expect(surfaces.closeSurfaceByIndex).toHaveBeenCalledTimes(100)
      // A SurfaceGroup is one stable official BrowserContext. Tab churn must not recreate the
      // MCP connection or discard context-scoped route/offline/trace state between calls.
      expect(harness.outputDirectories).toHaveLength(1)
      await Promise.all(
        harness.outputDirectories.map(async (directory) => {
          await expect(access(directory)).resolves.toBeUndefined()
        })
      )

      for (let index = 0; index < 100; index += 1) {
        const runId = `route-run-${index}`
        const pattern = `http://127.0.0.1/route-${index}*`
        await expect(
          harness.host.callTool(
            'browser_route',
            { pattern, status: 204, call_reason: `Install bounded route ${index}.` },
            { authorizationContext: authorization(runId, `route-${index}`, 'browser_route') }
          )
        ).resolves.toMatchObject({ isError: false })
        expect(context.snapshot().routes).toBe(1)
        await expect(
          harness.host.callTool(
            'browser_unroute',
            { pattern, call_reason: `Remove bounded route ${index}.` },
            { authorizationContext: authorization(runId, `unroute-${index}`, 'browser_unroute') }
          )
        ).resolves.toMatchObject({ isError: false })
        expect(context.snapshot().routes).toBe(0)
      }
      expect(context.route).toHaveBeenCalledTimes(100)
      expect(context.unroute).toHaveBeenCalledTimes(100)
      expect(context.listenerCount('close')).toBe(1)
      await expect(
        harness.host.callTool(
          'browser_route_list',
          { call_reason: 'Verify route authority returned to baseline.' },
          {
            authorizationContext: authorization(
              'route-baseline-probe',
              'route-list',
              'browser_route_list'
            )
          }
        )
      ).resolves.toEqual({
        content: [{ type: 'text', text: 'No active routes' }],
        isError: false
      })
    } finally {
      await harness.host.close()
    }
    expect(surfaces.snapshot()).toEqual({ active: 1, surfaces: 1 })
    expect(context.snapshot()).toEqual({ closeListeners: 0, offline: false, routes: 0 })
    expect(harness.detachAutomation).toHaveBeenCalledOnce()
    await Promise.all(
      harness.outputDirectories.map(async (directory) => {
        await expect(access(directory)).rejects.toMatchObject({ code: 'ENOENT' })
      })
    )
  }, 30_000)

  it('consumes 100 local-file authorities and removes every private copy with an injected clock', async () => {
    const parent = await temporaryDirectory('mycopilot-round3-file-')
    const source = join(parent, 'fixture.txt')
    const privateRoot = join(parent, 'browser-automation-files')
    await writeFile(source, 'repository-local fixture')
    const clock = { now: () => FIXED_NOW }
    const broker = new BrowserFileBroker({
      rootDirectory: privateRoot,
      selectionProvider: { selectFiles: async () => [source] },
      clock,
      ttlMs: 1_000,
      maxHandles: 1
    })
    try {
      for (let index = 0; index < 100; index += 1) {
        const selectionOwner = fileOwner(`select-${index}`)
        const [reference] = await broker.selectForRead({
          owner: selectionOwner,
          multiple: false
        })
        const lease = await broker.consumeForRead({
          owner: fileOwner(`consume-${index}`),
          handles: [reference.handle]
        })
        await lease.finish()
        expect(broker.snapshot()).toEqual({
          handles: 0,
          bytes: 0,
          retained: { leases: 0, files: 0, bytes: 0 }
        })
      }
      expect(await readdir(privateRoot)).toEqual([])
    } finally {
      await broker.shutdown()
    }
    expect(broker.snapshot()).toEqual({
      handles: 0,
      bytes: 0,
      retained: { leases: 0, files: 0, bytes: 0 }
    })
    await expect(access(privateRoot)).rejects.toMatchObject({ code: 'ENOENT' })
  }, 30_000)

  it('exports and revokes 100 protected storage states without retaining artifacts, sessions, timers, or secrets', async () => {
    vi.spyOn(Date, 'now').mockReturnValue(FIXED_NOW)
    const parent = await temporaryDirectory('mycopilot-round3-storage-')
    const timers = new Set<ReturnType<typeof setTimeout>>()
    const clock: BrowserArtifactBrokerClock = {
      now: () => FIXED_NOW,
      setTimeout: () => {
        const timer = {} as ReturnType<typeof setTimeout>
        timers.add(timer)
        return timer
      },
      clearTimeout: (timer) => {
        timers.delete(timer)
      }
    }
    const artifactRoot = join(parent, 'browser-automation-artifacts')
    const broker = new BrowserArtifactBroker({ rootDirectory: artifactRoot, clock })
    const context = new BrowserContextHarness()
    let invocation = 0
    const upstream = vi.fn<ManagedMcpClient['callTool']>(
      async ({ arguments: upstreamArguments }) => {
        const meta = upstreamArguments._meta as { cwd: string }
        const secret = `PRIVATE_STORAGE_CANARY_${invocation++}`
        await writeFile(
          join(meta.cwd, String(upstreamArguments.filename)),
          JSON.stringify({
            cookies: [{ name: 'session', value: secret }],
            origins: [
              {
                origin: 'http://127.0.0.1',
                localStorage: [{ name: 'token', value: secret }]
              }
            ]
          })
        )
        return { content: [{ type: 'text', text: secret }], isError: false }
      }
    )
    const harness = createHostHarness({
      artifactBroker: broker,
      callTool: upstream,
      getActiveSurfaceIdentity: () => ({ surfaceId: 'surface-storage', generation: 1 }),
      getBrowserContext: async () => context.asBrowserContext(),
      surfaceGroup: new SurfaceGroupHarness()
    })
    try {
      for (let index = 0; index < 100; index += 1) {
        const runId = `storage-run-${index}`
        const argumentsValue = {
          call_reason: `Export bounded storage state ${index}.`
        }
        const result = await harness.host.callTool('browser_storage_state', argumentsValue, {
          authorizationContext: sensitiveAuthorization(
            'browser_storage_state',
            argumentsValue,
            runId,
            `storage-call-${index}`,
            index
          )
        })
        expect(result).toMatchObject({
          structuredContent: { artifacts: [{ kind: 'json', preview: 'none' }] },
          isError: false
        })
        expect(JSON.stringify(result)).not.toContain('PRIVATE_STORAGE_CANARY_')
        expect(broker.snapshot()).toEqual({
          artifacts: 1,
          bytes: expect.any(Number),
          reservations: 0,
          sessions: 1,
          runs: 1
        })
        await broker.releaseRun(runId)
        expect(broker.snapshot()).toEqual({
          artifacts: 0,
          bytes: 0,
          reservations: 0,
          sessions: 1,
          runs: 0
        })
      }
      expect(upstream).toHaveBeenCalledTimes(100)
    } finally {
      await harness.host.close()
    }
    expect(context.listenerCount('close')).toBe(0)
    expect(broker.snapshot()).toEqual({
      artifacts: 0,
      bytes: 0,
      reservations: 0,
      sessions: 0,
      runs: 0
    })
    await broker.shutdown()
    expect(timers.size).toBe(0)
    await expect(access(artifactRoot)).rejects.toMatchObject({ code: 'ENOENT' })
  }, 30_000)

  it('recovers 25 target crashes and 25 failed relay generations with all listeners and timers at baseline', async () => {
    const broker = new BrowserTargetBroker(PARTITION, EXPECTED_SESSION)
    for (let index = 0; index < 25; index += 1) {
      const host = new FakeWebContents(index * 2 + 1, 'window')
      const guest = new FakeWebContents(
        index * 2 + 2,
        'webview',
        host.asWebContents(),
        EXPECTED_SESSION
      )
      const surfaceId = `round3-crash-${index}`
      broker.registerManagedGuest({
        guest: guest.asWebContents(),
        host: host.asWebContents(),
        partition: PARTITION
      })
      broker.claimSurface({
        generation: 1,
        guestWebContentsId: guest.id,
        host: host.asWebContents(),
        surfaceId
      })
      const transport = await broker.connect(surfaceId)
      const closed = vi.fn()
      transport.onclose = closed
      expect(broker.snapshot()).toEqual({
        activeConnections: 1,
        claimedSurfaces: 1,
        registeredGuests: 1
      })
      guest.destroy()
      expect(closed).toHaveBeenCalledWith('target_closed')
      expect(broker.snapshot()).toEqual({
        activeConnections: 0,
        claimedSurfaces: 0,
        registeredGuests: 0
      })
      expect(guest.listenerCount('destroyed')).toBe(0)
      expect(host.listenerCount('destroyed')).toBe(0)
      expect(guest.fixtureDebugger.listenerCount('message')).toBe(0)
      expect(guest.fixtureDebugger.listenerCount('detach')).toBe(0)
    }
    broker.dispose()
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })

    vi.useFakeTimers()
    const core = new RelayCore()
    const generations: RelayGenerationHost[] = []
    let shouldFail = true
    const bridge = new ManagedPlaywrightBridgeHost({
      core,
      now: () => FIXED_NOW,
      sensitiveTargetBindings: new ManagedPlaywrightSensitiveTargetBindingBroker({
        beginDispatchFence: () => ({ finish: () => undefined }),
        getActiveTarget: () => ({
          surfaceId: 'surface-1',
          generation: 1,
          navigationEpoch: 1,
          origin: 'http://127.0.0.1'
        }),
        now: () => FIXED_NOW
      }),
      createHost: () => {
        const generation = new RelayGenerationHost(shouldFail)
        shouldFail = !shouldFail
        generations.push(generation)
        return generation as unknown as ManagedPlaywrightMcpHost
      }
    })
    expect(core.snapshot()).toEqual({ cancelListeners: 1, commandListeners: 1 })
    for (let index = 0; index < 25; index += 1) {
      const failed = await core.dispatch(relayCommand({ type: 'connect' }))
      expect(failed.outcome).toEqual({
        type: 'error',
        code: 'target_closed',
        dispatchCertainty: 'definitely_not_dispatched'
      })
      const recovered = await core.dispatch(relayCommand({ type: 'connect' }))
      expect(recovered.outcome).toMatchObject({ type: 'connected' })
      const closed = await core.dispatch(relayCommand({ type: 'close' }))
      expect(closed.outcome).toEqual({ type: 'closed' })
      expect(vi.getTimerCount()).toBe(0)
    }
    await bridge.close()
    expect(core.snapshot()).toEqual({ cancelListeners: 0, commandListeners: 0 })
    expect(vi.getTimerCount()).toBe(0)
    expect(generations).toHaveLength(50)
    for (const generation of generations) {
      expect(generation.closeCalls).toBe(1)
      expect(generation.connectSignals).toHaveLength(1)
      expect(getEventListeners(generation.connectSignals[0], 'abort')).toHaveLength(0)
    }
  }, 30_000)
})

interface HostHarnessOptions {
  artifactBroker?: BrowserArtifactBroker
  callTool?: ManagedMcpClient['callTool']
  getActiveSurfaceIdentity?: ManagedPlaywrightMcpHostOptions['getActiveSurfaceIdentity']
  getBrowserContext?: () => Promise<BrowserContext>
  listTools?: ManagedMcpClient['listTools']
  surfaceGroup?: ManagedPlaywrightSurfaceGroupAdapter
}

function createHostHarness(options: HostHarnessOptions = {}): {
  callTool: ManagedMcpClient['callTool'] & ReturnType<typeof vi.fn>
  detachAutomation: ReturnType<typeof vi.fn>
  host: ManagedPlaywrightMcpHost
  listTools: ManagedMcpClient['listTools'] & ReturnType<typeof vi.fn>
  outputDirectories: string[]
} {
  const outputDirectories: string[] = []
  const detachAutomation = vi.fn(async () => undefined)
  const listTools =
    options.listTools ?? vi.fn<ManagedMcpClient['listTools']>(async () => officialCatalogPage())
  const callTool =
    options.callTool ??
    vi.fn<ManagedMcpClient['callTool']>(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
  const createOfficialConnection: ManagedPlaywrightConnectionFactory = async (config) => {
    if (!config || typeof config.outputDir !== 'string') {
      throw new Error('fixture output directory missing')
    }
    outputDirectories.push(config.outputDir)
    return {
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }
  }
  const host = new ManagedPlaywrightMcpHost({
    artifactBroker: options.artifactBroker,
    getActiveSurfaceIdentity: options.getActiveSurfaceIdentity,
    getBrowserContext:
      options.getBrowserContext ??
      (async () => {
        throw new Error('browser context must stay lazy in this fixture')
      }),
    surfaceGroup: options.surfaceGroup,
    sensitiveTargetBindings: fakeSensitiveTargetBindings(options.surfaceGroup),
    closeSurface: vi.fn(async () => undefined),
    detachAutomation,
    createOfficialConnection,
    createClient: () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined),
      listTools,
      callTool
    })
  })
  return {
    callTool: callTool as ManagedMcpClient['callTool'] & ReturnType<typeof vi.fn>,
    detachAutomation,
    host,
    listTools: listTools as ManagedMcpClient['listTools'] & ReturnType<typeof vi.fn>,
    outputDirectories
  }
}

function officialCatalogPage() {
  return {
    tools: MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      inputSchema: structuredClone(tool.inputSchema),
      annotations: structuredClone(tool.annotations ?? {})
    }))
  }
}

class BrowserContextHarness extends EventEmitter {
  private readonly activeRoutes = new Map<unknown, string>()
  private offline = false

  readonly route = vi.fn(async (pattern: unknown, handler: unknown) => {
    this.activeRoutes.set(handler, String(pattern))
  })
  readonly unroute = vi.fn(async (_pattern: unknown, handler: unknown) => {
    this.activeRoutes.delete(handler)
  })
  readonly setOffline = vi.fn(async (offline: boolean) => {
    this.offline = offline
  })
  readonly tracing = {
    start: vi.fn(async () => undefined),
    stop: vi.fn(async () => undefined)
  }

  asBrowserContext(): BrowserContext {
    return this as unknown as BrowserContext
  }

  snapshot(): { closeListeners: number; offline: boolean; routes: number } {
    return {
      closeListeners: this.listenerCount('close'),
      offline: this.offline,
      routes: this.activeRoutes.size
    }
  }
}

class SurfaceGroupHarness implements ManagedPlaywrightSurfaceGroupAdapter {
  private generation = 1
  private surfaces: ManagedPlaywrightSurfaceView[] = [this.surface(0, true)]

  readonly ensureActiveSurface = vi.fn(async () => {
    const active = this.surfaces.find((surface) => surface.isActive)
    if (!active) throw new Error('fixture has no active surface')
    return active
  })

  readonly listSurfaces = (): readonly ManagedPlaywrightSurfaceView[] => this.surfaces

  readonly getSensitiveTargetIdentity = () => {
    const surface = this.surfaces.find((candidate) => candidate.isActive)
    if (!surface) return null
    return {
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      navigationEpoch: 1,
      origin: new URL(surface.url).origin
    }
  }

  readonly createSurface = vi.fn(async (input: { url?: string } = {}) => {
    this.surfaces = this.surfaces.map((surface) => ({ ...surface, isActive: false }))
    const created = this.surface(this.surfaces.length, true, input.url)
    this.surfaces.push(created)
    return created
  })

  readonly selectSurface = vi.fn(async ({ index }: { index: number }) => {
    this.surfaces = this.surfaces.map((surface) => ({
      ...surface,
      isActive: surface.index === index
    }))
    const selected = this.surfaces[index]
    if (!selected) throw new Error('fixture surface missing')
    return selected
  })

  readonly closeSurfaceByIndex = vi.fn(async (index?: number) => {
    const selected = index ?? this.surfaces.find((surface) => surface.isActive)?.index ?? 0
    this.surfaces = this.surfaces
      .filter((surface) => surface.index !== selected)
      .map((surface, nextIndex) => ({ ...surface, index: nextIndex }))
    if (this.surfaces.length > 0 && !this.surfaces.some((surface) => surface.isActive)) {
      this.surfaces[0] = { ...this.surfaces[0], isActive: true }
    }
  })

  snapshot(): { active: number; surfaces: number } {
    return {
      active: this.surfaces.filter((surface) => surface.isActive).length,
      surfaces: this.surfaces.length
    }
  }

  private surface(index: number, isActive: boolean, url?: string): ManagedPlaywrightSurfaceView {
    const generation = this.generation++
    return {
      surfaceId: `round3-surface-${generation}`,
      index,
      title: `Round 3 tab ${index}`,
      url: url ?? `http://127.0.0.1/tab-${index}`,
      isActive,
      generation
    }
  }
}

function fakeSensitiveTargetBindings(
  surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter | undefined
): ManagedPlaywrightSensitiveTargetBindingStore {
  const consumed = new Set<string>()
  return {
    acquire: (authorization) => {
      const grant = authorization.builtinToolGrant
      const target = grant?.origin === null ? undefined : surfaceGroup?.getSensitiveTargetIdentity()
      if (!grant || (grant.origin !== null && !target)) {
        throw new ManagedPlaywrightSensitiveTargetBindingError('origin_drifted')
      }
      if (consumed.has(grant.targetBindingId)) {
        throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
      }
      let dispatched = false
      return {
        target: target ? Object.freeze({ ...target }) : undefined,
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

function authorization(
  runId: string,
  callId: string,
  triggerToolName: string
): BrowserRiskAuthorizationContext {
  return {
    runId,
    capabilityId: 'browser_automation',
    activationId: '123e4567-e89b-42d3-a456-426614174000',
    manifestDigest: `sha256:${'a'.repeat(64)}`,
    policyRevision: 1,
    grantExpiresAtMs: FIXED_NOW + 200_000,
    invocationId: '123e4567-e89b-42d3-a456-426614174001',
    callId,
    triggerToolName,
    callReason: `Round 3 ${triggerToolName}.`
  }
}

function sensitiveAuthorization(
  toolName: string,
  argumentsValue: Record<string, unknown>,
  runId: string,
  callId: string,
  index: number
): BrowserRiskAuthorizationContext {
  const policy = sensitivePolicyForTool(toolName)
  if (!policy) throw new Error(`fixture sensitive policy missing for ${toolName}`)
  const bindingScope = sensitiveBindingScopeForInvocation(toolName, argumentsValue)
  const origin = bindingScope === 'managed_browser_profile' ? null : 'http://127.0.0.1'
  const argumentsDigest = canonicalSha256(argumentsValue)
  const base = authorization(runId, callId, toolName)
  const targetBindingId = fixtureUuid(index * 2 + 3)
  const targetBindingDigest = canonicalSha256({ targetBindingId })
  return {
    ...base,
    builtinToolGrant: {
      grantId: fixtureUuid(index * 2 + 1),
      approvalId: fixtureUuid(index * 2 + 2),
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
      expiresAtMs: FIXED_NOW + 100_000
    }
  }
}

function fixtureUuid(value: number): string {
  return `00000000-0000-4000-8000-${value.toString(16).padStart(12, '0')}`
}

function fileOwner(toolCallId: string): BrowserFileOwner {
  return {
    runId: 'round3-file-run',
    activationId: 'round3-file-activation',
    capabilityId: 'browser_automation',
    toolCallId
  }
}

class FakeDebugger extends EventEmitter {
  private attached = false

  readonly attach = vi.fn(() => {
    if (this.attached) throw new Error('already attached')
    this.attached = true
  })
  readonly detach = vi.fn(() => {
    this.attached = false
  })
  readonly isAttached = vi.fn(() => this.attached)
  readonly sendCommand = vi.fn(async (method: string): Promise<unknown> => {
    if (method === 'Browser.getVersion') {
      return {
        jsVersion: '1',
        product: 'Chrome/142.0.0.0',
        protocolVersion: '1.3',
        revision: 'round3-fixture',
        userAgent: 'round3-fixture'
      }
    }
    return {}
  })
}

class FakeWebContents extends EventEmitter {
  readonly fixtureDebugger = new FakeDebugger()
  readonly debugger = this.fixtureDebugger as unknown as Debugger
  readonly focus = vi.fn()
  private destroyed = false

  constructor(
    readonly id: number,
    private readonly kind: ReturnType<WebContents['getType']>,
    readonly hostWebContents: WebContents | null = null,
    readonly session: Session = EXPECTED_SESSION
  ) {
    super()
  }

  getTitle(): string {
    return `Round 3 fixture ${this.id}`
  }

  getType(): ReturnType<WebContents['getType']> {
    return this.kind
  }

  getURL(): string {
    return `http://127.0.0.1/round3-${this.id}`
  }

  isDestroyed(): boolean {
    return this.destroyed
  }

  async insertText(): Promise<void> {
    return undefined
  }

  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true
    this.emit('destroyed')
  }

  asWebContents(): WebContents {
    return this as unknown as WebContents
  }
}

class RelayCore {
  private cancel?: (input: ManagedPlaywrightCancelNotification) => void
  private command?: (input: ManagedPlaywrightCommandNotification) => void
  private readonly pending = new Map<
    string,
    (completion: ManagedPlaywrightCompletionInput) => void
  >()

  async completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<boolean> {
    this.pending.get(input.requestId)?.(input)
    this.pending.delete(input.requestId)
    return true
  }

  async acknowledgeManagedPlaywrightDispatchPhase(
    input: ManagedPlaywrightDispatchPhaseInput
  ): Promise<boolean> {
    void input
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

  async dispatch(
    input: ManagedPlaywrightCommandNotification
  ): Promise<ManagedPlaywrightCompletionInput> {
    const completion = new Promise<ManagedPlaywrightCompletionInput>((resolve) => {
      this.pending.set(input.requestId, resolve)
    })
    this.command?.(input)
    return await completion
  }

  snapshot(): { cancelListeners: number; commandListeners: number } {
    return {
      cancelListeners: this.cancel ? 1 : 0,
      commandListeners: this.command ? 1 : 0
    }
  }
}

class RelayGenerationHost {
  readonly connectSignals: AbortSignal[] = []
  closeCalls = 0

  constructor(private readonly failConnect: boolean) {}

  async connect(signal?: AbortSignal): Promise<void> {
    if (!signal) throw new Error('relay fixture signal missing')
    this.connectSignals.push(signal)
    if (this.failConnect) {
      throw new ManagedPlaywrightMcpHostError('browser.target_closed')
    }
  }

  protocolSnapshot(): ManagedPlaywrightProtocolSnapshot {
    return {
      negotiatedVersion: '2025-11-25',
      lifecycle: 'initialize_fallback',
      server: { name: '@playwright/mcp', version: '0.0.79' },
      capabilities: {
        tools: true,
        toolsListChanged: false,
        resources: false,
        resourcesListChanged: false,
        resourcesSubscribe: false,
        prompts: false,
        promptsListChanged: false,
        logging: false,
        completions: false,
        tasks: false,
        extensions: []
      }
    }
  }

  async close(): Promise<void> {
    this.closeCalls += 1
  }
}

function relayCommand(
  command: ManagedPlaywrightCommandNotification['command']
): ManagedPlaywrightCommandNotification {
  return {
    schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
    requestId: randomUUID(),
    serverId: MANAGED_PLAYWRIGHT_SERVER_ID,
    deadlineMs: FIXED_NOW + 10_000,
    command
  }
}

async function temporaryDirectory(prefix: string): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), prefix))
  TEMPORARY_ROOTS.add(directory)
  return directory
}
