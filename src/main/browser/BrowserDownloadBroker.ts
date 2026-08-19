import type { DownloadItem, Event, Session, WebContents } from 'electron'
import type { BrowserArtifactReference } from '@mycopilot/protocol'

import {
  BrowserArtifactBroker,
  BrowserArtifactBrokerError,
  safeSuggestedFileName,
  type BrowserArtifactOwner,
  type BrowserArtifactOutputSession,
  type BrowserArtifactReservation
} from './BrowserArtifactBroker'

const DEFAULT_MAX_ACTIVE_DOWNLOADS = 4
const DEFAULT_MAX_SINGLE_DOWNLOAD_BYTES = 64 * 1024 * 1024
const DEFAULT_MAX_ACTIVE_BYTES = 128 * 1024 * 1024
const DOWNLOAD_SETTLE_TIMEOUT_MS = 30_000
const MAX_REGISTERED_GUESTS = 32

export type BrowserDownloadBrokerErrorCode =
  | 'browser.download.closed'
  | 'browser.download.busy'
  | 'browser.download.cancelled'
  | 'browser.download.target_closed'
  | 'browser.download.too_large'
  | 'browser.download.interrupted'
  | 'browser.download.artifact_failed'
  | 'browser.download.outcome_unknown'

export class BrowserDownloadBrokerError extends Error {
  readonly name = 'BrowserDownloadBrokerError'

  constructor(
    readonly code: BrowserDownloadBrokerErrorCode,
    readonly dispatchCertainty: 'definitely_not_dispatched' | 'possibly_dispatched'
  ) {
    super(code)
  }
}

export interface BrowserDownloadToolLease {
  ready?(): Promise<void>
  markDispatched(): void
  settle(): Promise<readonly BrowserArtifactReference[]>
  artifacts(): readonly BrowserArtifactReference[]
  finish(): void
}

export interface BrowserDownloadBrokerOptions {
  artifacts: BrowserArtifactBroker
  expectedSession: Session
  maxActiveBytes?: number
  maxActiveDownloads?: number
  maxSingleDownloadBytes?: number
}

interface GuestRecord {
  generation: number
  guest: WebContents
  surfaceId: string
}

interface ActiveTool {
  accepting: boolean
  artifacts: BrowserArtifactReference[]
  callerSignal?: AbortSignal
  cleanup?: Promise<void>
  dispatched: boolean
  downloads: Set<ActiveDownload>
  failure?: BrowserDownloadBrokerError
  finished: boolean
  guest: GuestRecord
  handleCallerAbort: () => void
  owner: BrowserArtifactOwner
  outputSession?: BrowserArtifactOutputSession
  ready: Promise<void>
  reservations: BrowserArtifactReservation[]
  settled: boolean
}

interface ActiveDownload {
  item: DownloadItem
  reservation: BrowserArtifactReservation
  settle: () => void
  lifetime: Promise<void>
  tool: ActiveTool
  handleDone: (event: Event, state: 'completed' | 'cancelled' | 'interrupted') => void
  handleUpdated: (event: Event, state: 'progressing' | 'interrupted') => void
  terminal: boolean
}

/**
 * Binds Electron downloads to the exact active Browser Tool and publishes them through the same
 * private ArtifactBroker as explicit Playwright file tools. A Tool lease does not settle until
 * every admitted DownloadItem has either committed or reached a typed terminal failure.
 */
export class BrowserDownloadBroker {
  private readonly artifactsBroker: BrowserArtifactBroker
  private readonly expectedSession: Session
  private readonly maxActiveBytes: number
  private readonly maxActiveDownloads: number
  private readonly maxSingleDownloadBytes: number
  private readonly guests = new Map<number, GuestRecord>()
  private readonly tools = new Set<ActiveTool>()
  private readonly activeDownloads = new Set<ActiveDownload>()
  private readonly closedSurfaces = new Set<string>()
  private readonly closedToolCalls = new Set<string>()
  private readonly finalizedRuns = new Set<string>()
  private readonly revokedActivations = new Set<string>()
  private disposed = false
  private installed = false

