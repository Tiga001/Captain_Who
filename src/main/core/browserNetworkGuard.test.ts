import { EventEmitter } from 'node:events'
import type {
  DownloadItem,
  Event,
  OnBeforeRedirectListenerDetails,
  OnBeforeRequestListenerDetails,
  Session,
  WebContents
} from 'electron'
import { describe, expect, it, vi } from 'vitest'

import { BrowserNetworkGuard } from '../browser/BrowserNetworkGuard'
import { BrowserNetworkPolicy, type BrowserDnsResolver } from '../browser/BrowserNetworkPolicy'
import {
  BrowserRiskCoordinator,
  type BrowserRiskAuthorizationContext,
  type BrowserRiskAuthorizationDecision,
  type BrowserRiskAuthorizationRequest,
  type BrowserRiskAuthorizer
} from '../browser/BrowserRiskCoordinator'

const CONTEXT: BrowserRiskAuthorizationContext = {
  runId: 'run-1',
  capabilityId: 'browser_automation',
  activationId: '123e4567-e89b-42d3-a456-426614174000',
  manifestDigest: `sha256:${'a'.repeat(64)}`,
  policyRevision: 1,
  grantExpiresAtMs: 2_000_000,
  invocationId: '123e4567-e89b-42d3-a456-426614174001',
  callId: 'call-1',
  triggerToolName: 'browser_navigate',
  callReason: 'Use the local fixture.'
}

class FakeWebRequest {
  beforeRequest:
    | ((
        details: OnBeforeRequestListenerDetails,
        callback: (result: { cancel?: boolean }) => void
      ) => void)
    | null = null
  beforeRedirect: ((details: OnBeforeRedirectListenerDetails) => void) | null = null
  completed: ((details: { id: number }) => void) | null = null
  error: ((details: { id: number }) => void) | null = null

  onBeforeRequest(listener: typeof this.beforeRequest): void {
    this.beforeRequest = listener
  }

  onBeforeRedirect(listener: typeof this.beforeRedirect): void {
    this.beforeRedirect = listener
  }

  onCompleted(listener: typeof this.completed): void {
    this.completed = listener
  }

  onErrorOccurred(listener: typeof this.error): void {
    this.error = listener
  }
}

function createSession() {
  const emitter = new EventEmitter()
  const webRequest = new FakeWebRequest()
  const value = Object.assign(emitter, { webRequest }) as unknown as Session
  return { emitter, session: value, webRequest }
}

function createGuest(session: Session, id = 42): WebContents {
  const emitter = new EventEmitter()
  let destroyed = false
  return Object.assign(emitter, {
    id,
    session,
    getType: () => 'webview',
    isDestroyed: () => destroyed,
    loadURL: vi.fn(async () => undefined),
    destroyFixture: () => {
      destroyed = true
      emitter.emit('destroyed')
    }
  }) as unknown as WebContents
}

class Resolver implements BrowserDnsResolver {
  constructor(private readonly answer: readonly string[]) {}
  async resolve(): Promise<readonly string[]> {
    return this.answer
  }
}

class Authorizer implements BrowserRiskAuthorizer {
  readonly requests: BrowserRiskAuthorizationRequest[] = []

  constructor(
    private readonly decision: BrowserRiskAuthorizationDecision = {
      decision: 'approved',
      grantId: '123e4567-e89b-42d3-a456-426614174003'
    }
  ) {}

  async authorize(
    request: BrowserRiskAuthorizationRequest
  ): Promise<BrowserRiskAuthorizationDecision> {
    this.requests.push(request)
    return this.decision
  }
}

function createHarness(
  decision?: BrowserRiskAuthorizationDecision,
  resolved: readonly string[] = ['127.0.0.1']
) {
  const { emitter, session, webRequest } = createSession()
  const authorizer = new Authorizer(decision)
  const policy = new BrowserNetworkPolicy({ dnsResolver: new Resolver(resolved) })
  const coordinator = new BrowserRiskCoordinator({
    authorizer,
    now: () => 1_000_000,
    policy
  })
  const guard = new BrowserNetworkGuard({ coordinator, expectedSession: session, policy })
  guard.install()
  const guest = createGuest(session)
  guard.registerGuest({ generation: 1, guest, surfaceId: 'surface-1' })
  return { authorizer, emitter, guard, guest, webRequest }
}

