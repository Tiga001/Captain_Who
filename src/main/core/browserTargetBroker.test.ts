import { EventEmitter } from 'node:events'
import { runInNewContext } from 'node:vm'
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
  private runtimeEvaluateGate?: Promise<void>
  private runtimeEvaluateValue = true
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
      if (method === 'Runtime.evaluate') {
        await this.runtimeEvaluateGate
        return { result: { type: 'boolean', value: this.runtimeEvaluateValue } }
      }
      return {}
    }
  )

  holdTargetInfo(): () => void {
    let resume!: () => void
    this.targetInfoGate = new Promise<void>((resolve) => {
      resume = resolve
    })
    return resume
  }

  holdRuntimeEvaluate(): () => void {
    let resume!: () => void
    this.runtimeEvaluateGate = new Promise<void>((resolve) => {
      resume = resolve
    })
    return resume
  }

  setRuntimeEvaluateValue(value: boolean): void {
    this.runtimeEvaluateValue = value
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

  it('safely adapts bounded Input.insertText for the exact guest selection', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const text = `safe'); globalThis.__mcp_injected = true; //\u2028next`
    const response = await harness.send('Input.insertText', {
      params: { text },
      sessionId
    })

    expect(response.result).toEqual({})
    const evaluate = (guest.debugger as unknown as FakeDebugger).commands.find(
      (command) => command.method === 'Runtime.evaluate'
    )
    const expression = (evaluate?.params as { expression?: unknown } | undefined)?.expression
    expect(typeof expression).toBe('string')
    expect(evaluate?.sessionId).toBeUndefined()

    const calls: unknown[][] = []
    const fakeDocument = Object.assign(
      Object.create({
        execCommand(...arguments_: unknown[]) {
          calls.push(arguments_)
          return true
        }
      }),
      { activeElement: {} }
    )
    const isolatedContext = { document: fakeDocument }
    expect(runInNewContext(String(expression), isolatedContext)).toBe(true)
    expect(calls).toEqual([['insertText', false, text]])
    expect((isolatedContext as { __mcp_injected?: boolean }).__mcp_injected).toBeUndefined()
    transport.close()
  })

  it('adapts only character dispatch for guest keyboard typing', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()

    const response = await harness.send('Input.dispatchKeyEvent', {
      params: { type: 'keyDown', key: 'h', text: 'h', unmodifiedText: 'h' },
      sessionId
    })

    expect(response.result).toEqual({})
    const commands = (guest.debugger as unknown as FakeDebugger).commands
    expect(commands.filter((command) => command.method === 'Runtime.evaluate')).toHaveLength(1)
    expect(commands.filter((command) => command.method === 'Input.dispatchKeyEvent')).toHaveLength(
      0
    )
    transport.close()
  })

  it('rejects malformed, oversized, or unfocused Input.insertText without forwarding it', async () => {
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
    first.transport.close()

    const second = await createConnectedTransportHarness()
    const secondDebugger = second.guest.debugger as unknown as FakeDebugger
    secondDebugger.setRuntimeEvaluateValue(false)
    const noFocus = await second.harness.send('Input.insertText', {
      params: { text: 'hello' },
      sessionId: second.sessionId
    })
    expect(noFocus.error).toEqual(
      expect.objectContaining({ message: 'Unable to insert text into managed target' })
    )
    second.transport.close()

    const third = await createConnectedTransportHarness()
    const thirdDebugger = third.guest.debugger as unknown as FakeDebugger
    thirdDebugger.sendCommand.mockRejectedValueOnce(new Error('fixture evaluate failure'))
    const exception = await third.harness.send('Input.insertText', {
      params: { text: 'hello' },
      sessionId: third.sessionId
    })
    expect(exception.error).toEqual(
      expect.objectContaining({ message: 'Unable to insert text into managed target' })
    )
    third.transport.close()
  })

  it('closes an in-flight Input.insertText when the exact guest target disappears', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    const releaseEvaluate = fakeDebugger.holdRuntimeEvaluate()
    const onclose = vi.fn()
    transport.onclose = onclose

    transport.send({ id: 99, method: 'Input.insertText', params: { text: 'hello' }, sessionId })
    await vi.waitFor(() =>
      expect(fakeDebugger.commands.some((command) => command.method === 'Runtime.evaluate')).toBe(
        true
      )
    )
    guest.destroy()
    releaseEvaluate()
    await new Promise((resolve) => setImmediate(resolve))

    expect(onclose).toHaveBeenCalledWith('target_closed')
    expect(harness.events.some((event) => event.id === 99)).toBe(false)
  })

  it('rejects oversized CDP command results before they enter Playwright', async () => {
    const { guest, harness, sessionId, transport } = await createConnectedTransportHarness()
    const fakeDebugger = guest.debugger as unknown as FakeDebugger
    fakeDebugger.sendCommand.mockResolvedValueOnce({
      nodes: Array.from({ length: 4_097 }, (_, index) => ({ nodeId: index }))
    })

    const response = await harness.send('Accessibility.getFullAXTree', { sessionId })

    expect(response.error).toEqual(
      expect.objectContaining({ message: 'CDP payload exceeds managed limits' })
    )
    expect(harness.events.some((event) => JSON.stringify(event).includes('nodeId'))).toBe(false)
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