  private readonly handleWillDownload = (
    _event: Event,
    item: DownloadItem,
    webContents: WebContents
  ): void => {
    const guest = this.guests.get(webContents.id)
    if (!guest || guest.guest !== webContents) {
      safeCancel(item)
      return
    }
    const tool = [...this.tools].find(
      (candidate) => candidate.guest === guest && candidate.accepting && !candidate.finished
    )
    // Without an exact active Agent owner there is no safe run/capability binding and no approved
    // user export path in this round. Cancelling prevents Electron from falling back to the real
    // user Downloads directory. A future manual-download UX can introduce its own typed owner.
    if (!tool) {
      safeCancel(item)
      return
    }
    this.admitDownload(tool, item)
  }

  constructor(options: BrowserDownloadBrokerOptions) {
    this.artifactsBroker = options.artifacts
    this.expectedSession = options.expectedSession
    this.maxActiveBytes = positiveBound(options.maxActiveBytes, DEFAULT_MAX_ACTIVE_BYTES)
    this.maxActiveDownloads = positiveBound(
      options.maxActiveDownloads,
      DEFAULT_MAX_ACTIVE_DOWNLOADS
    )
    this.maxSingleDownloadBytes = positiveBound(
      options.maxSingleDownloadBytes,
      DEFAULT_MAX_SINGLE_DOWNLOAD_BYTES
    )
  }

  install(): void {
    this.assertUsable()
    if (this.installed) return
    this.installed = true
    this.expectedSession.on('will-download', this.handleWillDownload)
  }

  registerGuest(input: { guest: WebContents; surfaceId: string; generation: number }): void {
    this.assertUsable()
    if (
      input.guest.session !== this.expectedSession ||
      input.guest.getType() !== 'webview' ||
      !Number.isSafeInteger(input.generation) ||
      input.generation <= 0
    ) {
      throw new BrowserDownloadBrokerError(
        'browser.download.target_closed',
        'definitely_not_dispatched'
      )
    }
    const previous = this.guests.get(input.guest.id)
    if (
      previous?.guest === input.guest &&
      previous.surfaceId === input.surfaceId &&
      previous.generation === input.generation
    ) {
      return
    }
    if (!previous && this.guests.size >= MAX_REGISTERED_GUESTS) {
      throw new BrowserDownloadBrokerError('browser.download.busy', 'definitely_not_dispatched')
    }
    if (previous) this.unregisterGuest(previous.guest, previous.generation)
    this.guests.set(input.guest.id, { ...input })
  }

  unregisterGuest(guest: WebContents, generation?: number): void {
    const record = this.guests.get(guest.id)
    if (!record || record.guest !== guest || (generation && record.generation !== generation))
      return
    this.guests.delete(guest.id)
    for (const tool of [...this.tools]) {
      if (tool.guest === record) void this.abortTool(tool, 'browser.download.target_closed')
    }
  }

  beginTool(input: {
    guest: WebContents
    owner: BrowserArtifactOwner
    signal?: AbortSignal
  }): BrowserDownloadToolLease {
    this.assertUsable()
    if (
      this.finalizedRuns.has(input.owner.runId) ||
      this.revokedActivations.has(input.owner.activationId) ||
      this.closedToolCalls.has(toolCallKey(input.owner.runId, input.owner.toolCallId))
    ) {
      throw new BrowserDownloadBrokerError('browser.download.closed', 'definitely_not_dispatched')
    }
    if (this.closedSurfaces.has(surfaceKey(input.owner.surfaceId, input.owner.generation))) {
      throw new BrowserDownloadBrokerError(
        'browser.download.target_closed',
        'definitely_not_dispatched'
      )
    }
    const guest = this.guests.get(input.guest.id)
    if (
      !guest ||
      guest.guest !== input.guest ||
      guest.surfaceId !== input.owner.surfaceId ||
      guest.generation !== input.owner.generation ||
      input.guest.isDestroyed()
    ) {
      throw new BrowserDownloadBrokerError(
        'browser.download.target_closed',
        'definitely_not_dispatched'
      )
    }
    if ([...this.tools].some((tool) => tool.guest === guest && !tool.finished)) {
      throw new BrowserDownloadBrokerError('browser.download.busy', 'definitely_not_dispatched')
    }
    const tool = {} as ActiveTool
    const handleCallerAbort = (): void => {
      void this.abortTool(tool, 'browser.download.cancelled')
    }
    Object.assign(tool, {
      accepting: false,
      artifacts: [],
      callerSignal: input.signal,
      dispatched: false,
      downloads: new Set<ActiveDownload>(),
      finished: false,
      guest,
      handleCallerAbort,
      owner: { ...input.owner },
      reservations: [],
      settled: false
    })
    this.tools.add(tool)
    input.signal?.addEventListener('abort', handleCallerAbort, { once: true })
    tool.ready = this.prepareTool(tool, input)
    return {
      ready: () => tool.ready,
      markDispatched: () => {
        if (!tool.finished) {
          tool.dispatched = true
          // A prepared lease owns private destinations but has no provenance authority yet.
          // Admission starts only at the exact upstream dispatch boundary so a page timer or a
          // user's manual click cannot be misattributed to an Agent Tool while Catalog/artifact
          // preparation is still in progress.
          tool.accepting = true
        }
      },
      settle: () => this.settleTool(tool),
      artifacts: () => tool.artifacts.map((artifact) => structuredClone(artifact)),
      finish: () => this.finishTool(tool)
    }
  }

