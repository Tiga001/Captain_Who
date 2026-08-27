import { createHash, randomUUID } from 'node:crypto'
import { constants, createReadStream, lstatSync, realpathSync } from 'node:fs'
import { copyFile, link, lstat, unlink } from 'node:fs/promises'
import { basename, extname, isAbsolute, join, parse } from 'node:path'
import type { DownloadItem, Event, Session, WebContents } from 'electron'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  parseBrowserDownloadRecord,
  parseBrowserDownloadSettingsRecord,
  type BrowserDownloadRecord,
  type BrowserDownloadReference,
  type BrowserDownloadRegistrationInput,
  type BrowserDownloadSettingsRecord
} from '@mycopilot/protocol'

import { safeSuggestedFileName, type BrowserArtifactOwner } from './BrowserArtifactBroker'

const DEFAULT_MAX_ACTIVE_DOWNLOADS = 8
const DEFAULT_MAX_SINGLE_AGENT_DOWNLOAD_BYTES = 64 * 1024 * 1024
const DEFAULT_MAX_ACTIVE_AGENT_BYTES = 128 * 1024 * 1024
const DOWNLOAD_SETTLE_TIMEOUT_MS = 30_000
const DOWNLOAD_PROMPT_SETTLE_TIMEOUT_MS = 5 * 60_000
const MAX_REGISTERED_GUESTS = 32
const MAX_FILE_NAME_ATTEMPTS = 10_000

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
}

type ToolOwner = Omit<BrowserDownloadOwner, 'generation' | 'surfaceId'>

interface ActiveTool {
  accepting: boolean
  callerSignal?: AbortSignal
  createdGuestClaimsStarted: Record<'new' | 'popup', number>
  dispatched: boolean
  downloads: Set<ActiveDownload>
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
  destination: string | null
  downloadId: string
  displayName: string
  guest: GuestRecord
  handleDone: (event: Event, state: 'completed' | 'cancelled' | 'interrupted') => void
  handleUpdated: (event: Event, state: 'progressing' | 'interrupted') => void
  item: DownloadItem
  lifetime: Promise<void>
  mimeType: string
  settle: () => void
  sourceOrigin: string | null
  tempPath: string | null
  terminal: boolean
  tool?: ActiveTool
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
  private readonly maxActiveBytes: number
  private readonly maxActiveDownloads: number
  private readonly maxSingleDownloadBytes: number
  private settingsRecord: BrowserDownloadSettingsRecord
  private destinationDirectory: string
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