function request(
  harness: ReturnType<typeof createHarness>,
  overrides: Partial<OnBeforeRequestListenerDetails> = {}
): Promise<{ cancel?: boolean }> {
  return new Promise((resolve) => {
    harness.webRequest.beforeRequest?.(
      {
        id: 1,
        url: 'http://127.0.0.1:3000/',
        method: 'GET',
        webContentsId: harness.guest.id,
        webContents: harness.guest,
        resourceType: 'mainFrame',
        referrer: '',
        timestamp: 1,
        uploadData: [],
        ...overrides
      },
      resolve
    )
  })
}

function begin(harness: ReturnType<typeof createHarness>, signal?: AbortSignal) {
  return harness.guard.beginOperation(harness.guest, {
    authorizationContext: CONTEXT,
    parentRequestId: '123e4567-e89b-42d3-a456-426614174002',
    signal
  })
}

describe('BrowserNetworkGuard', () => {
  it('holds a risky request before dispatch and calls its callback exactly once', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    const callback = vi.fn<(result: { cancel?: boolean }) => void>()

    harness.webRequest.beforeRequest?.(
      {
        id: 1,
        url: 'http://127.0.0.1:3000/',
        method: 'GET',
        webContentsId: harness.guest.id,
        webContents: harness.guest,
        resourceType: 'mainFrame',
        referrer: '',
        timestamp: 1,
        uploadData: []
      },
      callback
    )
    await vi.waitFor(() => expect(callback).toHaveBeenCalledOnce())

    expect(callback).toHaveBeenCalledWith({})
    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'main_frame',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(lease.failure()).toBeNull()
    lease.finish()
  })

  it('uses the exact webContents object when Electron omits webContentsId', async () => {
    const harness = createHarness()
    const lease = begin(harness)

    await expect(request(harness, { webContentsId: undefined })).resolves.toEqual({})
    expect(harness.authorizer.requests).toHaveLength(1)
    lease.finish()
  })

  it('rejects inconsistent webContents identities before authorization', async () => {
    const harness = createHarness()
    const lease = begin(harness)

    await expect(request(harness, { webContentsId: harness.guest.id + 1 })).resolves.toEqual({
      cancel: true
    })
    expect(harness.authorizer.requests).toHaveLength(0)
    lease.finish()
  })

  it('cancels before dispatch and exposes a typed rejection instead of a Chromium error', async () => {
    const harness = createHarness({ decision: 'rejected', reason: 'Not this local service.' })
    const lease = begin(harness)

    await expect(request(harness)).resolves.toEqual({ cancel: true })
    expect(lease.failure()).toEqual({
      code: 'browser.risk_rejected',
      dispatchCertainty: 'definitely_not_dispatched',
      reason: 'Not this local service.'
    })
    lease.finish()
  })

  it('marks redirect targets and rechecks them as redirects', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    lease.markDispatched()
    harness.webRequest.beforeRedirect?.({
      id: 8,
      url: 'https://public.test/',
      method: 'GET',
      webContentsId: harness.guest.id,
      webContents: harness.guest,
      resourceType: 'mainFrame',
      referrer: '',
      timestamp: 1,
      redirectURL: 'http://127.0.0.1:3000/private?token=hidden',
      statusCode: 302,
      statusLine: 'Found',
      fromCache: false
    })

    await expect(
      request(harness, {
        id: 8,
        url: 'http://127.0.0.1:3000/private?token=hidden'
      })
    ).resolves.toEqual({})
    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'redirect',
      riskKinds: expect.arrayContaining(['risk_escalation']),
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(JSON.stringify(harness.authorizer.requests[0])).not.toContain('hidden')
    lease.finish()
  })

  it('marks redirects by the exact webContents object when Electron omits its ID', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    lease.markDispatched()
    harness.webRequest.beforeRedirect?.({
      id: 9,
      url: 'https://public.test/',
      method: 'GET',
      webContentsId: undefined,
      webContents: harness.guest,
      resourceType: 'mainFrame',
      referrer: '',
      timestamp: 1,
      redirectURL: 'http://127.0.0.1:3000/private',
      statusCode: 302,
      statusLine: 'Found',
      fromCache: false
    })

    await expect(
      request(harness, { id: 9, url: 'http://127.0.0.1:3000/private' })
    ).resolves.toEqual({})
    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'redirect',
      riskKinds: expect.arrayContaining(['risk_escalation'])
    })
    lease.finish()
  })

  it('detects file upload metadata without reading or serializing its contents', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const lease = begin(harness)
    lease.markDispatched()
    const canary = 'UPLOAD_SECRET_CANARY'

    await expect(
      request(harness, {
        url: 'https://public.test/upload',
        method: 'POST',
        resourceType: 'xhr',
        uploadData: [{ bytes: Buffer.alloc(0), blobUUID: canary }]
      })
    ).resolves.toEqual({})
    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'upload',
      riskKinds: ['file_upload'],
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(JSON.stringify(harness.authorizer.requests)).not.toContain(canary)
    lease.finish()
  })

  it('does not mistake an ordinary POST body for a file upload', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const lease = begin(harness)
    lease.markDispatched()

    await expect(
      request(harness, {
        url: 'https://public.test/form',
        method: 'POST',
        resourceType: 'xhr',
        uploadData: [{ bytes: Buffer.from('ordinary form data') }]
      })
    ).resolves.toEqual({})

    expect(harness.authorizer.requests).toHaveLength(0)
    lease.finish()
  })

  it('denies a popup, authorizes its exact URL, and only then navigates the current guest', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    lease.markDispatched()

    harness.guard.handleWindowOpen(harness.guest, 'http://127.0.0.1:3000/popup')
    await vi.waitFor(() => expect(harness.guest.loadURL).toHaveBeenCalledOnce())

    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'new_window',
      riskKinds: expect.arrayContaining(['new_window']),
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(harness.guest.loadURL).toHaveBeenCalledWith('http://127.0.0.1:3000/popup')
    lease.finish()
  })

  it('navigates an ordinary public HTTPS popup without creating a risk approval', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const lease = begin(harness)
    lease.markDispatched()

    harness.guard.handleWindowOpen(harness.guest, 'https://public.test/popup')
    await vi.waitFor(() => expect(harness.guest.loadURL).toHaveBeenCalledOnce())

    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.guest.loadURL).toHaveBeenCalledWith('https://public.test/popup')
    lease.finish()
  })

  it('releases a target after cancellation interrupts a non-settling popup navigation', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const loadURL = vi.fn(async () => await new Promise<void>(() => undefined))
    Object.assign(harness.guest, { loadURL })
    const controller = new AbortController()
    const lease = begin(harness, controller.signal)
    lease.markDispatched()
    harness.guard.handleWindowOpen(harness.guest, 'https://public.test/popup')
    await vi.waitFor(() => expect(loadURL).toHaveBeenCalledOnce())
    const settling = lease.settle()

    controller.abort('task_cancelled')

    await expect(settling).rejects.toThrow()
    expect(lease.failure()).toEqual({
      code: 'browser.risk_outcome_unknown',
      dispatchCertainty: 'possibly_dispatched'
    })
    lease.finish()
    expect(harness.guard.snapshot().activeOperations).toBe(0)
    expect(() => begin(harness)).not.toThrow()
    harness.guard.deactivateAutomation()
  })

  it('treats a click-triggered local request as possibly dispatched', async () => {
    const harness = createHarness({ decision: 'rejected', reason: 'No local access.' })
    const lease = begin(harness)
    lease.markDispatched()

    await expect(
      request(harness, { resourceType: 'xhr', url: 'http://127.0.0.1:3000/action' })
    ).resolves.toEqual({ cancel: true })

    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'subresource',
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(lease.failure()).toMatchObject({
      code: 'browser.risk_rejected',
      dispatchCertainty: 'possibly_dispatched'
    })
    lease.finish()
  })

  it('bounds concurrent iframe, fetch, and image risk checks per operation', async () => {
    const harness = createHarness()
    harness.authorizer.authorize = vi.fn(
      async () => await new Promise<BrowserRiskAuthorizationDecision>(() => undefined)
    )
    const lease = begin(harness)
    lease.markDispatched()
    const resourceTypes: OnBeforeRequestListenerDetails['resourceType'][] = [
      'subFrame',
      'xhr',
      'image'
    ]

    const requests = Array.from({ length: 33 }, (_, index) =>
      request(harness, {
        id: index + 1,
        resourceType: resourceTypes[index % resourceTypes.length],
        url: `http://127.0.0.1:3000/resource-${index}`
      })
    )

    await expect(requests.at(-1)).resolves.toEqual({ cancel: true })
    expect(lease.failure()).toEqual({
      code: 'browser.risk_busy',
      dispatchCertainty: 'possibly_dispatched'
    })
    lease.finish()
    await expect(Promise.all(requests.slice(0, -1))).resolves.toEqual(
      Array.from({ length: 32 }, () => ({ cancel: true }))
    )
  })

  it('records a synchronously blocked privileged navigation on the active operation', () => {
    const harness = createHarness()
    const lease = begin(harness)
    lease.markDispatched()

    harness.guard.recordBlockedNavigation(harness.guest)

    expect(lease.failure()).toEqual({
      code: 'browser.unsupported_host_boundary',
      dispatchCertainty: 'possibly_dispatched'
    })
    lease.finish()
  })

  it('pauses a download and resumes the exact item after approval without replay', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    const event = { preventDefault: vi.fn() } as unknown as Event
    const itemEvents = new EventEmitter()
    const item = Object.assign(itemEvents, {
      cancel: vi.fn(),
      getURL: () => 'http://127.0.0.1:3000/download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem

    harness.emitter.emit('will-download', event, item, harness.guest)
    await vi.waitFor(() => expect(item.resume).toHaveBeenCalledOnce())

    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(item.pause).toHaveBeenCalledOnce()
    expect(item.cancel).not.toHaveBeenCalled()
    expect(harness.authorizer.requests[0]).toMatchObject({
      trigger: 'download',
      riskKinds: expect.arrayContaining(['file_download']),
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(lease.failure()).toBeNull()
    expect(harness.guard.snapshot().downloads).toBe(1)
    const settlement = lease.settle()
    let settled = false
    void settlement.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(false)
    itemEvents.emit('done', event, 'completed')
    await settlement
    expect(harness.guard.snapshot().downloads).toBe(0)
    lease.finish()
  })

  it('cancels a paused download when risk approval is rejected', async () => {
    const harness = createHarness({ decision: 'rejected', reason: 'Do not download.' })
    const lease = begin(harness)
    const itemEvents = new EventEmitter()
    const item = Object.assign(itemEvents, {
      cancel: vi.fn(),
      getURL: () => 'http://127.0.0.1:3000/download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem

    harness.emitter.emit('will-download', {} as Event, item, harness.guest)
    await vi.waitFor(() => expect(item.cancel).toHaveBeenCalledOnce())

    expect(item.pause).toHaveBeenCalledOnce()
    expect(item.resume).not.toHaveBeenCalled()
    expect(lease.failure()).toEqual({
      code: 'browser.risk_rejected',
      dispatchCertainty: 'possibly_dispatched',
      reason: 'Do not download.'
    })
    expect(harness.guard.snapshot().downloads).toBe(0)
    lease.finish()
  })

  it('cancels an unknown-length download when received bytes exceed the hard limit', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    lease.markDispatched()
    let receivedBytes = 0
    const itemEvents = new EventEmitter()
    const item = Object.assign(itemEvents, {
      cancel: vi.fn(),
      getReceivedBytes: () => receivedBytes,
      getTotalBytes: () => -1,
      getURL: () => 'http://127.0.0.1:3000/download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem
    harness.emitter.emit('will-download', {} as Event, item, harness.guest)
    await vi.waitFor(() => expect(item.resume).toHaveBeenCalledOnce())

    const settlement = lease.settle()
    let settled = false
    void settlement.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(false)

    receivedBytes = 64 * 1024 * 1024 + 1
    itemEvents.emit('updated', {} as Event, 'progressing')
    await settlement

    expect(item.cancel).toHaveBeenCalledOnce()
    expect(lease.failure()).toEqual({
      code: 'browser.risk_outcome_unknown',
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(harness.guard.snapshot().downloads).toBe(0)
    lease.finish()
  })

  it('enforces the aggregate active download budget without replaying requests', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    lease.markDispatched()
    const items = Array.from({ length: 3 }, (_, index) =>
      Object.assign(new EventEmitter(), {
        cancel: vi.fn(),
        getReceivedBytes: () => 0,
        getTotalBytes: () => 50 * 1024 * 1024,
        getURL: () => `http://127.0.0.1:3000/download-${index}`,
        pause: vi.fn(),
        resume: vi.fn()
      })
    ) as unknown as DownloadItem[]

    harness.emitter.emit('will-download', {} as Event, items[0], harness.guest)
    harness.emitter.emit('will-download', {} as Event, items[1], harness.guest)
    await vi.waitFor(() => expect(items[0].resume).toHaveBeenCalledOnce())
    await vi.waitFor(() => expect(items[1].resume).toHaveBeenCalledOnce())
    harness.emitter.emit('will-download', {} as Event, items[2], harness.guest)

    expect(items[2].cancel).toHaveBeenCalledOnce()
    expect(items[2].pause).not.toHaveBeenCalled()
    expect(harness.guard.snapshot().downloads).toBe(2)
    harness.guard.deactivateAutomation()
    expect(items[0].cancel).toHaveBeenCalledOnce()
    expect(items[1].cancel).toHaveBeenCalledOnce()
    lease.finish()
  })

  it('cancels a pending download when its Browser target closes', async () => {
    let settle!: (value: BrowserRiskAuthorizationDecision) => void
    const harness = createHarness()
    const authorize = vi.fn(
      async () =>
        await new Promise<BrowserRiskAuthorizationDecision>((resolve) => {
          settle = resolve
        })
    )
    harness.authorizer.authorize = authorize
    const lease = begin(harness)
    const item = Object.assign(new EventEmitter(), {
      cancel: vi.fn(),
      getURL: () => 'http://127.0.0.1:3000/download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem

    harness.emitter.emit('will-download', {} as Event, item, harness.guest)
    await vi.waitFor(() => expect(authorize).toHaveBeenCalledOnce())
    harness.guest.emit('destroyed')
    await vi.waitFor(() => expect(item.cancel).toHaveBeenCalledOnce())

    expect(item.resume).not.toHaveBeenCalled()
    expect(harness.guard.snapshot().downloads).toBe(0)
    settle({ decision: 'approved', grantId: 'late' })
    lease.finish()
  })

  it('fails closed for risky requests without a task-scoped automation operation', async () => {
    const harness = createHarness()

    await expect(request(harness)).resolves.toEqual({ cancel: true })
    expect(harness.authorizer.requests).toHaveLength(0)
  })

  it('allows ordinary public HTTPS without retaining a completed run authority', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const lease = begin(harness)
    lease.finish()

    await expect(request(harness, { url: 'https://public.test/page' })).resolves.toEqual({})
    await expect(
      request(harness, { id: 2, url: 'http://127.0.0.1:3000/late-subresource' })
    ).resolves.toEqual({ cancel: true })
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.guard.snapshot().stickyContexts).toBe(0)
  })

  it('aborts an active request when automation detaches or the target closes', async () => {
    let settle!: (value: BrowserRiskAuthorizationDecision) => void
    const harness = createHarness()
    const authorize = vi.fn(
      async () =>
        await new Promise<BrowserRiskAuthorizationDecision>((resolve) => {
          settle = resolve
        })
    )
    harness.authorizer.authorize = authorize
    const lease = begin(harness)
    const pending = request(harness)
    await vi.waitFor(() => expect(authorize).toHaveBeenCalledOnce())

    harness.guard.deactivateAutomation('surface-1')

    await expect(pending).resolves.toEqual({ cancel: true })
    expect(lease.failure()).toMatchObject({ code: 'browser.risk_cancelled' })
    settle({ decision: 'approved', grantId: 'late' })
  })

  it('removes listeners, operations, redirect markers, and guest authority on shutdown', async () => {
    const harness = createHarness()
    const lease = begin(harness)
    harness.webRequest.beforeRedirect?.({
      id: 9,
      url: 'https://public.test/',
      method: 'GET',
      webContentsId: harness.guest.id,
      webContents: harness.guest,
      resourceType: 'mainFrame',
      referrer: '',
      timestamp: 1,
      redirectURL: 'https://other.test/',
      statusCode: 302,
      statusLine: 'Found',
      fromCache: false
    })

    await harness.guard.shutdown()

    expect(harness.guard.snapshot()).toEqual({
      activeOperations: 0,
      downloads: 0,
      guests: 0,
      redirectMarkers: 0,
      stickyContexts: 0
    })
    expect(harness.webRequest.beforeRequest).toBeNull()
    expect(harness.webRequest.beforeRedirect).toBeNull()
    expect(harness.emitter.listenerCount('will-download')).toBe(0)
    lease.finish()
  })
})