  private async prepareTool(
    tool: ActiveTool,
    input: { guest: WebContents; owner: BrowserArtifactOwner; signal?: AbortSignal }
  ): Promise<void> {
    let outputSession: BrowserArtifactOutputSession | undefined
    const reservations: BrowserArtifactReservation[] = []
    try {
      outputSession = await this.artifactsBroker.openSession()
      for (let index = 0; index < this.maxActiveDownloads; index += 1) {
        reservations.push(
          await outputSession.reserveFile({
            owner: input.owner,
            kind: 'download',
            mimeType: 'application/octet-stream',
            suggestedFileName: `download-${index + 1}.bin`
          })
        )
      }
      this.assertUsable()
      const currentGuest = this.guests.get(input.guest.id)
      if (
        tool.finished ||
        input.signal?.aborted ||
        currentGuest !== tool.guest ||
        input.guest.isDestroyed() ||
        this.finalizedRuns.has(input.owner.runId) ||
        this.revokedActivations.has(input.owner.activationId) ||
        this.closedToolCalls.has(toolCallKey(input.owner.runId, input.owner.toolCallId)) ||
        this.closedSurfaces.has(surfaceKey(input.owner.surfaceId, input.owner.generation))
      ) {
        throw new BrowserDownloadBrokerError(
          currentGuest === tool.guest
            ? 'browser.download.cancelled'
            : 'browser.download.target_closed',
          'definitely_not_dispatched'
        )
      }
      tool.outputSession = outputSession
      tool.reservations.push(...reservations)
      if (input.signal?.aborted) {
        await this.abortTool(tool, 'browser.download.cancelled')
        throw new BrowserDownloadBrokerError(
          'browser.download.cancelled',
          'definitely_not_dispatched'
        )
      }
    } catch (error) {
      await Promise.allSettled(reservations.map((reservation) => reservation.discard()))
      await outputSession?.close().catch(() => undefined)
      tool.finished = true
      tool.accepting = false
      tool.callerSignal?.removeEventListener('abort', tool.handleCallerAbort)
      this.tools.delete(tool)
      if (error instanceof BrowserDownloadBrokerError) throw error
      throw new BrowserDownloadBrokerError(
        'browser.download.artifact_failed',
        'definitely_not_dispatched'
      )
    }
  }

