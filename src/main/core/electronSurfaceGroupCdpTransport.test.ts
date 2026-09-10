import { describe, expect, it, vi } from 'vitest'
import { chromium } from 'playwright'
import {
  ElectronSurfaceGroupCdpTransport,
  type ElectronSurfaceGroupCdpSurface
} from '../browser/ElectronSurfaceGroupCdpTransport'
import type {
  ElectronGuestCdpIdentity,
  ElectronGuestCdpTransport
} from '../browser/ElectronGuestCdpTransport'

class FakeGuestTransport {
  onclose?: (reason?: string) => void
  onmessage?: (message: object) => void
  readonly commands: Array<{ method: string; params?: object; sessionId?: string }> = []
  readonly identity: ElectronGuestCdpIdentity
  private closed = false
  readonly hangingMethods = new Set<string>()
  closeCount = 0
  failAutoAttach = false

  constructor(readonly name: string) {
    this.identity = {
      browserContextId: `raw-context-${name}`,
      sessionId: `raw-root-session-${name}`,
      targetId: `raw-root-target-${name}`,
      targetInfo: {
        attached: true,
        browserContextId: `raw-context-${name}`,
        canAccessOpener: false,
        targetId: `raw-root-target-${name}`,
        title: `Title ${name}`,
        type: 'page',
        url: `http://fixture.test/${name}`
      }
    }
  }

  managedIdentity(): ElectronGuestCdpIdentity {
    return this.identity
  }

  send(message: object): void {
    const request = message as {
      id: number
      method: string
      params?: object
      sessionId?: string
    }
    this.commands.push({
      method: request.method,
      ...(request.params ? { params: request.params } : {}),
      ...(request.sessionId ? { sessionId: request.sessionId } : {})
    })
    if (this.hangingMethods.has(request.method)) return
    if (request.method === 'Target.setAutoAttach') {
      this.onmessage?.({
        method: 'Target.attachedToTarget',
        params: {
          sessionId: this.identity.sessionId,
          targetInfo: this.identity.targetInfo,
          waitingForDebugger: false
        }
      })
    }
    if (request.method === 'Target.setAutoAttach' && this.failAutoAttach) {
      this.onmessage?.({
        id: request.id,
        error: { code: -32_000, message: 'fixture_auto_attach_failed' }
      })
      return
    }
    const result =
      request.method === 'Browser.getVersion'
        ? { product: `Chrome/${this.name}`, protocolVersion: '1.3' }
        : request.method === 'Storage.getCookies'
          ? { cookies: [] }
          : {}
    this.onmessage?.({
      id: request.id,
      ...(request.sessionId ? { sessionId: request.sessionId } : {}),
      result
    })
  }

  emitChild(options: {
    parentSessionId?: string
    rawSessionId: string
    rawTargetId: string
    type?: 'iframe' | 'worker'
  }): void {
    this.onmessage?.({
      method: 'Target.attachedToTarget',
      params: {
        sessionId: options.rawSessionId,
        targetInfo: {
          attached: true,
          browserContextId: this.identity.browserContextId,
          parentFrameId: this.identity.targetId,
          targetId: options.rawTargetId,
          title: '',
          type: options.type ?? 'iframe',
          url: 'http://fixture.test/frame'
        },
        waitingForDebugger: false
      },
      sessionId: options.parentSessionId ?? this.identity.sessionId
    })
  }

  emitChildDetached(rawSessionId: string, rawTargetId?: string): void {
    this.onmessage?.({
      method: 'Target.detachedFromTarget',
      params: { sessionId: rawSessionId, ...(rawTargetId ? { targetId: rawTargetId } : {}) },
      sessionId: this.identity.sessionId
    })
  }

  emit(method: string, params: object, sessionId?: string): void {
    this.onmessage?.({ method, params, ...(sessionId ? { sessionId } : {}) })
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    this.closeCount += 1
    this.onmessage?.({
      method: 'Target.detachedFromTarget',
      params: { sessionId: this.identity.sessionId, targetId: this.identity.targetId }
    })
    this.onclose?.('target_closed')
  }

