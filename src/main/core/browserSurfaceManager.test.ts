import { EventEmitter } from 'node:events'
import type { Browser, BrowserContext } from 'playwright'
import type { Debugger, Session, WebContents } from 'electron'
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  BROWSER_WEBVIEW_PARTITION,
  createBrowserSurfaceBootstrapUrl,
  type BrowserSurfaceCommand
} from '@mycopilot/protocol'
import { BrowserTargetBroker } from '../browser/BrowserTargetBroker'
import {
  BrowserNetworkGuard,
  type BrowserNetworkOperationLease
} from '../browser/BrowserNetworkGuard'
import type { BrowserRiskOperationInput } from '../browser/BrowserRiskCoordinator'
import {
  BrowserSurfaceManager,
  type BrowserSurfaceManagerOptions
} from '../browser/BrowserSurfaceManager'
import type { ElectronGuestCdpTransport } from '../browser/ElectronGuestCdpTransport'

const EXPECTED_SESSION = {} as Session
const SURFACE_ID = 'right-sidebar-browser-browser-fixture'
const RISK_OPERATION_INPUT: BrowserRiskOperationInput = {
  authorizationContext: {
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
  },
  parentRequestId: '123e4567-e89b-42d3-a456-426614174002'
}

class FakeDebugger extends EventEmitter {
  private attached = false

  attach = vi.fn(() => {
    if (this.attached) throw new Error('already attached')
    this.attached = true
  })
  detach = vi.fn(() => {
    this.attached = false
  })
  isAttached = vi.fn(() => this.attached)
  sendCommand = vi.fn(async (method: string) => {
    if (method === 'Browser.getVersion') {
      return {
        jsVersion: '1',
        product: 'Chrome/142.0.0.0',
        protocolVersion: '1.3',
        revision: 'fixture',
        userAgent: 'fixture'
      }
    }
    return {}
  })
}

class FakeWebContents extends EventEmitter {
  readonly debugger = new FakeDebugger() as unknown as Debugger
  destroyed = false

  constructor(
    readonly id: number,
    readonly kind: ReturnType<WebContents['getType']>,
    public url: string,
    public hostWebContents: WebContents | null = null,
    public session: Session = EXPECTED_SESSION
  ) {
    super()
  }

  getTitle(): string {
    return `Fixture ${this.id}`
  }
  getType(): ReturnType<WebContents['getType']> {
    return this.kind
  }
  getURL(): string {
    return this.url
  }
  isDestroyed(): boolean {
    return this.destroyed
  }
  async loadURL(url: string): Promise<void> {
    this.url = url
  }
  printToPDF = vi.fn(async () => Buffer.from('%PDF-1.7\nfixture\n%%EOF\n'))
  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true
    this.emit('destroyed')
  }
  asWebContents(): WebContents {
    return this as unknown as WebContents
  }
}

interface FakeBrowserHarness {
  browser: Browser
  context: BrowserContext
}

function createFakeBrowser(transport: ElectronGuestCdpTransport): FakeBrowserHarness {
  const emitter = new EventEmitter()
  const context = {} as BrowserContext
  let connected = true
  const browser = Object.assign(emitter, {
    contexts: () => [context],
    isConnected: () => connected
  }) as unknown as Browser
  transport.onclose = () => {
    connected = false
    emitter.emit('disconnected')
  }
  return { browser, context }
}

function createHarness(overrides: Partial<BrowserSurfaceManagerOptions> = {}) {
  const host = new FakeWebContents(1, 'window', 'file:///renderer.html')
  const commands: BrowserSurfaceCommand[] = []
  const browsers: FakeBrowserHarness[] = []
  const broker = new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, EXPECTED_SESSION)
  const manager = new BrowserSurfaceManager({
    attachTimeoutMs: 1_000,
    broker,
    closeTimeoutMs: 1_000,
    createSurfaceId: () => SURFACE_ID,
    connectOverCdp: async (transport) => {
      const browser = createFakeBrowser(transport)
      browsers.push(browser)
      return browser.browser
    },
    resolveHost: () => host.asWebContents(),
    sendCommand: (_host, command) => commands.push(command),
    ...overrides
  })
  return { broker, browsers, commands, host, manager }
}

function attachGuest(
  manager: BrowserSurfaceManager,
  host: FakeWebContents,
  id = 2,
  surfaceId = SURFACE_ID
): FakeWebContents {
  const guest = new FakeWebContents(
    id,
    'webview',
    createBrowserSurfaceBootstrapUrl(surfaceId),
    host.asWebContents()
  )
  manager.registerManagedGuest({
    guest: guest.asWebContents(),
    host: host.asWebContents(),
    partition: BROWSER_WEBVIEW_PARTITION
  })
  return guest
}