  async finalizeRun(runId: string): Promise<void> {
    this.finalizedRuns.add(runId)
    await Promise.allSettled(
      [...this.tools]
        .filter((tool) => tool.owner.runId === runId)
        .map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    await this.artifactsBroker.finalizeRun(runId)
  }

  async releaseRun(runId: string): Promise<void> {
    this.finalizedRuns.add(runId)
    await Promise.allSettled(
      [...this.tools]
        .filter((tool) => tool.owner.runId === runId)
        .map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    await this.artifactsBroker.releaseRun(runId)
  }

  async releaseCapability(activationId: string): Promise<void> {
    this.revokedActivations.add(activationId)
    await Promise.allSettled(
      [...this.tools]
        .filter((tool) => tool.owner.activationId === activationId)
        .map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    await this.artifactsBroker.releaseCapability(activationId)
  }

  async releaseToolCall(input: { runId: string; toolCallId: string }): Promise<void> {
    this.closedToolCalls.add(toolCallKey(input.runId, input.toolCallId))
    await Promise.allSettled(
      [...this.tools]
        .filter(
          (tool) => tool.owner.runId === input.runId && tool.owner.toolCallId === input.toolCallId
        )
        .map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    await this.artifactsBroker.releaseToolCall(input)
  }

  async releaseSurface(input: { surfaceId: string; generation: number }): Promise<void> {
    this.closedSurfaces.add(surfaceKey(input.surfaceId, input.generation))
    await Promise.allSettled(
      [...this.tools]
        .filter(
          (tool) =>
            tool.owner.surfaceId === input.surfaceId && tool.owner.generation === input.generation
        )
        .map((tool) => this.abortTool(tool, 'browser.download.target_closed'))
    )
    await this.artifactsBroker.releaseSurface(input)
  }

  snapshot(): { downloads: number; guests: number; tools: number; activeBytes: number } {
    return {
      downloads: this.activeDownloads.size,
      guests: this.guests.size,
      tools: this.tools.size,
      activeBytes: this.activeByteEstimate()
    }
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    if (this.installed) {
      this.expectedSession.removeListener('will-download', this.handleWillDownload)
      this.installed = false
    }
    await Promise.allSettled(
      [...this.tools].map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    this.guests.clear()
    this.closedSurfaces.clear()
    this.closedToolCalls.clear()
    this.finalizedRuns.clear()
    this.revokedActivations.clear()
    await Promise.allSettled(
      [...this.activeDownloads].map((download) => this.cancelDownload(download))
    )
    this.activeDownloads.clear()
    this.tools.clear()
  }

  private admitDownload(tool: ActiveTool, item: DownloadItem): void {
    if (
      this.disposed ||
      this.activeDownloads.size >= this.maxActiveDownloads ||
      this.downloadExceedsBudget(item) ||
      tool.reservations.length === 0
    ) {
      this.failTool(tool, 'browser.download.too_large')
      safeCancel(item)
      return
    }
    const reservation = tool.reservations.shift()
    if (!reservation) {
      this.failTool(tool, 'browser.download.too_large')
      safeCancel(item)
      return
    }
    const download = createActiveDownload(
      tool,
      item,
      reservation,
      (active, state) => this.finishDownload(active, state),
      (active) => {
        if (!this.downloadExceedsBudget(active.item, active)) return
        this.failTool(active.tool, 'browser.download.too_large')
        void this.cancelDownload(active)
      }
    )
    try {
      reservation.updateMetadata({
        suggestedFileName: sanitizedDownloadFileName(safeDownloadString(item, 'getFilename')),
        mimeType: sanitizedDownloadMimeType(safeDownloadString(item, 'getMimeType'))
      })
      tool.downloads.add(download)
      this.activeDownloads.add(download)
      item.once('done', download.handleDone)
      item.on('updated', download.handleUpdated)
      // Electron requires the managed destination during the synchronous will-download turn.
      // It must be set before returning to avoid a native save dialog or a stalled transfer.
      item.setSavePath(reservation.managedPath)
    } catch {
      this.removeDownloadListeners(download)
      tool.downloads.delete(download)
      this.activeDownloads.delete(download)
      download.terminal = true
      download.settle()
      this.failTool(tool, 'browser.download.artifact_failed')
      void reservation.discard()
      safeCancel(item)
      return
    }
    const state = safeDownloadState(item)
    if (state !== 'progressing') {
      void this.finishDownload(download, state)
    } else if (this.downloadExceedsBudget(item, download)) {
      this.failTool(tool, 'browser.download.too_large')
      void this.cancelDownload(download)
    }
  }

  private async finishDownload(
    download: ActiveDownload,
    state: 'completed' | 'cancelled' | 'interrupted'
  ): Promise<void> {
    if (download.terminal) return
    download.terminal = true
    this.removeDownloadListeners(download)
    this.activeDownloads.delete(download)
    try {
      if (state !== 'completed') {
        this.failTool(
          download.tool,
          state === 'interrupted' ? 'browser.download.interrupted' : 'browser.download.cancelled'
        )
        await download.reservation.discard()
        return
      }
      const artifact = await download.reservation.commit()
      download.tool.artifacts.push(artifact)
    } catch (error) {
      this.failTool(
        download.tool,
        error instanceof BrowserArtifactBrokerError &&
          (error.code === 'browser.artifact.too_large' ||
            error.code === 'browser.artifact.capacity')
          ? 'browser.download.too_large'
          : 'browser.download.artifact_failed'
      )
      await download.reservation.discard().catch(() => undefined)
    } finally {
      download.tool.downloads.delete(download)
      download.settle()
    }
  }

  private async cancelDownload(download: ActiveDownload): Promise<void> {
    if (download.terminal) return
    download.terminal = true
    this.removeDownloadListeners(download)
    this.activeDownloads.delete(download)
    safeCancel(download.item)
    await download.reservation.discard().catch(() => undefined)
    download.tool.downloads.delete(download)
    download.settle()
  }

  private async settleTool(tool: ActiveTool): Promise<readonly BrowserArtifactReference[]> {
    await tool.ready
    if (tool.settled) return tool.artifacts.map((artifact) => structuredClone(artifact))
    // Drain Electron events queued by the page action without a time-based sleep. Once this turn
    // completes, later downloads are no longer attributed to this settled Tool.
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
    tool.accepting = false
    if (tool.downloads.size > 0) {
      const lifetimes = [...tool.downloads].map((download) => download.lifetime)
      const completed = await settleDownloadsWithin(lifetimes, DOWNLOAD_SETTLE_TIMEOUT_MS)
      if (!completed) {
        this.failTool(tool, 'browser.download.outcome_unknown')
        await Promise.allSettled(
          [...tool.downloads].map((download) => this.cancelDownload(download))
        )
      }
    }
    await this.cleanupTool(tool)
    tool.settled = true
    if (tool.failure) throw tool.failure
    return tool.artifacts.map((artifact) => structuredClone(artifact))
  }

  private finishTool(tool: ActiveTool): void {
    if (tool.finished) return
    tool.finished = true
    tool.accepting = false
    tool.callerSignal?.removeEventListener('abort', tool.handleCallerAbort)
    this.tools.delete(tool)
    if (!tool.settled && tool.downloads.size > 0) {
      void this.abortTool(tool, 'browser.download.outcome_unknown')
    } else if (!tool.settled) void this.cleanupTool(tool)
  }

  private async abortTool(tool: ActiveTool, code: BrowserDownloadBrokerErrorCode): Promise<void> {
    if (tool.finished && tool.downloads.size === 0 && tool.cleanup) {
      await tool.cleanup
      return
    }
    this.failTool(tool, code)
    tool.accepting = false
    tool.finished = true
    tool.callerSignal?.removeEventListener('abort', tool.handleCallerAbort)
    this.tools.delete(tool)
    await Promise.allSettled([...tool.downloads].map((download) => this.cancelDownload(download)))
    await this.cleanupTool(tool)
  }

  private cleanupTool(tool: ActiveTool): Promise<void> {
    tool.cleanup ??= (async () => {
      const unused = tool.reservations.splice(0)
      await Promise.allSettled(unused.map((reservation) => reservation.discard()))
      await tool.outputSession?.close().catch(() => undefined)
    })()
    return tool.cleanup
  }

  private failTool(tool: ActiveTool, code: BrowserDownloadBrokerErrorCode): void {
    tool.failure ??= new BrowserDownloadBrokerError(
      code,
      tool.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
    )
  }

  private removeDownloadListeners(download: ActiveDownload): void {
    download.item.removeListener('done', download.handleDone)
    download.item.removeListener('updated', download.handleUpdated)
  }

  private downloadExceedsBudget(item: DownloadItem, exclude?: ActiveDownload): boolean {
    const candidate = downloadByteEstimate(item)
    return (
      candidate > this.maxSingleDownloadBytes ||
      candidate + this.activeByteEstimate(exclude) > this.maxActiveBytes
    )
  }

  private activeByteEstimate(exclude?: ActiveDownload): number {
    let bytes = 0
    for (const download of this.activeDownloads) {
      if (download === exclude) continue
      bytes += downloadByteEstimate(download.item)
      if (bytes > this.maxActiveBytes) return bytes
    }
    return bytes
  }

  private assertUsable(): void {
    if (this.disposed) {
      throw new BrowserDownloadBrokerError('browser.download.closed', 'definitely_not_dispatched')
    }
  }
}

function createActiveDownload(
  tool: ActiveTool,
  item: DownloadItem,
  reservation: BrowserArtifactReservation,
  finish: (
    download: ActiveDownload,
    state: 'completed' | 'cancelled' | 'interrupted'
  ) => Promise<void>,
  updated: (download: ActiveDownload) => void
): ActiveDownload {
  let settle!: () => void
  const lifetime = new Promise<void>((resolveLifetime) => {
    settle = once(resolveLifetime)
  })
  const download = {} as ActiveDownload
  Object.assign(download, {
    item,
    reservation,
    settle,
    lifetime,
    tool,
    handleDone: (_event: Event, state: 'completed' | 'cancelled' | 'interrupted') => {
      void finish(download, state)
    },
    handleUpdated: () => updated(download),
    terminal: false
  })
  return download
}

function sanitizedDownloadFileName(value: string): string {
  const pathFree = value.split(/[\\/]/u).at(-1) ?? ''
  const normalized = sanitizeDownloadCharacters(pathFree.normalize('NFKC'))
    .replace(/^\.+/u, '')
    .trim()
    .slice(0, 120)
  try {
    return safeSuggestedFileName(normalized || 'download.bin')
  } catch {
    return 'download.bin'
  }
}

function sanitizeDownloadCharacters(value: string): string {
  let sanitized = ''
  for (const character of value) {
    const code = character.charCodeAt(0)
    sanitized += code <= 0x1f || code === 0x7f || '<>:"/\\|?*'.includes(character) ? '_' : character
  }
  return sanitized
}

function sanitizedDownloadMimeType(value: string): string {
  const normalized = value.split(';', 1)[0]?.trim().toLowerCase() ?? ''
  const allowed = new Set([
    'application/json',
    'application/octet-stream',
    'application/pdf',
    'application/zip',
    'image/gif',
    'image/jpeg',
    'image/png',
    'image/webp',
    'text/csv',
    'text/plain',
    'video/mp4',
    'video/webm'
  ])
  return allowed.has(normalized) ? normalized : 'application/octet-stream'
}

function safeDownloadString(item: DownloadItem, method: 'getFilename' | 'getMimeType'): string {
  try {
    const value = item[method]()
    return typeof value === 'string' ? value : ''
  } catch {
    return ''
  }
}

function safeDownloadState(
  item: DownloadItem
): 'progressing' | 'completed' | 'cancelled' | 'interrupted' {
  try {
    const state = item.getState()
    return state === 'completed' || state === 'cancelled' || state === 'interrupted'
      ? state
      : 'progressing'
  } catch {
    return 'progressing'
  }
}

function downloadByteEstimate(item: DownloadItem): number {
  return Math.max(
    safeDownloadBytes(item, 'getTotalBytes'),
    safeDownloadBytes(item, 'getReceivedBytes')
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

function safeCancel(item: DownloadItem): void {
  try {
    item.cancel()
  } catch {
    // A terminal DownloadItem may reject cancellation.
  }
}

function once(callback: () => void): () => void {
  let called = false
  return () => {
    if (called) return
    called = true
    callback()
  }
}

async function settleDownloadsWithin(
  lifetimes: readonly Promise<void>[],
  timeoutMs: number
): Promise<boolean> {
  if (lifetimes.length === 0) return true
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      Promise.allSettled(lifetimes).then(() => true),
      new Promise<boolean>((resolve) => {
        timer = setTimeout(() => resolve(false), timeoutMs)
      })
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

function positiveBound(value: number | undefined, fallback: number): number {
  return Number.isSafeInteger(value) && (value ?? 0) > 0 ? (value as number) : fallback
}

function surfaceKey(surfaceId: string, generation: number): string {
  return `${surfaceId}\u0000${generation}`
}

function toolCallKey(runId: string, toolCallId: string): string {
  return `${runId}\u0000${toolCallId}`
}
