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

import {
  BrowserNetworkGuard,
  type BrowserNetworkAccessPolicy
} from '../browser/BrowserNetworkGuard'
import {
  BrowserDownloadBrokerError,
  type BrowserDownloadBroker,
  type BrowserDownloadToolLease
} from '../browser/BrowserDownloadBroker'
import {
  BrowserNetworkPolicy,
  type BrowserDnsResolver,
  type BrowserNetworkPolicyOptions
} from '../browser/BrowserNetworkPolicy'
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
  const mainFrame = { parent: null } as WebContents['mainFrame']
  Object.assign(mainFrame, { top: mainFrame })
  let destroyed = false
  return Object.assign(emitter, {
    id,
    mainFrame,
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
  calls = 0

  constructor(private readonly answer: readonly string[] | Error) {}
  async resolve(): Promise<readonly string[]> {
    this.calls += 1
    if (this.answer instanceof Error) throw this.answer
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
  resolved: readonly string[] | Error = ['127.0.0.1'],
  policyOptions: Omit<BrowserNetworkPolicyOptions, 'dnsResolver'> = {},
  accessPolicy: BrowserNetworkAccessPolicy = 'risk_approval',
  downloadBroker?: BrowserDownloadBroker
) {
  const { emitter, session, webRequest } = createSession()
  const authorizer = new Authorizer(decision)
  const resolver = new Resolver(resolved)
  const policy = new BrowserNetworkPolicy({ ...policyOptions, dnsResolver: resolver })
  const coordinator = new BrowserRiskCoordinator({
    authorizer,
    now: () => 1_000_000,
    policy
  })
  const guard = new BrowserNetworkGuard({
    accessPolicy,
    coordinator,
    downloadBroker,
    expectedSession: session,
    policy
  })
  guard.install()
  const guest = createGuest(session)
  guard.registerGuest({ generation: 1, guest, surfaceId: 'surface-1' })
  return { authorizer, emitter, guard, guest, resolver, webRequest }
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
  it('retains only an exact Main-authored internal document until history releases it', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'], {}, 'host_boundaries_only')
    const internalUrl = 'mycopilot-browser-internal://page/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
    const lease = harness.guard.beginInternalNavigation({
      generation: 1,
      guest: harness.guest,
      url: internalUrl
    })

    await expect(request(harness, { url: internalUrl })).resolves.toEqual({})
    expect(harness.authorizer.requests).toHaveLength(0)
    await expect(request(harness, { url: `${internalUrl}%20different` })).resolves.toEqual({
      cancel: true
    })

    lease.finish()
    await expect(request(harness, { url: internalUrl })).resolves.toEqual({})
    harness.guard.forgetInternalNavigation(harness.guest, internalUrl)
    await expect(request(harness, { url: internalUrl })).resolves.toEqual({ cancel: true })
    await harness.guard.shutdown()
  })

  it('binds a zero-tab tabs-new authority to one exact created guest before load', async () => {
    const claimCreatedGuest = vi.fn(async () => undefined)
    const downloadLease: BrowserDownloadToolLease = {
      artifacts: vi.fn(() => []),
      claimCreatedGuest,
      expectTargetClose: vi.fn(),
      finish: vi.fn(),
      markDispatched: vi.fn(),
      ready: vi.fn(async () => undefined),
      settle: vi.fn(async () => [])
    }
    const downloadBroker = {
      beginTargetCreationTool: vi.fn(() => downloadLease),
      install: vi.fn(),
      registerGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined),
      snapshot: vi.fn(() => ({ downloads: 0 })),
      unregisterGuest: vi.fn()
    } as unknown as BrowserDownloadBroker
    const harness = createHarness(
      undefined,
      ['93.184.216.34'],
      {},
      'host_boundaries_only',
      downloadBroker
    )
    const context = { ...CONTEXT, callId: 'tabs-new-call', triggerToolName: 'browser_tabs' }
    const lease = harness.guard.beginTargetCreationOperation({
      authorizationContext: context,
      parentRequestId: 'tabs-new-parent'
    })
    await lease.ready()
    const authority = await lease.beginTargetCreationAuthority({
      action: 'new',
      activationId: context.activationId,
      capabilityId: context.capabilityId,
      runId: context.runId,
      toolCallId: context.callId,
      toolId: context.triggerToolName,
      url: 'https://public.test/new'
    })
    lease.markDispatched()
    const created = createGuest(harness.guest.session, 71)
    harness.guard.registerGuest({ generation: 2, guest: created, surfaceId: 'created-surface' })
    await authority.claim({ generation: 2, guest: created, surfaceId: 'created-surface' })
    expect(claimCreatedGuest).toHaveBeenCalledWith({
      action: 'new',
      generation: 2,
      guest: created,
      surfaceId: 'created-surface'
    })
    await expect(
      lease.beginTargetCreationAuthority({
        action: 'new',
        activationId: context.activationId,
        capabilityId: context.capabilityId,
        runId: context.runId,
        toolCallId: context.callId,
        toolId: context.triggerToolName,
        url: 'https://public.test/second'
      })
    ).rejects.toThrow('browser.network_guard.target_creation_capacity')
    lease.finish()
    expect(harness.guard.snapshot().activeOperations).toBe(0)
    await harness.guard.shutdown()
  })

  it('admits only the exact about:blank target bootstrap without URL risk preflight', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const context = { ...CONTEXT, callId: 'tabs-blank-call', triggerToolName: 'browser_tabs' }
    const lease = harness.guard.beginTargetCreationOperation({
      authorizationContext: context,
      parentRequestId: 'tabs-blank-parent'
    })
    const authority = await lease.beginTargetCreationAuthority({
      action: 'new',
      activationId: context.activationId,
      capabilityId: context.capabilityId,
      runId: context.runId,
      toolCallId: context.callId,
      toolId: context.triggerToolName,
      url: 'about:blank'
    })

    expect(harness.resolver.calls).toBe(0)
    expect(harness.authorizer.requests).toEqual([])
    lease.markDispatched()
    const created = createGuest(harness.guest.session, 73)
    harness.guard.registerGuest({ generation: 1, guest: created, surfaceId: 'blank-created' })
    await expect(
      authority.claim({ generation: 1, guest: created, surfaceId: 'blank-created' })
    ).resolves.toBeUndefined()
    authority.finish()
    lease.finish()
    await harness.guard.shutdown()
  })

  it('revokes a targetless creation authority on caller cancellation before any guest claim', async () => {
    const downloadLease: BrowserDownloadToolLease = {
      artifacts: vi.fn(() => []),
      claimCreatedGuest: vi.fn(async () => undefined),
      expectTargetClose: vi.fn(),
      finish: vi.fn(),
      markDispatched: vi.fn(),
      ready: vi.fn(async () => undefined),
      settle: vi.fn(async () => [])
    }
    const downloadBroker = {
      beginTargetCreationTool: vi.fn(() => downloadLease),
      install: vi.fn(),
      registerGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined),
      snapshot: vi.fn(() => ({ downloads: 0 })),
      unregisterGuest: vi.fn()
    } as unknown as BrowserDownloadBroker
    const harness = createHarness(
      undefined,
      ['93.184.216.34'],
      {},
      'host_boundaries_only',
      downloadBroker
    )
    const controller = new AbortController()
    const context = { ...CONTEXT, callId: 'cancelled-new', triggerToolName: 'browser_tabs' }
    const lease = harness.guard.beginTargetCreationOperation({
      authorizationContext: context,
      parentRequestId: 'cancelled-parent',
      signal: controller.signal
    })
    const authority = await lease.beginTargetCreationAuthority({
      action: 'new',
      activationId: context.activationId,
      capabilityId: context.capabilityId,
      runId: context.runId,
      toolCallId: context.callId,
      toolId: context.triggerToolName,
      url: 'https://public.test/cancelled'
    })
    lease.markDispatched()
    controller.abort('task_cancelled')
    expect(harness.guard.snapshot().activeOperations).toBe(0)
    expect(downloadLease.finish).toHaveBeenCalledOnce()

    const created = createGuest(harness.guest.session, 72)
    harness.guard.registerGuest({ generation: 1, guest: created, surfaceId: 'late-created' })
    await expect(
      authority.claim({ generation: 1, guest: created, surfaceId: 'late-created' })
    ).rejects.toThrow('browser.network_guard.target_creation_unavailable')
    await harness.guard.shutdown()
  })

  it('grants bounded exact popup children but gives manual popup callbacks no authority', async () => {
    const claimCreatedGuest = vi.fn(async () => undefined)
    const downloadLease: BrowserDownloadToolLease = {
      artifacts: vi.fn(() => []),
      claimCreatedGuest,
      expectTargetClose: vi.fn(),
      finish: vi.fn(),
      markDispatched: vi.fn(),
      ready: vi.fn(async () => undefined),
      settle: vi.fn(async () => [])
    }
    const downloadBroker = {
      beginTool: vi.fn(() => downloadLease),
      install: vi.fn(),
      registerGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined),
      snapshot: vi.fn(() => ({ downloads: 0 })),
      unregisterGuest: vi.fn()
    } as unknown as BrowserDownloadBroker
    const harness = createHarness(
      undefined,
      ['93.184.216.34'],
      {},
      'host_boundaries_only',
      downloadBroker
    )
    const context = { ...CONTEXT, triggerToolName: 'browser_click' }
    const lease = harness.guard.beginOperation(harness.guest, {
      authorizationContext: context,
      parentRequestId: 'popup-parent'
    })
    await lease.ready()
    lease.markDispatched()
    const claimedSurfaces: string[] = []
    const popupSurfaces = ['popup-one', 'popup-two', 'popup-three', 'popup-four', 'popup-five']
    for (const [index, surfaceId] of popupSurfaces.entries()) {
      harness.guard.handleWindowOpen(
        harness.guest,
        `https://public.test/${surfaceId}`,
        async (authority) => {
          expect(authority?.action).toBe('popup')
          const popup = createGuest(harness.guest.session, 80 + index)
          harness.guard.registerGuest({ generation: 1, guest: popup, surfaceId })
          await authority?.claim({ generation: 1, guest: popup, surfaceId })
          claimedSurfaces.push(surfaceId)
        }
      )
    }
    await vi.waitFor(() => expect(claimedSurfaces).toHaveLength(4))
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
    expect([...claimedSurfaces].sort()).toEqual([...popupSurfaces.slice(0, 4)].sort())
    expect(claimCreatedGuest).toHaveBeenCalledTimes(4)
    lease.finish()

    const manualCallback = vi.fn(async (authority?: unknown) => {
      expect(authority).toBeUndefined()
    })
    harness.guard.handleWindowOpen(
      harness.guest,
      'https://public.test/manual-popup',
      manualCallback
    )
    await vi.waitFor(() => expect(manualCallback).toHaveBeenCalledOnce())
    await harness.guard.shutdown()
  })

  it('cancels exact-guest main-frame requests while a sensitive document fence is active', async () => {
    const harness = createHarness(undefined, ['127.0.0.1'], {}, 'host_boundaries_only')
    const fence = harness.guard.beginMainFrameNavigationFence(harness.guest, 1)

    await expect(request(harness)).resolves.toEqual({ cancel: true })
    expect(fence.blocked()).toBe(true)
    fence.finish()
    await expect(request(harness, { id: 2 })).resolves.toEqual({})
    await harness.guard.shutdown()
  })

  it('keeps provisional generation zero network-only until the SurfaceGroup registers authority', () => {
    const { session } = createSession()
    const policy = new BrowserNetworkPolicy({ dnsResolver: new Resolver(['93.184.216.34']) })
    const downloadBroker = {
      install: vi.fn(),
      registerGuest: vi.fn(),
      unregisterGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined)
    } as unknown as BrowserDownloadBroker
    const guard = new BrowserNetworkGuard({
      accessPolicy: 'host_boundaries_only',
      coordinator: new BrowserRiskCoordinator({ authorizer: new Authorizer(), policy }),
      downloadBroker,
      expectedSession: session,
      policy
    })
    const guest = createGuest(session)

    guard.registerGuest({ generation: 0, guest, surfaceId: 'surface-1' })
    expect(downloadBroker.registerGuest).not.toHaveBeenCalled()

    guard.registerGuest({ generation: 1, guest, surfaceId: 'surface-1' })
    expect(downloadBroker.registerGuest).toHaveBeenCalledOnce()
    expect(downloadBroker.registerGuest).toHaveBeenCalledWith({
      generation: 1,
      guest,
      surfaceId: 'surface-1'
    })
  })

  it('records an OutcomeUnknown when a managed download fails after dispatch', async () => {
    const downloadLease: BrowserDownloadToolLease = {
      claimCreatedGuest: vi.fn(async () => undefined),
      expectTargetClose: vi.fn(),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => {
        throw new BrowserDownloadBrokerError('browser.download.too_large', 'possibly_dispatched')
      }),
      artifacts: vi.fn(() => []),
      finish: vi.fn()
    }
    const downloadBroker = {
      install: vi.fn(),
      registerGuest: vi.fn(),
      beginTool: vi.fn(() => downloadLease),
      unregisterGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      finalizeRun: vi.fn(async () => undefined),
      releaseCapability: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined),
      snapshot: vi.fn(() => ({ downloads: 0 }))
    } as unknown as BrowserDownloadBroker
    const harness = createHarness(
      undefined,
      ['93.184.216.34'],
      {},
      'host_boundaries_only',
      downloadBroker
    )
    const lease = begin(harness)
    lease.markDispatched()

    await expect(lease.settle()).rejects.toMatchObject({ code: 'browser.download.too_large' })
    expect(lease.failure()).toEqual({
      code: 'browser.risk_outcome_unknown',
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(downloadLease.markDispatched).toHaveBeenCalledOnce()
    lease.finish()
    expect(downloadLease.finish).toHaveBeenCalledOnce()
  })

  it('treats one exact planned tab close as normal completion while crashes remain fail-closed', async () => {
    const expectTargetClose = vi.fn()
    const downloadLease: BrowserDownloadToolLease = {
      artifacts: vi.fn(() => []),
      claimCreatedGuest: vi.fn(async () => undefined),
      expectTargetClose,
      finish: vi.fn(),
      markDispatched: vi.fn(),
      ready: vi.fn(async () => undefined),
      settle: vi.fn(async () => [])
    }
    const downloadBroker = {
      beginTool: vi.fn(() => downloadLease),
      install: vi.fn(),
      registerGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined),
      snapshot: vi.fn(() => ({ downloads: 0 })),
      unregisterGuest: vi.fn()
    } as unknown as BrowserDownloadBroker
    const harness = createHarness(
      undefined,
      ['93.184.216.34'],
      {},
      'host_boundaries_only',
      downloadBroker
    )
    const lease = begin(harness)
    lease.expectTargetClose({ surfaceId: 'surface-1', generation: 1 })
    lease.markDispatched()
    harness.guest.emit('destroyed')

    expect(expectTargetClose).toHaveBeenCalledWith({ surfaceId: 'surface-1', generation: 1 })
    expect(lease.failure()).toBeNull()
    await expect(lease.settle()).resolves.toBeUndefined()
    lease.finish()
    expect(harness.guard.snapshot().activeOperations).toBe(0)
    await harness.guard.shutdown()
  })

  it('releases operation ownership when download staging cannot become ready', async () => {
    const failedFinish = vi.fn()
    const healthyFinish = vi.fn()
    const failedLease: BrowserDownloadToolLease = {
      claimCreatedGuest: vi.fn(async () => undefined),
      expectTargetClose: vi.fn(),
      ready: vi.fn(async () => {
        throw new BrowserDownloadBrokerError(
          'browser.download.artifact_failed',
          'definitely_not_dispatched'
        )
      }),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => []),
      artifacts: vi.fn(() => []),
      finish: failedFinish
    }
    const healthyLease: BrowserDownloadToolLease = {
      claimCreatedGuest: vi.fn(async () => undefined),
      expectTargetClose: vi.fn(),
      ready: vi.fn(async () => undefined),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => []),
      artifacts: vi.fn(() => []),
      finish: healthyFinish
    }
    const beginTool = vi.fn().mockReturnValueOnce(failedLease).mockReturnValueOnce(healthyLease)
    const downloadBroker = {
      install: vi.fn(),
      registerGuest: vi.fn(),
      beginTool,
      unregisterGuest: vi.fn(),
      releaseSurface: vi.fn(async () => undefined),
      shutdown: vi.fn(async () => undefined),
      snapshot: vi.fn(() => ({ downloads: 0 }))
    } as unknown as BrowserDownloadBroker
    const harness = createHarness(
      undefined,
      ['93.184.216.34'],
      {},
      'host_boundaries_only',
      downloadBroker
    )

    const failed = begin(harness)
    await expect(failed.ready()).rejects.toMatchObject({
      code: 'browser.download.artifact_failed'
    })
    expect(failedFinish).toHaveBeenCalledOnce()
    expect(harness.guard.snapshot().activeOperations).toBe(0)

    const healthy = begin(harness)
    await expect(healthy.ready()).resolves.toBeUndefined()
    healthy.finish()
    expect(healthyFinish).toHaveBeenCalledOnce()
  })

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

  it('allows a complex automated page without DNS, approvals, or the 32-request risk cap', async () => {
    const harness = createHarness(
      undefined,
      new Error('ordinary automated traffic must not resolve through the risk policy'),
      {},
      'host_boundaries_only'
    )
    const lease = begin(harness)
    lease.markDispatched()
    const resourceTypes: OnBeforeRequestListenerDetails['resourceType'][] = [
      'script',
      'stylesheet',
      'image',
      'font',
      'xhr',
      'subFrame'
    ]

    const responses = await Promise.all(
      Array.from({ length: 128 }, (_, index) =>
        request(harness, {
          id: index + 1,
          resourceType: resourceTypes[index % resourceTypes.length],
          url: `https://asset-${index}.public.test/resource`
        })
      )
    )

    expect(responses).toEqual(Array.from({ length: 128 }, () => ({})))
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
    expect(lease.failure()).toBeNull()
    await expect(lease.settle()).resolves.toBeUndefined()
    lease.finish()
  })

  it('uses only the non-approvable Host boundary for automated navigation preflight', async () => {
    const harness = createHarness(
      undefined,
      ['127.0.0.1'],
      { blockedOrigins: ['http://127.0.0.1:5173/'] },
      'host_boundaries_only'
    )
    const lease = begin(harness)

    await expect(lease.preflight('http://127.0.0.1:3000/page')).resolves.toBeUndefined()
    await expect(lease.preflight('http://127.0.0.1:5173/private')).rejects.toMatchObject({
      failure: {
        code: 'browser.unsupported_host_boundary',
        dispatchCertainty: 'definitely_not_dispatched'
      }
    })
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
    lease.finish()
  })

  it('allows identity-less service-worker traffic during host-boundary-only automation', async () => {
    const harness = createHarness(undefined, ['198.18.0.42'], {}, 'host_boundaries_only')
    const lease = begin(harness)
    lease.markDispatched()

    await expect(
      request(harness, {
        resourceType: 'other',
        url: 'https://public.test/service-worker.js',
        webContents: undefined,
        webContentsId: undefined
      })
    ).resolves.toEqual({})
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
    lease.finish()
  })

  it('allows an automated WebSocket without DNS or a destination approval', async () => {
    const harness = createHarness(
      undefined,
      new Error('must not resolve'),
      {},
      'host_boundaries_only'
    )
    const lease = begin(harness)
    lease.markDispatched()

    await expect(
      request(harness, {
        resourceType: 'webSocket',
        url: 'wss://socket.public.test/connect'
      })
    ).resolves.toEqual({})
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
    lease.finish()
  })

  it('allows blob and data subresources but not top-level embedded navigation', async () => {
    const harness = createHarness(undefined, [], {}, 'host_boundaries_only')
    const lease = begin(harness)
    lease.markDispatched()

    await expect(
      request(harness, {
        id: 1,
        resourceType: 'other',
        url: 'blob:https://public.test/123e4567-e89b-42d3-a456-426614174000'
      })
    ).resolves.toEqual({})
    await expect(
      request(harness, {
        id: 2,
        resourceType: 'image',
        url: 'data:image/png;base64,AA=='
      })
    ).resolves.toEqual({})
    await expect(
      request(harness, {
        id: 3,
        resourceType: 'mainFrame',
        url: 'data:text/html,blocked'
      })
    ).resolves.toEqual({ cancel: true })
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
    lease.finish()
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

  it('allows manual HTTP and localhost requests without borrowing Agent approval authority', async () => {
    const harness = createHarness()

    await expect(request(harness)).resolves.toEqual({})
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
  })

  it('allows manual public HTTPS when a proxy resolver returns fake-IP or is unavailable', async () => {
    const fakeIpHarness = createHarness(undefined, ['198.18.0.42'])
    const unavailableHarness = createHarness(undefined, new Error('proxy owns DNS'))

    await expect(request(fakeIpHarness, { url: 'https://public.test/page' })).resolves.toEqual({})
    await expect(request(unavailableHarness, { url: 'https://public.test/page' })).resolves.toEqual(
      {}
    )
    expect(fakeIpHarness.resolver.calls).toBe(0)
    expect(unavailableHarness.resolver.calls).toBe(0)
  })

  it('allows unattributed manual and service-worker traffic only when no automation is active', async () => {
    const harness = createHarness()

    await expect(
      request(harness, {
        resourceType: 'other',
        url: 'https://public.test/service-worker.js',
        webContents: undefined,
        webContentsId: undefined
      })
    ).resolves.toEqual({})

    const lease = begin(harness)
    await expect(
      request(harness, {
        id: 2,
        resourceType: 'other',
        url: 'https://public.test/service-worker.js',
        webContents: undefined,
        webContentsId: undefined
      })
    ).resolves.toEqual({ cancel: true })
    lease.finish()
  })

  it('always rejects an explicit but unregistered WebContents identity', async () => {
    const harness = createHarness()

    await expect(
      request(harness, {
        url: 'https://public.test/unregistered',
        webContents: undefined,
        webContentsId: harness.guest.id + 1
      })
    ).resolves.toEqual({ cancel: true })
    expect(harness.resolver.calls).toBe(0)
  })

  it('admits only Chromium PDF Viewer resources from its internal WebContents', async () => {
    const harness = createHarness()
    const viewerOrigin = 'chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai'
    const internalWebContentsId = harness.guest.id + 1

    await expect(
      request(harness, {
        resourceType: 'script',
        url: 'chrome://resources/js/load_time_data.js',
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({ cancel: true })
    await expect(
      request(harness, {
        id: 2,
        url: `${viewerOrigin}/index.html`,
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({})
    await expect(
      request(harness, {
        id: 3,
        resourceType: 'script',
        url: `${viewerOrigin}/main.js`,
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({})
    await expect(
      request(harness, {
        id: 4,
        resourceType: 'script',
        url: 'chrome://resources/lit/v3_0/lit.rollup.js',
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({})

    await expect(
      request(harness, {
        id: 5,
        resourceType: 'script',
        url: 'chrome://resources/js/load_time_data.js',
        webContents: undefined,
        webContentsId: internalWebContentsId + 1
      })
    ).resolves.toEqual({ cancel: true })
    await expect(
      request(harness, {
        id: 6,
        url: `${viewerOrigin}/options.html`,
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({ cancel: true })
    await expect(
      request(harness, {
        id: 7,
        method: 'POST',
        url: `${viewerOrigin}/index.html`,
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({ cancel: true })
    await expect(
      request(harness, {
        id: 8,
        url: 'chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index.html',
        webContents: undefined,
        webContentsId: internalWebContentsId
      })
    ).resolves.toEqual({ cancel: true })
    await expect(
      request(harness, {
        id: 9,
        url: `${viewerOrigin}/index.html`
      })
    ).resolves.toEqual({ cancel: true })

    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
  })

  it('revokes Chromium PDF Viewer resource access when its internal WebContents closes', async () => {
    const harness = createHarness()
    const internalGuest = createGuest(harness.guest.session, harness.guest.id + 1)

    await expect(
      request(harness, {
        url: 'chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/index.html',
        webContents: internalGuest,
        webContentsId: internalGuest.id
      })
    ).resolves.toEqual({})
    internalGuest.emit('destroyed')
    await expect(
      request(harness, {
        id: 2,
        resourceType: 'script',
        url: 'chrome://resources/js/load_time_data.js',
        webContents: undefined,
        webContentsId: internalGuest.id
      })
    ).resolves.toEqual({ cancel: true })
  })

  it('attributes frame-only requests to the exact registered guest', async () => {
    const harness = createHarness()
    const unknownTopFrame = { parent: null } as WebContents['mainFrame']
    Object.assign(unknownTopFrame, { top: unknownTopFrame })

    await expect(
      request(harness, {
        frame: harness.guest.mainFrame,
        url: 'https://public.test/frame-resource',
        webContents: undefined,
        webContentsId: undefined
      })
    ).resolves.toEqual({})
    await expect(
      request(harness, {
        frame: unknownTopFrame,
        id: 2,
        url: 'https://public.test/unknown-frame',
        webContents: undefined,
        webContentsId: undefined
      })
    ).resolves.toEqual({ cancel: true })
  })

  it('keeps static Host boundaries closed during manual and unattributed requests', async () => {
    const harness = createHarness(undefined, ['127.0.0.1'], {
      blockedOrigins: ['http://localhost:5173/'],
      debugEndpoints: [{ host: '127.0.0.1', port: 9222 }],
      mcpControlEndpoints: [{ host: '127.0.0.1', port: 8765 }]
    })

    await expect(request(harness, { url: 'http://127.0.0.1:5173/private' })).resolves.toEqual({
      cancel: true
    })
    await expect(
      request(harness, {
        id: 2,
        url: 'http://127.0.0.1:9222/json',
        webContents: undefined,
        webContentsId: undefined
      })
    ).resolves.toEqual({ cancel: true })
    await expect(request(harness, { id: 3, url: 'http://127.0.0.1:8765/rpc' })).resolves.toEqual({
      cancel: true
    })
    expect(harness.resolver.calls).toBe(0)
  })

  it('navigates a manual popup in the current guest without an Agent approval', async () => {
    const harness = createHarness()

    harness.guard.handleWindowOpen(harness.guest, 'http://127.0.0.1:3000/manual-popup')

    await vi.waitFor(() => expect(harness.guest.loadURL).toHaveBeenCalledOnce())
    expect(harness.guest.loadURL).toHaveBeenCalledWith('http://127.0.0.1:3000/manual-popup')
    expect(harness.authorizer.requests).toHaveLength(0)
  })

  it('navigates an automated popup without creating a destination approval', async () => {
    const harness = createHarness(undefined, ['10.0.0.4'], {}, 'host_boundaries_only')
    const lease = begin(harness)
    lease.markDispatched()

    harness.guard.handleWindowOpen(harness.guest, 'http://10.0.0.4:8080/automated-popup')

    await vi.waitFor(() => expect(harness.guest.loadURL).toHaveBeenCalledOnce())
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.resolver.calls).toBe(0)
    lease.finish()
  })

  it('keeps a registered manual tab usable while another surface has active automation', async () => {
    const harness = createHarness()
    const manualGuest = createGuest(harness.guest.session, harness.guest.id + 1)
    harness.guard.registerGuest({ generation: 1, guest: manualGuest, surfaceId: 'surface-2' })
    const lease = begin(harness)

    await expect(
      request(harness, {
        url: 'http://127.0.0.1:3000/manual-tab',
        webContents: manualGuest,
        webContentsId: manualGuest.id
      })
    ).resolves.toEqual({})
    harness.guard.handleWindowOpen(manualGuest, 'https://public.test/manual-popup')
    await vi.waitFor(() => expect(manualGuest.loadURL).toHaveBeenCalledOnce())

    lease.finish()
  })

  it('does not navigate a popup for an unregistered guest', () => {
    const harness = createHarness()
    const unknownGuest = createGuest(harness.guest.session, harness.guest.id + 1)

    harness.guard.handleWindowOpen(unknownGuest, 'https://public.test/unknown-popup')

    expect(unknownGuest.loadURL).not.toHaveBeenCalled()
  })

  it('does not cancel a manual download without an Agent operation', () => {
    const harness = createHarness()
    const item = Object.assign(new EventEmitter(), {
      cancel: vi.fn(),
      getURL: () => 'http://127.0.0.1:3000/manual-download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem

    harness.emitter.emit('will-download', {} as Event, item, harness.guest)

    expect(item.cancel).not.toHaveBeenCalled()
    expect(item.pause).not.toHaveBeenCalled()
    expect(item.resume).not.toHaveBeenCalled()
  })

  it('does not pause an automated download for approval in host-boundary-only mode', () => {
    const harness = createHarness(undefined, ['10.0.0.4'], {}, 'host_boundaries_only')
    const lease = begin(harness)
    lease.markDispatched()
    const item = Object.assign(new EventEmitter(), {
      cancel: vi.fn(),
      getReceivedBytes: () => 0,
      getTotalBytes: () => 1024,
      getURL: () => 'http://10.0.0.4:8080/automated-download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem

    harness.emitter.emit('will-download', {} as Event, item, harness.guest)

    expect(item.cancel).not.toHaveBeenCalled()
    expect(item.pause).not.toHaveBeenCalled()
    expect(item.resume).not.toHaveBeenCalled()
    expect(harness.authorizer.requests).toHaveLength(0)
    expect(harness.guard.snapshot().downloads).toBe(1)
    item.emit('done', {} as Event, 'completed')
    expect(harness.guard.snapshot().downloads).toBe(0)
    lease.finish()
  })

  it('cancels a download attributed to an unregistered guest', () => {
    const harness = createHarness()
    const unknownGuest = createGuest(harness.guest.session, harness.guest.id + 1)
    const item = Object.assign(new EventEmitter(), {
      cancel: vi.fn(),
      getURL: () => 'https://public.test/unknown-download',
      pause: vi.fn(),
      resume: vi.fn()
    }) as unknown as DownloadItem

    harness.emitter.emit('will-download', {} as Event, item, unknownGuest)

    expect(item.cancel).toHaveBeenCalledOnce()
    expect(item.pause).not.toHaveBeenCalled()
    expect(item.resume).not.toHaveBeenCalled()
  })

  it('returns to manual browsing semantics without retaining a completed run authority', async () => {
    const harness = createHarness(undefined, ['93.184.216.34'])
    const lease = begin(harness)
    lease.finish()

    await expect(request(harness, { url: 'https://public.test/page' })).resolves.toEqual({})
    await expect(
      request(harness, { id: 2, url: 'http://127.0.0.1:3000/late-subresource' })
    ).resolves.toEqual({})
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