  asTransport(): ElectronGuestCdpTransport {
    return this as unknown as ElectronGuestCdpTransport
  }
}

class GroupHarness {
  readonly events: Record<string, unknown>[] = []
  private nextId = 1
  private readonly pending = new Map<number, (message: Record<string, unknown>) => void>()

  constructor(readonly transport: ElectronSurfaceGroupCdpTransport) {
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

function surface(
  name: string,
  generation = 1
): {
  delegate: FakeGuestTransport
  surface: ElectronSurfaceGroupCdpSurface
} {
  const delegate = new FakeGuestTransport(name)
  return {
    delegate,
    surface: {
      generation,
      surfaceId: `surface-${name}`,
      transport: delegate.asTransport()
    }
  }
}

function createGroup(
  options: { failCreatedAutoAttach?: boolean; hangContextControl?: boolean } = {}
) {
  const active = { surfaceId: 'surface-a' }
  const created: Array<ReturnType<typeof surface>> = []
  const activateSurface = vi.fn(async (input: { surfaceId: string }) => {
    active.surfaceId = input.surfaceId
  })
  const closeSurface = vi.fn(async () => undefined)
  const createSurface = vi.fn(async ({ activate }: { activate: boolean; url: string }) => {
    const next = surface(`created-${created.length + 1}`)
    if (options.failCreatedAutoAttach) next.delegate.failAutoAttach = true
    if (options.hangContextControl) next.delegate.hangingMethods.add('Storage.getCookies')
    created.push(next)
    if (activate) active.surfaceId = next.surface.surfaceId
    return next.surface
  })
  const transport = new ElectronSurfaceGroupCdpTransport({
    activateSurface,
    closeSurface,
    createSurface,
    getActiveSurfaceId: () => active.surfaceId
  })
  return { activateSurface, active, closeSurface, createSurface, created, transport }
}

function attachedRoots(events: readonly Record<string, unknown>[]) {
  return events.filter((event) => event.method === 'Target.attachedToTarget')
}

describe('ElectronSurfaceGroupCdpTransport', () => {
  it('connects fixed Playwright to an empty persistent context without creating a page', async () => {
    const group = createGroup()
    const browser = await chromium.connectOverCDP(group.transport, {
      isLocal: true,
      noDefaults: true,
      timeout: 1_000
    })

    expect(browser.contexts()).toHaveLength(1)
    const context = browser.contexts()[0]
    expect(context?.pages()).toEqual([])
    expect(group.createSurface).not.toHaveBeenCalled()
    await expect(context?.cookies()).resolves.toEqual([])
    expect(group.createSurface).toHaveBeenCalledWith({
      activate: false,
      intent: 'background',
      purpose: 'context-control',
      url: 'about:blank'
    })
    expect(context?.pages()).toEqual([])
    expect(group.transport.surfaceCount()).toBe(0)
    group.transport.close()
    await vi.waitFor(() => expect(browser.isConnected()).toBe(false))
  })

  it('closes and rejects an in-flight context-control command with the group', async () => {
    const group = createGroup({ hangContextControl: true })
    const browser = await chromium.connectOverCDP(group.transport, {
      isLocal: true,
      noDefaults: true,
      timeout: 1_000
    })
    const context = browser.contexts()[0]
    if (!context) throw new Error('context missing')
    const cookies = context.cookies()
    await vi.waitFor(() => expect(group.created).toHaveLength(1))

    group.transport.close()
    await expect(cookies).rejects.toThrow()
    expect(group.created[0]?.delegate.closeCount).toBe(1)
    await vi.waitFor(() => expect(browser.isConnected()).toBe(false))
  })

  it('bounds a silent context-control transport without leaving it registered', async () => {
    vi.useFakeTimers()
    try {
      const group = createGroup({ hangContextControl: true })
      const harness = new GroupHarness(group.transport)
      const cookies = harness.send('Storage.getCookies', { params: {} })
      await vi.advanceTimersByTimeAsync(10_001)

      await expect(cookies).resolves.toEqual(
        expect.objectContaining({
          error: expect.objectContaining({ message: 'context_control_timeout' })
        })
      )
      expect(group.created).toHaveLength(1)
      expect(group.created[0]?.delegate.closeCount).toBe(1)
      expect(group.transport.surfaceCount()).toBe(0)
      group.transport.close()
    } finally {
      vi.useRealTimers()
    }
  })

  it('presents admitted surfaces as one context in selected-first insertion order', async () => {
    const group = createGroup()
    const first = surface('a')
    const second = surface('b')
    await group.transport.addSurface(first.surface)
    await group.transport.addSurface(second.surface)
    const harness = new GroupHarness(group.transport)

    expect(
      await harness.send('Target.setAutoAttach', {
        params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: true }
      })
    ).toEqual(expect.objectContaining({ result: {} }))
    const roots = attachedRoots(harness.events)
    expect(roots).toHaveLength(2)
    const infos = roots.map(
      (event) => (event.params as { targetInfo: Record<string, unknown> }).targetInfo
    )
    expect(infos.map((info) => info.url)).toEqual([
      'http://fixture.test/a',
      'http://fixture.test/b'
    ])
    expect(new Set(infos.map((info) => info.browserContextId)).size).toBe(1)
    expect(JSON.stringify(infos)).not.toContain('raw-context')
    expect(JSON.stringify(infos)).not.toContain('surface-a')

    const listed = await harness.send('Target.getTargets')
    expect((listed.result as { targetInfos: unknown[] }).targetInfos).toEqual(infos)
  })

  it('creates and attaches a managed target before returning its targetId', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: true }
    })
    const baseline = attachedRoots(harness.events).length

