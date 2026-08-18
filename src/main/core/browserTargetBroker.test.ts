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
  private attached = false
  private targetInfoGate?: Promise<void>

  attach = vi.fn(() => {
    if (this.attached) throw new Error('already attached')
    this.attached = true
  })

  detach = vi.fn(() => {
    this.attached = false
  })

  isAttached = vi.fn(() => this.attached)

  sendCommand = vi.fn(async (method: string, params?: unknown, sessionId?: string) => {
    this.commands.push({ method, params, sessionId })
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
    return {}
  })

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
  destroyed = false

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
    guestWebContentsId: guest.id,
    host: host.asWebContents(),
    surfaceId
  })
  return { guest, host, surfaceId }
}

function createBroker(): BrowserTargetBroker {
  return new BrowserTargetBroker(PARTITION, EXPECTED_SESSION)
}

describe('BrowserTargetBroker', () => {
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
        guestWebContentsId: second.guest.id,
        host: second.host.asWebContents(),
        surfaceId: first.surfaceId
      })
    ).toThrow(expect.objectContaining({ code: 'surface_conflict' }))
    expect(() =>
      broker.claimSurface({
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
