import { createHash, randomUUID } from 'node:crypto'
import { constants, createReadStream, lstatSync, realpathSync } from 'node:fs'
import { copyFile, link, lstat, unlink } from 'node:fs/promises'
import { basename, extname, isAbsolute, join, parse } from 'node:path'
import type { DownloadItem, Event, Session, WebContents } from 'electron'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  parseBrowserDownloadRecord,
  parseBrowserDownloadSettingsRecord,
  type BrowserAgentDownloadSnapshot,
  type BrowserAgentDownloadStatus,
  type BrowserDownloadCenterAction,
  type BrowserDownloadCenterItem,
  type BrowserDownloadCenterSnapshot,
  type BrowserDownloadRecord,
  type BrowserDownloadReference,
  type BrowserDownloadRegistrationInput,
  type BrowserDownloadSettingsRecord
} from '@mycopilot/protocol'

import { safeSuggestedFileName, type BrowserArtifactOwner } from './BrowserArtifactBroker'

const DEFAULT_MAX_ACTIVE_DOWNLOADS = 8
const DEFAULT_MAX_SINGLE_AGENT_DOWNLOAD_BYTES = 2 * 1024 * 1024 * 1024
const DEFAULT_MAX_ACTIVE_AGENT_BYTES = 4 * 1024 * 1024 * 1024
const MAX_REGISTERED_GUESTS = 32
const MAX_FILE_NAME_ATTEMPTS = 10_000
const MAX_DOWNLOAD_CENTER_ENTRIES = 50
const MAX_AGENT_DOWNLOAD_ENTRIES = 50
const DOWNLOAD_CENTER_UPDATE_INTERVAL_MS = 100

export type BrowserDownloadBrokerErrorCode =
  | 'browser.download.closed'
  | 'browser.download.busy'
  | 'browser.download.cancelled'
  | 'browser.download.target_closed'
  | 'browser.download.too_large'
  | 'browser.download.interrupted'
  | 'browser.download.destination_unavailable'
  | 'browser.download.registration_failed'
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

export interface BrowserDownloadOwner extends BrowserArtifactOwner {
  conversationId?: string
}

export interface BrowserDownloadToolLease {
  claimCreatedGuest(input: {
    action: 'new' | 'popup'
    generation: number
    guest: WebContents
    surfaceId: string
  }): Promise<void>
  expectTargetClose(input: { generation: number; surfaceId: string }): void
  ready?(): Promise<void>
  markDispatched(): void
  settle(): Promise<readonly BrowserDownloadReference[]>
  downloads(): readonly BrowserDownloadReference[]
  progress?(): readonly BrowserAgentDownloadStatus[]
  finish(): void
}

export interface BrowserDownloadBrokerOptions {
  expectedSession: Session
  initialSettings: BrowserDownloadSettingsRecord
  registerDownload(input: BrowserDownloadRegistrationInput): Promise<BrowserDownloadRecord>
  systemDownloadDirectory: string
  onHistoryChanged?: () => void
  maxActiveBytes?: number
  maxActiveDownloads?: number
  maxSingleDownloadBytes?: number
}

interface GuestRecord {
  generation: number
  guest: WebContents
  surfaceId: string
  trustedNativePopup?: boolean
  nativePopupReady?: () => boolean
}

type ToolOwner = Omit<BrowserDownloadOwner, 'generation' | 'surfaceId'>

interface ActiveTool {
  accepting: boolean
  callerSignal?: AbortSignal
  createdGuestClaimsStarted: Record<'new' | 'popup', number>
  dispatched: boolean
  downloads: Set<ActiveDownload>
  downloadIds: Set<string>
  expectedTargetCloses: Set<string>
  failure?: BrowserDownloadBrokerError
  finished: boolean
  guests: Set<GuestRecord>
  handleCallerAbort: () => void
  maxCreatedGuests: Record<'new' | 'popup', number>
  owner: ToolOwner
  references: BrowserDownloadReference[]
  ready: Promise<void>
  settled: boolean
}

interface ActiveDownload {
  centerRecord: DownloadCenterRecord
  centerVisible: boolean
  destination: string | null
  downloadId: string
  displayName: string
  failureCode?: BrowserDownloadBrokerErrorCode
  finalization?: Promise<void>
  guest: GuestRecord
  handleDone: (event: Event, state: 'completed' | 'cancelled' | 'interrupted') => void
  handleUpdated: (event: Event, state: 'progressing' | 'interrupted') => void
  item: DownloadItem
  mimeType: string
  owner?: ToolOwner
  reference?: BrowserDownloadReference
  sourceOrigin: string | null
  tempPath: string | null
  terminal: boolean
  tool?: ActiveTool
  waitsForDestinationConfirmation: boolean
}

interface AgentDownloadRecord {
  owner: ToolOwner
  status: BrowserAgentDownloadStatus
}

interface DownloadCenterRecord {
  absolutePath: string | null
  bytesPerSecond: number
  displayName: string
  downloadId: string
  receivedBytes: number
  source: 'manual' | 'agent'
  sourceUrl: string | null
  startedAt: number
  state: BrowserDownloadCenterItem['state']
  totalBytes: number
  updatedAt: number
}

export interface BrowserDownloadCenterResource {
  absolutePath: string | null
  sourceUrl: string | null
}

/**
 * Owns every download from the managed browser partition.
 *
 * Automatic downloads first land in a hidden temporary file inside the configured destination.
 * When the user enables save prompts, Electron owns the native dialog and writes to the explicitly
 * selected path instead. Both paths are hashed and committed as durable Core records; public
 * references never contain the destination or user name.
 */