  updateSettings(value: BrowserDownloadSettingsRecord): void {
    this.assertUsable()
    const next = parseBrowserDownloadSettingsRecord(value)
    const destination = this.configuredDestination(next)
    this.settingsRecord = next
    this.destinationDirectory = destination
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
    await Promise.allSettled(
      [...this.tools].map((tool) => this.abortTool(tool, 'browser.download.cancelled'))
    )
    await Promise.allSettled(
      [...this.activeDownloads].map((download) => this.cancelDownload(download))
    )
    this.guests.clear()
    this.tools.clear()
    this.activeDownloads.clear()
    this.closedSurfaces.clear()
    this.closedToolCalls.clear()
    this.finalizedRuns.clear()
    this.revokedActivations.clear()
    this.historyChangedListeners.clear()
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
    const promptForDestination = this.settingsRecord.askWhereToSave
    const destination = promptForDestination ? null : configuredDestination
    const tempPath = promptForDestination
      ? null
      : join(configuredDestination, `.mycopilot-download-${downloadId.slice(17)}.part`)
    const download = createActiveDownload({
      destination,
      downloadId,
      displayName,
      guest,
      item,
      mimeType: sanitizedDownloadMimeType(safeDownloadString(item, 'getMimeType')),
      sourceOrigin: safeDownloadOrigin(item),
      tempPath,
      tool,
      finish: (active, state) => this.finishDownload(active, state),
      updated: (active) => {
        if (!active.tool || !this.downloadExceedsAgentBudget(active.item, active)) return
        this.failTool(active.tool, 'browser.download.too_large')
        void this.cancelDownload(active)
      }
    })
    try {
      tool?.downloads.add(download)
      this.activeDownloads.add(download)
      item.once('done', download.handleDone)
      item.on('updated', download.handleUpdated)
      if (promptForDestination) {
        item.setSaveDialogOptions({ defaultPath: join(configuredDestination, displayName) })
      } else {
        item.setSavePath(tempPath as string)
      }
    } catch {
      this.removeDownloadListeners(download)
      tool?.downloads.delete(download)
      this.activeDownloads.delete(download)
      download.terminal = true
      download.settle()
      if (tool) this.failTool(tool, 'browser.download.destination_unavailable')
      if (tempPath) void removeFile(tempPath)
      safeCancel(item)
      return
    }
    const state = safeDownloadState(item)
    if (state !== 'progressing') {
      void this.finishDownload(download, state)
    } else if (tool && this.downloadExceedsAgentBudget(item, download)) {
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
        if (download.tool) {
          this.failTool(
            download.tool,
            state === 'interrupted' ? 'browser.download.interrupted' : 'browser.download.cancelled'
          )
        }
        if (download.tempPath) await removeFile(download.tempPath)
        return
      }

      const completedPath = download.tempPath ?? safeCompletedDownloadPath(download.item)
      const identity = await hashRegularFile(completedPath)
      if (download.tool && identity.sizeBytes > this.maxSingleDownloadBytes) {
        this.failTool(download.tool, 'browser.download.too_large')
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
      let record: BrowserDownloadRecord
      try {
        record = parseBrowserDownloadRecord(
          await this.registerDownloadRecord({
            schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
            downloadId: download.downloadId,
            source: download.tool ? 'agent' : 'manual',
            displayName: basename(absolutePath),
            mimeType: download.mimeType,
            sizeBytes: identity.sizeBytes,
            sha256: identity.sha256,
            absolutePath,
            sourceOrigin: download.sourceOrigin,
            conversationId: download.tool?.owner.conversationId ?? null,
            runId: download.tool?.owner.runId ?? null,
            callId: download.tool?.owner.toolCallId ?? null,
            createdAt: Date.now()
          })
        )
      } catch {
        // A native save dialog makes the selected path user-owned. Never delete that file merely
        // because metadata registration failed; automatic staging remains transactional.
        if (download.tempPath) await removeFile(absolutePath)
        throw new BrowserDownloadBrokerError(
          'browser.download.registration_failed',
          download.tool?.dispatched ? 'possibly_dispatched' : 'definitely_not_dispatched'
        )
      }
      download.tool?.references.push(publicDownloadReference(record))
      notifyHistoryChanged(this.onHistoryChanged, this.historyChangedListeners)
    } catch (error) {
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
      download.tool?.downloads.delete(download)
      download.settle()
    }
  }

  private async cancelDownload(download: ActiveDownload): Promise<void> {
    if (download.terminal) return
    download.terminal = true
    this.removeDownloadListeners(download)
    this.activeDownloads.delete(download)
    safeCancel(download.item)
    if (download.tempPath) await removeFile(download.tempPath)
    download.tool?.downloads.delete(download)
    download.settle()
  }

  private async settleTool(tool: ActiveTool): Promise<readonly BrowserDownloadReference[]> {
    await tool.ready
    if (tool.settled) return tool.references.map((reference) => structuredClone(reference))
    await new Promise<void>((resolveImmediate) => setImmediate(resolveImmediate))
    tool.accepting = false
    if (tool.downloads.size > 0) {
      const timeoutMs = [...tool.downloads].some((download) => download.tempPath === null)
        ? DOWNLOAD_PROMPT_SETTLE_TIMEOUT_MS
        : DOWNLOAD_SETTLE_TIMEOUT_MS
      const completed = await settleDownloadsWithin(
        [...tool.downloads].map((download) => download.lifetime),
        timeoutMs
      )
      if (!completed) {
        this.failTool(tool, 'browser.download.outcome_unknown')
        await Promise.allSettled(
          [...tool.downloads].map((download) => this.cancelDownload(download))
        )
      }
    }
    tool.settled = true
    if (tool.failure) throw tool.failure
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
      if (download === exclude || (agentOnly && !download.tool)) continue
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
  destination: string | null
  downloadId: string
  displayName: string
  guest: GuestRecord
  item: DownloadItem
  mimeType: string
  sourceOrigin: string | null
  tempPath: string | null
  tool?: ActiveTool
  finish: (
    download: ActiveDownload,
    state: 'completed' | 'cancelled' | 'interrupted'
  ) => Promise<void>
  updated: (download: ActiveDownload) => void
}): ActiveDownload {
  let settle!: () => void
  const lifetime = new Promise<void>((resolve) => {
    settle = once(resolve)
  })
  const download = {} as ActiveDownload
  Object.assign(download, {
    ...input,
    settle,
    lifetime,
    handleDone: (_event: Event, state: 'completed' | 'cancelled' | 'interrupted') => {
      void input.finish(download, state)
    },
    handleUpdated: () => input.updated(download),
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
    // Terminal DownloadItems can reject cancellation.
  }
}

async function removeFile(path: string): Promise<void> {
  await unlink(path).catch(() => undefined)
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