    const response = await harness.send('Target.createTarget', {
      params: { url: 'about:blank' }
    })
    const targetId = (response.result as { targetId: string }).targetId
    const newAttachment = attachedRoots(harness.events).slice(baseline)
    expect(newAttachment).toHaveLength(1)
    expect(
      (newAttachment[0].params as { targetInfo: { targetId: string } }).targetInfo.targetId
    ).toBe(targetId)
    expect(group.createSurface).toHaveBeenCalledWith({
      activate: true,
      intent: 'interactive',
      purpose: 'target',
      url: 'about:blank'
    })
    expect(group.activateSurface).toHaveBeenCalledWith({
      generation: 1,
      surfaceId: 'surface-created-1'
    })
  })

  it('creates a managed target for a local file URL', async () => {
    const group = createGroup()
    await group.transport.addSurface(surface('a').surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: true }
    })
    const fileUrl = 'file:///Users/docs/Predici%20.pdf'
    const response = await harness.send('Target.createTarget', {
      params: { url: fileUrl }
    })
    expect(response.error).toBeUndefined()
    expect(group.createSurface).toHaveBeenCalledWith({
      activate: true,
      intent: 'interactive',
      purpose: 'target',
      url: fileUrl
    })
  })

  it.each(['javascript:alert(1)', 'data:text/html,hi', 'chrome://settings'])(
    'rejects Target.createTarget for %s',
    async (url) => {
      const group = createGroup()
      const harness = new GroupHarness(group.transport)
      await expect(harness.send('Target.createTarget', { params: { url } })).resolves.toEqual(
        expect.objectContaining({
          error: expect.objectContaining({ message: 'target_creation_not_permitted' })
        })
      )
      expect(group.createSurface).not.toHaveBeenCalled()
    }
  )

  it('uses one-call background intent without changing the public target contract', async () => {
    const group = createGroup()
    await group.transport.addSurface(surface('a').surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const finish = group.transport.setTargetCreationIntent('background')
    const response = await harness.send('Target.createTarget', {
      params: { url: 'about:blank' }
    })
    finish()
    expect(response.error).toBeUndefined()
    expect(group.createSurface).toHaveBeenCalledWith({
      activate: false,
      intent: 'background',
      purpose: 'target',
      url: 'about:blank'
    })
  })

  it('closes a newly created surface when its attach handshake fails', async () => {
    const group = createGroup({ failCreatedAutoAttach: true })
    await group.transport.addSurface(surface('a').surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const response = await harness.send('Target.createTarget', {
      params: { url: 'about:blank' }
    })
    expect(response.error).toEqual(
      expect.objectContaining({ message: 'fixture_auto_attach_failed' })
    )
    expect(group.closeSurface).toHaveBeenCalledWith({
      generation: 1,
      surfaceId: 'surface-created-1'
    })
    expect(group.transport.surfaceCount()).toBe(1)
  })

  it('activates the exact surface for Page.bringToFront and keeps the group connected', async () => {
    const group = createGroup()
    const first = surface('a')
    const second = surface('b')
    await group.transport.addSurface(first.surface)
    await group.transport.addSurface(second.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const secondRoot = attachedRoots(harness.events)[1]
    const secondSession = (secondRoot.params as { sessionId: string }).sessionId
    await harness.send('Page.bringToFront', { sessionId: secondSession })
    expect(group.active.surfaceId).toBe('surface-b')

    second.delegate.close()
    expect(group.transport.surfaceCount()).toBe(1)
    expect((await harness.send('Target.getTargets')).error).toBeUndefined()
  })

  it('routes the fixed PDF stream sequence to the exact admitted page session', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const root = attachedRoots(harness.events)[0]
    const outwardSession = (root.params as { sessionId: string }).sessionId
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

    await harness.send('Page.printToPDF', { params: pdfParams, sessionId: outwardSession })
    await harness.send('IO.read', {
      params: { handle: 'pdf-stream', size: 1_048_576 },
      sessionId: outwardSession
    })
    await harness.send('IO.close', {
      params: { handle: 'pdf-stream' },
      sessionId: outwardSession
    })

    expect(first.delegate.commands.slice(-3)).toEqual([
      {
        method: 'Page.printToPDF',
        params: pdfParams,
        sessionId: first.delegate.identity.sessionId
      },
      {
        method: 'IO.read',
        params: { handle: 'pdf-stream', size: 1_048_576 },
        sessionId: first.delegate.identity.sessionId
      },
      {
        method: 'IO.close',
        params: { handle: 'pdf-stream' },
        sessionId: first.delegate.identity.sessionId
      }
    ])
  })

  it('closes only the requested target and leaves the other target usable', async () => {
    const group = createGroup()
    await group.transport.addSurface(surface('a').surface)
    await group.transport.addSurface(surface('b').surface)
    const harness = new GroupHarness(group.transport)
    const targets = (await harness.send('Target.getTargets')).result as {
      targetInfos: Array<{ targetId: string }>
    }
    const response = await harness.send('Target.closeTarget', {
      params: { targetId: targets.targetInfos[1].targetId }
    })
    expect(response.result).toEqual({ success: true })
    expect(group.closeSurface).toHaveBeenCalledWith({ generation: 1, surfaceId: 'surface-b' })
    expect(group.transport.surfaceCount()).toBe(1)
  })

  it('namespaces equal child session IDs while preserving raw OOPIF frame target IDs', async () => {
    const group = createGroup()
    const first = surface('a')
    const second = surface('b')
    await group.transport.addSurface(first.surface)
    await group.transport.addSurface(second.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })

    first.delegate.emitChild({ rawSessionId: 'same-raw-session', rawTargetId: 'oopif-frame-a' })
    second.delegate.emitChild({ rawSessionId: 'same-raw-session', rawTargetId: 'oopif-frame-b' })
    const children = attachedRoots(harness.events).filter((event) => Boolean(event.sessionId))
    expect(children).toHaveLength(2)
    const childSessions = children.map((event) => (event.params as { sessionId: string }).sessionId)
    expect(new Set(childSessions).size).toBe(2)
    expect(
      children.map(
        (event) => (event.params as { targetInfo: { targetId: string } }).targetInfo.targetId
      )
    ).toEqual(['oopif-frame-a', 'oopif-frame-b'])

    first.delegate.emitChildDetached('same-raw-session', 'oopif-frame-a')
    const remainingInfo = await harness.send('Target.getTargetInfo', {
      sessionId: childSessions[1]
    })
    expect((remainingInfo.result as { targetInfo: { targetId: string } }).targetInfo.targetId).toBe(
      'oopif-frame-b'
    )
  })

  it('uses admitted child identity when detachedFromTarget omits deprecated targetId', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    first.delegate.emitChild({ rawSessionId: 'raw-child', rawTargetId: 'oopif-frame' })
    first.delegate.emitChildDetached('raw-child')
    const detached = harness.events.find(
      (event) =>
        event.method === 'Target.detachedFromTarget' &&
        (event.params as { targetId?: string }).targetId === 'oopif-frame'
    )
    expect(detached).toBeDefined()
  })

  it('rolls back an attachment if a dynamically added delegate rejects auto-attach', async () => {
    const group = createGroup()
    await group.transport.addSurface(surface('a').surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const failing = surface('failing')
    failing.delegate.failAutoAttach = true
    await expect(group.transport.addSurface(failing.surface)).rejects.toThrow(
      'fixture_auto_attach_failed'
    )
    const lifecycle = harness.events.filter((event) => {
      const params = (event.params ?? {}) as {
        targetInfo?: { url?: string }
        targetId?: string
      }
      return (
        params.targetInfo?.url === 'http://fixture.test/failing' ||
        params.targetId === failing.delegate.identity.targetId
      )
    })
    expect(lifecycle.map((event) => event.method)).toEqual([
      'Target.attachedToTarget',
      'Target.detachedFromTarget'
    ])
    expect(group.transport.surfaceCount()).toBe(1)
  })

  it('drops unadmitted targetInfoChanged and preserves page-scoped Tracing events', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const root = attachedRoots(harness.events)[0]
    const outwardRootSession = (root.params as { sessionId: string }).sessionId
    const baseline = harness.events.length
    expect(() =>
      first.delegate.emit(
        'Target.targetInfoChanged',
        { targetInfo: { targetId: 'unadmitted-target', title: 'secret', url: 'file:///secret' } },
        first.delegate.identity.sessionId
      )
    ).not.toThrow()
    expect(harness.events).toHaveLength(baseline)

    first.delegate.emit(
      'Tracing.tracingComplete',
      { stream: 'stream-1' },
      first.delegate.identity.sessionId
    )
    expect(harness.events.at(-1)).toEqual({
      method: 'Tracing.tracingComplete',
      params: { stream: 'stream-1' },
      sessionId: outwardRootSession
    })
  })

  it('recursively retires nested child sessions without disturbing a sibling', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    first.delegate.emitChild({ rawSessionId: 'parent', rawTargetId: 'parent-frame' })
    first.delegate.emitChild({
      parentSessionId: 'parent',
      rawSessionId: 'nested-worker',
      rawTargetId: 'nested-worker-target',
      type: 'worker'
    })
    first.delegate.emitChild({ rawSessionId: 'sibling', rawTargetId: 'sibling-frame' })
    const children = attachedRoots(harness.events).filter((event) => Boolean(event.sessionId))
    const sessionForTarget = (targetId: string): string => {
      const event = children.find(
        (candidate) =>
          (candidate.params as { targetInfo: { targetId: string } }).targetInfo.targetId ===
          targetId
      )
      if (!event) throw new Error('child missing')
      return (event.params as { sessionId: string }).sessionId
    }
    const nestedSession = sessionForTarget('nested-worker-target')
    const siblingSession = sessionForTarget('sibling-frame')
    first.delegate.emitChildDetached('parent', 'parent-frame')
    const detachTargets = harness.events
      .filter((event) => event.method === 'Target.detachedFromTarget')
      .map((event) => (event.params as { targetId?: string }).targetId)
    expect(detachTargets.slice(-2)).toEqual(['nested-worker-target', 'parent-frame'])
    expect(
      (await harness.send('Target.getTargetInfo', { sessionId: nestedSession })).error
    ).toEqual(expect.objectContaining({ message: 'target_session_not_admitted' }))
    expect(
      (await harness.send('Target.getTargetInfo', { sessionId: siblingSession })).error
    ).toBeUndefined()
  })

  it('admits child events and detach commands only through their exact parent session', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    const rootSession = (attachedRoots(harness.events)[0].params as { sessionId: string }).sessionId

    first.delegate.emitChild({
      parentSessionId: 'unadmitted-parent',
      rawSessionId: 'rejected-child',
      rawTargetId: 'rejected-target'
    })
    expect(
      attachedRoots(harness.events).some(
        (event) =>
          (event.params as { targetInfo: { targetId: string } }).targetInfo.targetId ===
          'rejected-target'
      )
    ).toBe(false)

    first.delegate.emitChild({ rawSessionId: 'parent', rawTargetId: 'parent-frame' })
    first.delegate.emitChild({
      parentSessionId: 'parent',
      rawSessionId: 'nested',
      rawTargetId: 'nested-worker',
      type: 'worker'
    })
    const childEvent = (targetId: string) => {
      const event = attachedRoots(harness.events).find(
        (candidate) =>
          (candidate.params as { targetInfo: { targetId: string } }).targetInfo.targetId ===
          targetId
      )
      if (!event) throw new Error('child attachment missing')
      return event
    }
    const parentSession = (childEvent('parent-frame').params as { sessionId: string }).sessionId
    const nestedSession = (childEvent('nested-worker').params as { sessionId: string }).sessionId

    // A real child identity cannot be detached by replaying its event through a sibling/root
    // envelope, even though both sessions belong to the same admitted guest.
    first.delegate.emit(
      'Target.detachedFromTarget',
      { sessionId: 'nested', targetId: 'nested-worker' },
      first.delegate.identity.sessionId
    )
    expect(
      (await harness.send('Target.getTargetInfo', { sessionId: nestedSession })).error
    ).toBeUndefined()
    expect(
      (
        await harness.send('Target.detachFromTarget', {
          params: { sessionId: nestedSession },
          sessionId: rootSession
        })
      ).error
    ).toEqual(expect.objectContaining({ message: 'invalid_child_session' }))

    expect(
      (
        await harness.send('Target.detachFromTarget', {
          params: { sessionId: nestedSession },
          sessionId: parentSession
        })
      ).error
    ).toBeUndefined()
    expect(first.delegate.commands.at(-1)).toEqual({
      method: 'Target.detachFromTarget',
      params: { sessionId: 'nested' },
      sessionId: 'parent'
    })
  })

  it('uses a bounded background control surface for Storage after the last tab closes', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    await harness.send('Target.setAutoAttach', {
      params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false }
    })
    group.transport.removeSurface(first.surface.surfaceId, first.surface.generation)
    expect(group.transport.surfaceCount()).toBe(0)

    const response = await harness.send('Storage.getCookies', { params: {} })
    expect(response.error).toBeUndefined()
    expect(group.createSurface).toHaveBeenCalledWith({
      activate: false,
      intent: 'background',
      purpose: 'context-control',
      url: 'about:blank'
    })
    expect(group.closeSurface).toHaveBeenCalledWith({
      generation: 1,
      surfaceId: 'surface-created-1'
    })
    expect(group.transport.surfaceCount()).toBe(0)
  })

  it('accepts fixed Playwright default-context Storage params and rejects a foreign context', async () => {
    const group = createGroup()
    const first = surface('a')
    await group.transport.addSurface(first.surface)
    const harness = new GroupHarness(group.transport)
    expect(
      (await harness.send('Storage.setCookies', { params: { cookies: [] } })).error
    ).toBeUndefined()
    expect(first.delegate.commands.at(-1)).toEqual({
      method: 'Storage.setCookies',
      params: { cookies: [] }
    })
    expect(
      (
        await harness.send('Storage.getCookies', {
          params: { browserContextId: 'foreign-context' }
        })
      ).error
    ).toEqual(expect.objectContaining({ message: 'browser_context_not_admitted' }))
  })
})
