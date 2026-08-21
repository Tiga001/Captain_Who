import { EventEmitter } from 'node:events'
import type { Browser, BrowserContext, ConnectOverCDPTransport } from 'playwright'
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
  type BrowserNetworkOperationLease,
  type BrowserTargetCreationAuthority
} from '../browser/BrowserNetworkGuard'
import type { BrowserRiskOperationInput } from '../browser/BrowserRiskCoordinator'
import {
  BrowserSurfaceManager,
  type BrowserSurfaceManagerOptions
} from '../browser/BrowserSurfaceManager'

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
    if (method === 'Network.getAllCookies') return { cookies: [] }
    return {}
  })
}

class FakeWebContents extends EventEmitter {
  readonly debugger = new FakeDebugger() as unknown as Debugger
  destroyed = false
  loadingMainFrame = false

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
  isLoadingMainFrame(): boolean {
    return this.loadingMainFrame
  }
  async loadURL(url: string): Promise<void> {
    this.url = url
  }
  close = vi.fn(() => this.destroy())
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

function createFakeBrowser(transport: ConnectOverCDPTransport): FakeBrowserHarness {
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

function createCdpHarness(transport: ConnectOverCDPTransport) {
  let nextId = 1
  const pending = new Map<number, (message: Record<string, unknown>) => void>()
  const events: Record<string, unknown>[] = []
  transport.onmessage = (message) => {
    const record = message as Record<string, unknown>
    events.push(record)
    if (typeof record.id !== 'number') return
    const resolve = pending.get(record.id)
    pending.delete(record.id)
    resolve?.(record)
  }
  return {
    events,
    send(method: string, params?: Record<string, unknown>, sessionId?: string) {
      const id = nextId++
      return new Promise<Record<string, unknown>>((resolve) => {
        pending.set(id, resolve)
        transport.send({
          id,
          method,
          ...(params ? { params } : {}),
          ...(sessionId ? { sessionId } : {})
        })
      })
    }
  }
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

function selectBoundSurface(
  manager: BrowserSurfaceManager,
  host: FakeWebContents,
  surfaceId: string,
  selectionRevision: number
) {
  const surfaceInstanceId = probeSurfaceInstance(manager, host, surfaceId, selectionRevision)
  return manager.selectManualSurface(host.asWebContents(), {
    schemaVersion: 1,
    surfaceId,
    surfaceInstanceId,
    selectionRevision
  })
}

function probeSurfaceInstance(
  manager: BrowserSurfaceManager,
  host: FakeWebContents,
  surfaceId: string,
  selectionRevision: number
): string {
  const probe = manager.selectManualSurface(host.asWebContents(), {
    schemaVersion: 1,
    surfaceId,
    surfaceInstanceId: null,
    selectionRevision
  })
  if (probe.reason !== 'instance_required' || !probe.surfaceInstanceId) {
    throw new Error(`surface instance probe failed: ${probe.reason}`)
  }
  return probe.surfaceInstanceId
}

function attachSurface(
  manager: BrowserSurfaceManager,
  host: WebContents,
  input: Parameters<BrowserSurfaceManager['attach']>[1]
): ReturnType<BrowserSurfaceManager['attach']> {
  if (input.surfaceInstanceId !== undefined) return manager.attach(host, input)
  const probe = manager.selectManualSurface(host, {
    schemaVersion: 1,
    surfaceId: input.surfaceId,
    surfaceInstanceId: null,
    selectionRevision: Number.MAX_SAFE_INTEGER
  })
  return manager.attach(host, {
    ...input,
    ...(probe.reason === 'instance_required' && probe.surfaceInstanceId
      ? { surfaceInstanceId: probe.surfaceInstanceId }
      : {})
  })
}

afterEach(() => {
  vi.useRealTimers()
})

describe('BrowserSurfaceManager', () => {
  it('keeps a production ensure pending until Renderer acknowledges the exact instance', async () => {
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, command.surfaceId)
    const surfaceInstanceId = probeSurfaceInstance(
      manager,
      host,
      command.surfaceId,
      Number.MAX_SAFE_INTEGER
    )

    expect(
      manager.attach(host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId
      })
    ).toEqual(
      expect.objectContaining({
        accepted: false,
        reason: 'instance_mismatch',
        retryable: true,
        surfaceInstanceId
      })
    )
    let settled = false
    void pending.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(false)

    expect(
      manager.attach(host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId,
        surfaceInstanceId
      })
    ).toEqual(expect.objectContaining({ accepted: true, reason: 'surface_ready' }))
    await expect(pending).resolves.toEqual(
      expect.objectContaining({ generation: 1, surfaceId: command.surfaceId })
    )
    await manager.shutdown()
  })

  it('registers a selectable instance early but keeps pending ensure behind document readiness', async () => {
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    guest.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: guest.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    const probe = manager.selectManualSurface(host.asWebContents(), {
      schemaVersion: 1,
      surfaceId: command.surfaceId,
      surfaceInstanceId: null,
      selectionRevision: 1
    })
    expect(probe).toEqual(
      expect.objectContaining({
        reason: 'instance_required',
        surfaceInstanceId: expect.any(String)
      })
    )
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    let settled = false
    void pending.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(false)

    guest.emit('dom-ready')
    await expect(pending).resolves.toEqual(
      expect.objectContaining({ surfaceId: command.surfaceId })
    )
    await manager.shutdown()
  })

  it('keeps an initially loading surface alive throughout the configured readiness budget', async () => {
    vi.useFakeTimers()
    const { commands, host, manager } = createHarness({ attachTimeoutMs: 10_000 })
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    guest.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: guest.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })

    await vi.advanceTimersByTimeAsync(4_999)
    expect(guest.close).not.toHaveBeenCalled()
    expect(guest.isDestroyed()).toBe(false)

    guest.emit('dom-ready')
    await expect(pending).resolves.toEqual(
      expect.objectContaining({ generation: 1, surfaceId: command.surfaceId })
    )
    expect(guest.close).not.toHaveBeenCalled()
    await manager.shutdown()
  })

  it('fails closed when initial document readiness exceeds the configured attach timeout', async () => {
    vi.useFakeTimers()
    const { host, manager } = createHarness({ attachTimeoutMs: 10_000 })
    const guest = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(SURFACE_ID),
      host.asWebContents()
    )
    guest.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: guest.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: SURFACE_ID
    })

    await vi.advanceTimersByTimeAsync(9_999)
    expect(guest.close).not.toHaveBeenCalled()
    expect(guest.isDestroyed()).toBe(false)

    await vi.advanceTimersByTimeAsync(1)
    expect(guest.close).toHaveBeenCalledTimes(1)
    expect(guest.isDestroyed()).toBe(true)
    expect(manager.listSurfaces()).toHaveLength(0)
    await manager.shutdown()
  })

  it('hands a startup ready acknowledgement to the exact StrictMode replacement generation', async () => {
    vi.useFakeTimers()
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    // Keep a rejection observer attached while this test deliberately retires generation 1.
    void pending.catch(() => undefined)
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')

    const first = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    first.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: first.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })

    // React StrictMode removes the first layout-effect webview before its bootstrap document is
    // ready. Its Renderer IPC can already be queued, so Main must not settle the logical request
    // until the replacement incarnation has either registered or a bounded handoff expires.
    first.destroy()
    expect(() =>
      attachSurface(manager, host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId
      })
    ).not.toThrow()

    const replacement = new FakeWebContents(
      3,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    replacement.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: replacement.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    replacement.emit('dom-ready')
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })

    await expect(pending).resolves.toEqual(
      expect.objectContaining({ generation: 2, surfaceId: command.surfaceId, title: 'Fixture 3' })
    )
    expect(
      attachSurface(manager, host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId
      })
    ).toEqual(
      expect.objectContaining({
        accepted: false,
        status: 'noop',
        reason: 'already_ready',
        requestId: command.requestId
      })
    )
    await manager.shutdown()
  })

  it('never transfers generation-one Renderer readiness to a generation-two replacement', async () => {
    vi.useFakeTimers()
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const first = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    first.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: first.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    first.destroy()

    const replacement = new FakeWebContents(
      3,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    replacement.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: replacement.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    replacement.emit('dom-ready')
    let settled = false
    void pending.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(false)

    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    await expect(pending).resolves.toEqual(
      expect.objectContaining({ generation: 2, surfaceId: command.surfaceId })
    )
    await manager.shutdown()
  })

  it('recognizes a bootstrap document whose dom-ready event preceded Main registration', async () => {
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    guest.loadingMainFrame = false
    manager.registerManagedGuest({
      documentReady: false,
      guest: guest.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })

    await expect(pending).resolves.toEqual(
      expect.objectContaining({ generation: 1, surfaceId: command.surfaceId, title: 'Fixture 2' })
    )
    expect(guest.close).not.toHaveBeenCalled()
    await manager.shutdown()
  })

  it('waits for the acknowledged bootstrap navigation to settle before stripping its marker', async () => {
    const { commands, host, manager } = createHarness()
    const context = manager.getBrowserContext()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    guest.loadingMainFrame = true
    const loadURL = vi.spyOn(guest, 'loadURL')
    manager.registerManagedGuest({
      documentReady: false,
      guest: guest.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    guest.emit('dom-ready')
    await Promise.resolve()
    expect(loadURL).not.toHaveBeenCalled()

    guest.loadingMainFrame = false
    guest.emit('did-stop-loading')
    await expect(context).resolves.toBeDefined()
    expect(loadURL).toHaveBeenCalledWith('about:blank')
    await manager.shutdown()
  })

  it('returns typed stale readiness after a bounded handoff without reviving the old request', async () => {
    vi.useFakeTimers()
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const rejection = expect(pending).rejects.toEqual(
      expect.objectContaining({ code: 'browser.target_closed' })
    )
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    guest.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: guest.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    guest.destroy()
    await vi.advanceTimersByTimeAsync(100)
    await rejection

    expect(
      attachSurface(manager, host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId
      })
    ).toEqual({
      schemaVersion: 1,
      accepted: false,
      status: 'stale',
      reason: 'target_closed',
      retryable: false,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    expect(manager.snapshot()).toMatchObject({ pendingEnsure: false, surfaces: 0 })

    const otherHost = new FakeWebContents(99, 'window', 'file:///other-renderer.html')
    expect(() =>
      attachSurface(manager, otherHost.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId
      })
    ).toThrow(expect.objectContaining({ code: 'browser.surface_unavailable' }))
    await manager.shutdown()
  })

  it('never queues an old-generation close for a replacement registered inside the handoff', async () => {
    vi.useFakeTimers()
    const delivered: BrowserSurfaceCommand[] = []
    const { host, manager } = createHarness({
      sendCommand: (_host, command) => delivered.push(command)
    })
    const pending = manager.ensureActiveSurface()
    const command = delivered[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const first = new FakeWebContents(
      2,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    first.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: first.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    first.destroy()
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })

    await vi.advanceTimersByTimeAsync(50)
    const replacement = new FakeWebContents(
      3,
      'webview',
      createBrowserSurfaceBootstrapUrl(command.surfaceId),
      host.asWebContents()
    )
    replacement.loadingMainFrame = true
    manager.registerManagedGuest({
      documentReady: false,
      guest: replacement.asWebContents(),
      host: host.asWebContents(),
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId: command.surfaceId
    })
    replacement.emit('dom-ready')
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    await expect(pending).resolves.toEqual(
      expect.objectContaining({ generation: 2, surfaceId: command.surfaceId })
    )
    await vi.advanceTimersByTimeAsync(100)

    // Model the Renderer consuming every scalar close that Main managed to enqueue. A stale
    // generationless close would destroy the healthy replacement here.
    for (const deliveredCommand of delivered) {
      if (deliveredCommand.kind === 'closeSurface') replacement.destroy()
    }
    expect(
      delivered.filter(
        (deliveredCommand) =>
          deliveredCommand.kind === 'closeSurface' &&
          deliveredCommand.surfaceId === command.surfaceId
      )
    ).toHaveLength(0)
    expect(replacement.isDestroyed()).toBe(false)
    await manager.shutdown()
  })

  it.each([
    ['same-origin', 'https://mail.example.test/compose'],
    ['cross-origin', 'https://other.example.test/compose']
  ])('does not freeze a sensitive target during %s main-frame navigation', async (_name, url) => {
    const { commands, host, manager } = createHarness()
    const pending = manager.ensureActiveSurface()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    attachSurface(manager, host.asWebContents(), {
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
    attachSurface(manager, host.asWebContents(), {
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
      attachSurface(manager, host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: SURFACE_ID
      })
    ).toEqual({
      schemaVersion: 1,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId: command.requestId,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: expect.any(String)
    })

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
    const { browsers, commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'browser-over-capacity',
      maxSurfaces: 2
    })
    const firstContext = manager.getBrowserContext()
    const ensure = commands.at(-1)
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const firstGuest = attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    const initialContext = await firstContext

    const create = manager.createSurface({ url: 'http://127.0.0.1/second' })
    const createCommand = commands.at(-1)
    if (!createCommand || createCommand.kind !== 'createSurface') {
      throw new Error('create command missing')
    }
    attachGuest(manager, host, 3, createCommand.surfaceId)
    attachSurface(manager, host.asWebContents(), {
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
    attachSurface(manager, host.asWebContents(), {
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

    const sameContext = manager.getBrowserContext()
    await expect(sameContext).resolves.toBe(initialContext)
    expect(browsers).toHaveLength(1)
    const resized = manager.resizeActiveSurface({ height: 720, width: 1280 })
    const resizeCommand = commands.at(-1)
    if (!resizeCommand || resizeCommand.kind !== 'resizeSurface') {
      throw new Error('resize command missing')
    }
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: resizeCommand.requestId,
      surfaceId: resizeCommand.surfaceId,
      viewport: { height: 720, width: 1280 }
    })
    await expect(resized).resolves.toEqual({ height: 720, width: 1280 })
    await expect(manager.printActiveSurfaceToPdf()).resolves.toEqual(
      Uint8Array.from(Buffer.from('%PDF-1.7\nfixture\n%%EOF\n'))
    )
    expect(firstGuest.printToPDF).toHaveBeenCalledWith({ printBackground: false })

    const clipped = manager.resizeActiveSurface({ height: 720, width: 1280 })
    const clippedAssertion = expect(clipped).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    const clippedCommand = commands.at(-1)
    if (!clippedCommand || clippedCommand.kind !== 'resizeSurface') {
      throw new Error('clipped resize command missing')
    }
    expect(() =>
      attachSurface(manager, host.asWebContents(), {
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

  it('keeps a profile context at zero visible tabs then materializes exactly one claimed blank tab', async () => {
    const { browsers, commands, host, manager } = createHarness({
      createSurfaceId: () => 'zero-tab-created'
    })
    const profileContext = await manager.getBrowserContext({ createVisiblePage: false })
    expect(commands).toEqual([])
    expect(manager.listSurfaces()).toEqual([])
    expect(manager.snapshot()).toMatchObject({ attached: true, surfaces: 0 })

    const events: string[] = []
    const authority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async ({ guest }) => {
        events.push('claim')
        expect(guest.getURL()).toBe('about:blank')
      }),
      finish: vi.fn()
    }
    const creation = manager.createInitialTargetSurface({ authority })
    const command = commands.at(-1)
    if (!command || command.kind !== 'createSurface') throw new Error('create command missing')
    const guest = attachGuest(manager, host, 2, command.surfaceId)
    vi.spyOn(guest, 'loadURL').mockImplementation(async (url) => {
      events.push(`load:${url}`)
      guest.url = url
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })

    await expect(creation).resolves.toEqual(
      expect.objectContaining({ index: 0, isActive: true, surfaceId: 'zero-tab-created' })
    )
    expect(events).toEqual(['load:about:blank', 'claim'])
    expect(manager.listSurfaces()).toHaveLength(1)
    expect(await manager.getBrowserContext({ createVisiblePage: false })).toBe(profileContext)
    expect(browsers).toHaveLength(1)
    await manager.shutdown()
  })

  it('uses one background control surface for zero-tab Storage and leaves no Renderer surface', async () => {
    let groupTransport!: ConnectOverCDPTransport
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => 'zero-tab-storage-control',
      connectOverCdp: async (transport) => {
        groupTransport = transport
        return createFakeBrowser(transport).browser
      }
    })
    await manager.getBrowserContext({ createVisiblePage: false })
    expect(manager.listSurfaces()).toEqual([])

    const cdp = createCdpHarness(groupTransport)
    const storage = cdp.send('Storage.getCookies', {})
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({
          activate: false,
          kind: 'createSurface',
          surfaceId: 'zero-tab-storage-control'
        })
      )
    )
    const create = commands.find(
      (command) =>
        command.kind === 'createSurface' && command.surfaceId === 'zero-tab-storage-control'
    )
    if (!create || create.kind !== 'createSurface') throw new Error('control create missing')
    const control = attachGuest(manager, host, 2, create.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({
          kind: 'closeSurface',
          surfaceId: 'zero-tab-storage-control'
        })
      )
    )
    control.destroy()

    await expect(storage).resolves.toEqual(expect.objectContaining({ result: { cookies: [] } }))
    expect(manager.listSurfaces()).toEqual([])
    expect(manager.snapshot()).toMatchObject({ attached: true, surfaces: 0 })
    await manager.shutdown()
  })

  it('force-retires a zero-tab Storage control when Renderer loses its close acknowledgement', async () => {
    let groupTransport!: ConnectOverCDPTransport
    let networkGuests = 0
    let downloadGuests = 0
    const networkGuard = {
      deactivateAutomation: vi.fn(),
      registerGuest: vi.fn(({ guest }: { guest: WebContents }) => {
        networkGuests += 1
        downloadGuests += 1
        guest.once('destroyed', () => {
          networkGuests -= 1
          downloadGuests -= 1
        })
      }),
      shutdown: vi.fn(async () => undefined)
    } as unknown as BrowserNetworkGuard
    const { broker, commands, host, manager } = createHarness({
      closeTimeoutMs: 10,
      createSurfaceId: () => 'zero-tab-storage-timeout',
      connectOverCdp: async (transport) => {
        groupTransport = transport
        return createFakeBrowser(transport).browser
      },
      networkGuard
    })
    await manager.getBrowserContext({ createVisiblePage: false })
    const cdp = createCdpHarness(groupTransport)
    const storage = cdp.send('Storage.getCookies', {})
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({
          kind: 'createSurface',
          surfaceId: 'zero-tab-storage-timeout'
        })
      )
    )
    const create = commands.find(
      (command) =>
        command.kind === 'createSurface' && command.surfaceId === 'zero-tab-storage-timeout'
    )
    if (!create || create.kind !== 'createSurface') throw new Error('control create missing')
    const control = attachGuest(manager, host, 2, create.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })

    await expect(storage).resolves.toEqual(expect.objectContaining({ result: { cookies: [] } }))
    expect(control.close).toHaveBeenCalledWith({ waitForBeforeUnload: false })
    expect(control.isDestroyed()).toBe(true)
    expect(manager.listSurfaces()).toEqual([])
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
    expect(networkGuests).toBe(0)
    expect(downloadGuests).toBe(0)
    await vi.waitFor(() =>
      expect(
        commands.filter(
          (command) =>
            command.kind === 'closeSurface' && command.surfaceId === 'zero-tab-storage-timeout'
        ).length
      ).toBeGreaterThanOrEqual(2)
    )
    await manager.shutdown()
  })

  it('removes the zero-tab creation guest when authority claim fails', async () => {
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => 'zero-tab-rejected'
    })
    const authority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async () => {
        throw new Error('fixture authority rejected')
      }),
      finish: vi.fn()
    }
    const creation = manager.createInitialTargetSurface({ authority })
    const rejection = expect(creation).rejects.toThrow('fixture authority rejected')
    const command = commands.at(-1)
    if (!command || command.kind !== 'createSurface') throw new Error('create command missing')
    const guest = attachGuest(manager, host, 2, command.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'closeSurface', surfaceId: 'zero-tab-rejected' })
      )
    )
    guest.destroy()
    await rejection
    expect(manager.listSurfaces()).toHaveLength(0)
    await manager.shutdown()
  })

  it('claims a popup from a targetless initial tab before Context connection and first URL', async () => {
    const surfaceIds = ['preconnect-opener', 'preconnect-popup']
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-preconnect-surface'
    })
    const initialAuthority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const initial = manager.createInitialTargetSurface({ authority: initialAuthority })
    const initialCommand = commands.at(-1)
    if (!initialCommand || initialCommand.kind !== 'createSurface') {
      throw new Error('initial create command missing')
    }
    const opener = attachGuest(manager, host, 2, initialCommand.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: initialCommand.requestId,
      surfaceId: initialCommand.surfaceId
    })
    await initial
    expect(manager.snapshot().attached).toBe(false)

    const events: string[] = []
    const popupAuthority: BrowserTargetCreationAuthority = {
      action: 'popup',
      claim: vi.fn(async ({ guest }) => {
        events.push('claim')
        expect(guest.getURL()).toBe('about:blank')
      }),
      finish: vi.fn()
    }
    const popupCreation = manager.handlePopup({
      authority: popupAuthority,
      guest: opener.asWebContents(),
      url: 'https://fixture.example.test/preconnect-popup'
    })
    const popupCommand = commands.at(-1)
    if (!popupCommand || popupCommand.kind !== 'createSurface') {
      throw new Error('popup create command missing')
    }
    const popup = attachGuest(manager, host, 3, popupCommand.surfaceId)
    vi.spyOn(popup, 'loadURL').mockImplementation(async (url) => {
      events.push(`load:${url}`)
      popup.url = url
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: popupCommand.requestId,
      surfaceId: popupCommand.surfaceId
    })
    await popupCreation

    expect(events).toEqual([
      'load:about:blank',
      'claim',
      'load:https://fixture.example.test/preconnect-popup'
    ])
    expect(popupAuthority.claim).toHaveBeenCalledOnce()
    expect(manager.listSurfaces()).toHaveLength(2)
    await manager.shutdown()
  })

  it('removes a pre-connect popup when its exact authority was cancelled', async () => {
    const surfaceIds = ['cancel-popup-opener', 'cancel-popup-child']
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-cancel-popup-surface'
    })
    const initial = manager.createInitialTargetSurface({
      authority: {
        action: 'new',
        claim: vi.fn(async () => undefined),
        finish: vi.fn()
      }
    })
    const initialCommand = commands.at(-1)
    if (!initialCommand || initialCommand.kind !== 'createSurface') {
      throw new Error('initial create command missing')
    }
    const opener = attachGuest(manager, host, 2, initialCommand.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: initialCommand.requestId,
      surfaceId: initialCommand.surfaceId
    })
    await initial

    const events: string[] = []
    const popupCreation = manager.handlePopup({
      authority: {
        action: 'popup',
        claim: vi.fn(async () => {
          events.push('claim-cancelled')
          throw new Error('fixture popup authority cancelled')
        }),
        finish: vi.fn()
      },
      guest: opener.asWebContents(),
      url: 'https://fixture.example.test/must-not-load'
    })
    const popupCommand = commands.at(-1)
    if (!popupCommand || popupCommand.kind !== 'createSurface') {
      throw new Error('popup create command missing')
    }
    const popup = attachGuest(manager, host, 3, popupCommand.surfaceId)
    vi.spyOn(popup, 'loadURL').mockImplementation(async (url) => {
      events.push(`load:${url}`)
      popup.url = url
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: popupCommand.requestId,
      surfaceId: popupCommand.surfaceId
    })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'closeSurface', surfaceId: 'cancel-popup-child' })
      )
    )
    popup.destroy()
    await expect(popupCreation).rejects.toThrow('fixture popup authority cancelled')
    expect(events).toEqual(['load:about:blank', 'claim-cancelled'])
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual([
      'cancel-popup-opener'
    ])
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
    attachSurface(manager, host.asWebContents(), {
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
    attachSurface(manager, host.asWebContents(), {
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

  it('reuses the trusted manually selected tab while admitting only its managed SurfaceGroup', async () => {
    const { broker, host, manager } = createHarness()
    const manualOne = attachGuest(manager, host, 2, 'manual-one')
    const manualTwo = attachGuest(manager, host, 3, 'manual-two')
    manualOne.url = 'https://one.example.test/inbox'
    manualTwo.url = 'https://two.example.test/inbox'

    expect(selectBoundSurface(manager, host, 'manual-two', 1)).toEqual(
      expect.objectContaining({
        schemaVersion: 1,
        status: 'applied',
        surfaceId: 'manual-two',
        selectionRevision: 1
      })
    )
    await expect(manager.ensureActiveSurface()).resolves.toEqual(
      expect.objectContaining({ isActive: true, surfaceId: 'manual-two' })
    )
    await manager.getBrowserContext()
    const firstLease = await manager.beginToolSurfaceLease()
    expect(firstLease).toEqual(
      expect.objectContaining({ index: 1, surfaceId: 'manual-two', selectionRevision: 1 })
    )
    expect(broker.snapshot().claimedSurfaces).toBe(2)
    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: 'manual-two' })

    selectBoundSurface(manager, host, 'manual-one', 2)
    expect(manager.getSensitiveTargetIdentity()).toEqual(
      expect.objectContaining({ origin: 'https://two.example.test', surfaceId: 'manual-two' })
    )
    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: 'manual-two' })
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: true, surfaceId: 'manual-one' }),
      expect.objectContaining({ isActive: false, surfaceId: 'manual-two' })
    ])
    firstLease.finish()
    // Approval preparation happens before the next Tool lease. It must bind the latest trusted UI
    // selection, not the previous Playwright current tab.
    expect(manager.getSensitiveTargetIdentity()).toEqual(
      expect.objectContaining({ origin: 'https://one.example.test', surfaceId: 'manual-one' })
    )
    const secondLease = await manager.beginToolSurfaceLease()
    expect(secondLease).toEqual(
      expect.objectContaining({ index: 0, surfaceId: 'manual-one', selectionRevision: 2 })
    )
    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: 'manual-one' })
    await expect(secondLease.resolveIndex()).resolves.toBe(0)
    secondLease.finish()

    await manager.detachAutomation()
    expect(broker.snapshot().claimedSurfaces).toBe(2)
    expect(manager.snapshot().surfaces).toBe(2)
    await manager.shutdown()
  })

  it('returns a typed negative acknowledgement for an unregistered Renderer page', async () => {
    const { host, manager } = createHarness()

    expect(
      manager.selectManualSurface(host.asWebContents(), {
        schemaVersion: 1,
        surfaceId: 'renderer-page-not-yet-registered',
        surfaceInstanceId: null,
        selectionRevision: 1
      })
    ).toEqual({
      schemaVersion: 1,
      status: 'noop',
      reason: 'not_registered',
      retryable: true,
      surfaceId: 'renderer-page-not-yet-registered',
      surfaceInstanceId: null,
      selectionRevision: 1,
      authoritativeRevision: 0
    })
    expect(manager.listSurfaces()).toEqual([])
    await manager.shutdown()
  })

  it('clears the active UI selection with a revision-idempotent null intent', async () => {
    const { host, manager } = createHarness()
    attachGuest(manager, host, 2, 'clear-selection')
    selectBoundSurface(manager, host, 'clear-selection', 1)
    const clearInput = {
      schemaVersion: 1 as const,
      surfaceId: null,
      surfaceInstanceId: null,
      selectionRevision: 2
    }

    expect(manager.selectManualSurface(host.asWebContents(), clearInput)).toEqual(
      expect.objectContaining({ status: 'applied', authoritativeRevision: 2, surfaceId: null })
    )
    expect(manager.selectManualSurface(host.asWebContents(), clearInput)).toEqual(
      expect.objectContaining({ status: 'noop', reason: 'selection_unchanged' })
    )
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: false, surfaceId: 'clear-selection' })
    ])
    await manager.shutdown()
  })

  it('rejects stale A-B-A arrival order and keeps duplicate selection lease-idempotent', async () => {
    const { host, manager } = createHarness()
    attachGuest(manager, host, 2, 'ordered-a')
    attachGuest(manager, host, 3, 'ordered-b')
    const firstA = selectBoundSurface(manager, host, 'ordered-a', 1)
    const tokenA = firstA.surfaceInstanceId
    const tokenB = probeSurfaceInstance(manager, host, 'ordered-b', 2)
    if (!tokenA) throw new Error('bound A token missing')

    // These intents were created A1 -> B2 -> A3, but A3 reaches Main before delayed B2.
    const latestA = manager.selectManualSurface(host.asWebContents(), {
      schemaVersion: 1,
      surfaceId: 'ordered-a',
      surfaceInstanceId: tokenA,
      selectionRevision: 3
    })
    expect(latestA).toEqual(
      expect.objectContaining({
        status: 'noop',
        reason: 'selection_unchanged',
        authoritativeRevision: 3
      })
    )
    expect(
      manager.selectManualSurface(host.asWebContents(), {
        schemaVersion: 1,
        surfaceId: 'ordered-b',
        surfaceInstanceId: tokenB,
        selectionRevision: 2
      })
    ).toEqual(
      expect.objectContaining({
        status: 'stale',
        reason: 'stale_revision',
        authoritativeRevision: 3
      })
    )
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: true, surfaceId: 'ordered-a' }),
      expect.objectContaining({ isActive: false, surfaceId: 'ordered-b' })
    ])

    await manager.getBrowserContext()
    const lease = await manager.beginToolSurfaceLease()
    // A3 advanced Renderer ordering but did not mutate the same Main selection a second time.
    expect(lease.selectionRevision).toBe(1)
    lease.finish()
    await manager.shutdown()
  })

  it('binds selection to one exact surface incarnation across StrictMode-style ABA remounts', async () => {
    const { commands, host, manager } = createHarness()
    const first = attachGuest(manager, host, 2, 'strict-aba')
    const oldInstanceId = probeSurfaceInstance(manager, host, 'strict-aba', 1)
    first.destroy()
    const replacement = attachGuest(manager, host, 3, 'strict-aba')
    await new Promise<void>((resolve) => setImmediate(resolve))
    expect(
      commands.filter(
        (command) => command.kind === 'closeSurface' && command.surfaceId === 'strict-aba'
      )
    ).toHaveLength(0)

    expect(
      manager.selectManualSurface(host.asWebContents(), {
        schemaVersion: 1,
        surfaceId: 'strict-aba',
        surfaceInstanceId: oldInstanceId,
        selectionRevision: 1
      })
    ).toEqual(
      expect.objectContaining({
        status: 'stale',
        reason: 'instance_mismatch',
        authoritativeRevision: 0
      })
    )
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ generation: 2, isActive: false, surfaceId: 'strict-aba' })
    ])
    expect(selectBoundSurface(manager, host, 'strict-aba', 2)).toEqual(
      expect.objectContaining({ status: 'applied', authoritativeRevision: 2 })
    )

    replacement.destroy()
    await manager.shutdown()
  })

  it('does not mutate selection for a closing surface and never guesses a sidebar fallback', async () => {
    const { commands, host, manager } = createHarness()
    const first = attachGuest(manager, host, 2, 'closing-one')
    const second = attachGuest(manager, host, 3, 'closing-two')
    const firstSelection = selectBoundSurface(manager, host, 'closing-one', 1)
    if (!firstSelection.surfaceInstanceId) throw new Error('bound closing token missing')

    const closing = manager.closeSurface('closing-one')
    await vi.waitFor(() =>
      expect(commands.at(-1)).toEqual(expect.objectContaining({ kind: 'closeSurface' }))
    )
    expect(
      manager.selectManualSurface(host.asWebContents(), {
        schemaVersion: 1,
        surfaceId: 'closing-one',
        surfaceInstanceId: firstSelection.surfaceInstanceId,
        selectionRevision: 2
      })
    ).toEqual(expect.objectContaining({ status: 'stale', reason: 'surface_closing' }))
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: false, surfaceId: 'closing-two' })
    ])

    first.destroy()
    await expect(closing).resolves.toBeUndefined()
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: false, surfaceId: 'closing-two' })
    ])
    expect(selectBoundSurface(manager, host, 'closing-two', 3)).toEqual(
      expect.objectContaining({ status: 'applied', surfaceId: 'closing-two' })
    )

    second.destroy()
    await manager.shutdown()
  })

  it('hard-rejects a different host even with a well-formed surface selection', async () => {
    const { host, manager } = createHarness()
    const otherHost = new FakeWebContents(99, 'window', 'file:///other-renderer.html')
    attachGuest(manager, host, 2, 'host-bound')
    const instanceId = probeSurfaceInstance(manager, host, 'host-bound', 1)
    selectBoundSurface(manager, host, 'host-bound', 1)

    expect(() =>
      manager.selectManualSurface(otherHost.asWebContents(), {
        schemaVersion: 1,
        surfaceId: 'host-bound',
        surfaceInstanceId: instanceId,
        selectionRevision: 1
      })
    ).toThrow('browser.surface_unavailable')
    expect(() =>
      manager.selectManualSurface(otherHost.asWebContents(), {
        schemaVersion: 1,
        surfaceId: null,
        surfaceInstanceId: null,
        selectionRevision: 2
      })
    ).toThrow('browser.surface_unavailable')
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: true, surfaceId: 'host-bound' })
    ])
    await manager.shutdown()
  })

  it('keeps network preflight on the leased tab when the user selects another tab mid-call', async () => {
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
    const { commands, host, manager } = createHarness({ networkGuard })
    const first = attachGuest(manager, host, 2, 'lease-one')
    const second = attachGuest(manager, host, 3, 'lease-two')
    selectBoundSurface(manager, host, 'lease-one', 1)
    await manager.getBrowserContext()
    const toolLease = await manager.beginToolSurfaceLease()
    selectBoundSurface(manager, host, 'lease-two', 2)

    await expect(manager.beginNetworkOperation(RISK_OPERATION_INPUT)).resolves.toBe(lease)
    expect(networkGuard.beginOperation).toHaveBeenCalledWith(
      first.asWebContents(),
      RISK_OPERATION_INPUT
    )
    const resize = toolLease.resizeSurface({ height: 640, width: 960 })
    const resizeCommand = commands.at(-1)
    if (!resizeCommand || resizeCommand.kind !== 'resizeSurface') {
      throw new Error('resize request missing')
    }
    expect(resizeCommand.surfaceId).toBe('lease-one')
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: resizeCommand.requestId,
      surfaceId: 'lease-one',
      viewport: { height: 640, width: 960 }
    })
    await expect(resize).resolves.toEqual({ height: 640, width: 960 })
    await expect(toolLease.printToPdf()).resolves.toEqual(
      Uint8Array.from(Buffer.from('%PDF-1.7\nfixture\n%%EOF\n'))
    )
    expect(first.printToPDF).toHaveBeenCalledWith({ printBackground: false })
    expect(second.printToPDF).not.toHaveBeenCalled()
    expect(manager.getActiveSurfaceIdentity()).toEqual({ generation: 1, surfaceId: 'lease-one' })
    toolLease.finish()
    await manager.shutdown()
  })

  it('rejects stale-index bringToFront when a preceding tab closes after index resolution', async () => {
    let groupTransport!: ConnectOverCDPTransport
    const { host, manager } = createHarness({
      connectOverCdp: async (transport) => {
        groupTransport = transport
        return createFakeBrowser(transport).browser
      }
    })
    const first = attachGuest(manager, host, 2, 'index-first')
    attachGuest(manager, host, 3, 'index-leased')
    attachGuest(manager, host, 4, 'index-wrong-after-close')
    selectBoundSurface(manager, host, 'index-leased', 1)
    await manager.getBrowserContext()
    const cdp = createCdpHarness(groupTransport)
    await cdp.send('Target.setAutoAttach', {
      autoAttach: true,
      flatten: true,
      waitForDebuggerOnStart: false
    })
    const rootSessions = cdp.events
      .filter(
        (event) => event.method === 'Target.attachedToTarget' && event.sessionId === undefined
      )
      .map((event) => (event.params as { sessionId: string }).sessionId)
    expect(rootSessions).toHaveLength(3)

    const lease = await manager.beginToolSurfaceLeaseByIndex(1)
    await expect(lease.resolveIndex()).resolves.toBe(1)
    first.destroy()
    await vi.waitFor(() => expect(manager.listSurfaces()).toHaveLength(2))

    // The previously resolved numeric index 1 now names the third page. The exact lease guard
    // rejects its bringToFront instead of silently committing Playwright's current tab to it.
    const wrongPage = await cdp.send('Page.bringToFront', undefined, rootSessions[2])
    expect(wrongPage.error).toEqual(expect.objectContaining({ code: -32_000 }))
    expect(manager.getActiveSurfaceIdentity()).toEqual({
      generation: 1,
      surfaceId: 'index-leased'
    })
    await expect(lease.resolveIndex()).resolves.toBe(0)
    lease.finish()
    await manager.shutdown()
  })

  it('does not create a visible tab for an existing-only Tool lease', async () => {
    const { commands, manager } = createHarness()
    await expect(manager.beginExistingToolSurfaceLease()).resolves.toBeNull()
    expect(commands).toHaveLength(0)
    await manager.shutdown()
  })

  it('rolls Broker registration back when the network guard rejects a guest', async () => {
    const networkGuard = {
      deactivateAutomation: vi.fn(),
      registerGuest: vi.fn(() => {
        throw new Error('fixture registration failure')
      }),
      shutdown: vi.fn(async () => undefined)
    } as unknown as BrowserNetworkGuard
    const { broker, host, manager } = createHarness({ networkGuard })
    expect(() => attachGuest(manager, host)).toThrow(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
    await manager.shutdown()
  })

  it('reconciles a guest registered while the group connection is still being published', async () => {
    let releaseConnect!: () => void
    const connectBarrier = new Promise<void>((resolve) => {
      releaseConnect = resolve
    })
    const { broker, commands, host, manager } = createHarness({
      connectOverCdp: async (transport) => {
        await connectBarrier
        return createFakeBrowser(transport).browser
      }
    })
    const context = manager.getBrowserContext()
    const ensure = commands[0]
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await vi.waitFor(() => expect(broker.snapshot().activeConnections).toBe(1))
    attachGuest(manager, host, 3, 'registered-during-connect')
    releaseConnect()

    await context
    await vi.waitFor(() => expect(broker.snapshot().activeConnections).toBe(2))
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual([
      ensure.surfaceId,
      'registered-during-connect'
    ])
    await manager.shutdown()
  })

  it('serializes dynamic group admissions in stable surface creation order', async () => {
    const { broker, commands, host, manager } = createHarness()
    const context = manager.getBrowserContext()
    const ensure = commands[0]
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context

    const originalAdd = broker.addSurfaceToGroup.bind(broker)
    const starts: string[] = []
    let releaseFirst!: () => void
    const firstBarrier = new Promise<void>((resolve) => {
      releaseFirst = resolve
    })
    vi.spyOn(broker, 'addSurfaceToGroup').mockImplementation(async (group, identity) => {
      starts.push(identity.surfaceId)
      if (identity.surfaceId === 'dynamic-one') await firstBarrier
      await originalAdd(group, identity)
    })
    attachGuest(manager, host, 3, 'dynamic-one')
    attachGuest(manager, host, 4, 'dynamic-two')
    await vi.waitFor(() => expect(starts).toEqual(['dynamic-one']))
    releaseFirst()
    await vi.waitFor(() => expect(starts).toEqual(['dynamic-one', 'dynamic-two']))
    await vi.waitFor(() => expect(broker.snapshot().activeConnections).toBe(3))
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual([
      ensure.surfaceId,
      'dynamic-one',
      'dynamic-two'
    ])
    await manager.shutdown()
  })

  it('reserves Target.createTarget order before a later manual surface can attach', async () => {
    const surfaceIds = ['ordered-initial', 'ordered-target']
    let groupTransport!: ConnectOverCDPTransport
    const { broker, commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-ordered-surface',
      connectOverCdp: async (transport) => {
        groupTransport = transport
        return createFakeBrowser(transport).browser
      }
    })
    const context = manager.getBrowserContext()
    const ensure = commands[0]
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context
    const cdp = createCdpHarness(groupTransport)
    await cdp.send('Target.setAutoAttach', {
      autoAttach: true,
      flatten: true,
      waitForDebuggerOnStart: false
    })

    const creationEvents: string[] = []
    const authority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async ({ guest }) => {
        creationEvents.push('claim')
        expect(guest.getURL()).toBe('about:blank')
      }),
      finish: vi.fn()
    }
    const finishIntent = manager.beginTargetCreationIntent('interactive', authority)

    const originalAdd = broker.addSurfaceToGroup.bind(broker)
    const starts: string[] = []
    vi.spyOn(broker, 'addSurfaceToGroup').mockImplementation(async (group, identity) => {
      starts.push(identity.surfaceId)
      creationEvents.push(`attach:${identity.surfaceId}`)
      await originalAdd(group, identity)
    })
    const target = cdp.send('Target.createTarget', { url: 'https://fixture.example.test/new' })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'createSurface', surfaceId: 'ordered-target' })
      )
    )
    const create = commands.find(
      (command) => command.kind === 'createSurface' && command.surfaceId === 'ordered-target'
    )
    if (!create || create.kind !== 'createSurface') throw new Error('create command missing')
    const targetGuest = attachGuest(manager, host, 3, create.surfaceId)
    let releaseLoad!: () => void
    let markLoadStarted!: () => void
    const loadStarted = new Promise<void>((resolve) => {
      markLoadStarted = resolve
    })
    const loadBarrier = new Promise<void>((resolve) => {
      releaseLoad = resolve
    })
    vi.spyOn(targetGuest, 'loadURL').mockImplementation(async (url) => {
      markLoadStarted()
      await loadBarrier
      targetGuest.url = url
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await loadStarted

    attachGuest(manager, host, 4, 'ordered-manual-later')
    await new Promise((resolve) => setImmediate(resolve))
    expect(starts).toEqual([])
    releaseLoad()
    await vi.waitFor(() => expect(starts).toEqual(['ordered-target', 'ordered-manual-later']))
    expect(creationEvents.indexOf('claim')).toBeLessThan(
      creationEvents.indexOf('attach:ordered-target')
    )
    expect(authority.claim).toHaveBeenCalledOnce()
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'selectSurface', surfaceId: 'ordered-target' })
      )
    )
    const select = commands.find(
      (command) => command.kind === 'selectSurface' && command.surfaceId === 'ordered-target'
    )
    if (!select || select.kind !== 'selectSurface') throw new Error('select command missing')
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: select.requestId,
      surfaceId: select.surfaceId
    })
    await expect(target).resolves.toEqual(
      expect.objectContaining({ result: expect.objectContaining({ targetId: expect.any(String) }) })
    )
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual([
      'ordered-initial',
      'ordered-target',
      'ordered-manual-later'
    ])
    finishIntent()
    expect(authority.finish).toHaveBeenCalledOnce()
    await manager.shutdown()
  })

  it('rolls back a hidden Target.createTarget surface when its second-stage load fails', async () => {
    const surfaceIds = ['rollback-initial', 'rollback-hidden']
    let groupTransport!: ConnectOverCDPTransport
    const { broker, commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-rollback-surface',
      connectOverCdp: async (transport) => {
        groupTransport = transport
        return createFakeBrowser(transport).browser
      }
    })
    const context = manager.getBrowserContext()
    const ensure = commands[0]
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context
    const cdp = createCdpHarness(groupTransport)
    await cdp.send('Target.setAutoAttach', {
      autoAttach: true,
      flatten: true,
      waitForDebuggerOnStart: false
    })

    const finishIntent = manager.beginTargetCreationIntent('interactive', {
      action: 'new',
      claim: vi.fn(async () => undefined),
      finish: vi.fn()
    })
    const target = cdp.send('Target.createTarget', { url: 'https://fixture.example.test/fail' })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'createSurface', surfaceId: 'rollback-hidden' })
      )
    )
    const create = commands.find(
      (command) => command.kind === 'createSurface' && command.surfaceId === 'rollback-hidden'
    )
    if (!create || create.kind !== 'createSurface') throw new Error('create command missing')
    const hidden = attachGuest(manager, host, 3, create.surfaceId)
    vi.spyOn(hidden, 'loadURL').mockRejectedValueOnce(new Error('fixture second-stage failure'))
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'closeSurface', surfaceId: 'rollback-hidden' })
      )
    )
    hidden.destroy()
    await expect(target).resolves.toEqual(
      expect.objectContaining({ error: expect.objectContaining({ code: -32_000 }) })
    )
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual(['rollback-initial'])
    expect(broker.snapshot()).toEqual({
      activeConnections: 1,
      claimedSurfaces: 1,
      registeredGuests: 1
    })
    finishIntent()
    await manager.shutdown()
  })

  it('does not let an old target-creation finish clear a newer intent record', async () => {
    const { manager } = createHarness()
    const oldAuthority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const finishOld = manager.beginTargetCreationIntent('interactive', oldAuthority)
    await manager.detachAutomation()
    expect(oldAuthority.finish).toHaveBeenCalledOnce()

    const newAuthority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const finishNew = manager.beginTargetCreationIntent('interactive', newAuthority)
    finishOld()
    expect(newAuthority.finish).not.toHaveBeenCalled()
    expect(() =>
      manager.beginTargetCreationIntent('interactive', {
        action: 'new',
        claim: vi.fn(async () => undefined),
        finish: vi.fn()
      })
    ).toThrow(expect.objectContaining({ code: 'browser.surface_unavailable' }))

    finishNew()
    expect(newAuthority.finish).toHaveBeenCalledOnce()
    await manager.shutdown()
  })

  it('rolls back a pending Target.createTarget when its exact intent is cancelled', async () => {
    const surfaceIds = ['cancel-intent-initial', 'cancel-intent-created']
    let groupTransport!: ConnectOverCDPTransport
    const { broker, commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-cancel-intent-surface',
      connectOverCdp: async (transport) => {
        groupTransport = transport
        return createFakeBrowser(transport).browser
      }
    })
    const context = manager.getBrowserContext()
    const ensure = commands[0]
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context
    const cdp = createCdpHarness(groupTransport)
    await cdp.send('Target.setAutoAttach', {
      autoAttach: true,
      flatten: true,
      waitForDebuggerOnStart: false
    })
    const events: string[] = []
    const authority: BrowserTargetCreationAuthority = {
      action: 'new',
      claim: vi.fn(async () => {
        events.push('claim')
      }),
      finish: vi.fn()
    }
    const finishIntent = manager.beginTargetCreationIntent('interactive', authority)
    const target = cdp.send('Target.createTarget', { url: 'https://fixture.example.test/cancel' })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'createSurface', surfaceId: 'cancel-intent-created' })
      )
    )
    finishIntent()
    const create = commands.find(
      (command) => command.kind === 'createSurface' && command.surfaceId === 'cancel-intent-created'
    )
    if (!create || create.kind !== 'createSurface') throw new Error('create command missing')
    const hidden = attachGuest(manager, host, 3, create.surfaceId)
    vi.spyOn(hidden, 'loadURL').mockImplementation(async (url) => {
      events.push(`load:${url}`)
      hidden.url = url
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'closeSurface', surfaceId: 'cancel-intent-created' })
      )
    )
    hidden.destroy()

    await expect(target).resolves.toEqual(
      expect.objectContaining({ error: expect.objectContaining({ code: -32_000 }) })
    )
    expect(events).toEqual(['load:about:blank'])
    expect(authority.claim).not.toHaveBeenCalled()
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toEqual([
      'cancel-intent-initial'
    ])
    expect(broker.snapshot()).toEqual({
      activeConnections: 1,
      claimedSurfaces: 1,
      registeredGuests: 1
    })
    await manager.shutdown()
  })

  it('keeps a healthy manual surface when detach invalidates its queued admission', async () => {
    const { broker, commands, host, manager } = createHarness()
    const context = manager.getBrowserContext()
    const ensure = commands[0]
    if (!ensure || ensure.kind !== 'ensureAttached') throw new Error('ensure command missing')
    attachGuest(manager, host, 2, ensure.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context
    const originalAdd = broker.addSurfaceToGroup.bind(broker)
    let releaseAdmission!: () => void
    let markAdmissionStarted!: () => void
    const admissionStarted = new Promise<void>((resolve) => {
      markAdmissionStarted = resolve
    })
    const admissionBarrier = new Promise<void>((resolve) => {
      releaseAdmission = resolve
    })
    vi.spyOn(broker, 'addSurfaceToGroup').mockImplementation(async (group, identity) => {
      if (identity.surfaceId === 'detach-manual') {
        markAdmissionStarted()
        await admissionBarrier
      }
      await originalAdd(group, identity)
    })
    const manual = attachGuest(manager, host, 3, 'detach-manual')
    await admissionStarted
    const detach = manager.detachAutomation()
    releaseAdmission()
    await detach
    await new Promise((resolve) => setImmediate(resolve))

    expect(manual.destroyed).toBe(false)
    expect(manager.listSurfaces().map((surface) => surface.surfaceId)).toContain('detach-manual')
    expect(
      commands.some(
        (command) => command.kind === 'closeSurface' && command.surfaceId === 'detach-manual'
      )
    ).toBe(false)
    await manager.shutdown()
  })

  it('closes the exact newly created surface when its initial URL load fails', async () => {
    const { broker, commands, host, manager } = createHarness({
      createSurfaceId: () => 'failed-load-surface'
    })
    const creation = manager.createSurface({ url: 'https://fixture.example.test/fail' })
    const create = commands[0]
    if (!create || create.kind !== 'createSurface') throw new Error('create command missing')
    const guest = attachGuest(manager, host, 2, create.surfaceId)
    vi.spyOn(guest, 'loadURL').mockRejectedValueOnce(new Error('fixture load failure'))
    const rejection = expect(creation).rejects.toEqual(
      expect.objectContaining({ code: 'browser.surface_unavailable' })
    )
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await vi.waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ kind: 'closeSurface', surfaceId: create.surfaceId })
      )
    )
    guest.destroy()
    await rejection
    expect(manager.listSurfaces()).toHaveLength(0)
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
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
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensure.requestId,
      surfaceId: ensure.surfaceId
    })
    await context

    const events: string[] = []
    const authority: BrowserTargetCreationAuthority = {
      action: 'popup',
      claim: vi.fn(async () => {
        events.push('claim')
      }),
      finish: vi.fn()
    }
    const popup = manager.handlePopup({
      authority,
      guest: opener.asWebContents(),
      url: 'http://127.0.0.1/popup'
    })
    const create = commands.at(-1)
    if (!create || create.kind !== 'createSurface') throw new Error('popup command missing')
    expect(create.activate).toBe(false)
    const child = attachGuest(manager, host, 3, create.surfaceId)
    vi.spyOn(child, 'loadURL').mockImplementation(async (url) => {
      events.push(`load:${url}`)
      child.url = url
    })
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await popup

    expect(events).toEqual(['load:about:blank', 'claim', 'load:http://127.0.0.1/popup'])
    expect(authority.claim).toHaveBeenCalledWith({
      generation: 1,
      guest: child.asWebContents(),
      surfaceId: 'popup-child'
    })

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

  it('keeps a manual popup as a new background tab before automation is attached', async () => {
    const surfaceIds = ['manual-popup-opener', 'manual-popup-child']
    const { commands, host, manager } = createHarness({
      createSurfaceId: () => surfaceIds.shift() ?? 'unexpected-manual-popup'
    })
    const ensure = manager.ensureActiveSurface()
    const ensureCommand = commands.at(-1)
    if (!ensureCommand || ensureCommand.kind !== 'ensureAttached') {
      throw new Error('ensure command missing')
    }
    const opener = attachGuest(manager, host, 2, ensureCommand.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: ensureCommand.requestId,
      surfaceId: ensureCommand.surfaceId
    })
    await ensure
    opener.url = 'https://fixture.example.test/opener'
    const openerLoad = vi.spyOn(opener, 'loadURL')

    const popup = manager.handlePopup({
      guest: opener.asWebContents(),
      url: 'https://fixture.example.test/popup'
    })
    const create = commands.at(-1)
    if (!create || create.kind !== 'createSurface') throw new Error('popup command missing')
    expect(create.activate).toBe(false)
    const child = attachGuest(manager, host, 3, create.surfaceId)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: create.requestId,
      surfaceId: create.surfaceId
    })
    await popup

    expect(openerLoad).not.toHaveBeenCalled()
    expect(opener.getURL()).toBe('https://fixture.example.test/opener')
    expect(child.getURL()).toBe('https://fixture.example.test/popup')
    expect(manager.listSurfaces()).toEqual([
      expect.objectContaining({ isActive: true, surfaceId: 'manual-popup-opener' }),
      expect.objectContaining({ isActive: false, surfaceId: 'manual-popup-child' })
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
    attachSurface(manager, host.asWebContents(), {
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
    const releaseSurfaceResources = vi.fn(async () => undefined)
    const { commands, host, manager } = createHarness({ releaseSurfaceResources })
    const context = manager.getBrowserContext()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const otherHost = new FakeWebContents(9, 'window', 'file:///other.html')

    expect(() =>
      attachSurface(manager, otherHost.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: SURFACE_ID
      })
    ).toThrow(expect.objectContaining({ code: 'browser.surface_unavailable' }))

    const guest = attachGuest(manager, host)
    attachSurface(manager, host.asWebContents(), {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    guest.destroy()
    await expect(context).rejects.toEqual(
      expect.objectContaining({ code: 'browser.target_closed' })
    )
    await vi.waitFor(() =>
      expect(commands.at(-1)).toEqual(
        expect.objectContaining({ kind: 'closeSurface', surfaceId: SURFACE_ID })
      )
    )
    await vi.waitFor(() =>
      expect(releaseSurfaceResources).toHaveBeenCalledWith({
        surfaceId: SURFACE_ID,
        generation: 1
      })
    )
    await manager.shutdown()
  })

  it('detaches automation without destroying the manually usable guest', async () => {
    const { broker, browsers, commands, host, manager } = createHarness()
    const contextPromise = manager.getBrowserContext()
    const command = commands[0]
    if (!command || command.kind !== 'ensureAttached') throw new Error('ensure command missing')
    const guest = attachGuest(manager, host)
    attachSurface(manager, host.asWebContents(), {
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
    expect(broker.snapshot().claimedSurfaces).toBe(1)
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
    expect(
      attachSurface(manager, host.asWebContents(), {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: SURFACE_ID
      })
    ).toEqual(
      expect.objectContaining({
        accepted: false,
        status: 'stale',
        reason: 'request_cancelled',
        requestId: command.requestId
      })
    )
    await manager.shutdown()
  })

  it('revokes an in-flight CDP connection before it can become active', async () => {
    let finishConnect!: (browser: Browser) => void
    let transport: ConnectOverCDPTransport | undefined
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
    attachSurface(manager, host.asWebContents(), {
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
    let transport: ConnectOverCDPTransport | undefined
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
    attachSurface(manager, host.asWebContents(), {
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
    expect(guest.isDestroyed()).toBe(true)
    expect(guest.close).toHaveBeenCalledWith({ waitForBeforeUnload: false })
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
