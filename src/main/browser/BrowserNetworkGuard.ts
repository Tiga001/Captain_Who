import type {
  DownloadItem,
  Event,
  OnBeforeRedirectListenerDetails,
  OnBeforeRequestListenerDetails,
  Session,
  WebContents
} from 'electron'
import { parseBrowserSurfaceBootstrapUrl } from '@mycopilot/protocol'

import { BrowserNetworkPolicy, type BrowserRiskKind } from './BrowserNetworkPolicy'
import {
  BrowserRiskCoordinator,
  BrowserRiskError,
  type BrowserRiskFailure,
  type BrowserRiskOperation,
  type BrowserRiskOperationInput,
  type BrowserRiskTrigger
} from './BrowserRiskCoordinator'

const MAX_REGISTERED_GUESTS = 32
const MAX_REDIRECT_MARKERS = 1_024
const MAX_ACTIVE_DOWNLOADS = 4
const MAX_SINGLE_DOWNLOAD_BYTES = 64 * 1024 * 1024
const MAX_TOTAL_ACTIVE_DOWNLOAD_BYTES = 128 * 1024 * 1024

interface GuestRecord {
  active?: ActiveOperation
  generation: number
  guest: WebContents
  handleDestroyed: () => void
  surfaceId: string
}

interface ActiveOperation {
  controller: AbortController
  dispatched: boolean
  operation: BrowserRiskOperation
  unlinkCaller: () => void
}

interface ActiveDownload {
  handleDone: (event: Event, state: 'completed' | 'cancelled' | 'interrupted') => void
  handleUpdated: (event: Event, state: 'progressing' | 'interrupted') => void
  item: DownloadItem
  record: GuestRecord
  settleLifetime: () => void
  view: OperationView
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
    private readonly markLeaseDispatched: () => void,
    private readonly finishLease: () => void
  ) {}

  markDispatched(): void {
    if (!this.finished) {
      this.dispatched = true
      this.markLeaseDispatched()
    }
  }

  failure(): BrowserRiskFailure | null {
    return this.operation.failure()
  }

  async settle(): Promise<void> {
    if (this.finished) return
    try {
      await this.operation.settle()
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

  finish(): void {
    if (this.finished) return
    this.finished = true
    this.finishLease()
  }
}

export class BrowserNetworkGuard {
  private readonly coordinator: BrowserRiskCoordinator
  private readonly downloads = new Set<ActiveDownload>()
  private readonly expectedSession: Session
  private readonly guests = new Map<number, GuestRecord>()
  private readonly policy: BrowserNetworkPolicy
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
    const record = this.guests.get(webContents.id)
    if (!record || record.guest !== webContents) return
    const view = this.operationFor(record)
    if (!view) {
      // Downloads have already crossed a side-effect boundary. Without a task-scoped automation
      // operation there is no typed BrowserRiskApproval to bind, so fail closed on the exact item.
      item.cancel()
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
    coordinator: BrowserRiskCoordinator
    expectedSession: Session
    policy: BrowserNetworkPolicy
  }) {
    this.coordinator = options.coordinator
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
      surfaceId: input.surfaceId
    }
    this.guests.set(input.guest.id, record)
    input.guest.once('destroyed', record.handleDestroyed)
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

    const controller = new AbortController()
    const abortFromCaller = (): void => controller.abort(input.signal?.reason)
    input.signal?.addEventListener('abort', abortFromCaller, { once: true })
    if (input.signal?.aborted) controller.abort(input.signal.reason)
    const operationInput = { ...input, signal: controller.signal }
    const operation = this.coordinator.beginOperation(operationInput)
    const active: ActiveOperation = {
      controller,
      dispatched: false,
      operation,
      unlinkCaller: () => input.signal?.removeEventListener('abort', abortFromCaller)
    }
    record.active = active
    return new BrowserNetworkOperationLease(
      operation,
      () => {
        if (record.active === active) active.dispatched = true
      },
      () => {
        if (record.active === active) record.active = undefined
        this.cancelDownloadsFor(record, true)
        active.controller.abort('operation_finished')
        active.unlinkCaller()
        operation.close()
      }
    )
  }

  deactivateAutomation(surfaceId?: string): void {
    for (const record of this.guests.values()) {
      if (surfaceId && record.surfaceId !== surfaceId) continue
      record.active?.controller.abort('automation_detached')
      record.active?.unlinkCaller()
      record.active?.operation.close()
      record.active = undefined
      this.cancelDownloadsFor(record, true)
    }
  }

  async preflight(
    lease: BrowserNetworkOperationLease,
    url: string,
    trigger: BrowserRiskTrigger = 'tool_argument'
  ): Promise<void> {
    await lease.operation.check({
      url,
      trigger,
      dispatchCertainty: 'definitely_not_dispatched'
    })
  }

  handleWindowOpen(guest: WebContents, url: string): void {
    const record = this.guests.get(guest.id)
    if (!record || record.guest !== guest) return
    const view = this.operationFor(record)
    if (!view) return
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
    await this.coordinator.shutdown()
  }

  snapshot(): {
    activeOperations: number
    downloads: number
    guests: number
    redirectMarkers: number
    stickyContexts: number
  } {
    let activeOperations = 0
    const stickyContexts = 0
    for (const record of this.guests.values()) {
      if (record.active) activeOperations += 1
    }
    return {
      activeOperations,
      downloads: this.downloads.size,
      guests: this.guests.size,
      redirectMarkers: this.redirectTargets.size,
      stickyContexts
    }
  }

  private async authorizeRequest(details: OnBeforeRequestListenerDetails): Promise<void> {
    if (this.disposed) throw new Error('browser.network_guard.closed')
    if (details.url === 'about:blank' || parseBrowserSurfaceBootstrapUrl(details.url) !== null) {
      return
    }
    const record = this.requestGuest(details)
    if (!record) {
      throw new Error('browser.network_guard.unregistered_request')
    }
    if (
      (details.webContents !== undefined && record.guest !== details.webContents) ||
      record.guest.isDestroyed()
    ) {
      throw new Error('browser.network_guard.unregistered_request')
    }

    const view = this.operationFor(record)
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

    // There is no typed task/run identity to bind a BrowserRiskApproval to. Public HTTPS stays
    // usable, but any reviewable network boundary fails closed instead of inheriting authority
    // from a completed or previous automation operation.
    const assessment = await this.policy.assess(details.url)
    if (assessment.disposition !== 'allow') {
      throw new Error('browser.unsupported_host_boundary')
    }
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
    webContents?: WebContents
    webContentsId?: number
  }): GuestRecord | undefined {
    const objectWebContentsId = details.webContents?.id
    if (
      details.webContentsId !== undefined &&
      objectWebContentsId !== undefined &&
      details.webContentsId !== objectWebContentsId
    ) {
      return undefined
    }
    const record = this.guests.get(details.webContentsId ?? objectWebContentsId ?? -1)
    if (details.webContents !== undefined && record?.guest !== details.webContents) return undefined
    return record
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
    if (abort) record.active?.controller.abort('target_closed')
    record.active?.unlinkCaller()
    record.active?.operation.close()
    record.active = undefined
    this.cancelDownloadsFor(record, true)
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

  private downloadBudgetExceeded(item: DownloadItem, exclude?: ActiveDownload): boolean {
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
    return false
  }

  private assertUsable(): void {
    if (this.disposed) throw new Error('browser.network_guard.closed')
  }
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

export function mapBrowserRiskError(error: unknown): BrowserRiskFailure | null {
  return error instanceof BrowserRiskError ? error.failure : null
}
