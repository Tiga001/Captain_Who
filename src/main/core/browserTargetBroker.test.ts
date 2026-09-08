import { EventEmitter } from 'node:events'
import type { Debugger, Session, WebContents } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { BrowserTargetBroker, BrowserTargetBrokerError } from '../browser/BrowserTargetBroker'
import type { ElectronGuestCdpTransport } from '../browser/ElectronGuestCdpTransport'

const PARTITION = 'persist:mycopilot-browser'
const EXPECTED_SESSION = {} as Session
const OTHER_SESSION = {} as Session

class FakeDebugger extends EventEmitter {
  readonly commands: Array<{ method: string; params?: unknown; sessionId?: string }> = []
  readonly focusedSessions = new Set<string>()
  readonly mainWorldFocusSpoofs = new Set<string>()
  readonly frameSessions = new Map<string, string>()
  readonly focusContextSessions = new Map<number, string>()
  private readonly methodResponses = new Map<string, unknown[]>()
  private attached = false
  private nextFocusContextId = 7
  private targetInfoGate?: Promise<void>

  attach = vi.fn(() => {
    if (this.attached) throw new Error('already attached')
    this.attached = true
  })

  detach = vi.fn(() => {
    this.attached = false
  })

  isAttached = vi.fn(() => this.attached)

  sendCommand = vi.fn(
    async (method: string, params?: unknown, sessionId?: string): Promise<unknown> => {
      this.commands.push({ method, params, sessionId })
      const queuedResponses = this.methodResponses.get(method)
      if (queuedResponses?.length) return queuedResponses.shift()
      if (method === 'Target.getTargetInfo') await this.targetInfoGate
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
      if (
        method === 'Runtime.evaluate' &&
        typeof sessionId === 'string' &&
        (params as { expression?: unknown } | undefined)?.expression ===
          'document.hasFocus() === true'
      ) {
        return {
          result: {
            value: this.focusedSessions.has(sessionId) || this.mainWorldFocusSpoofs.has(sessionId)
          }
        }
      }
      if (
        method === 'Page.createIsolatedWorld' &&
        typeof (params as { worldName?: unknown } | undefined)?.worldName === 'string' &&
        String((params as { worldName: string }).worldName).startsWith('mycopilot-focus-')
      ) {
        const frameId = (params as { frameId?: unknown }).frameId
        const frameSession =
          typeof frameId === 'string' ? this.frameSessions.get(frameId) : undefined
        if (!frameSession) return {}
        const executionContextId = this.nextFocusContextId++
        this.focusContextSessions.set(executionContextId, frameSession)
        return { executionContextId }
      }
      if (
        method === 'Runtime.evaluate' &&
        (params as { expression?: unknown; contextId?: unknown } | undefined)?.expression ===
          'Document.prototype.hasFocus.call(document) === true'
      ) {
        const contextId = (params as { contextId?: unknown }).contextId
        const frameSession =
          typeof contextId === 'number' ? this.focusContextSessions.get(contextId) : undefined
        return { result: { value: frameSession ? this.focusedSessions.has(frameSession) : false } }
      }
      return {}
    }
  )

  respondOnce(method: string, response: unknown): void {
    const queuedResponses = this.methodResponses.get(method) ?? []
    queuedResponses.push(response)
    this.methodResponses.set(method, queuedResponses)
  }

  holdTargetInfo(): () => void {
    let resume!: () => void
    this.targetInfoGate = new Promise<void>((resolve) => {
      resume = resolve
    })
    return resume
  }
}

class FakeWebContents extends EventEmitter {
  readonly debugger = new FakeDebugger() as unknown as Debugger
  readonly focus = vi.fn()
  readonly insertedTexts: string[] = []
  destroyed = false
  private insertTextFailure = false
  private insertTextGate?: Promise<void>

  constructor(
    readonly id: number,
    public kind: ReturnType<WebContents['getType']>,
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
    return `http://127.0.0.1/fixture-${this.id}`
  }

  isDestroyed(): boolean {
    return this.destroyed
  }

  async insertText(text: string): Promise<void> {
    this.insertedTexts.push(text)
    await this.insertTextGate
    if (this.insertTextFailure) throw new Error('fixture insertText failure')
  }

  holdInsertText(): () => void {
    let resume!: () => void
    this.insertTextGate = new Promise<void>((resolve) => {
      resume = resolve
    })
    return resume
  }

