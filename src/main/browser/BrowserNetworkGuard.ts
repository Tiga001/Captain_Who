import type {
  DownloadItem,
  Event,
  OnBeforeRedirectListenerDetails,
  OnBeforeRequestListenerDetails,
  Session,
  WebContents
} from 'electron'
import { parseBrowserSurfaceBootstrapUrl, type BrowserDownloadReference } from '@mycopilot/protocol'

import { BrowserNetworkPolicy, type BrowserRiskKind } from './BrowserNetworkPolicy'
import {
  BrowserRiskCoordinator,
  BrowserRiskError,
  type BrowserRiskFailure,
  type BrowserRiskOperation,
  type BrowserRiskOperationInput,
  type BrowserRiskTrigger
} from './BrowserRiskCoordinator'
import { BrowserDownloadBroker, type BrowserDownloadToolLease } from './BrowserDownloadBroker'
import { ChromiumPdfViewerRequestGate } from './ChromiumPdfViewer'
import { isBrowserInternalPageUrl } from './BrowserInternalPageStore'

const MAX_REGISTERED_GUESTS = 32
const MAX_REDIRECT_MARKERS = 1_024
const MAX_ACTIVE_DOWNLOADS = 4
const MAX_SINGLE_DOWNLOAD_BYTES = 64 * 1024 * 1024
const MAX_TOTAL_ACTIVE_DOWNLOAD_BYTES = 128 * 1024 * 1024
const MAX_TOOL_POPUP_AUTHORITIES = 4
const MAX_TRUSTED_INTERNAL_PAGES_PER_GUEST = 128

/** Host-owned policy; Renderer and model cannot select or weaken it. */
export type BrowserNetworkAccessPolicy = 'host_boundaries_only' | 'risk_approval'

interface GuestRecord {
  active?: ActiveOperation
  activeInternalNavigation?: InternalNavigationRecord
  generation: number
  guest: WebContents
  handleDestroyed: () => void
  internalNavigationUrls: Set<string>
  navigationFence?: MainFrameNavigationFenceRecord
  surfaceId: string
}

interface InternalNavigationRecord {
  url: string
}

interface MainFrameNavigationFenceRecord {
  blocked: boolean
}

export interface BrowserMainFrameNavigationFence {
  blocked(): boolean
  finish(): void
}

export interface BrowserInternalNavigationLease {
  finish(): void
}

interface ActiveOperation {
  authorizationContext: BrowserRiskOperationInput['authorizationContext']
  closed: boolean
  controller: AbortController
  creationAuthoritiesIssued: Record<BrowserTargetCreationAction, number>
  creationClaimsStarted: Record<BrowserTargetCreationAction, number>
  dispatched: boolean
  downloadLease?: BrowserDownloadToolLease
  expectedTargetCloses: Set<string>
  operation: BrowserRiskOperation
  records: Set<GuestRecord>
  unlinkCaller: () => void
}

export type BrowserTargetCreationAction = 'new' | 'popup'

export interface BrowserTargetCreationAuthorityInput {
  action: BrowserTargetCreationAction
  activationId: string
  capabilityId: 'browser_automation'
  runId: string
  toolCallId: string
  toolId: string
  url: string
}

export interface BrowserTargetCreationAuthority {
  readonly action: BrowserTargetCreationAction
  claim(input: { generation: number; guest: WebContents; surfaceId: string }): Promise<void>
  finish(): void
}

interface ActiveDownload {
  handleDone: (event: Event, state: 'completed' | 'cancelled' | 'interrupted') => void
  handleUpdated: (event: Event, state: 'progressing' | 'interrupted') => void
  item: DownloadItem
  record: GuestRecord
  settleLifetime: () => void
  view: OperationView
}

interface PassiveDownload {
  handleDone: () => void
  handleUpdated: () => void
  item: DownloadItem
  record: GuestRecord
}

interface OperationView {
  dispatchCertainty: 'definitely_not_dispatched' | 'possibly_dispatched'
  finish: () => void
  operation: BrowserRiskOperation
}

export class BrowserNetworkOperationLease {
  private dispatched = false
  private finished = false

  constructor(
    readonly operation: BrowserRiskOperation,
    private readonly preflightNavigation: (url: string) => Promise<void>,
    private readonly markLeaseDispatched: () => void,
    private readonly finishLease: () => void,
    private readonly downloadLease?: BrowserDownloadToolLease,
    private readonly createTargetAuthority?: (
      input: BrowserTargetCreationAuthorityInput
    ) => Promise<BrowserTargetCreationAuthority>,
    private readonly expectLeaseTargetClose?: (input: {
      generation: number
      surfaceId: string
    }) => void
  ) {}