export class BrowserDownloadBroker {
  private readonly expectedSession: Session
  private readonly systemDownloadDirectory: string
  private readonly registerDownloadRecord: BrowserDownloadBrokerOptions['registerDownload']
  private readonly onHistoryChanged?: () => void
  private readonly historyChangedListeners = new Set<() => void>()
  private readonly downloadCenterChangedListeners = new Set<
    (snapshot: BrowserDownloadCenterSnapshot) => void
  >()
  private readonly maxActiveBytes: number
  private readonly maxActiveDownloads: number
  private readonly maxSingleDownloadBytes: number
  private settingsRecord: BrowserDownloadSettingsRecord
  private destinationDirectory: string
  private readonly guests = new Map<number, GuestRecord>()
  private readonly tools = new Set<ActiveTool>()
  private readonly activeDownloads = new Set<ActiveDownload>()
  private readonly downloadCenterRecords = new Map<string, DownloadCenterRecord>()
  private readonly agentDownloadRecords = new Map<string, AgentDownloadRecord>()
  private readonly closedSurfaces = new Set<string>()
  private readonly closedToolCalls = new Set<string>()
  private readonly finalizedRuns = new Set<string>()
  private readonly revokedActivations = new Set<string>()
  private disposed = false
  private installed = false
  private downloadCenterRevision = 0
  private agentDownloadRevision = 0
  private downloadCenterChangedTimer: ReturnType<typeof setTimeout> | undefined

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
    if (guest.trustedNativePopup && !guest.nativePopupReady?.()) {
      safeCancel(item)
      return
    }
    const tool = [...this.tools].find(
      (candidate) => candidate.guests.has(guest) && !candidate.finished
    )
    if (tool && !tool.accepting) {
      this.failTool(tool, 'browser.download.outcome_unknown')
      safeCancel(item)
      return
    }
    this.admitDownload(tool, guest, item)
  }

  constructor(options: BrowserDownloadBrokerOptions) {
    this.expectedSession = options.expectedSession
    if (!options.systemDownloadDirectory || !isAbsolute(options.systemDownloadDirectory)) {
      throw new Error('browser.download.destination_unavailable')
    }
    this.systemDownloadDirectory = options.systemDownloadDirectory
    this.registerDownloadRecord = options.registerDownload
    this.onHistoryChanged = options.onHistoryChanged
    this.maxActiveBytes = positiveBound(options.maxActiveBytes, DEFAULT_MAX_ACTIVE_AGENT_BYTES)
    this.maxActiveDownloads = positiveBound(
      options.maxActiveDownloads,
      DEFAULT_MAX_ACTIVE_DOWNLOADS
    )
    this.maxSingleDownloadBytes = positiveBound(
      options.maxSingleDownloadBytes,
      DEFAULT_MAX_SINGLE_AGENT_DOWNLOAD_BYTES
    )
    this.settingsRecord = parseBrowserDownloadSettingsRecord(options.initialSettings)
    this.destinationDirectory = this.configuredDestination(this.settingsRecord)
  }

  install(): void {
    this.assertUsable()
    if (this.installed) return
    this.installed = true
    this.expectedSession.on('will-download', this.handleWillDownload)
  }

  settings(): BrowserDownloadSettingsRecord {
    return structuredClone(this.settingsRecord)
  }

  downloadDirectory(): string {
    return this.destinationDirectory
  }

  onHistoryChangedEvent(listener: () => void): () => void {
    this.assertUsable()
    this.historyChangedListeners.add(listener)
    return () => this.historyChangedListeners.delete(listener)
  }

  downloadCenterSnapshot(): BrowserDownloadCenterSnapshot {
    this.assertUsable()
    const activeById = new Map(
      [...this.activeDownloads].map((download) => [download.downloadId, download] as const)
    )
    const downloads = [...this.downloadCenterRecords.values()]
      .sort(compareDownloadCenterRecords)
      .map((record) => {
        const active = activeById.get(record.downloadId)
        const canResume = Boolean(
          active &&
          !active.terminal &&
          (record.state === 'paused' || record.state === 'interrupted') &&
          safeCanResume(active.item)
        )
        return {
          downloadId: record.downloadId,
          displayName: record.displayName,
          source: record.source,
          state: record.state,
          receivedBytes: record.receivedBytes,
          totalBytes: record.totalBytes,
          bytesPerSecond: record.bytesPerSecond,
          startedAt: record.startedAt,
          updatedAt: record.updatedAt,
          canPause: Boolean(active && !active.terminal && record.state === 'progressing'),
          canResume,
          canCancel: Boolean(active && !active.terminal),
          canReveal: record.absolutePath !== null,
          canCopyUrl: record.sourceUrl !== null,
          canCopyPath: record.absolutePath !== null
        }
      })
    return {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      revision: this.downloadCenterRevision,
      downloads
    }
  }

  /** Returns only path-free downloads owned by the current Agent run. */
  agentDownloadSnapshot(input: {
    runId: string
    activationId: string
    conversationId?: string
  }): BrowserAgentDownloadSnapshot {
    this.assertUsable()
    const downloads = [...this.agentDownloadRecords.values()]
      .filter(
        (record) =>
          record.owner.runId === input.runId &&
          record.owner.activationId === input.activationId &&
          (input.conversationId === undefined ||
            record.owner.conversationId === input.conversationId)
      )
      .sort((left, right) => right.status.startedAt - left.status.startedAt)
      .map((record) => structuredClone(record.status))
    return {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      revision: this.agentDownloadRevision,
      downloads
    }
  }

  onDownloadCenterChangedEvent(
    listener: (snapshot: BrowserDownloadCenterSnapshot) => void
  ): () => void {
    this.assertUsable()
    this.downloadCenterChangedListeners.add(listener)
    return () => this.downloadCenterChangedListeners.delete(listener)
  }

  downloadCenterResource(downloadId: string): BrowserDownloadCenterResource | null {
    this.assertUsable()
    const record = this.downloadCenterRecords.get(downloadId)
    return record ? { absolutePath: record.absolutePath, sourceUrl: record.sourceUrl } : null
  }

  async performDownloadCenterAction(
    downloadId: string,
    action: Extract<BrowserDownloadCenterAction, 'pause' | 'resume' | 'cancel' | 'remove'>
  ): Promise<'performed' | 'unavailable'> {
    this.assertUsable()
    const record = this.downloadCenterRecords.get(downloadId)
    if (!record) return 'unavailable'
    const download = [...this.activeDownloads].find(
      (candidate) => candidate.downloadId === downloadId && !candidate.terminal
    )

    if (action === 'remove') {
      if (download) return 'unavailable'
      this.downloadCenterRecords.delete(downloadId)
      this.markDownloadCenterChanged(true)
      return 'performed'
    }
    if (!download) return 'unavailable'

    if (action === 'pause') {
      if (record.state !== 'progressing' || !safePause(download.item)) return 'unavailable'
      this.refreshDownloadCenterRecord(download, 'paused', true)
      return 'performed'
    }
    if (action === 'resume') {
      if (
        (record.state !== 'paused' && record.state !== 'interrupted') ||
        !safeResume(download.item)
      ) {
        return 'unavailable'
      }
      this.refreshDownloadCenterRecord(download, 'progressing', true)
      return 'performed'
    }

    if (download.tool) this.failTool(download.tool, 'browser.download.cancelled')
    await this.cancelDownload(download)
    return 'performed'
  }

  updateSettings(value: BrowserDownloadSettingsRecord): void {
    this.assertUsable()
    const next = parseBrowserDownloadSettingsRecord(value)
    const destination = this.configuredDestination(next)
    this.settingsRecord = next
    this.destinationDirectory = destination
  }

  registerGuest(input: {
    guest: WebContents
    surfaceId: string
    generation: number
    /** Main-only proof for the exact child admitted by the managed native popup host. */
    trustedNativePopup?: boolean
    /** Exact Main-owned admission state; pending native popups cannot become manual downloads. */
    nativePopupReady?: () => boolean
  }): void {
    this.assertUsable()
    if (
      input.guest.session !== this.expectedSession ||
      (input.guest.getType() !== 'webview' &&
        !(input.trustedNativePopup && input.guest.getType() === 'window')) ||
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
    if (!record || record.guest !== guest || (generation && record.generation !== generation)) {
      return
    }
    this.guests.delete(guest.id)
    for (const tool of [...this.tools]) {
      if (!tool.guests.has(record)) continue
      const key = surfaceKey(record.surfaceId, record.generation)
      if (tool.expectedTargetCloses.delete(key)) {
        tool.guests.delete(record)
        for (const download of [...tool.downloads]) {
          if (download.guest === record) void this.cancelDownload(download)
        }
      } else {
        void this.abortTool(tool, 'browser.download.target_closed')
      }
    }
  }

  beginTool(input: {
    guest: WebContents
    owner: BrowserDownloadOwner
    signal?: AbortSignal
  }): BrowserDownloadToolLease {
    this.assertToolOwner(input.owner)
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
    if ([...this.tools].some((tool) => tool.guests.has(guest) && !tool.finished)) {
      throw new BrowserDownloadBrokerError('browser.download.busy', 'definitely_not_dispatched')
    }
    return this.createTool(ownerBase(input.owner), [guest], input.signal, { new: 0, popup: 4 })
  }

  beginTargetCreationTool(input: {
    owner: ToolOwner
    signal?: AbortSignal
  }): BrowserDownloadToolLease {
    this.assertToolOwner(input.owner)
    return this.createTool({ ...input.owner }, [], input.signal, { new: 1, popup: 4 })
  }

  private createTool(
    owner: ToolOwner,
    guests: GuestRecord[],
    signal: AbortSignal | undefined,
    maxCreatedGuests: Record<'new' | 'popup', number>
  ): BrowserDownloadToolLease {
    const tool = {} as ActiveTool
    const handleCallerAbort = (): void => {
      void this.abortTool(tool, 'browser.download.cancelled')
    }
    Object.assign(tool, {
      accepting: false,
      callerSignal: signal,
      createdGuestClaimsStarted: { new: 0, popup: 0 },
      dispatched: false,
      downloads: new Set<ActiveDownload>(),
      downloadIds: new Set<string>(),
      expectedTargetCloses: new Set<string>(),
      finished: false,
      guests: new Set(guests),
      handleCallerAbort,
      maxCreatedGuests,
      owner,
      references: [],
      ready: Promise.resolve(),
      settled: false
    })
    this.tools.add(tool)
    signal?.addEventListener('abort', handleCallerAbort, { once: true })
    if (signal?.aborted) void this.abortTool(tool, 'browser.download.cancelled')
    return this.toolLease(tool)
  }

  private toolLease(tool: ActiveTool): BrowserDownloadToolLease {
    return {
      claimCreatedGuest: async (input) => await this.claimCreatedGuest(tool, input),
      expectTargetClose: (input) => {
        const guest = [...tool.guests].find(
          (candidate) =>
            candidate.surfaceId === input.surfaceId && candidate.generation === input.generation
        )
        if (tool.finished || !guest) {
          throw new BrowserDownloadBrokerError(
            'browser.download.target_closed',
            tool.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
          )
        }
        tool.expectedTargetCloses.add(surfaceKey(input.surfaceId, input.generation))
      },
      ready: () => tool.ready,
      markDispatched: () => {
        if (!tool.finished) {
          tool.dispatched = true
          tool.accepting = true
        }
      },
      settle: () => this.settleTool(tool),
      downloads: () => tool.references.map((reference) => structuredClone(reference)),
      progress: () => this.agentProgressForTool(tool),
      finish: () => this.finishTool(tool)
    }
  }

  private async claimCreatedGuest(
    tool: ActiveTool,
    input: {
      action: 'new' | 'popup'
      generation: number
      guest: WebContents
      surfaceId: string
    }
  ): Promise<void> {
    await tool.ready
    this.assertUsable()
    const guest = this.guests.get(input.guest.id)
    if (
      tool.finished ||
      !tool.dispatched ||
      tool.createdGuestClaimsStarted[input.action] >= tool.maxCreatedGuests[input.action] ||
      !guest ||
      guest.guest !== input.guest ||
      guest.generation !== input.generation ||
      guest.surfaceId !== input.surfaceId ||
      input.guest.isDestroyed() ||
      tool.guests.has(guest) ||
      [...this.tools].some(
        (candidate) => candidate !== tool && candidate.guests.has(guest) && !candidate.finished
      ) ||
      this.closedSurfaces.has(surfaceKey(input.surfaceId, input.generation))
    ) {
      throw new BrowserDownloadBrokerError(
        'browser.download.target_closed',
        tool.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
      )
    }
    tool.createdGuestClaimsStarted[input.action] += 1
    tool.guests.add(guest)
    if (input.action === 'popup' && guest.trustedNativePopup) {
      // Native OAuth callbacks may close themselves. Mark the exact child at claim time,
      // before any manager/guard destruction listener can race to release its resources.
      tool.expectedTargetCloses.add(surfaceKey(guest.surfaceId, guest.generation))
    }
  }

  async finalizeRun(runId: string): Promise<void> {
    this.finalizedRuns.add(runId)
    await this.abortMatchingTools((tool) => tool.owner.runId === runId)
  }

  async releaseRun(runId: string): Promise<void> {
    await this.finalizeRun(runId)
  }

  async releaseCapability(activationId: string): Promise<void> {
    this.revokedActivations.add(activationId)
    await this.abortMatchingTools((tool) => tool.owner.activationId === activationId)
  }

  async releaseToolCall(input: { runId: string; toolCallId: string }): Promise<void> {
    this.closedToolCalls.add(toolCallKey(input.runId, input.toolCallId))
    await this.abortMatchingTools(
      (tool) => tool.owner.runId === input.runId && tool.owner.toolCallId === input.toolCallId
    )
  }

  async releaseSurface(input: { surfaceId: string; generation: number }): Promise<void> {
    this.closedSurfaces.add(surfaceKey(input.surfaceId, input.generation))
    const cleanups: Promise<unknown>[] = []
    for (const tool of [...this.tools]) {
      const guest = [...tool.guests].find(
        (candidate) =>
          candidate.surfaceId === input.surfaceId && candidate.generation === input.generation
      )
      if (!guest) continue
      const key = surfaceKey(input.surfaceId, input.generation)
      if (tool.expectedTargetCloses.delete(key)) {
        tool.guests.delete(guest)
        for (const download of [...tool.downloads]) {
          if (download.guest === guest) cleanups.push(this.cancelDownload(download))
        }
      } else {
        cleanups.push(this.abortTool(tool, 'browser.download.target_closed'))
      }
    }
    await Promise.allSettled(cleanups)
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
    if (this.downloadCenterChangedTimer) {
      clearTimeout(this.downloadCenterChangedTimer)
      this.downloadCenterChangedTimer = undefined
    }
    await Promise.allSettled(
      [...this.tools].map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    await Promise.allSettled(
      [...this.activeDownloads].map((download) => this.cancelDownload(download))
    )
    this.guests.clear()
    this.tools.clear()
    this.activeDownloads.clear()
    this.downloadCenterRecords.clear()
    this.agentDownloadRecords.clear()
    this.closedSurfaces.clear()
    this.closedToolCalls.clear()
    this.finalizedRuns.clear()
    this.revokedActivations.clear()
    this.historyChangedListeners.clear()
    this.downloadCenterChangedListeners.clear()
  }

  private admitDownload(
    tool: ActiveTool | undefined,
    guest: GuestRecord,
    item: DownloadItem
  ): void {
    if (tool && !tool.owner.conversationId) {
      this.failTool(tool, 'browser.download.registration_failed')
      safeCancel(item)
      return
    }
    if (
      this.disposed ||
      this.activeDownloads.size >= this.maxActiveDownloads ||
      (tool && this.downloadExceedsAgentBudget(item))
    ) {
      if (tool) this.failTool(tool, 'browser.download.too_large')
      safeCancel(item)
      return
    }

    let configuredDestination: string
    try {
      configuredDestination = canonicalDownloadDirectory(this.destinationDirectory)
    } catch {
      if (tool) this.failTool(tool, 'browser.download.destination_unavailable')
      safeCancel(item)
      return
    }

    const downloadId = `browser-download:${randomUUID()}`
    const displayName = sanitizedDownloadFileName(safeDownloadString(item, 'getFilename'))
    const mimeType = sanitizedDownloadMimeType(safeDownloadString(item, 'getMimeType'))
    const owner = tool?.owner
    const now = Date.now()
    const centerRecord: DownloadCenterRecord = {
      absolutePath: null,
      bytesPerSecond: safeDownloadBytesPerSecond(item),
      displayName,
      downloadId,
      receivedBytes: safeDownloadBytes(item, 'getReceivedBytes'),
      source: owner ? 'agent' : 'manual',
      sourceUrl: safeDownloadUrl(item),
      startedAt: safeDownloadStartedAt(item, now),
      state: safeDownloadTransferState(item, 'progressing'),
      totalBytes: safeDownloadBytes(item, 'getTotalBytes'),
      updatedAt: now
    }
    // Native save dialogs are outside Browser Tool control and would strand an Agent run.
    // Only an admitted Agent tool can bypass the user's manual-download prompt preference.
    const promptForDestination = !owner && this.settingsRecord.askWhereToSave
    const destination = promptForDestination ? null : configuredDestination
    const tempPath = promptForDestination
      ? null
      : join(configuredDestination, `.mycopilot-download-${downloadId.slice(17)}.part`)
    const download = createActiveDownload({
      centerRecord,
      destination,
      downloadId,
      displayName,
      guest,
      item,
      mimeType,
      owner,
      sourceOrigin: safeDownloadOrigin(item),
      tempPath,
      tool,
      waitsForDestinationConfirmation: promptForDestination,
      finish: (active, state) => this.finishDownload(active, state),
      updated: (active, state) => {
        this.refreshDownloadCenterRecord(active, safeDownloadTransferState(active.item, state))
        if (!active.owner || !this.downloadExceedsAgentBudget(active.item, active)) return
        if (active.tool) this.failTool(active.tool, 'browser.download.too_large')
        void this.cancelDownload(active, 'browser.download.too_large')
      }
    })
    try {
      if (tool) {
        tool.downloads.add(download)
        tool.downloadIds.add(downloadId)
        this.agentDownloadRecords.set(downloadId, {
          owner: { ...tool.owner },
          status: {
            downloadId,
            displayName,
            mimeType,
            state: promptForDestination ? 'awaiting_destination' : 'progressing',
            receivedBytes: centerRecord.receivedBytes,
            totalBytes: centerRecord.totalBytes,
            bytesPerSecond: promptForDestination ? 0 : centerRecord.bytesPerSecond,
            startedAt: centerRecord.startedAt,
            updatedAt: now
          }
        })
        this.markAgentDownloadChanged()
      }
      this.activeDownloads.add(download)
      item.once('done', download.handleDone)
      item.on('updated', download.handleUpdated)
      if (promptForDestination) {
        item.setSaveDialogOptions({ defaultPath: join(configuredDestination, displayName) })
      } else {
        item.setSavePath(tempPath as string)
      }
      if (!promptForDestination) {
        this.refreshDownloadCenterRecord(
          download,
          safeDownloadTransferState(item, 'progressing'),
          true
        )
      }
    } catch {
      this.removeDownloadListeners(download)
      tool?.downloads.delete(download)
      this.activeDownloads.delete(download)
      this.downloadCenterRecords.delete(downloadId)
      this.agentDownloadRecords.delete(downloadId)
      download.terminal = true
      if (tool) this.failTool(tool, 'browser.download.destination_unavailable')
      if (tempPath) void removeFile(tempPath)
      safeCancel(item)
      return
    }
    const state = safeDownloadState(item)
    if (state !== 'progressing') {
      download.finalization ??= this.finishDownload(download, state)
    } else if (owner && this.downloadExceedsAgentBudget(item, download)) {
      if (tool) this.failTool(tool, 'browser.download.too_large')
      void this.cancelDownload(download, 'browser.download.too_large')
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
    this.refreshDownloadCenterRecord(download, state, true)
    let terminalState: BrowserDownloadCenterItem['state'] = state
    let retainedPath: string | null = null
    let completedSize: number | undefined
    try {
      if (state !== 'completed') {
        if (download.tool) {
          this.failTool(
            download.tool,
            state === 'interrupted' ? 'browser.download.interrupted' : 'browser.download.cancelled'
          )
        }
        download.failureCode =
          state === 'interrupted' ? 'browser.download.interrupted' : 'browser.download.cancelled'
        if (download.tempPath) await removeFile(download.tempPath)
        return
      }

      const completedPath = download.tempPath ?? safeCompletedDownloadPath(download.item)
      const identity = await hashRegularFile(completedPath)
      completedSize = identity.sizeBytes
      if (download.owner && identity.sizeBytes > this.maxSingleDownloadBytes) {
        if (download.tool) this.failTool(download.tool, 'browser.download.too_large')
        download.failureCode = 'browser.download.too_large'
        terminalState = 'cancelled'
        if (download.tempPath) await removeFile(download.tempPath)
        return
      }
      const absolutePath = download.tempPath
        ? await publishWithoutOverwrite(
            download.tempPath,
            download.destination as string,
            download.displayName
          )
        : completedPath
      download.centerRecord.displayName = basename(absolutePath)
      let record: BrowserDownloadRecord
      try {
        record = parseBrowserDownloadRecord(
          await this.registerDownloadRecord({
            schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
            downloadId: download.downloadId,
            source: download.owner ? 'agent' : 'manual',
            displayName: basename(absolutePath),
            mimeType: download.mimeType,
            sizeBytes: identity.sizeBytes,
            sha256: identity.sha256,
            absolutePath,
            sourceOrigin: download.sourceOrigin,
            conversationId: download.owner?.conversationId ?? null,
            runId: download.owner?.runId ?? null,
            callId: download.owner?.toolCallId ?? null,
            createdAt: Date.now()
          })
        )
      } catch {
        // A native save dialog makes the selected path user-owned. Never delete that file merely
        // because metadata registration failed; automatic staging remains transactional.
        if (download.tempPath) {
          await removeFile(absolutePath)
        } else {
          retainedPath = absolutePath
        }
        throw new BrowserDownloadBrokerError(
          'browser.download.registration_failed',
          download.tool?.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
        )
      }
      retainedPath = absolutePath
      download.centerRecord.displayName = record.displayName
      const reference = publicDownloadReference(record)
      download.reference = reference
      download.tool?.references.push(reference)
      notifyHistoryChanged(this.onHistoryChanged, this.historyChangedListeners)
    } catch (error) {
      terminalState = retainedPath ? 'completed' : 'interrupted'
      download.failureCode =
        error instanceof BrowserDownloadBrokerError
          ? error.code
          : 'browser.download.registration_failed'
      if (download.tool) {
        this.failTool(
          download.tool,
          error instanceof BrowserDownloadBrokerError
            ? error.code
            : 'browser.download.registration_failed'
        )
      }
      if (download.tempPath) await removeFile(download.tempPath)
    } finally {
      this.finishDownloadCenterRecord(download, terminalState, retainedPath, completedSize)
      this.finishAgentDownloadRecord(download, terminalState)
      download.tool?.downloads.delete(download)
    }
  }

  private async cancelDownload(
    download: ActiveDownload,
    failureCode: BrowserDownloadBrokerErrorCode = 'browser.download.cancelled'
  ): Promise<void> {
    if (download.terminal) return
    download.terminal = true
    this.removeDownloadListeners(download)
    this.activeDownloads.delete(download)
    safeCancel(download.item)
    download.failureCode = failureCode
    if (download.tempPath) await removeFile(download.tempPath)
    this.finishDownloadCenterRecord(download, 'cancelled', null)
    this.finishAgentDownloadRecord(download, 'cancelled')
    download.tool?.downloads.delete(download)
  }

  private refreshDownloadCenterRecord(
    download: ActiveDownload,
    state: BrowserDownloadCenterItem['state'],
    immediate = false
  ): void {
    if (!this.ensureDownloadCenterRecord(download)) return
    const record = download.centerRecord
    record.state = state
    record.receivedBytes = safeDownloadBytes(download.item, 'getReceivedBytes')
    record.totalBytes = safeDownloadBytes(download.item, 'getTotalBytes')
    record.bytesPerSecond = state === 'progressing' ? safeDownloadBytesPerSecond(download.item) : 0
    record.updatedAt = Date.now()
    this.refreshAgentDownloadRecord(download, state)
    this.markDownloadCenterChanged(immediate)
  }

  private finishDownloadCenterRecord(
    download: ActiveDownload,
    state: BrowserDownloadCenterItem['state'],
    absolutePath: string | null,
    completedSize?: number
  ): void {
    if (!this.ensureDownloadCenterRecord(download)) return
    const record = download.centerRecord
    record.absolutePath = absolutePath
    record.state = state
    record.receivedBytes = completedSize ?? safeDownloadBytes(download.item, 'getReceivedBytes')
    record.totalBytes = completedSize ?? safeDownloadBytes(download.item, 'getTotalBytes')
    record.bytesPerSecond = 0
    record.updatedAt = Date.now()
    this.trimDownloadCenterRecords()
    this.markDownloadCenterChanged(true)
  }

  private refreshAgentDownloadRecord(
    download: ActiveDownload,
    state: BrowserDownloadCenterItem['state']
  ): void {
    const record = this.agentDownloadRecords.get(download.downloadId)
    if (!record) return
    const selectedDestination =
      !download.waitsForDestinationConfirmation || hasSelectedDownloadPath(download.item)
    record.status.state = !selectedDestination
      ? 'awaiting_destination'
      : state === 'completed' && !download.reference
        ? 'finalizing'
        : state
    record.status.receivedBytes = safeDownloadBytes(download.item, 'getReceivedBytes')
    record.status.totalBytes = safeDownloadBytes(download.item, 'getTotalBytes')
    record.status.bytesPerSecond =
      selectedDestination && state === 'progressing' ? safeDownloadBytesPerSecond(download.item) : 0
    record.status.updatedAt = Date.now()
    this.markAgentDownloadChanged()
  }

  private finishAgentDownloadRecord(
    download: ActiveDownload,
    state: BrowserDownloadCenterItem['state']
  ): void {
    const record = this.agentDownloadRecords.get(download.downloadId)
    if (!record) return
    record.status.displayName = download.centerRecord.displayName
    record.status.receivedBytes =
      download.reference?.sizeBytes ?? download.centerRecord.receivedBytes
    record.status.totalBytes = download.reference?.sizeBytes ?? download.centerRecord.totalBytes
    record.status.bytesPerSecond = 0
    record.status.updatedAt = Date.now()
    if (download.reference) {
      record.status.state = 'completed'
      record.status.reference = structuredClone(download.reference)
      delete record.status.errorCode
    } else if (download.failureCode && download.failureCode !== 'browser.download.cancelled') {
      record.status.state =
        download.failureCode === 'browser.download.interrupted' ? 'interrupted' : 'failed'
      record.status.errorCode = download.failureCode
      delete record.status.reference
    } else {
      record.status.state = state === 'interrupted' ? 'interrupted' : 'cancelled'
      record.status.errorCode =
        state === 'interrupted' ? 'browser.download.interrupted' : 'browser.download.cancelled'
      delete record.status.reference
    }
    this.trimAgentDownloadRecords()
    this.markAgentDownloadChanged()
  }

  private agentProgressForTool(tool: ActiveTool): readonly BrowserAgentDownloadStatus[] {
    return [...tool.downloadIds]
      .map((downloadId) => this.agentDownloadRecords.get(downloadId)?.status)
      .filter((status): status is BrowserAgentDownloadStatus => status !== undefined)
      .map((status) => structuredClone(status))
  }

  private markAgentDownloadChanged(): void {
    this.agentDownloadRevision += 1
  }

  private trimAgentDownloadRecords(): void {
    const terminal = [...this.agentDownloadRecords.entries()]
      .filter(([, record]) =>
        ['completed', 'cancelled', 'interrupted', 'failed'].includes(record.status.state)
      )
      .sort((left, right) => right[1].status.updatedAt - left[1].status.updatedAt)
    for (const [downloadId] of terminal.slice(MAX_AGENT_DOWNLOAD_ENTRIES)) {
      this.agentDownloadRecords.delete(downloadId)
    }
  }

  private ensureDownloadCenterRecord(download: ActiveDownload): boolean {
    if (download.centerVisible) return true
    if (download.waitsForDestinationConfirmation && !hasSelectedDownloadPath(download.item)) {
      return false
    }
    download.centerVisible = true
    this.downloadCenterRecords.set(download.downloadId, download.centerRecord)
    return true
  }

  private trimDownloadCenterRecords(): void {
    const terminal = [...this.downloadCenterRecords.values()]
      .filter(
        (record) =>
          ![...this.activeDownloads].some(
            (download) => download.downloadId === record.downloadId && !download.terminal
          )
      )
      .sort((left, right) => right.updatedAt - left.updatedAt)
    for (const record of terminal.slice(MAX_DOWNLOAD_CENTER_ENTRIES)) {
      this.downloadCenterRecords.delete(record.downloadId)
    }
  }

  private markDownloadCenterChanged(immediate: boolean): void {
    this.downloadCenterRevision += 1
    if (immediate) {
      if (this.downloadCenterChangedTimer) {
        clearTimeout(this.downloadCenterChangedTimer)
        this.downloadCenterChangedTimer = undefined
      }
      this.emitDownloadCenterChanged()
      return
    }
    if (this.downloadCenterChangedTimer) return
    this.downloadCenterChangedTimer = setTimeout(() => {
      this.downloadCenterChangedTimer = undefined
      this.emitDownloadCenterChanged()
    }, DOWNLOAD_CENTER_UPDATE_INTERVAL_MS)
  }

  private emitDownloadCenterChanged(): void {
    if (this.disposed || this.downloadCenterChangedListeners.size === 0) return
    const snapshot = this.downloadCenterSnapshot()
    for (const listener of this.downloadCenterChangedListeners) {
      try {
        listener(snapshot)
      } catch {
        // Live observers cannot affect transfer ownership or durable publication.
      }
    }
  }

  private async settleTool(tool: ActiveTool): Promise<readonly BrowserDownloadReference[]> {
    await tool.ready
    if (tool.settled) return tool.references.map((reference) => structuredClone(reference))
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
    tool.accepting = false
    await Promise.allSettled(
      [...tool.downloads]
        .map((download) => download.finalization)
        .filter((finalization): finalization is Promise<void> => finalization !== undefined)
    )
    if (tool.failure) throw tool.failure
    // DownloadItem belongs to the browser session, not to one MCP call. Once will-download has
    // synchronously admitted it, detach the transfer so a large file or native save dialog can
    // outlive the initiating click while remaining visible to subsequent Browser Tool calls.
    for (const download of [...tool.downloads]) {
      download.tool = undefined
      tool.downloads.delete(download)
    }
    tool.settled = true
    return tool.references.map((reference) => structuredClone(reference))
  }

  private finishTool(tool: ActiveTool): void {
    if (tool.finished) return
    tool.finished = true
    tool.accepting = false
    tool.callerSignal?.removeEventListener('abort', tool.handleCallerAbort)
    this.tools.delete(tool)
    if (!tool.settled && tool.downloads.size > 0) {
      void this.abortTool(tool, 'browser.download.outcome_unknown')
    }
  }

  private async abortTool(tool: ActiveTool, code: BrowserDownloadBrokerErrorCode): Promise<void> {
    this.failTool(tool, code)
    tool.accepting = false
    tool.finished = true
    tool.callerSignal?.removeEventListener('abort', tool.handleCallerAbort)
    this.tools.delete(tool)
    await Promise.allSettled([...tool.downloads].map((download) => this.cancelDownload(download)))
  }

  private async abortMatchingTools(predicate: (tool: ActiveTool) => boolean): Promise<void> {
    await Promise.allSettled(
      [...this.tools]
        .filter(predicate)
        .map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
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

  private downloadExceedsAgentBudget(item: DownloadItem, exclude?: ActiveDownload): boolean {
    const candidate = downloadByteEstimate(item)
    return (
      candidate > this.maxSingleDownloadBytes ||
      candidate + this.activeByteEstimate(exclude, true) > this.maxActiveBytes
    )
  }

  private activeByteEstimate(exclude?: ActiveDownload, agentOnly = false): number {
    let bytes = 0
    for (const download of this.activeDownloads) {
      if (download === exclude || (agentOnly && !download.owner)) continue
      bytes += downloadByteEstimate(download.item)
    }
    return bytes
  }

  private assertToolOwner(owner: ToolOwner | BrowserDownloadOwner): void {
    this.assertUsable()
    if (
      this.finalizedRuns.has(owner.runId) ||
      this.revokedActivations.has(owner.activationId) ||
      this.closedToolCalls.has(toolCallKey(owner.runId, owner.toolCallId)) ||
      ('surfaceId' in owner &&
        this.closedSurfaces.has(surfaceKey(owner.surfaceId, owner.generation)))
    ) {
      throw new BrowserDownloadBrokerError('browser.download.closed', 'definitely_not_dispatched')
    }
  }

  private assertUsable(): void {
    if (this.disposed) {
      throw new BrowserDownloadBrokerError('browser.download.closed', 'definitely_not_dispatched')
    }
  }

  private configuredDestination(settings: BrowserDownloadSettingsRecord): string {
    const destination =
      settings.locationMode === 'custom'
        ? (settings.customDirectory ?? '')
        : this.systemDownloadDirectory
    if (!destination || !isAbsolute(destination)) {
      throw new Error('browser.download.destination_unavailable')
    }
    // Persisted custom folders may live on removable volumes. Keep the configured path so the
    // app and Settings remain usable while that volume is offline; each transfer revalidates the
    // directory immediately before assigning Electron's save path and fails closed if unavailable.
    return destination
  }
}

function createActiveDownload(input: {
  centerRecord: DownloadCenterRecord
  destination: string | null
  downloadId: string
  displayName: string
  guest: GuestRecord
  item: DownloadItem
  mimeType: string
  owner?: ToolOwner
  sourceOrigin: string | null
  tempPath: string | null
  tool?: ActiveTool
  waitsForDestinationConfirmation: boolean
  finish: (
    download: ActiveDownload,
    state: 'completed' | 'cancelled' | 'interrupted'
  ) => Promise<void>
  updated: (download: ActiveDownload, state: 'progressing' | 'interrupted') => void
}): ActiveDownload {
  const download = {} as ActiveDownload
  Object.assign(download, {
    ...input,
    centerVisible: false,
    handleDone: (_event: Event, state: 'completed' | 'cancelled' | 'interrupted') => {
      download.finalization ??= input.finish(download, state)
    },
    handleUpdated: (_event: Event, state: 'progressing' | 'interrupted') =>
      input.updated(download, state),
    terminal: false
  })
  return download
}

function safeCompletedDownloadPath(item: DownloadItem): string {
  try {
    const path = item.getSavePath()
    if (!path || !isAbsolute(path)) throw new Error('browser.download.destination_unavailable')
    return path
  } catch {
    throw new Error('browser.download.destination_unavailable')
  }
}

function hasSelectedDownloadPath(item: DownloadItem): boolean {
  try {
    const path = item.getSavePath()
    return Boolean(path && isAbsolute(path))
  } catch {
    return false
  }
}

function canonicalDownloadDirectory(value: string): string {
  if (!value || !isAbsolute(value)) throw new Error('browser.download.destination_unavailable')
  const metadata = lstatSync(value)
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new Error('browser.download.destination_unavailable')
  }
  return realpathSync.native(value)
}

async function hashRegularFile(path: string): Promise<{ sha256: string; sizeBytes: number }> {
  const metadata = await lstat(path)
  if (
    metadata.isSymbolicLink() ||
    !metadata.isFile() ||
    metadata.size < 0 ||
    !Number.isSafeInteger(metadata.size)
  ) {
    throw new Error('browser.download.invalid_file')
  }
  const hash = createHash('sha256')
  await new Promise<void>((resolve, reject) => {
    const stream = createReadStream(path)
    stream.on('data', (chunk) => hash.update(chunk))
    stream.once('error', reject)
    stream.once('end', resolve)
  })
  return { sha256: hash.digest('hex'), sizeBytes: metadata.size }
}

async function publishWithoutOverwrite(
  tempPath: string,
  destination: string,
  displayName: string
): Promise<string> {
  const safeName = sanitizedDownloadFileName(displayName)
  for (let index = 0; index < MAX_FILE_NAME_ATTEMPTS; index += 1) {
    const candidate = join(destination, indexedFileName(safeName, index))
    try {
      await publishCandidate(tempPath, candidate)
      try {
        await unlink(tempPath)
      } catch (error) {
        await removeFile(candidate)
        throw error
      }
      return candidate
    } catch (error) {
      if (isNodeError(error, 'EEXIST')) continue
      throw error
    }
  }
  throw new Error('browser.download.destination_full')
}

async function publishCandidate(tempPath: string, candidate: string): Promise<void> {
  try {
    await link(tempPath, candidate)
    return
  } catch (error) {
    if (isNodeError(error, 'EEXIST') || !isUnsupportedLinkError(error)) throw error
  }

  try {
    await copyFile(tempPath, candidate, constants.COPYFILE_EXCL)
  } catch (error) {
    if (!isNodeError(error, 'EEXIST')) await removeFile(candidate)
    throw error
  }
}

function isUnsupportedLinkError(error: unknown): boolean {
  return (
    isNodeError(error, 'EPERM') ||
    isNodeError(error, 'ENOSYS') ||
    isNodeError(error, 'ENOTSUP') ||
    isNodeError(error, 'EOPNOTSUPP') ||
    isNodeError(error, 'EXDEV')
  )
}

function notifyHistoryChanged(
  initial: (() => void) | undefined,
  listeners: ReadonlySet<() => void>
): void {
  for (const listener of initial ? [initial, ...listeners] : listeners) {
    try {
      listener()
    } catch {
      // Notification observers never participate in durable download publication.
    }
  }
}

function indexedFileName(fileName: string, index: number): string {
  if (index === 0) return fileName
  const extension = extname(fileName)
  const stem = parse(fileName).name || 'download'
  return `${stem} (${index})${extension}`
}

function publicDownloadReference(record: BrowserDownloadRecord): BrowserDownloadReference {
  return {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloadId: record.downloadId,
    displayName: record.displayName,
    mimeType: record.mimeType,
    sizeBytes: record.sizeBytes,
    sha256: record.sha256,
    createdAt: record.createdAt,
    source: record.source
  }
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
  return /^[a-z0-9][a-z0-9!#$&^_.+-]{0,63}\/[a-z0-9][a-z0-9!#$&^_.+-]{0,127}$/u.test(normalized)
    ? normalized
    : 'application/octet-stream'
}

function safeDownloadOrigin(item: DownloadItem): string | null {
  try {
    const url = item.getURL()
    const parsed = new URL(url)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.origin : null
  } catch {
    return null
  }
}

function safeDownloadUrl(item: DownloadItem): string | null {
  try {
    const value = item.getURL()
    if (typeof value !== 'string' || value.length === 0 || value.length > 8192) return null
    const parsed = new URL(value)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.href : null
  } catch {
    return null
  }
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

function safeDownloadTransferState(
  item: DownloadItem,
  fallback: 'progressing' | 'interrupted'
): BrowserDownloadCenterItem['state'] {
  if (safeIsPaused(item)) return 'paused'
  return fallback
}

function safeDownloadStartedAt(item: DownloadItem, fallback: number): number {
  try {
    const value = item.getStartTime()
    if (!Number.isFinite(value) || value <= 0) return fallback
    const milliseconds = value < 1_000_000_000_000 ? Math.round(value * 1_000) : Math.round(value)
    return Number.isSafeInteger(milliseconds) && milliseconds > 0 ? milliseconds : fallback
  } catch {
    return fallback
  }
}

function safeDownloadBytesPerSecond(item: DownloadItem): number {
  try {
    const value = item.getCurrentBytesPerSecond()
    return Number.isSafeInteger(value) && value > 0 ? value : 0
  } catch {
    return 0
  }
}

function safeIsPaused(item: DownloadItem): boolean {
  try {
    return item.isPaused()
  } catch {
    return false
  }
}

function safeCanResume(item: DownloadItem): boolean {
  try {
    return item.canResume()
  } catch {
    return false
  }
}

function safePause(item: DownloadItem): boolean {
  try {
    item.pause()
    return true
  } catch {
    return false
  }
}

function safeResume(item: DownloadItem): boolean {
  try {
    if (!item.canResume()) return false
    item.resume()
    return true
  } catch {
    return false
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
    // Terminal DownloadItems can reject cancellation.
  }
}

function compareDownloadCenterRecords(
  left: DownloadCenterRecord,
  right: DownloadCenterRecord
): number {
  const leftRank = left.state === 'progressing' || left.state === 'paused' ? 0 : 1
  const rightRank = right.state === 'progressing' || right.state === 'paused' ? 0 : 1
  return (
    leftRank - rightRank || right.startedAt - left.startedAt || right.updatedAt - left.updatedAt
  )
}

async function removeFile(path: string): Promise<void> {
  await unlink(path).catch(() => undefined)
}

function positiveBound(value: number | undefined, fallback: number): number {
  return Number.isSafeInteger(value) && (value ?? 0) > 0 ? (value as number) : fallback
}

function ownerBase(owner: BrowserDownloadOwner): ToolOwner {
  return {
    activationId: owner.activationId,
    capabilityId: owner.capabilityId,
    conversationId: owner.conversationId,
    runId: owner.runId,
    toolCallId: owner.toolCallId
  }
}

function surfaceKey(surfaceId: string, generation: number): string {
  return `${surfaceId}\u0000${generation}`
}

function toolCallKey(runId: string, toolCallId: string): string {
  return `${runId}\u0000${toolCallId}`
}

function isNodeError(error: unknown, code: string): boolean {
  return (
    error instanceof Error &&
    'code' in error &&
    typeof (error as NodeJS.ErrnoException).code === 'string' &&
    (error as NodeJS.ErrnoException).code === code
  )
}
