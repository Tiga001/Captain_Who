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
    readonly url: string,
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
    const { browsers, commands, host, manager } = createHarness()
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