  rejectInsertText(): void {
    this.insertTextFailure = true
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

class TransportHarness {
  readonly events: Array<Record<string, unknown>> = []
  private nextId = 1
  private readonly pending = new Map<number, (message: Record<string, unknown>) => void>()

  constructor(readonly transport: ElectronGuestCdpTransport) {
    transport.onmessage = (message) => {
      const record = message as Record<string, unknown>
      this.events.push(record)
      if (typeof record.id !== 'number') return
      const resolve = this.pending.get(record.id)
      this.pending.delete(record.id)
      resolve?.(record)
    }
  }

  send(method: string, options: { params?: object; sessionId?: string } = {}) {
    const id = this.nextId++
    return new Promise<Record<string, unknown>>((resolve) => {
      this.pending.set(id, resolve)
      this.transport.send({ id, method, ...options })
    })
  }
}

function createRegisteredGuest(broker: BrowserTargetBroker, id = 2, surfaceId = 'browser-one') {
  const host = new FakeWebContents(1, 'window')
  const guest = new FakeWebContents(id, 'webview', host.asWebContents())
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
  return { guest, host, surfaceId }
}

function createBroker(): BrowserTargetBroker {
  return new BrowserTargetBroker(PARTITION, EXPECTED_SESSION)
}

async function createConnectedTransportHarness() {
  const broker = createBroker()
  const registered = createRegisteredGuest(broker)
  const transport = await broker.connect(registered.surfaceId)
  const harness = new TransportHarness(transport)
  await harness.send('Target.setAutoAttach', {
    params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: true }
  })
  const attached = harness.events.find((event) => event.method === 'Target.attachedToTarget')
  const params = attached?.params as Record<string, unknown> | undefined
  if (typeof params?.sessionId !== 'string') throw new Error('synthetic session missing')
  return { broker, harness, sessionId: params.sessionId, transport, ...registered }
}

function syntheticTargetIdentity(harness: TransportHarness): {
  browserContextId: string
  targetId: string
} {
  const attached = harness.events.find((event) => event.method === 'Target.attachedToTarget')
  const params = attached?.params as Record<string, unknown> | undefined
  const targetInfo = params?.targetInfo as Record<string, unknown> | undefined
  if (typeof targetInfo?.browserContextId !== 'string' || typeof targetInfo.targetId !== 'string') {
    throw new Error('synthetic target identity missing')
  }
  return {
    browserContextId: targetInfo.browserContextId,
    targetId: targetInfo.targetId
  }
}

function emitIframeAttached(
  guest: FakeWebContents,
  identity: { browserContextId: string; targetId: string },
  options: {
    childSessionId: string
    childTargetId: string
    parentSessionId?: string
    parentTargetId?: string
    waitingForDebugger?: boolean
  }
): void {
  ;(guest.debugger as unknown as FakeDebugger).frameSessions.set(
    options.childTargetId,
    options.childSessionId
  )
  ;(guest.debugger as unknown as FakeDebugger).emit(
    'message',
    {},
    'Target.attachedToTarget',
    {
      sessionId: options.childSessionId,
      targetInfo: {
        attached: true,
        browserContextId: identity.browserContextId,
        parentFrameId: options.parentTargetId ?? identity.targetId,
        targetId: options.childTargetId,
        type: 'iframe',
        url: 'http://localhost/frame'
      },
      waitingForDebugger: options.waitingForDebugger ?? false
    },
    options.parentSessionId ?? ''
  )
  if (options.waitingForDebugger !== true) {
    ;(guest.debugger as unknown as FakeDebugger).emit(
      'message',
      {},
      'Page.frameNavigated',
      { frame: { id: options.childTargetId } },
      options.childSessionId
    )
  }
}

function emitWorkerAttached(
  guest: FakeWebContents,
  identity: { browserContextId: string; targetId: string },
  options: {
    childSessionId: string
    childTargetId: string
    includeOwnerFrameId?: boolean
    ownerFrameId?: string
    parentSessionId?: string
  }
): void {
  ;(guest.debugger as unknown as FakeDebugger).emit(
    'message',
    {},
    'Target.attachedToTarget',
    {
      sessionId: options.childSessionId,
      targetInfo: {
        attached: true,
        browserContextId: identity.browserContextId,
        ...(options.includeOwnerFrameId === false
          ? {}
          : { parentFrameId: options.ownerFrameId ?? identity.targetId }),
        targetId: options.childTargetId,
        type: 'worker',
        url: 'http://localhost/worker.js'
      },
      waitingForDebugger: false
    },
    options.parentSessionId ?? ''
  )
}

describe('BrowserTargetBroker', () => {
  it('admits only an exact Main-created popup of an already claimed managed opener', async () => {
    const broker = createBroker()
    const { guest: opener, host } = createRegisteredGuest(broker)
    const popup = new FakeWebContents(3, 'window')
    const registration = {
      guest: popup.asWebContents(),
      host: host.asWebContents(),
      openerGuest: opener.asWebContents(),
      partition: PARTITION
    }
    broker.registerManagedPopup(registration)
    broker.registerManagedPopup(registration)
    broker.claimSurface({
      generation: 1,
      guestWebContentsId: popup.id,
      host: host.asWebContents(),
      surfaceId: 'popup-one'
    })
    const transport = await broker.connect('popup-one')
    expect(transport).toBeDefined()
    expect(broker.snapshot()).toEqual({
      activeConnections: 1,
      registeredGuests: 2,
      claimedSurfaces: 2
    })
    expect(() => broker.registerManagedGuest(registration)).toThrow(
      expect.objectContaining({ code: 'invalid_guest' })
    )
    broker.dispose()
  })

  it('rejects unclaimed, substituted, or foreign opener identities for native popups', () => {
    const broker = createBroker()
    const { guest: opener, host } = createRegisteredGuest(broker)
    const popup = new FakeWebContents(5, 'window')
    const unclaimed = new FakeWebContents(4, 'webview', host.asWebContents())
    broker.registerManagedGuest({
      guest: unclaimed.asWebContents(),
      host: host.asWebContents(),
      partition: PARTITION
    })
    const substituted = new FakeWebContents(opener.id, 'webview', host.asWebContents())
    for (const openerGuest of [
      unclaimed,
      substituted,
      new FakeWebContents(99, 'webview', host.asWebContents())
    ]) {
      expect(() =>
        broker.registerManagedPopup({
          guest: popup.asWebContents(),
          host: host.asWebContents(),
          openerGuest: openerGuest.asWebContents(),
          partition: PARTITION
        })
      ).toThrow(expect.objectContaining({ code: 'invalid_guest' }))
    }
    expect(() =>
      broker.registerManagedPopup({
        guest: popup.asWebContents(),
        host: new FakeWebContents(10, 'window').asWebContents(),
        openerGuest: opener.asWebContents(),
        partition: PARTITION
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_guest' }))
    broker.dispose()
  })

  it('rejects wrong-session popups and cannot register the host as its own managed popup', () => {
    const broker = createBroker()
    const { guest: opener, host } = createRegisteredGuest(broker)
    const foreignPopup = new FakeWebContents(3, 'window', null, OTHER_SESSION)
    expect(() =>
      broker.registerManagedPopup({
        guest: foreignPopup.asWebContents(),
        host: host.asWebContents(),
        openerGuest: opener.asWebContents(),
        partition: PARTITION
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_partition' }))
    expect(() =>
      broker.registerManagedPopup({
        guest: host.asWebContents(),
        host: host.asWebContents(),
        openerGuest: opener.asWebContents(),
        partition: PARTITION
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_guest' }))
    broker.dispose()
  })

  it.each(['session', 'kind'] as const)(
    'revalidates a native popup when its %s identity drifts',
    async (drift) => {
      const broker = createBroker()
      const { guest: opener, host } = createRegisteredGuest(broker)
      const popup = new FakeWebContents(3, 'window')
      broker.registerManagedPopup({
        guest: popup.asWebContents(),
        host: host.asWebContents(),
        openerGuest: opener.asWebContents(),
        partition: PARTITION
      })
      broker.claimSurface({
        generation: 1,
        guestWebContentsId: popup.id,
        host: host.asWebContents(),
        surfaceId: 'popup-one'
      })
      if (drift === 'session') popup.session = OTHER_SESSION
      else popup.kind = 'webview'
      await expect(broker.connect('popup-one')).rejects.toMatchObject({
        code: drift === 'session' ? 'invalid_partition' : 'invalid_guest'
      })
      expect(broker.snapshot()).toEqual({
        activeConnections: 0,
        registeredGuests: 1,
        claimedSurfaces: 1
      })
      expect(popup.listenerCount('destroyed')).toBe(0)
      broker.dispose()
    }
  )

  it('only registers a managed webview owned by the expected host and partition', () => {
    const broker = createBroker()
    const host = new FakeWebContents(1, 'window')
    const otherHost = new FakeWebContents(2, 'window')
    const mainRenderer = new FakeWebContents(3, 'window', host.asWebContents())
    const wrongHostGuest = new FakeWebContents(4, 'webview', otherHost.asWebContents())
    const validGuest = new FakeWebContents(5, 'webview', host.asWebContents())
    const wrongPartitionGuest = new FakeWebContents(
      6,
      'webview',
      host.asWebContents(),
      OTHER_SESSION
    )

    expect(() =>
      broker.registerManagedGuest({
        guest: mainRenderer.asWebContents(),
        host: host.asWebContents(),
        partition: PARTITION
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_guest' }))
    expect(() =>
      broker.registerManagedGuest({
        guest: wrongHostGuest.asWebContents(),
        host: host.asWebContents(),
        partition: PARTITION
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_guest' }))
    expect(() =>
      broker.registerManagedGuest({
        guest: validGuest.asWebContents(),
        host: host.asWebContents(),
        partition: 'persist:other'
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_partition' }))
    expect(() =>
      broker.registerManagedGuest({
        guest: wrongPartitionGuest.asWebContents(),
        host: host.asWebContents(),
        partition: PARTITION
      })
    ).toThrow(expect.objectContaining({ code: 'invalid_partition' }))

    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
  })

  it('binds surface and guest identities once and rejects cross-guest claims', () => {
    const broker = createBroker()
    const first = createRegisteredGuest(broker, 2, 'browser-one')
    const second = createRegisteredGuest(broker, 3, 'browser-two')

    expect(() =>
      broker.claimSurface({
        generation: 1,
        guestWebContentsId: second.guest.id,
        host: second.host.asWebContents(),
        surfaceId: first.surfaceId
      })
    ).toThrow(expect.objectContaining({ code: 'surface_conflict' }))
    expect(() =>
      broker.claimSurface({
        generation: 2,
        guestWebContentsId: first.guest.id,
        host: first.host.asWebContents(),
        surfaceId: 'browser-rebound'
      })
    ).toThrow(expect.objectContaining({ code: 'surface_conflict' }))
  })

  it('presents one synthetic target and rejects creation or access to another guest', async () => {
    const broker = createBroker()
    const first = createRegisteredGuest(broker, 2, 'browser-one')
    const second = createRegisteredGuest(broker, 3, 'browser-two')
    const transport = await broker.connect(first.surfaceId)
    const harness = new TransportHarness(transport)

    const autoAttach = await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: true }
    })
    expect(autoAttach.result).toEqual({})
    const attached = harness.events.find((event) => event.method === 'Target.attachedToTarget')
    const attachedParams = attached?.params as Record<string, unknown>
    const selectedSessionId = attachedParams.sessionId as string
    expect(selectedSessionId).toMatch(/^mycopilot-page-/)

    const targets = await harness.send('Target.getTargets')
    const targetInfos = (targets.result as { targetInfos: unknown[] }).targetInfos
    expect(targetInfos).toHaveLength(1)
    expect(JSON.stringify(targetInfos)).toContain('fixture-2')
    expect(JSON.stringify(targetInfos)).not.toContain('fixture-3')

    const sessionTargets = await harness.send('Target.getTargets', {
      sessionId: selectedSessionId
    })
    expect((sessionTargets.result as { targetInfos: unknown[] }).targetInfos).toEqual(targetInfos)

    const selectedTargetId = (attachedParams.targetInfo as Record<string, unknown>).targetId
    const otherTarget = await harness.send('Target.getTargetInfo', {
      params: { targetId: `${String(selectedTargetId)}-other` },
      sessionId: selectedSessionId
    })
    expect(otherTarget.error).toEqual(
      expect.objectContaining({ message: 'Target access is not permitted' })
    )

    const createTarget = await harness.send('Target.createTarget', {
      params: { url: 'http://127.0.0.1/second' }
    })
    expect(createTarget.error).toEqual(
      expect.objectContaining({ message: 'Browser-wide target access is not permitted' })
    )

    const runtimeEnable = await harness.send('Runtime.enable', { sessionId: selectedSessionId })
    expect(runtimeEnable.result).toEqual({})
    expect((first.guest.debugger as unknown as FakeDebugger).commands).toContainEqual({
      method: 'Runtime.enable',
      params: undefined,
      sessionId: undefined
    })
    expect((second.guest.debugger as unknown as FakeDebugger).commands).toEqual([])
  })

  it('bridges only bounded cookie commands for the exact managed BrowserContext', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const identity = syntheticTargetIdentity(harness)
    const cookie = {
      name: 'fixture',
      value: 'managed-cookie-value',
      url: 'http://127.0.0.1/fixture',
      httpOnly: true,
      secure: false,
      sameSite: 'Lax'
    }

    await expect(
      harness.send('Storage.setCookies', {
        params: { browserContextId: identity.browserContextId, cookies: [cookie] }
      })
    ).resolves.toMatchObject({ result: {} })
    await expect(
      harness.send('Storage.getCookies', {
        params: { browserContextId: identity.browserContextId }
      })
    ).resolves.toMatchObject({ result: { cookies: [] } })
    await expect(
      harness.send('Storage.clearCookies', {
        params: { browserContextId: identity.browserContextId }
      })
    ).resolves.toMatchObject({ result: {} })

    const commands = (guest.debugger as unknown as FakeDebugger).commands
    expect(commands).toContainEqual({
      method: 'Network.setCookies',
      params: { cookies: [cookie] },
      sessionId: undefined
    })
    expect(commands).toContainEqual({
      method: 'Network.getAllCookies',
      params: {},
      sessionId: undefined
    })
    expect(commands).toContainEqual({
      method: 'Network.clearBrowserCookies',
      params: {},
      sessionId: undefined
    })

    const wrongContext = await harness.send('Storage.getCookies', {
      params: { browserContextId: `${identity.browserContextId}-other` }
    })
    expect(wrongContext.error).toEqual(
      expect.objectContaining({
        message: 'Cookie command is not bound to the managed BrowserContext'
      })
    )
    const targetSession = await harness.send('Storage.getCookies', {
      params: { browserContextId: identity.browserContextId },
      sessionId
    })
    expect(targetSession.error).toEqual(
      expect.objectContaining({
        message: 'Storage commands require the managed BrowserContext session'
      })
    )
    const unrelatedStorage = await harness.send('Storage.clearDataForOrigin', {
      params: { origin: 'http://127.0.0.1', storageTypes: 'all' }
    })
    expect(unrelatedStorage.error).toEqual(
      expect.objectContaining({ message: 'Unsupported browser-level command' })
    )
    const oversized = await harness.send('Storage.setCookies', {
      params: {
        browserContextId: identity.browserContextId,
        cookies: Array.from({ length: 513 }, (_value, index) => ({
          name: `fixture-${index}`,
          value: 'value',
          url: 'http://127.0.0.1/fixture'
        }))
      }
    })
    expect(oversized.error).toEqual(
      expect.objectContaining({ message: 'Managed cookie payload exceeds its limit' })
    )
    expect(
      commands.filter(
        (command) =>
          command.method.startsWith('Network.') && command.method !== 'Network.getAllCookies'
      )
    ).toHaveLength(2)
    transport.close()
  })

  it('safely adapts bounded Input.insertText for the exact guest selection', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const text = `safe'); globalThis.__mcp_injected = true; //\u2028next`
    const response = await harness.send('Input.insertText', {
      params: { text },
      sessionId
    })

    expect(response.result).toEqual({})
    expect(guest.insertedTexts).toEqual([text])
    expect(
      (guest.debugger as unknown as FakeDebugger).commands.some(
        (command) => command.method === 'Runtime.evaluate'
      )
    ).toBe(false)
    transport.close()
  })

  it('preserves native key events while delivering their text exactly once', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()

    const keyParams = { type: 'keyDown', key: 'h', text: 'h', unmodifiedText: 'h' }
    const response = await harness.send('Input.dispatchKeyEvent', {
      params: keyParams,
      sessionId
    })

    expect(response.result).toEqual({})
    const commands = (guest.debugger as unknown as FakeDebugger).commands
    expect(commands.filter((command) => command.method === 'Runtime.evaluate')).toHaveLength(0)
    expect(commands).toContainEqual({
      method: 'Input.dispatchKeyEvent',
      params: { type: 'keyDown', key: 'h' },
      sessionId: undefined
    })
    expect(guest.insertedTexts).toEqual(['h'])
    transport.close()
  })

  it('admits nested OOPIF sessions, disables debugger pausing, and inserts into the focused child', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    const identity = syntheticTargetIdentity(harness)

    emitIframeAttached(guest, identity, {
      childSessionId: 'child-one',
      childTargetId: 'target-child-one'
    })
    emitIframeAttached(guest, identity, {
      childSessionId: 'child-nested',
      childTargetId: 'target-child-nested',
      parentSessionId: 'child-one',
      parentTargetId: 'target-child-one'
    })
    const attachedEvents = harness.events.filter(
      (event) => event.method === 'Target.attachedToTarget'
    )
    expect(attachedEvents).toHaveLength(3)
    expect(attachedEvents.at(-1)?.sessionId).toBe('child-one')

    const autoAttach = await harness.send('Target.setAutoAttach', {
      params: {
        autoAttach: true,
        flatten: true,
        waitForDebuggerOnStart: true
      },
      sessionId
    })
    expect(autoAttach.result).toEqual({})
    expect(fakeDebugger.commands).toContainEqual({
      method: 'Target.setAutoAttach',
      params: {
        autoAttach: true,
        flatten: true,
        waitForDebuggerOnStart: false
      },
      sessionId: undefined
    })

    fakeDebugger.focusedSessions.add('child-one')
    fakeDebugger.focusedSessions.add('child-nested')
    const inserted = await harness.send('Input.insertText', {
      params: { text: '你好' },
      sessionId
    })
    expect(inserted.result).toEqual({})
    expect(guest.insertedTexts).toEqual([])
    expect(fakeDebugger.commands).toContainEqual({
      method: 'Input.insertText',
      params: { text: '你好' },
      sessionId: 'child-nested'
    })

    const world = await harness.send('Page.createIsolatedWorld', {
      params: { frameId: 'frame-child-one', worldName: 'fixture-world' },
      sessionId: 'child-one'
    })
    expect(world.result).toEqual({})
    expect(fakeDebugger.commands).toContainEqual({
      method: 'Page.createIsolatedWorld',
      params: { frameId: 'frame-child-one', worldName: 'fixture-world' },
      sessionId: undefined
    })
    transport.close()
  })

  it('admits pre-existing dedicated workers without breaking top-frame text input', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const identity = syntheticTargetIdentity(harness)
    emitWorkerAttached(guest, identity, {
      childSessionId: 'worker-one',
      childTargetId: 'worker-target-one',
      // Real TargetInfo may omit this optional field. The exact admitted parent-session envelope
      // and BrowserContext still prove ownership.
      includeOwnerFrameId: false
    })
    emitWorkerAttached(guest, identity, {
      childSessionId: 'worker-nested',
      childTargetId: 'worker-target-nested',
      parentSessionId: 'worker-one'
    })

    expect(
      harness.events.filter((event) => event.method === 'Target.attachedToTarget')
    ).toHaveLength(3)
    await expect(
      harness.send('Input.insertText', { params: { text: '你好' }, sessionId })
    ).resolves.toMatchObject({ result: {} })
    expect(guest.insertedTexts).toEqual(['你好'])
    transport.close()
  })

  it('recursively retires nested worker ownership when its parent OOPIF detaches', async () => {
    const { guest, harness, transport } = await createConnectedTransportHarness()
    const identity = syntheticTargetIdentity(harness)
    const fakeDebugger = guest.debugger as unknown as FakeDebugger

    for (let index = 0; index < 25; index += 1) {
      emitIframeAttached(guest, identity, {
        childSessionId: 'reused-frame',
        childTargetId: 'reused-frame-target'
      })
      emitWorkerAttached(guest, identity, {
        childSessionId: 'reused-worker',
        childTargetId: 'reused-worker-target',
        ownerFrameId: 'reused-frame-target',
        parentSessionId: 'reused-frame'
      })
      fakeDebugger.emit(
        'message',
        {},
        'Target.detachedFromTarget',
        { sessionId: 'reused-frame', targetId: 'reused-frame-target' },
        ''
      )
    }
    const beforeFinal = harness.events.filter(
      (event) => event.method === 'Target.attachedToTarget'
    ).length
    emitIframeAttached(guest, identity, {
      childSessionId: 'reused-frame',
      childTargetId: 'reused-frame-target'
    })
    emitWorkerAttached(guest, identity, {
      childSessionId: 'reused-worker',
      childTargetId: 'reused-worker-target',
      ownerFrameId: 'reused-frame-target',
      parentSessionId: 'reused-frame'
    })
    expect(
      harness.events.filter((event) => event.method === 'Target.attachedToTarget')
    ).toHaveLength(beforeFinal + 2)
    transport.close()
  })

  it('ignores a page-controlled document.hasFocus spoof when routing sensitive text', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    const identity = syntheticTargetIdentity(harness)
    emitIframeAttached(guest, identity, {
      childSessionId: 'spoofed-child',
      childTargetId: 'spoofed-target'
    })
    fakeDebugger.mainWorldFocusSpoofs.add('spoofed-child')

    const inserted = await harness.send('Input.insertText', {
      params: { text: 'top-secret' },
      sessionId
    })

    expect(inserted.result).toEqual({})
    expect(guest.insertedTexts).toEqual(['top-secret'])
    expect(fakeDebugger.commands).not.toContainEqual(
      expect.objectContaining({
        method: 'Runtime.evaluate',
        params: expect.objectContaining({ expression: 'document.hasFocus() === true' })
      })
    )
    expect(fakeDebugger.commands).toContainEqual(
      expect.objectContaining({
        method: 'Runtime.evaluate',
        params: expect.objectContaining({
          expression: 'Document.prototype.hasFocus.call(document) === true',
          contextId: expect.any(Number)
        }),
        sessionId: 'spoofed-child'
      })
    )
    expect(
      fakeDebugger.commands.some(
        (command) => command.method === 'Input.insertText' && command.sessionId === 'spoofed-child'
      )
    ).toBe(false)
    transport.close()
  })

  it('releases child readiness and identities when an OOPIF detaches before it is ready', async () => {
    const { guest, harness, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    const identity = syntheticTargetIdentity(harness)
    emitIframeAttached(guest, identity, {
      childSessionId: 'child-race',
      childTargetId: 'target-child-race',
      waitingForDebugger: true
    })

    const pendingWorld = harness.send('Page.createIsolatedWorld', {
      params: { frameId: 'frame-race', worldName: 'fixture-world' },
      sessionId: 'child-race'
    })
    fakeDebugger.emit(
      'message',
      {},
      'Target.detachedFromTarget',
      { sessionId: 'child-race', targetId: 'target-child-race' },
      ''
    )
    await expect(pendingWorld).resolves.toEqual(
      expect.objectContaining({ error: expect.objectContaining({ message: 'frame_detached' }) })
    )

    const beforeReattach = harness.events.filter(
      (event) => event.method === 'Target.attachedToTarget'
    ).length
    emitIframeAttached(guest, identity, {
      childSessionId: 'child-race',
      childTargetId: 'target-child-race'
    })
    expect(
      harness.events.filter((event) => event.method === 'Target.attachedToTarget')
    ).toHaveLength(beforeReattach + 1)
    transport.close()
  })

  it('closes on an OOPIF attachment storm before child session state can grow unbounded', async () => {
    const { guest, harness, transport } = await createConnectedTransportHarness()
    const identity = syntheticTargetIdentity(harness)
    const onclose = vi.fn()
    transport.onclose = onclose

    for (let index = 0; index < 65; index += 1) {
      emitIframeAttached(guest, identity, {
        childSessionId: `storm-session-${index}`,
        childTargetId: `storm-target-${index}`
      })
    }

    expect(onclose).toHaveBeenCalledOnce()
    expect(onclose).toHaveBeenCalledWith('cdp_child_session_limit_exceeded')
    expect(
      harness.events.filter((event) => event.method === 'Target.attachedToTarget')
    ).toHaveLength(65)
    expect((guest.debugger as unknown as FakeDebugger).detach).toHaveBeenCalledOnce()
  })

  it('rejects malformed, oversized, or failed Input.insertText without forwarding it', async () => {
    const first = await createConnectedTransportHarness()
    const malformed = await first.harness.send('Input.insertText', {
      params: { text: 'hello', unexpected: true },
      sessionId: first.sessionId
    })
    expect(malformed.error).toEqual(
      expect.objectContaining({ message: 'Invalid text insertion request' })
    )

    const oversized = await first.harness.send('Input.insertText', {
      params: { text: 'x'.repeat(64 * 1_024 + 1) },
      sessionId: first.sessionId
    })
    expect(oversized.error).toEqual(
      expect.objectContaining({ message: 'Invalid text insertion request' })
    )
    expect(
      (first.guest.debugger as unknown as FakeDebugger).commands.filter(
        (command) => command.method === 'Runtime.evaluate'
      )
    ).toHaveLength(0)
    expect(first.guest.insertedTexts).toEqual([])
    first.transport.close()

    const second = await createConnectedTransportHarness()
    second.guest.rejectInsertText()
    const failed = await second.harness.send('Input.insertText', {
      params: { text: 'hello' },
      sessionId: second.sessionId
    })
    expect(failed.error).toEqual(
      expect.objectContaining({ message: 'frame_input_delivery_failed' })
    )
    second.transport.close()
  })

  it('closes an in-flight Input.insertText when the exact guest target disappears', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const releaseInsertText = guest.holdInsertText()
    const onclose = vi.fn()
    transport.onclose = onclose

    transport.send({ id: 99, method: 'Input.insertText', params: { text: 'hello' }, sessionId })
    await vi.waitFor(() => expect(guest.insertedTexts).toEqual(['hello']))
    guest.destroy()
    releaseInsertText()
    await new Promise((resolve) => setImmediate(resolve))

    expect(onclose).toHaveBeenCalledWith('target_closed')
    expect(harness.events.some((event) => event.id === 99)).toBe(false)
  })

  it('allows byte-bounded large AX trees and rejects them at the explicit output limit', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    fakeDebugger.sendCommand.mockResolvedValueOnce({
      nodes: Array.from({ length: 5_000 }, (_, index) => ({ nodeId: index }))
    })

    const response = await harness.send('Accessibility.getFullAXTree', { sessionId })
    expect((response.result as { nodes: unknown[] }).nodes).toHaveLength(5_000)

    fakeDebugger.sendCommand.mockResolvedValueOnce({ nodes: Array(65_537).fill(0) })
    const tooManyNodes = await harness.send('Accessibility.getFullAXTree', { sessionId })
    expect(tooManyNodes.error).toEqual(expect.objectContaining({ message: 'output_too_large' }))

    fakeDebugger.sendCommand.mockResolvedValueOnce({
      nodes: Array.from({ length: 5_000 }, (_, index) => ({
        nodeId: index,
        value: 'x'.repeat(1_000)
      }))
    })
    const oversized = await harness.send('Accessibility.getFullAXTree', { sessionId })

    expect(oversized.error).toEqual(expect.objectContaining({ message: 'output_too_large' }))
    expect(harness.events.some((event) => JSON.stringify(event).includes('x'.repeat(1_000)))).toBe(
      false
    )
    transport.close()
  })

  it('reports PDF unavailable and applies the artifact budget to IO.read responses', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    Object.defineProperty(guest, 'mainFrame', {
      value: {
        detached: false,
        processId: 1,
        framesInSubtree: [{ processId: 1 }, { processId: 2 }]
      }
    })
    const pdfParams = {
      displayHeaderFooter: false,
      footerTemplate: '',
      generateDocumentOutline: false,
      generateTaggedPDF: false,
      headerTemplate: '',
      landscape: false,
      marginBottom: 0,
      marginLeft: 0,
      marginRight: 0,
      marginTop: 0,
      pageRanges: '',
      paperHeight: 11,
      paperWidth: 8.5,
      preferCSSPageSize: false,
      printBackground: false,
      scale: 1,
      transferMode: 'ReturnAsStream'
    }

    await expect(
      harness.send('Page.printToPDF', { params: pdfParams, sessionId })
    ).resolves.toEqual(
      expect.objectContaining({
        error: expect.objectContaining({ message: 'browser.pdf_unavailable:cross_process_frame' })
      })
    )
    expect(fakeDebugger.commands.some((command) => command.method === 'Page.printToPDF')).toBe(
      false
    )

    const chunk = 'A'.repeat(1 * 1_024 * 1_024 + 1)
    fakeDebugger.respondOnce('IO.read', { base64Encoded: true, data: chunk, eof: false })
    const read = await harness.send('IO.read', {
      params: { handle: 'pdf-stream', size: 1 * 1_024 * 1_024 },
      sessionId
    })
    expect((read.result as { data: string }).data).toHaveLength(chunk.length)

    const artifactBase64Limit = Math.ceil((64 * 1_024 * 1_024) / 3) * 4
    const oversizedMarker = 'oversized-artifact-fixture'
    const originalByteLength = Buffer.byteLength.bind(Buffer)
    const byteLength = vi
      .spyOn(Buffer, 'byteLength')
      .mockImplementation(((
        value: Parameters<typeof Buffer.byteLength>[0],
        encoding?: BufferEncoding
      ) =>
        value === oversizedMarker
          ? artifactBase64Limit + 1
          : originalByteLength(value, encoding)) as typeof Buffer.byteLength)
    try {
      fakeDebugger.respondOnce('IO.read', {
        base64Encoded: true,
        data: oversizedMarker,
        eof: false
      })
      await expect(
        harness.send('IO.read', { params: { handle: 'pdf-stream' }, sessionId })
      ).resolves.toEqual(
        expect.objectContaining({
          error: expect.objectContaining({ message: 'artifact_too_large' })
        })
      )
    } finally {
      byteLength.mockRestore()
    }

    fakeDebugger.respondOnce('IO.close', {})
    await expect(
      harness.send('IO.close', { params: { handle: 'pdf-stream' }, sessionId })
    ).resolves.toEqual(expect.objectContaining({ result: {} }))
    expect(fakeDebugger.commands.findLast((command) => command.method === 'IO.close')).toEqual({
      method: 'IO.close',
      params: { handle: 'pdf-stream' },
      sessionId: undefined
    })
    transport.close()
  })

  it('closes the transport before forwarding oversized debugger event params', async () => {
    const { guest, harness, transport } = await createConnectedTransportHarness()
    const onclose = vi.fn()
    transport.onclose = onclose
    const fakeDebugger = guest.debugger as unknown as FakeDebugger

    fakeDebugger.emit(
      'message',
      {},
      'Accessibility.nodesUpdated',
      { nodes: Array.from({ length: 4_097 }, (_, index) => ({ nodeId: index })) },
      ''
    )

    expect(onclose).toHaveBeenCalledWith('cdp_event_budget_exceeded')
    expect(harness.events.some((event) => event.method === 'Accessibility.nodesUpdated')).toBe(
      false
    )
  })

  it('bounds the debugger notification queue and sustained event rate', async () => {
    const queueHarness = await createConnectedTransportHarness()
    const queueClose = vi.fn()
    queueHarness.transport.onclose = queueClose
    const queueDebugger = queueHarness.guest.debugger as unknown as FakeDebugger
    let injectedReentrantStorm = false
    queueHarness.transport.onmessage = (message) => {
      const event = message as Record<string, unknown>
      if (event.method !== 'Network.dataReceived' || injectedReentrantStorm) return
      injectedReentrantStorm = true
      for (let index = 0; index < 1_025; index += 1) {
        queueDebugger.emit('message', {}, 'Network.dataReceived', { index }, '')
      }
    }
    queueDebugger.emit('message', {}, 'Network.dataReceived', { index: -1 }, '')
    expect(queueClose).toHaveBeenCalledWith('cdp_event_queue_exceeded')

    const rateHarness = await createConnectedTransportHarness()
    const rateClose = vi.fn()
    rateHarness.transport.onclose = rateClose
    const rateDebugger = rateHarness.guest.debugger as unknown as FakeDebugger
    for (let batch = 0; batch < 5; batch += 1) {
      for (let index = 0; index < 900; index += 1) {
        rateDebugger.emit('message', {}, 'Network.dataReceived', { index }, '')
      }
      await new Promise((resolve) => setImmediate(resolve))
      if (rateClose.mock.calls.length > 0) break
    }
    expect(rateClose).toHaveBeenCalledWith('cdp_event_rate_exceeded')
  })

  it('cancels an in-flight attachment when its surface is released', async () => {
    const broker = createBroker()
    const { guest, surfaceId } = createRegisteredGuest(broker)
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    const resumeTargetInfo = fakeDebugger.holdTargetInfo()
    const connection = broker.connect(surfaceId)

    broker.releaseSurface(surfaceId)
    resumeTargetInfo()

    await expect(connection).rejects.toEqual(
      expect.objectContaining<Partial<BrowserTargetBrokerError>>({ code: 'target_closed' })
    )
    expect(fakeDebugger.detach).toHaveBeenCalledOnce()
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 1
    })
  })

  it('rejects a stale surface generation after the same identity is reclaimed', async () => {
    const broker = createBroker()
    const { guest, host, surfaceId } = createRegisteredGuest(broker)
    broker.releaseSurface(surfaceId, 1)
    broker.claimSurface({
      generation: 2,
      guestWebContentsId: guest.id,
      host: host.asWebContents(),
      surfaceId
    })

    await expect(broker.connect(surfaceId, 1)).rejects.toEqual(
      expect.objectContaining<Partial<BrowserTargetBrokerError>>({ code: 'guest_not_found' })
    )
    const current = await broker.connect(surfaceId, 2)
    current.close()
    broker.dispose()
  })

  it('fails closed when partition identity or guest ownership drifts before connect', async () => {
    const partitionBroker = createBroker()
    const partitionGuest = createRegisteredGuest(partitionBroker)
    partitionGuest.guest.session = OTHER_SESSION

    await expect(partitionBroker.connect(partitionGuest.surfaceId)).rejects.toEqual(
      expect.objectContaining<Partial<BrowserTargetBrokerError>>({ code: 'invalid_partition' })
    )
    expect(partitionBroker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
    expect(partitionGuest.guest.listenerCount('destroyed')).toBe(0)
    expect(partitionGuest.host.listenerCount('destroyed')).toBe(0)

    const ownershipBroker = createBroker()
    const ownershipGuest = createRegisteredGuest(ownershipBroker)
    ownershipGuest.guest.hostWebContents = new FakeWebContents(99, 'window').asWebContents()

    await expect(ownershipBroker.connect(ownershipGuest.surfaceId)).rejects.toEqual(
      expect.objectContaining<Partial<BrowserTargetBrokerError>>({ code: 'invalid_guest' })
    )
    expect(ownershipBroker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
  })

  it('cancels an in-flight attachment and unregisters the guest when its host closes', async () => {
    const broker = createBroker()
    const { guest, host, surfaceId } = createRegisteredGuest(broker)
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    const resumeTargetInfo = fakeDebugger.holdTargetInfo()
    const connection = broker.connect(surfaceId)

    host.destroy()
    resumeTargetInfo()

    await expect(connection).rejects.toEqual(
      expect.objectContaining<Partial<BrowserTargetBrokerError>>({ code: 'target_closed' })
    )
    expect(fakeDebugger.detach).toHaveBeenCalledOnce()
    expect(guest.listenerCount('destroyed')).toBe(0)
    expect(host.listenerCount('destroyed')).toBe(0)
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
  })

  it('admits only one client and deterministically detaches and unregisters on close', async () => {
    const broker = createBroker()
    const { guest, surfaceId } = createRegisteredGuest(broker)
    const transport = await broker.connect(surfaceId)
    const onclose = vi.fn()
    transport.onclose = onclose

    await expect(broker.connect(surfaceId)).rejects.toEqual(
      expect.objectContaining<Partial<BrowserTargetBrokerError>>({ code: 'target_busy' })
    )
    expect(broker.snapshot().activeConnections).toBe(1)

    transport.close()
    expect(onclose).toHaveBeenCalledWith('client_closed')
    expect((guest.debugger as unknown as FakeDebugger).detach).toHaveBeenCalledOnce()
    expect(broker.snapshot().activeConnections).toBe(0)

    const replacement = await broker.connect(surfaceId)
    const replacementClosed = vi.fn()
    replacement.onclose = replacementClosed
    guest.destroy()
    expect(replacementClosed).toHaveBeenCalledWith('target_closed')
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
  })

  it('releases an active connection and all listeners when its host closes', async () => {
    const broker = createBroker()
    const { guest, host, surfaceId } = createRegisteredGuest(broker)
    const transport = await broker.connect(surfaceId)
    const onclose = vi.fn()
    transport.onclose = onclose

    host.destroy()

    expect(onclose).toHaveBeenCalledWith('client_closed')
    expect((guest.debugger as unknown as FakeDebugger).detach).toHaveBeenCalledOnce()
    expect(guest.listenerCount('destroyed')).toBe(0)
    expect(host.listenerCount('destroyed')).toBe(0)
    expect(broker.snapshot()).toEqual({
      activeConnections: 0,
      claimedSurfaces: 0,
      registeredGuests: 0
    })
  })

  it('removes guest listeners on dispose and delivers a close registered after closure', async () => {
    const broker = createBroker()
    const { guest, host, surfaceId } = createRegisteredGuest(broker)
    expect(guest.listenerCount('destroyed')).toBe(1)
    expect(host.listenerCount('destroyed')).toBe(1)

    const transport = await broker.connect(surfaceId)
    transport.close()
    const lateClose = vi.fn()
    transport.onclose = lateClose
    expect(lateClose).toHaveBeenCalledOnce()
    expect(lateClose).toHaveBeenCalledWith('client_closed')

    broker.dispose()
    expect(guest.listenerCount('destroyed')).toBe(0)
    expect(host.listenerCount('destroyed')).toBe(0)
  })
})