  async preflight(url: string): Promise<void> {
    if (this.finished) {
      throw new BrowserRiskError({
        code: 'browser.risk_cancelled',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    }
    await this.preflightNavigation(url)
  }

  async ready(): Promise<void> {
    try {
      await this.downloadLease?.ready?.()
    } catch (error) {
      this.finish()
      throw error
    }
  }

  markDispatched(): void {
    if (!this.finished) {
      this.dispatched = true
      this.markLeaseDispatched()
      this.downloadLease?.markDispatched()
    }
  }

  async beginTargetCreationAuthority(
    input: BrowserTargetCreationAuthorityInput
  ): Promise<BrowserTargetCreationAuthority> {
    if (this.finished || !this.createTargetAuthority) {
      throw new Error('browser.network_guard.target_creation_unavailable')
    }
    return await this.createTargetAuthority(input)
  }

  expectTargetClose(input: { generation: number; surfaceId: string }): void {
    if (this.finished || !this.expectLeaseTargetClose) {
      throw new Error('browser.network_guard.target_closed')
    }
    this.expectLeaseTargetClose(input)
  }

  failure(): BrowserRiskFailure | null {
    return this.operation.failure()
  }

  async settle(): Promise<void> {
    if (this.finished) return
    try {
      await this.operation.settle()
      await this.downloadLease?.settle()
    } catch (error) {
      if (this.dispatched) {
        this.operation.recordFailure({
          code: 'browser.risk_outcome_unknown',
          dispatchCertainty: 'possibly_dispatched'
        })
      }
      throw error
    }
  }

  downloads(): readonly BrowserDownloadReference[] {
    return this.downloadLease?.downloads() ?? []
  }

  finish(): void {
    if (this.finished) return
    this.finished = true
    this.finishLease()
  }
}

export class BrowserNetworkGuard {
  private readonly accessPolicy: BrowserNetworkAccessPolicy
  private readonly chromiumPdfViewerRequests = new ChromiumPdfViewerRequestGate()
  private readonly coordinator: BrowserRiskCoordinator
  private readonly downloadBroker?: BrowserDownloadBroker
  private readonly downloads = new Set<ActiveDownload>()
  private readonly expectedSession: Session
  private readonly guests = new Map<number, GuestRecord>()
  private readonly activeOperations = new Set<ActiveOperation>()
  private readonly policy: BrowserNetworkPolicy
  private readonly passiveDownloads = new Set<PassiveDownload>()
  private readonly redirectTargets = new Map<number, string>()
  private disposed = false
  private installed = false

  private readonly handleBeforeRequest = (
    details: OnBeforeRequestListenerDetails,
    callback: (response: { cancel?: boolean }) => void
  ): void => {
    const complete = onceCallback(callback)
    void this.authorizeRequest(details).then(
      () => complete({}),
      () => complete({ cancel: true })
    )
  }

  private readonly handleBeforeRedirect = (details: OnBeforeRedirectListenerDetails): void => {
    if (this.accessPolicy === 'host_boundaries_only') return
    if (!this.requestGuest(details)) return
    if (this.redirectTargets.size >= MAX_REDIRECT_MARKERS) this.redirectTargets.clear()
    this.redirectTargets.set(details.id, safeRequestMarker(details.redirectURL))
  }

  private readonly clearRequest = (details: { id: number }): void => {
    this.redirectTargets.delete(details.id)
  }

  private readonly handleDownload = (
    _event: Event,
    item: DownloadItem,
    webContents: WebContents
  ): void => {
    if (this.downloadBroker) return
    const record = this.guests.get(webContents.id)
    if (!record || record.guest !== webContents) {
      item.cancel()
      return
    }
    const view = this.operationFor(record)
    if (!view) {
      // A manual download belongs to the user's browser session, not to an Agent operation. It
      // must not inherit or require a task-scoped BrowserRiskGrant.
      return
    }
    if (this.accessPolicy === 'host_boundaries_only') {
      this.trackPassiveDownload(record, item)
      return
    }
    if (this.downloads.size >= MAX_ACTIVE_DOWNLOADS || this.downloadBudgetExceeded(item)) {
      view.operation.recordFailure({
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      })
      item.cancel()
      return
    }

    // Pause the existing transfer. Approval resumes this exact DownloadItem; no click or request
    // is replayed. A refusal still has possibly-dispatched certainty because the initiating page
    // action and response may already have produced remote side effects.
    try {
      item.pause()
    } catch {
      view.operation.recordFailure({
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      })
      try {
        item.cancel()
      } catch {
        // The transfer may already have reached a terminal state.
      }
      return
    }
    const download = {} as ActiveDownload
    let settleLifetime!: () => void
    const lifetime = new Promise<void>((resolve) => {
      settleLifetime = onceVoid(resolve)
    })
    const handleDone: ActiveDownload['handleDone'] = (_event, state) =>
      this.finishDownload(download, state)
    const handleUpdated: ActiveDownload['handleUpdated'] = () => {
      if (!this.downloads.has(download) || !this.downloadBudgetExceeded(item, download)) return
      view.operation.recordFailure({
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      })
      this.cancelDownload(download, false)
    }
    Object.assign(download, {
      handleDone,
      handleUpdated,
      item,
      record,
      settleLifetime,
      view
    })
    this.downloads.add(download)
    item.once('done', handleDone)
    item.on('updated', handleUpdated)
    const approvalAndResume = view.operation
      .check({
        url: item.getURL(),
        trigger: 'download',
        additionalRisks: ['file_download'],
        dispatchCertainty: 'possibly_dispatched'
      })
      .then(
        async () => {
          if (!this.downloads.has(download)) return
          if (this.disposed || record.guest.isDestroyed()) {
            this.cancelDownload(download, true)
            return
          }
          try {
            item.resume()
          } catch {
            view.operation.recordFailure({
              code: 'browser.risk_outcome_unknown',
              dispatchCertainty: 'possibly_dispatched'
            })
            this.cancelDownload(download, false)
            return
          }
          // Keep the originating Browser Tool unsettled until this exact item reaches a terminal
          // state. Otherwise a late byte-budget overflow would be invisible after Tool success.
          await lifetime
        },
        () => this.cancelDownload(download, false)
      )
    void view.operation.track(approvalAndResume).catch(() => undefined)
  }

  constructor(options: {
    accessPolicy?: BrowserNetworkAccessPolicy
    coordinator: BrowserRiskCoordinator
    downloadBroker?: BrowserDownloadBroker
    expectedSession: Session
    policy: BrowserNetworkPolicy
  }) {
    this.accessPolicy = options.accessPolicy ?? 'risk_approval'
    this.coordinator = options.coordinator
    this.downloadBroker = options.downloadBroker
    this.expectedSession = options.expectedSession
    this.policy = options.policy
  }

  install(): void {
    this.assertUsable()
    if (this.installed) return
    this.installed = true
    this.expectedSession.webRequest.onBeforeRequest(this.handleBeforeRequest)
    this.expectedSession.webRequest.onBeforeRedirect(this.handleBeforeRedirect)
    this.expectedSession.webRequest.onCompleted(this.clearRequest)
    this.expectedSession.webRequest.onErrorOccurred(this.clearRequest)
    this.expectedSession.on('will-download', this.handleDownload)
    this.downloadBroker?.install()
  }

  registerGuest(input: { generation: number; guest: WebContents; surfaceId: string }): void {
    this.assertUsable()
    if (input.guest.session !== this.expectedSession || input.guest.getType() !== 'webview') {
      throw new Error('browser.network_guard.invalid_guest')
    }
    const previous = this.guests.get(input.guest.id)
    if (previous?.guest === input.guest && previous.generation === input.generation) return
    if (previous) this.unregisterGuest(previous, true)
    if (this.guests.size >= MAX_REGISTERED_GUESTS) {
      throw new Error('browser.network_guard.capacity')
    }
    const record: GuestRecord = {
      generation: input.generation,
      guest: input.guest,
      handleDestroyed: () => this.unregisterGuest(record, true),
      internalNavigationUrls: new Set(),
      surfaceId: input.surfaceId
    }
    this.guests.set(input.guest.id, record)
    input.guest.once('destroyed', record.handleDestroyed)
    try {
      // The webview security hook registers a generation-0 identity as soon as it can prove the
      // inert bootstrap surface. That provisional record closes the did-attach -> dom-ready
      // network window, but it is not Target authority and must never admit downloads. The
      // BrowserSurfaceGroup replaces it with the first positive generation before automation can
      // acquire a Tool/download lease.
      if (input.generation > 0) this.downloadBroker?.registerGuest(input)
    } catch (error) {
      input.guest.removeListener('destroyed', record.handleDestroyed)
      this.guests.delete(input.guest.id)
      throw error
    }
  }

  /** Registers one exact Main-authored internal document for this exact guest generation. */
  beginInternalNavigation(input: {
    generation: number
    guest: WebContents
    url: string
  }): BrowserInternalNavigationLease {
    this.assertUsable()
    const record = this.guests.get(input.guest.id)
    if (
      !record ||
      record.guest !== input.guest ||
      record.generation !== input.generation ||
      input.guest.isDestroyed() ||
      !isManagedInternalPageUrl(input.url)
    ) {
      throw new Error('browser.network_guard.internal_navigation_denied')
    }
    if (
      !record.internalNavigationUrls.has(input.url) &&
      record.internalNavigationUrls.size >= MAX_TRUSTED_INTERNAL_PAGES_PER_GUEST
    ) {
      throw new Error('browser.network_guard.internal_navigation_capacity')
    }
    record.internalNavigationUrls.add(input.url)
    const authorization: InternalNavigationRecord = { url: input.url }
    record.activeInternalNavigation = authorization
    let finished = false
    return {
      finish: () => {
        if (finished) return
        finished = true
        if (record.activeInternalNavigation === authorization) {
          record.activeInternalNavigation = undefined
        }
      }
    }
  }

  forgetInternalNavigation(guest: WebContents, url: string): void {
    const record = this.guests.get(guest.id)
    if (!record || record.guest !== guest) return
    if (record.activeInternalNavigation?.url === url) {
      record.activeInternalNavigation = undefined
    }
    record.internalNavigationUrls.delete(url)
  }

  isInternalNavigationAllowed(guest: WebContents, url: string): boolean {
    const record = this.guests.get(guest.id)
    return Boolean(
      !this.disposed &&
      record &&
      record.guest === guest &&
      !guest.isDestroyed() &&
      record.internalNavigationUrls.has(url)
    )
  }

  beginOperation(
    guest: WebContents,
    input: BrowserRiskOperationInput
  ): BrowserNetworkOperationLease {
    this.assertUsable()
    const record = this.guests.get(guest.id)
    if (!record || record.guest !== guest || guest.isDestroyed()) {
      throw new Error('browser.target_closed')
    }
    if (record.active) throw new Error('browser.network_guard.target_busy')

    const active = this.createActiveOperation(input)
    let downloadLease: BrowserDownloadToolLease | undefined
    try {
      downloadLease = this.downloadBroker
        ? this.downloadBroker.beginTool({
            guest,
            owner: {
              conversationId: input.authorizationContext.conversationId,
              runId: input.authorizationContext.runId,
              activationId: input.authorizationContext.activationId,
              capabilityId: input.authorizationContext.capabilityId,
              surfaceId: record.surfaceId,
              generation: record.generation,
              toolCallId: input.authorizationContext.callId
            },
            signal: active.controller.signal
          })
        : undefined
    } catch (error) {
      this.finishActiveOperation(active, 'download_setup_failed')
      throw error
    }
    if (active.closed) {
      downloadLease?.finish()
      throw new BrowserRiskError({
        code: 'browser.risk_cancelled',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    }
    active.downloadLease = downloadLease
    active.records.add(record)
    record.active = active
    return this.operationLease(active)
  }

  /**
   * Creates a Tool owner before `Target.createTarget` has produced any guest. This is the only
   * zero-tab path for `browser_tabs new` and first-page `browser_navigate`; it grants no existing
   * page or partition-wide authority.
   */
  beginTargetCreationOperation(input: BrowserRiskOperationInput): BrowserNetworkOperationLease {
    this.assertUsable()
    if (
      input.authorizationContext.triggerToolName !== 'browser_tabs' &&
      input.authorizationContext.triggerToolName !== 'browser_navigate'
    ) {
      throw new Error('browser.network_guard.target_creation_unavailable')
    }
    const active = this.createActiveOperation(input)
    try {
      active.downloadLease = this.downloadBroker?.beginTargetCreationTool({
        owner: {
          conversationId: input.authorizationContext.conversationId,
          runId: input.authorizationContext.runId,
          activationId: input.authorizationContext.activationId,
          capabilityId: input.authorizationContext.capabilityId,
          toolCallId: input.authorizationContext.callId
        },
        signal: active.controller.signal
      })
    } catch (error) {
      this.finishActiveOperation(active, 'download_setup_failed')
      throw error
    }
    if (active.closed) {
      active.downloadLease?.finish()
      throw new BrowserRiskError({
        code: 'browser.risk_cancelled',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    }
    return this.operationLease(active)
  }

  private createActiveOperation(input: BrowserRiskOperationInput): ActiveOperation {
    const controller = new AbortController()
    const abortFromCaller = (): void => controller.abort(input.signal?.reason)
    input.signal?.addEventListener('abort', abortFromCaller, { once: true })
    const operation = this.coordinator.beginOperation({ ...input, signal: controller.signal })
    const active: ActiveOperation = {
      authorizationContext: input.authorizationContext,
      closed: false,
      controller,
      creationAuthoritiesIssued: { new: 0, popup: 0 },
      creationClaimsStarted: { new: 0, popup: 0 },
      dispatched: false,
      expectedTargetCloses: new Set<string>(),
      operation,
      records: new Set(),
      unlinkCaller: () => input.signal?.removeEventListener('abort', abortFromCaller)
    }
    this.activeOperations.add(active)
    controller.signal.addEventListener(
      'abort',
      () => this.finishActiveOperation(active, 'operation_cancelled'),
      { once: true }
    )
    if (input.signal?.aborted) controller.abort(input.signal.reason)
    if (active.closed) {
      throw new BrowserRiskError({
        code: 'browser.risk_cancelled',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    }
    return active
  }

  private operationLease(active: ActiveOperation): BrowserNetworkOperationLease {
    return new BrowserNetworkOperationLease(
      active.operation,
      async (url) => await this.preflightActiveOperation(active, url, 'tool_argument'),
      () => {
        if (!active.closed) active.dispatched = true
      },
      () => this.finishActiveOperation(active, 'operation_finished'),
      active.downloadLease,
      async (input) => await this.createTargetCreationAuthority(active, input, false),
      (input) => this.expectTargetClose(active, input)
    )
  }

  private expectTargetClose(
    active: ActiveOperation,
    input: { generation: number; surfaceId: string }
  ): void {
    if (active.closed) throw new Error('browser.network_guard.target_closed')
    const record = [...active.records].find(
      (candidate) =>
        candidate.surfaceId === input.surfaceId && candidate.generation === input.generation
    )
    if (!record || active.expectedTargetCloses.size > 0) {
      throw new Error('browser.network_guard.target_closed')
    }
    const key = surfaceKey(input.surfaceId, input.generation)
    active.expectedTargetCloses.add(key)
    try {
      active.downloadLease?.expectTargetClose(input)
    } catch (error) {
      active.expectedTargetCloses.delete(key)
      throw error
    }
  }

  private async preflightActiveOperation(
    active: ActiveOperation,
    url: string,
    trigger: 'new_window' | 'tool_argument'
  ): Promise<void> {
    if (active.closed) {
      throw new BrowserRiskError({
        code: 'browser.risk_cancelled',
        dispatchCertainty: active.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
      })
    }
    if (this.accessPolicy === 'risk_approval') {
      await active.operation.check({
        url,
        trigger,
        ...(trigger === 'new_window' ? { contextualRisks: ['new_window'] as const } : {}),
        dispatchCertainty: active.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
      })
      return
    }
    await this.authorizeHostBoundary(url, {
      dispatchCertainty: active.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched',
      finish: () => undefined,
      operation: active.operation
    })
  }

  private async createTargetCreationAuthority(
    active: ActiveOperation,
    input: BrowserTargetCreationAuthorityInput,
    alreadyPreflighted: boolean
  ): Promise<BrowserTargetCreationAuthority> {
    this.assertUsable()
    const context = active.authorizationContext
    if (
      active.closed ||
      input.runId !== context.runId ||
      input.activationId !== context.activationId ||
      input.capabilityId !== context.capabilityId ||
      input.toolCallId !== context.callId ||
      input.toolId !== context.triggerToolName ||
      (input.action === 'new' &&
        input.toolId !== 'browser_tabs' &&
        input.toolId !== 'browser_navigate')
    ) {
      throw new Error('browser.network_guard.target_creation_unavailable')
    }
    const maximum = input.action === 'new' ? 1 : MAX_TOOL_POPUP_AUTHORITIES
    if (active.creationAuthoritiesIssued[input.action] >= maximum) {
      throw new Error('browser.network_guard.target_creation_capacity')
    }
    // Reserve the single authority before async preflight so racing callers cannot both pass.
    active.creationAuthoritiesIssued[input.action] += 1
    if (!alreadyPreflighted && input.url !== 'about:blank') {
      await this.preflightActiveOperation(
        active,
        input.url,
        input.action === 'popup' ? 'new_window' : 'tool_argument'
      )
    }
    let finished = false
    let claimStarted = false
    return {
      action: input.action,
      claim: async (claim) => {
        if (finished || claimStarted || active.closed || !active.dispatched) {
          throw new Error('browser.network_guard.target_creation_unavailable')
        }
        claimStarted = true
        const record = this.guests.get(claim.guest.id)
        if (
          !record ||
          record.guest !== claim.guest ||
          record.surfaceId !== claim.surfaceId ||
          record.generation !== claim.generation ||
          claim.guest.isDestroyed() ||
          record.active ||
          active.creationClaimsStarted[input.action] >= maximum
        ) {
          throw new Error('browser.target_closed')
        }
        active.creationClaimsStarted[input.action] += 1
        await active.downloadLease?.claimCreatedGuest({ ...claim, action: input.action })
        if (
          finished ||
          active.closed ||
          active.controller.signal.aborted ||
          this.guests.get(claim.guest.id) !== record ||
          claim.guest.isDestroyed() ||
          record.active
        ) {
          throw new Error('browser.target_closed')
        }
        active.records.add(record)
        record.active = active
      },
      finish: () => {
        finished = true
      }
    }
  }

  private finishActiveOperation(active: ActiveOperation, reason: string): void {
    if (active.closed) return
    active.closed = true
    this.activeOperations.delete(active)
    for (const record of active.records) {
      if (record.active === active) record.active = undefined
      this.cancelDownloadsFor(record, true)
    }
    active.records.clear()
    active.controller.abort(reason)
    active.unlinkCaller()
    active.operation.close()
    active.downloadLease?.finish()
  }

  /**
   * Blocks every main-frame network request for one exact registered guest generation.
   *
   * Sensitive page-context tools use this after proposal-time document freezing and before
   * dispatch. Subresources continue normally, while redirects, page script navigation and manual
   * navigation cannot swap the approved document underneath the in-flight tool.
   */
  beginMainFrameNavigationFence(
    guest: WebContents,
    generation: number
  ): BrowserMainFrameNavigationFence {
    this.assertUsable()
    const record = this.guests.get(guest.id)
    if (
      !record ||
      record.guest !== guest ||
      record.generation !== generation ||
      guest.isDestroyed()
    ) {
      throw new Error('browser.target_closed')
    }
    if (record.navigationFence) throw new Error('browser.network_guard.target_busy')
    const fence: MainFrameNavigationFenceRecord = { blocked: false }
    record.navigationFence = fence
    let finished = false
    return {
      blocked: () => fence.blocked,
      finish: () => {
        if (finished) return
        finished = true
        if (record.navigationFence === fence) record.navigationFence = undefined
      }
    }
  }

  deactivateAutomation(surfaceId?: string): void {
    for (const active of [...this.activeOperations]) {
      if (surfaceId && ![...active.records].some((record) => record.surfaceId === surfaceId)) {
        continue
      }
      this.finishActiveOperation(active, 'automation_detached')
    }
    for (const record of this.guests.values()) {
      if (!surfaceId || record.surfaceId === surfaceId) this.cancelPassiveDownloadsFor(record)
    }
  }

  async finalizeRun(runId: string): Promise<void> {
    for (const active of [...this.activeOperations]) {
      if (active.authorizationContext.runId === runId) {
        this.finishActiveOperation(active, 'run_finalized')
      }
    }
    await this.downloadBroker?.finalizeRun(runId)
  }

  async releaseCapability(activationId: string): Promise<void> {
    for (const active of [...this.activeOperations]) {
      if (active.authorizationContext.activationId === activationId) {
        this.finishActiveOperation(active, 'capability_revoked')
      }
    }
    await this.downloadBroker?.releaseCapability(activationId)
  }

  async releaseToolCall(input: { runId: string; toolCallId: string }): Promise<void> {
    for (const active of [...this.activeOperations]) {
      if (
        active.authorizationContext.runId === input.runId &&
        active.authorizationContext.callId === input.toolCallId
      ) {
        this.finishActiveOperation(active, 'tool_call_released')
      }
    }
    await this.downloadBroker?.releaseToolCall(input)
  }

  async preflight(lease: BrowserNetworkOperationLease, url: string): Promise<void> {
    await lease.preflight(url)
  }

  handleWindowOpen(
    guest: WebContents,
    url: string,
    createPopup?: (authority?: BrowserTargetCreationAuthority) => Promise<void>
  ): void {
    const record = this.guests.get(guest.id)
    if (!record || record.guest !== guest) return
    const view = this.operationFor(record)
    if (!view) {
      if (createPopup) {
        void this.authorizeManualRequest(url)
          .then(
            () => createPopup(),
            () => undefined
          )
          .catch(() => undefined)
        return
      }
      this.navigateManualGuest(guest, url)
      return
    }
    if (createPopup) {
      const active = record.active
      if (!active) return
      const popupCreation = this.preflightActiveOperation(active, url, 'new_window')
        .then(async () => {
          if (guest.isDestroyed() || active.closed) return
          const context = active.authorizationContext
          const authority = await this.createTargetCreationAuthority(
            active,
            {
              action: 'popup',
              activationId: context.activationId,
              capabilityId: context.capabilityId,
              runId: context.runId,
              toolCallId: context.callId,
              toolId: context.triggerToolName,
              url
            },
            true
          )
          try {
            await createPopup(authority)
          } finally {
            authority.finish()
          }
        })
        .catch(() => undefined)
      void active.operation.track(popupCreation).catch(() => undefined)
      return
    }
    if (this.accessPolicy === 'host_boundaries_only') {
      this.navigateAutomatedGuest(guest, url, view)
      return
    }
    const approvalAndNavigation = view.operation
      .check({
        url,
        trigger: 'new_window',
        contextualRisks: ['new_window'],
        dispatchCertainty: view.dispatchCertainty
      })
      .then(async () => {
        if (guest.isDestroyed()) return
        try {
          await guest.loadURL(url)
        } catch {
          view.operation.recordFailure({
            code: 'browser.risk_outcome_unknown',
            dispatchCertainty: 'possibly_dispatched'
          })
        }
      })
      .catch(() => undefined)
      .finally(view.finish)
    void view.operation.track(approvalAndNavigation).catch(() => undefined)
  }

  /** Records a privileged/malformed navigation that the synchronous webContents hook denied. */
  recordBlockedNavigation(guest: WebContents): void {
    const record = this.guests.get(guest.id)
    if (!record || record.guest !== guest) return
    const view = this.operationFor(record)
    if (!view) return
    view.operation.recordFailure({
      code: 'browser.unsupported_host_boundary',
      dispatchCertainty: view.dispatchCertainty
    })
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    this.chromiumPdfViewerRequests.shutdown()
    for (const active of [...this.activeOperations]) {
      this.finishActiveOperation(active, 'shutdown')
    }
    for (const record of [...this.guests.values()]) this.unregisterGuest(record, true)
    this.redirectTargets.clear()
    if (this.installed) {
      this.expectedSession.webRequest.onBeforeRequest(null)
      this.expectedSession.webRequest.onBeforeRedirect(null)
      this.expectedSession.webRequest.onCompleted(null)
      this.expectedSession.webRequest.onErrorOccurred(null)
      this.expectedSession.removeListener('will-download', this.handleDownload)
      this.installed = false
    }
    this.policy.shutdown()
    await this.downloadBroker?.shutdown()
    await this.coordinator.shutdown()
  }

  snapshot(): {
    activeOperations: number
    downloads: number
    guests: number
    redirectMarkers: number
    stickyContexts: number
  } {
    const stickyContexts = 0
    return {
      activeOperations: this.activeOperations.size,
      downloads:
        this.downloadBroker?.snapshot().downloads ??
        this.downloads.size + this.passiveDownloads.size,
      guests: this.guests.size,
      redirectMarkers: this.redirectTargets.size,
      stickyContexts
    }
  }

  private async authorizeRequest(details: OnBeforeRequestListenerDetails): Promise<void> {
    if (this.disposed) throw new Error('browser.network_guard.closed')
    const registeredRecord = this.requestGuest(details)
    if (registeredRecord?.navigationFence && details.resourceType === 'mainFrame') {
      registeredRecord.navigationFence.blocked = true
      registeredRecord.active?.operation.recordFailure({
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: registeredRecord.active.dispatched
          ? 'possibly_dispatched'
          : 'definitely_not_dispatched'
      })
      throw new Error('browser.sensitive_navigation_blocked')
    }
    if (
      registeredRecord &&
      details.resourceType === 'mainFrame' &&
      registeredRecord.internalNavigationUrls.has(details.url)
    ) {
      return
    }
    // Chromium's PDF MIME handler owns a separate, unregistered WebContents. Admit only its
    // compiled-in component resources; registered Browser surfaces still pass through policy.
    if (!registeredRecord && this.chromiumPdfViewerRequests.allows(details)) return
    if (details.url === 'about:blank' || parseBrowserSurfaceBootstrapUrl(details.url) !== null) {
      return
    }
    const record = registeredRecord
    if (
      !record ||
      (details.webContents !== undefined && record.guest !== details.webContents) ||
      record.guest.isDestroyed()
    ) {
      if (hasExplicitRequestIdentity(details)) {
        throw new Error('browser.network_guard.unregistered_request')
      }
      if (this.accessPolicy === 'host_boundaries_only' && isEmbeddedSubresource(details)) return
      await this.authorizeManualRequest(details.url)
      if (this.accessPolicy === 'risk_approval' && this.hasActiveOperation()) {
        throw new Error('browser.network_guard.unregistered_request')
      }
      return
    }

    const view = this.operationFor(record)
    if (this.accessPolicy === 'host_boundaries_only') {
      if (isEmbeddedSubresource(details)) return
      await this.authorizeHostBoundary(details.url, view)
      return
    }
    const trigger = this.requestTrigger(details)
    const additionalRisks: BrowserRiskKind[] = []
    const fileUpload = hasFileUpload(details.uploadData)
    if (fileUpload) additionalRisks.push('file_upload')

    if (view) {
      try {
        await view.operation.check({
          url: details.url,
          trigger: fileUpload ? 'upload' : trigger,
          additionalRisks,
          ...(trigger === 'redirect' ? { contextualRisks: ['risk_escalation'] as const } : {}),
          dispatchCertainty: view.dispatchCertainty
        })
      } finally {
        view.finish()
      }
      return
    }

    await this.authorizeManualRequest(details.url)
  }

  private async authorizeManualRequest(url: string): Promise<void> {
    if ((await this.policy.assessStaticHostBoundary(url)) !== null) {
      throw new Error('browser.unsupported_host_boundary')
    }
  }

  private async authorizeHostBoundary(url: string, view: OperationView | null): Promise<void> {
    if ((await this.policy.assessStaticHostBoundary(url)) === null) return
    const failure: BrowserRiskFailure = {
      code: 'browser.unsupported_host_boundary',
      dispatchCertainty: view?.dispatchCertainty ?? 'definitely_not_dispatched'
    }
    view?.operation.recordFailure(failure)
    throw new BrowserRiskError(failure)
  }

  private hasActiveOperation(): boolean {
    return this.activeOperations.size > 0
  }

  private navigateManualGuest(guest: WebContents, url: string): void {
    if (guest.isDestroyed() || guest.session !== this.expectedSession) return
    void this.authorizeManualRequest(url).then(
      () => {
        setImmediate(() => {
          if (guest.isDestroyed()) return
          void guest.loadURL(url).catch(() => {
            // Electron errors can contain query values; keep manual navigation failures out of
            // logs.
          })
        })
      },
      () => undefined
    )
  }

  private navigateAutomatedGuest(
    guest: WebContents,
    url: string,
    view: OperationView,
    createPopup?: () => Promise<void>
  ): void {
    const navigation = this.authorizeHostBoundary(url, view)
      .then(async () => {
        if (guest.isDestroyed()) return
        try {
          if (createPopup) await createPopup()
          else await guest.loadURL(url)
        } catch {
          view.operation.recordFailure({
            code: 'browser.risk_outcome_unknown',
            dispatchCertainty: 'possibly_dispatched'
          })
        }
      })
      .catch(() => undefined)
      .finally(view.finish)
    void view.operation.track(navigation).catch(() => undefined)
  }

  private requestTrigger(details: OnBeforeRequestListenerDetails): BrowserRiskTrigger {
    const marker = this.redirectTargets.get(details.id)
    if (marker && marker === safeRequestMarker(details.url)) {
      this.redirectTargets.delete(details.id)
      return 'redirect'
    }
    return details.resourceType === 'mainFrame' ? 'main_frame' : 'subresource'
  }

  private requestGuest(details: {
    frame?: OnBeforeRequestListenerDetails['frame']
    webContents?: WebContents
    webContentsId?: number
  }): GuestRecord | undefined {
    const candidates: (GuestRecord | undefined)[] = []
    if (details.webContentsId !== undefined) {
      candidates.push(this.guests.get(details.webContentsId))
    }
    if (details.webContents !== undefined) {
      const record = this.guests.get(details.webContents.id)
      candidates.push(record?.guest === details.webContents ? record : undefined)
    }
    if (details.frame !== undefined && details.frame !== null) {
      const topFrame = details.frame.top ?? (details.frame.parent === null ? details.frame : null)
      candidates.push(
        topFrame
          ? [...this.guests.values()].find((record) => record.guest.mainFrame === topFrame)
          : undefined
      )
    }
    if (candidates.length === 0 || candidates.some((candidate) => candidate === undefined)) {
      return undefined
    }
    const [first] = candidates as GuestRecord[]
    return candidates.every((candidate) => candidate === first) ? first : undefined
  }

  private operationFor(record: GuestRecord): OperationView | null {
    if (record.active) {
      return {
        dispatchCertainty: record.active.dispatched
          ? 'possibly_dispatched'
          : 'definitely_not_dispatched',
        finish: () => undefined,
        operation: record.active.operation
      }
    }
    return null
  }

  private unregisterGuest(record: GuestRecord, abort: boolean): void {
    if (this.guests.get(record.guest.id) !== record) return
    record.guest.removeListener('destroyed', record.handleDestroyed)
    let plannedClose = false
    if (record.active) {
      const active = record.active
      plannedClose = active.expectedTargetCloses.delete(
        surfaceKey(record.surfaceId, record.generation)
      )
      if (plannedClose || !abort) {
        active.records.delete(record)
        record.active = undefined
      } else {
        this.finishActiveOperation(active, 'target_closed')
      }
    }
    if (record.navigationFence) {
      record.navigationFence.blocked = true
      record.navigationFence = undefined
    }
    record.activeInternalNavigation = undefined
    record.internalNavigationUrls.clear()
    this.downloadBroker?.unregisterGuest(record.guest, record.generation)
    void this.downloadBroker
      ?.releaseSurface({ surfaceId: record.surfaceId, generation: record.generation })
      .catch(() => undefined)
    this.cancelDownloadsFor(record, !plannedClose)
    this.cancelPassiveDownloadsFor(record)
    this.guests.delete(record.guest.id)
  }

  private cancelDownload(download: ActiveDownload, recordOutcomeUnknown: boolean): void {
    if (!this.downloads.delete(download)) return
    download.item.removeListener('done', download.handleDone)
    download.item.removeListener('updated', download.handleUpdated)
    if (recordOutcomeUnknown) {
      download.view.operation.recordFailure({
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      })
    }
    try {
      download.item.cancel()
    } catch {
      // A terminal DownloadItem can reject cancellation; the tracked lifetime still settles.
    } finally {
      download.settleLifetime()
    }
  }

  private cancelDownloadsFor(record: GuestRecord, recordOutcomeUnknown: boolean): void {
    for (const download of [...this.downloads]) {
      if (download.record === record) this.cancelDownload(download, recordOutcomeUnknown)
    }
  }

  private trackPassiveDownload(record: GuestRecord, item: DownloadItem): void {
    if (
      this.downloads.size + this.passiveDownloads.size >= MAX_ACTIVE_DOWNLOADS ||
      this.downloadBudgetExceeded(item)
    ) {
      item.cancel()
      return
    }
    const download = {} as PassiveDownload
    const cleanup = (): void => {
      if (!this.passiveDownloads.delete(download)) return
      item.removeListener('done', download.handleDone)
      item.removeListener('updated', download.handleUpdated)
    }
    const handleDone = (): void => cleanup()
    const handleUpdated = (): void => {
      if (!this.passiveDownloads.has(download) || !this.downloadBudgetExceeded(item, download)) {
        return
      }
      cleanup()
      try {
        item.cancel()
      } catch {
        // The DownloadItem may already have reached a terminal state.
      }
    }
    Object.assign(download, { handleDone, handleUpdated, item, record })
    this.passiveDownloads.add(download)
    item.once('done', handleDone)
    item.on('updated', handleUpdated)
  }

  private cancelPassiveDownloadsFor(record: GuestRecord): void {
    for (const download of [...this.passiveDownloads]) {
      if (download.record !== record || !this.passiveDownloads.delete(download)) continue
      download.item.removeListener('done', download.handleDone)
      download.item.removeListener('updated', download.handleUpdated)
      try {
        download.item.cancel()
      } catch {
        // A terminal DownloadItem can reject cancellation.
      }
    }
  }

  private finishDownload(
    download: ActiveDownload,
    state: 'completed' | 'cancelled' | 'interrupted'
  ): void {
    if (!this.downloads.delete(download)) return
    download.item.removeListener('done', download.handleDone)
    download.item.removeListener('updated', download.handleUpdated)
    if (state !== 'completed') {
      download.view.operation.recordFailure({
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      })
    }
    download.settleLifetime()
  }

  private downloadBudgetExceeded(
    item: DownloadItem,
    exclude?: ActiveDownload | PassiveDownload
  ): boolean {
    const candidate = Math.max(
      safeDownloadBytes(item, 'getTotalBytes'),
      safeDownloadBytes(item, 'getReceivedBytes')
    )
    if (candidate > MAX_SINGLE_DOWNLOAD_BYTES) return true
    let total = candidate
    for (const active of this.downloads) {
      if (active === exclude) continue
      total += Math.max(
        safeDownloadBytes(active.item, 'getTotalBytes'),
        safeDownloadBytes(active.item, 'getReceivedBytes')
      )
      if (total > MAX_TOTAL_ACTIVE_DOWNLOAD_BYTES) return true
    }
    for (const passive of this.passiveDownloads) {
      if (passive === exclude) continue
      total += Math.max(
        safeDownloadBytes(passive.item, 'getTotalBytes'),
        safeDownloadBytes(passive.item, 'getReceivedBytes')
      )
      if (total > MAX_TOTAL_ACTIVE_DOWNLOAD_BYTES) return true
    }
    return false
  }

  private assertUsable(): void {
    if (this.disposed) throw new Error('browser.network_guard.closed')
  }
}

function isManagedInternalPageUrl(value: string): boolean {
  return isBrowserInternalPageUrl(value)
}

function onceCallback<T>(callback: (value: T) => void): (value: T) => void {
  let called = false
  return (value) => {
    if (called) return
    called = true
    callback(value)
  }
}

function onceVoid(callback: () => void): () => void {
  let called = false
  return () => {
    if (called) return
    called = true
    callback()
  }
}

function safeRequestMarker(url: string): string {
  try {
    const parsed = new URL(url)
    parsed.username = ''
    parsed.password = ''
    parsed.search = ''
    parsed.hash = ''
    return parsed.toString().slice(0, 2_048)
  } catch {
    return 'invalid'
  }
}

function hasFileUpload(uploadData: OnBeforeRequestListenerDetails['uploadData']): boolean {
  return (uploadData ?? []).some(
    (entry) =>
      entry !== null &&
      typeof entry === 'object' &&
      (Object.hasOwn(entry, 'file') || Object.hasOwn(entry, 'blobUUID'))
  )
}

function hasExplicitRequestIdentity(details: OnBeforeRequestListenerDetails): boolean {
  return (
    details.webContentsId !== undefined ||
    details.webContents !== undefined ||
    (details.frame !== undefined && details.frame !== null)
  )
}

function isEmbeddedSubresource(details: OnBeforeRequestListenerDetails): boolean {
  if (details.resourceType === 'mainFrame') return false
  try {
    const protocol = new URL(details.url).protocol
    return protocol === 'blob:' || protocol === 'data:'
  } catch {
    return false
  }
}

function safeDownloadBytes(
  item: DownloadItem,
  method: 'getReceivedBytes' | 'getTotalBytes'
): number {
  try {
    const value = item[method]()
    return Number.isSafeInteger(value) && value > 0 ? value : 0
  } catch {
    return 0
  }
}

function surfaceKey(surfaceId: string, generation: number): string {
  return `${surfaceId}\u0000${generation}`
}

export function mapBrowserRiskError(error: unknown): BrowserRiskFailure | null {
  return error instanceof BrowserRiskError ? error.failure : null
}