afterEach(() => {
  vi.useRealTimers()
})

describe('BrowserSurfaceManager', () => {
  it.each([
    ['same-origin', 'https://mail.example.test/compose'],
    ['cross-origin', 'https://other.example.test/compose']
  ])('does not freeze a sensitive target during %s main-frame navigation', async (_name, url) => {
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    await pending

    guest.url = 'https://mail.example.test/inbox'
    guest.emit('did-start-navigation', {}, url, false, true)
    expect(manager.getSensitiveTargetIdentity()).toBeNull()
    guest.url = url
    guest.emit('did-navigate', {}, url, 200, 'OK')
    expect(manager.getSensitiveTargetIdentity()).toEqual({
      surfaceId: SURFACE_ID,
      generation: 1,
      navigationEpoch: 2,
      origin: new URL(url).origin
    })
    await manager.shutdown()
  })

  it('blocks a navigation for the full sensitive dispatch fence and removes every listener', async () => {
    const networkFinish = vi.fn()
    const networkFence = {
      blocked: vi.fn(() => false),
      finish: networkFinish
    }
    const networkGuard = {
      beginMainFrameNavigationFence: vi.fn(() => networkFence),
      deactivateAutomation: vi.fn(),
      registerGuest: vi.fn(),
      shutdown: vi.fn(async () => undefined)
    } as unknown as BrowserNetworkGuard
    const { commands, host, manager } = createHarness({ networkGuard })
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    await pending
    guest.url = 'https://mail.example.test/inbox'
    const identity = manager.getSensitiveTargetIdentity()
    if (!identity) throw new Error('sensitive target missing')
    const baselineNavigateListeners = guest.listenerCount('will-navigate')
    const baselineRedirectListeners = guest.listenerCount('will-redirect')
    const fence = manager.beginSensitiveDispatchFence(identity)
    const preventDefault = vi.fn()
    guest.emit('will-redirect', { preventDefault }, 'https://mail.example.test/other', false, true)
    expect(preventDefault).toHaveBeenCalledOnce()
    expect(() => fence.finish()).toThrow(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    expect(networkFinish).toHaveBeenCalledOnce()
    expect(guest.listenerCount('will-navigate')).toBe(baselineNavigateListeners)
    expect(guest.listenerCount('will-redirect')).toBe(baselineRedirectListeners)
    await manager.shutdown()
  })

  it('single-flights concurrent first use and only connects after exact Renderer readiness', async () => {
    const { browsers, commands, host, manager } = createHarness()
    const first = manager.getBrowserContext()
    const second = manager.getBrowserContext()
    expect(commands).toHaveLength(1)
    expect(commands[0]).toEqual(
      expect.objectContaining({ schemaVersion: 1, kind: 'ensureAttached' })
    )

    attachGuest(manager, host)
    expect(browsers).toHaveLength(0)
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    expect(
      manager.attach(host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: SURFACE_ID
      })
    ).toEqual({ schemaVersion: 1, accepted: true, surfaceId: SURFACE_ID })

    const [firstContext, secondContext] = await Promise.all([first, second])
    expect(firstContext).toBe(secondContext)
    expect(firstContext).toBe(browsers[0]?.context)
    expect(manager.snapshot()).toEqual({
      attached: true,
      connecting: false,
      pendingEnsure: false,
      surfaces: 1
    })
    await manager.shutdown()
  })

  it('creates, lists, selects, resizes, and closes a bounded managed surface group', async () => {
    const surfaceIds = ['browser-one', 'browser-two']
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'browser-over-capacity',
      maxSurfaces: 2
    })
    const firstContext = manager.getBrowserContext()
    const ensure = commands.at(-1)
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await firstContext

    const create = manager.createSurface({ url: 'http://127.0.0.1/second' })
    const createCommand = commands.at(-1)
    if (!createCommand || createCommand.kind !== 'createSurface') {
      throw new Error('create command missing')
    }
    attachGuest(manager, host, 3, createCommand.surfaceId)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: createCommand.requestId,
      surfaceId: createCommand.surfaceId
    })
    await expect(create).resolves.toEqual(
      expect.objectContaining({ index: 1, isActive: true, surfaceId: 'browser-two' })
    )
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ index: 0, isActive: false, surfaceId: 'browser-one' }),
      expect.objectContaining({ index: 1, isActive: true, surfaceId: 'browser-two' })
    ])

    const selected = manager.selectSurface({ index: 0 })
    const selectCommand = commands.at(-1)
    if (!selectCommand || selectCommand.kind !== 'selectSurface') {
      throw new Error('select command missing')
    }
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: selectCommand.requestId,
      surfaceId: selectCommand.surfaceId
    })
    await expect(selected).resolves.toEqual(
      expect.objectContaining({ index: 0, isActive: true, surfaceId: 'browser-one' })
    )
    await expect(manager.createSurface()).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_capacity_exceeded' })
    )

    const reconnected = manager.getBrowserContext()
    await reconnected
    const resized = manager.resizeActiveSurface({ height: 720, width: 1280 })
    const resizeCommand = commands.at(-1)
    if (!resizeCommand || resizeCommand.kind !== 'resizeSurface') {
      throw new Error('resize command missing')
    }
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: resizeCommand.requestId,
      surfaceId: resizeCommand.surfaceId,
      viewport: { height: 720, width: 1280 }
    })
    await expect(resized).resolves.toEqual({ height: 720, width: 1280 })
    await expect(manager.printActiveSurfaceToPdf()).resolves.toEqual(
      Uint8Array.from(Buffer.from('%PDF-1.7\nfixture\n%%EOF\n'))
    )

    const clipped = manager.resizeActiveSurface({ height: 720, width: 1280 })
    const clippedAssertion = expect(clipped).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    const clippedCommand = commands.at(-1)
    if (!clippedCommand || clippedCommand.kind !== 'resizeSurface') {
      throw new Error('clipped resize command missing')
    }
    expect(() =>
      manager.attach(host.asWebContents(), {
        schemaVersion: 1,
        requestId: clippedCommand.requestId,
        surfaceId: clippedCommand.surfaceId,
        viewport: { height: 600, width: 800 }
      })
    ).toThrow(expect.objectContaining({ code: 'browser.surface_unavailable' }))
    await clippedAssertion

    const secondGuest = manager.listSurfaces()[1]
    if (!secondGuest) throw new Error('second surface missing')
    const close = manager.closeSurfaceByIndex(1)
    const closeCommand = commands.at(-1)
    if (!closeCommand || closeCommand.kind !== 'closeSurface') {
      throw new Error('close command missing')
    }
    manager.handleTargetClosed(closeCommand.surfaceId, secondGuest.generation)
    await close
    expect(manager.listSurfaces()).toHaveLength(1)
    await manager.shutdown()
  })

  it('serializes concurrent new-tab requests without dropping or duplicating either command', async () => {
    const surfaceIds = ['concurrent-one', 'concurrent-two']
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-surface',
      maxSurfaces: 2
    })
    const first = manager.createSurface()
    const second = manager.createSurface()
    expect(commands).toHaveLength(1)
    const firstCommand = commands[0]
    if (!firstCommand || firstCommand.kind !== 'createSurface') {
      throw new Error('first create command missing')
    }
    attachGuest(manager, host, 2, firstCommand.surfaceId)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: firstCommand.requestId,
      surfaceId: firstCommand.surfaceId
    })

    await vi.waitFor(() => expect(commands).toHaveLength(2))
    const secondCommand = commands[1]
    if (!secondCommand || secondCommand.kind !== 'createSurface') {
      throw new Error('second create command missing')
    }
    attachGuest(manager, host, 3, secondCommand.surfaceId)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: secondCommand.requestId,
      surfaceId: secondCommand.surfaceId
    })

    await expect(Promise.all([first, second])).resolves.toHaveLength(2)
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual([
      'concurrent-one',
      'concurrent-two'
    ])
    await manager.shutdown()
  })

  it('reuses the trusted manually selected tab without granting unselected targets', async () => {
    const { broker, host, manager } = createHarness()
    attachGuest(manager, host, 2, 'manual-one')
    attachGuest(manager, host, 3, 'manual-two')

    expect(
      manager.selectManualSurface(host.asWebContents(), {
        schemaVersion: 1,
        surfaceId: 'manual-two'
      })
    ).toEqual({ schemaVersion: 1, accepted: true, surfaceId: 'manual-two' })
    await expect(manager.ensureActiveSurface()).resolves.toEqual(
      expect.objectContaining({ isActive: true, surfaceId: 'manual-two' })
    )
    await manager.getBrowserContext()
    expect(broker.snapshot().claimedSurfaces).toBe(1)
    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: 'manual-two' })

    manager.selectManualSurface(host.asWebContents(), {
      schemaVersion: 1,
      surfaceId: 'manual-one'
    })
    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: 'manual-two' })
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: false, surfaceId: 'manual-one' }),
      expect.objectContaining({ isActive: true, surfaceId: 'manual-two' })
    ])

    await manager.detachAutomation()
    expect(broker.snapshot().claimedSurfaces).toBe(0)
    expect(manager.snapshot().surfaces).toBe(2)
    await manager.shutdown()
  })

  it('creates an automated popup in the background without disconnecting the opener call', async () => {
    const surfaceIds = ['popup-opener', 'popup-child']
    const { browsers, commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-popup'
    })
    const context = manager.getBrowserContext()
    const ensure = commands.at(-1)
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const opener = attachGuest(manager, host, 2, ensure.surfaceId)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context

    const popup = manager.handlePopup({
      guest: opener.asWebContents(),
      url: 'http://127.0.0.1/popup'
    })
    const create = commands.at(-1)
    if (!create || create.kind !== 'createSurface') throw new Error('popup command missing')
    expect(create.activate).toBe(false)
    attachGuest(manager, host, 3, create.surfaceId)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await popup

    expect(browsers[0]?.browser.isConnected()).toBe(true)
    expect(manager.getActiveSurfaceIdentity()).toEqual({
      generation: 1,
      surfaceId: 'popup-opener'
    })
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: true, surfaceId: 'popup-opener' }),
      expect.objectContaining({ isActive: false, surfaceId: 'popup-child' })
    ])
    await manager.shutdown()
  })

  it('reuses the risk-preflight surface when Playwright connects after approval', async () => {
    const lease = {
      failure: vi.fn(() => null),
      finish: vi.fn(),
      markDispatched: vi.fn(),
      operation: {},
      settle: vi.fn(async () => undefined)
    } as unknown as BrowserNetworkOperationLease
    const networkGuard = {
      beginOperation: vi.fn(() => lease),
      deactivateAutomation: vi.fn(),
      registerGuest: vi.fn(),
      shutdown: vi.fn(async () => undefined)
    } as unknown as BrowserNetworkGuard
    const { browsers, commands, host, manager } = createHarness({ networkGuard })

    const preflight = manager.beginNetworkOperation(RISK_OPERATION_INPUT)
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    await expect(preflight).resolves.toBe(lease)

    const context = await manager.getBrowserContext()
    expect(context).toBe(browsers[0]?.context)
    expect(commands.filter((candidate) => candidate.kind === 'ensureAttached')).toHaveLength(1)
    expect(networkGuard.beginOperation).toHaveBeenCalledOnce()
    await manager.shutdown()
  })

  it('fails closed for stale requests, another host, and target closure', async () => {
    const { commands, host, manager } = createHarness()
    const context = manager.getBrowserContext()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const otherHost = new FakeWebContents(9, 'window', 'file:///other.html')

    expect(() =>
      manager.attach(otherHost.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: SURFACE_ID
      })
    ).toThrow(expect.objectContaining({ code: 'browser.surface_unavailable' }))

    const guest = attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    guest.destroy()
    await expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.target_closed' })
    )
    expect(commands.at(-1)).toEqual(
      expect.objectContaining({ kind: 'closeSurface', surfaceId: SURFACE_ID })
    )
    await manager.shutdown()
  })

  it('detaches automation without destroying the manually usable guest', async () => {
    const { broker, browsers, commands, host, manager } = createHarness()
    const contextPromise = manager.getBrowserContext()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    await contextPromise

    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: SURFACE_ID })
    await manager.closeAutomation()
    expect(guest.isDestroyed()).toBe(false)
    expect(browsers[0]?.browser.isConnected()).toBe(false)
    expect(manager.getActiveSurfaceIdentity()).toBeNull()
    expect(manager.snapshot().surfaces).toBe(1)
    expect(broker.snapshot().claimedSurfaces).toBe(0)
    await manager.shutdown()
  })

  it('revokes a pending first attachment while retaining the already attached guest', async () => {
    const { commands, host, manager } = createHarness()
    const context = manager.getBrowserContext()
    const rejection = expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)

    await manager.detachAutomation()
    await rejection
    expect(guest.isDestroyed()).toBe(false)
    expect(manager.snapshot()).toEqual({
      attached: false,
      connecting: false,
      pendingEnsure: false,
      surfaces: 1
    })
    expect(() =>
      manager.attach(host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: SURFACE_ID
      })
    ).toThrow(expect.objectContaining({ code: 'browser.surface_unavailable' }))
    await manager.shutdown()
  })

  it('revokes an in-flight CDP connection before it can become active', async () => {
    let finishConnect!: (browser: Browser) => void
    let transport: ElectronGuestCdpTransport | undefined
    const browserPromise = new Promise<Browser>((resolve) => {
      finishConnect = resolve
    })
    const { commands, host, manager } = createHarness({
      connectOverCdp: async (nextTransport) => {
        transport = nextTransport
        return await browserPromise
      }
    })
    const context = manager.getBrowserContext()
    const rejection = expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    await vi.waitFor(() => expect(transport).toBeDefined())

    const detach = manager.detachAutomation()
    if (!transport) throw new Error('transport missing')
    const connected = createFakeBrowser(transport)
    finishConnect(connected.browser)
    await Promise.all([detach, rejection])

    expect(connected.browser.isConnected()).toBe(false)
    expect(guest.isDestroyed()).toBe(false)
    expect(manager.getActiveSurfaceIdentity()).toBeNull()
    await manager.shutdown()
  })

  it('rejects a pending first attachment when shutdown starts', async () => {
    const { manager } = createHarness()
    const context = manager.getBrowserContext()
    const rejection = expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.manager_shutdown' })
    )

    await manager.shutdown()
    await rejection
    expect(manager.snapshot()).toEqual({
      attached: false,
      connecting: false,
      pendingEnsure: false,
      surfaces: 0
    })
  })

  it('does not publish a late CDP connection after shutdown', async () => {
    let finishConnect!: (browser: Browser) => void
    let transport: ElectronGuestCdpTransport | undefined
    const browserPromise = new Promise<Browser>((resolve) => {
      finishConnect = resolve
    })
    const { commands, host, manager } = createHarness({
      connectOverCdp: async (nextTransport) => {
        transport = nextTransport
        return await browserPromise
      }
    })
    const context = manager.getBrowserContext()
    const rejection = expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.manager_shutdown' })
    )
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    manager.attach(host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    await vi.waitFor(() => expect(transport).toBeDefined())

    const shutdown = manager.shutdown()
    if (!transport) throw new Error('transport missing')
    const connected = createFakeBrowser(transport)
    finishConnect(connected.browser)
    await Promise.all([shutdown, rejection])

    expect(connected.browser.isConnected()).toBe(false)
    expect(guest.isDestroyed()).toBe(false)
    expect(manager.getActiveSurfaceIdentity()).toBeNull()
    expect(manager.snapshot()).toEqual({
      attached: false,
      connecting: false,
      pendingEnsure: false,
      surfaces: 0
    })
  })

  it('closes only the selected surface and reports a bounded failure when UI does not comply', async () => {
    vi.useFakeTimers()
    const { commands, host, manager } = createHarness({ closeTimeoutMs: 50 })
    const guest = attachGuest(manager, host)
    const close = manager.closeSurface(SURFACE_ID)
    expect(commands.at(-1)).toEqual(
      expect.objectContaining({ kind: 'closeSurface', surfaceId: SURFACE_ID })
    )
    guest.destroy()
    await close

    const replacement = attachGuest(manager, host, 3)
    const timedOut = manager.closeSurface(SURFACE_ID)
    const timeoutAssertion = expect(timedOut).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    await vi.advanceTimersByTimeAsync(100)
    await timeoutAssertion
    expect(replacement.isDestroyed()).toBe(false)

    const retry = manager.closeSurface(SURFACE_ID)
    replacement.destroy()
    await retry
    expect(
      commands.filter(
        (command) => command.kind === 'closeSurface' && command.surfaceId === SURFACE_ID
      )
    ).toHaveLength(3)
    await manager.shutdown()
  })

  it('times out an invisible/unresponsive Renderer without leaking pending work', async () => {
    vi.useFakeTimers()
    const { manager } = createHarness({ attachTimeoutMs: 50 })
    const context = manager.getBrowserContext()
    const timeoutAssertion = expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    await vi.advanceTimersByTimeAsync(100)
    await timeoutAssertion
    expect(manager.snapshot().pendingEnsure).toBe(false)
    await manager.shutdown()
  })

  it('reports target_closed when browser_close has no current attached surface', async () => {
    const { manager } = createHarness()
    await expect(manager.closeSurface()).rejects.toEqual(
      expect.objectContaining({ code: 'browser.target_closed' })
    )
    await manager.shutdown()
  })
})
