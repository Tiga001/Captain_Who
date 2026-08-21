import { createHash, randomUUID } from 'node:crypto'
import { constants, renameSync } from 'node:fs'
import {
  chmod,
  lstat,
  mkdir,
  open,
  readFile,
  realpath,
  rename,
  rm,
  writeFile
} from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'
import {
  BROWSER_ARTIFACT_SCHEMA_VERSION,
  parseBrowserArtifactReference,
  parseBrowserSurfaceId,
  type BrowserArtifactKind,
  type BrowserArtifactPreview,
  type BrowserArtifactReference
} from '@mycopilot/protocol'

const DEFAULT_MAX_ARTIFACT_BYTES = 64 * 1024 * 1024
const DEFAULT_MAX_ARTIFACTS = 256
const DEFAULT_MAX_ARTIFACTS_PER_RUN = 64
const DEFAULT_MAX_RUN_BYTES = 128 * 1024 * 1024
const DEFAULT_MAX_TOTAL_BYTES = 256 * 1024 * 1024
const DEFAULT_MAX_PREVIEW_BYTES = 8 * 1024 * 1024
const DEFAULT_MAX_TEXT_PREVIEW_BYTES = 256 * 1024
const DEFAULT_TTL_MS = 30 * 60 * 1_000
const MAX_TTL_MS = 24 * 60 * 60 * 1_000
const MAX_PENDING_RESERVATIONS = 64
const MAX_IDENTITY_LENGTH = 512

export type BrowserArtifactBrokerErrorCode =
  | 'browser.artifact.closed'
  | 'browser.artifact.invalid_name'
  | 'browser.artifact.invalid_type'
  | 'browser.artifact.invalid_file'
  | 'browser.artifact.not_found'
  | 'browser.artifact.identity_mismatch'
  | 'browser.artifact.too_large'
  | 'browser.artifact.capacity'
  | 'browser.artifact.expired'
  | 'browser.artifact.preview_unavailable'
  | 'browser.artifact.unavailable'

export class BrowserArtifactBrokerError extends Error {
  readonly name = 'BrowserArtifactBrokerError'

  constructor(readonly code: BrowserArtifactBrokerErrorCode) {
    super(code)
  }
}

/** Private ownership binding. Only the safe owner enum appears in BrowserArtifactReference. */
export interface BrowserArtifactOwner {
  runId: string
  activationId: string
  capabilityId: 'browser_automation'
  surfaceId: string
  generation: number
  toolCallId: string
}

export interface BrowserArtifactCreateInput {
  owner: BrowserArtifactOwner
  kind: BrowserArtifactKind
  mimeType: string
  suggestedFileName: string
  /** Defaults to true. Sensitive exports set false so bytes can never cross the preview IPC. */
  allowPreview?: boolean
}

export interface BrowserArtifactReservation {
  /** Main-only absolute path. It must never cross an IPC, MCP, or Renderer boundary. */
  readonly managedPath: string
  /** Safe basename suitable for an official Playwright `filename` argument under outputDir. */
  readonly fileName: string
  /** Main-only metadata refinement before publication; it never changes the managed path. */
  updateMetadata(input: { mimeType: string; suggestedFileName: string }): void
  commit(): Promise<BrowserArtifactReference>
  discard(): Promise<void>
}

export interface BrowserArtifactOutputSession {
  /** Main-only private outputDir passed to the fixed official Playwright MCP connection. */
  readonly outputDirectory: string
  reserveFile(input: BrowserArtifactCreateInput): Promise<BrowserArtifactReservation>
  adoptFile(
    input: BrowserArtifactCreateInput & { fileName: string }
  ): Promise<BrowserArtifactReference>
  close(): Promise<void>
}

export interface BrowserArtifactBrokerClock {
  now(): number
  setTimeout(handler: () => void, delayMs: number): ReturnType<typeof setTimeout>
  clearTimeout(timer: ReturnType<typeof setTimeout>): void
}

export interface BrowserArtifactBrokerOptions {
  rootDirectory: string
  /** Injectable deterministic publication barrier used by race/conformance tests. */
  beforePublish?: () => Promise<void>
  /** Injectable deterministic export barrier used by revocation/shutdown race tests. */
  beforeExportPublish?: () => Promise<void>
  /** Synchronous final export hook used only to prove the no-await TTL publication boundary. */
  beforeExportFinalPublish?: () => void
  clock?: BrowserArtifactBrokerClock
  maxArtifactBytes?: number
  maxArtifacts?: number
  maxArtifactsPerRun?: number
  maxPreviewBytes?: number
  maxTextPreviewBytes?: number
  maxRunBytes?: number
  maxTotalBytes?: number
  ttlMs?: number
}

interface ArtifactRecord {
  reference: BrowserArtifactReference
  owner: BrowserArtifactOwner
  path: string
  sha256: string
}

interface SessionRecord {
  closed: boolean
  directory: string
  id: string
  reservations: Set<string>
}

interface ReservationRecord {
  input: BrowserArtifactCreateInput
  managedPath: string
  fileName: string
  session: SessionRecord
  state: 'pending' | 'committing' | 'settled'
}

interface RunUsage {
  bytes: number
  count: number
}

const DEFAULT_CLOCK: BrowserArtifactBrokerClock = {
  now: Date.now,
  setTimeout: (handler, delayMs) => setTimeout(handler, delayMs),
  clearTimeout: (timer) => clearTimeout(timer)
}

/**
 * Host-owned ephemeral store for browser automation files.
 *
 * Official Playwright writes only into per-connection staging directories. Publication validates
 * the exact inode, moves it atomically into the private object directory, and returns a path-free
 * reference. The root is cleared on startup and shutdown; Browser Artifact authority never
 * survives an application restart.
 */
export class BrowserArtifactBroker {
  private readonly clock: BrowserArtifactBrokerClock
  private readonly beforeExportFinalPublish: () => void
  private readonly beforeExportPublish: () => Promise<void>
  private readonly beforePublish: () => Promise<void>
  private readonly maxArtifactBytes: number
  private readonly maxArtifacts: number
  private readonly maxArtifactsPerRun: number
  private readonly maxPreviewBytes: number
  private readonly maxRunBytes: number
  private readonly maxTextPreviewBytes: number
  private readonly maxTotalBytes: number
  private readonly objectsDirectory: string
  private readonly rootDirectory: string
  private readonly sessionsDirectory: string
  private readonly ttlMs: number

  private readonly artifacts = new Map<string, ArtifactRecord>()
  private readonly closedSurfaces = new Set<string>()
  private readonly closedToolCalls = new Set<string>()
  private readonly finalizedRuns = new Set<string>()
  private readonly releasedRuns = new Set<string>()
  private readonly revokedActivations = new Set<string>()
  private readonly reservations = new Map<string, ReservationRecord>()
  private readonly runUsage = new Map<string, RunUsage>()
  private readonly sessions = new Map<string, SessionRecord>()
  private expirationTimer?: ReturnType<typeof setTimeout>
  private initializePromise?: Promise<void>
  private mutationTail: Promise<void> = Promise.resolve()
  private closed = false
  private totalBytes = 0

  constructor(options: BrowserArtifactBrokerOptions) {
    if (!isAbsolute(options.rootDirectory)) {
      throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
    }
    this.rootDirectory = resolve(options.rootDirectory)
    if (
      basename(this.rootDirectory) !== 'browser-automation-artifacts' ||
      dirname(this.rootDirectory) === this.rootDirectory
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
    }
    this.objectsDirectory = join(this.rootDirectory, 'objects')
    this.sessionsDirectory = join(this.rootDirectory, 'sessions')
    this.clock = options.clock ?? DEFAULT_CLOCK
    this.beforeExportFinalPublish = options.beforeExportFinalPublish ?? (() => undefined)
    this.beforeExportPublish = options.beforeExportPublish ?? (() => Promise.resolve())
    this.beforePublish = options.beforePublish ?? (() => Promise.resolve())
    this.maxArtifactBytes = positiveBound(options.maxArtifactBytes, DEFAULT_MAX_ARTIFACT_BYTES)
    this.maxArtifacts = positiveBound(options.maxArtifacts, DEFAULT_MAX_ARTIFACTS)
    this.maxArtifactsPerRun = positiveBound(
      options.maxArtifactsPerRun,
      DEFAULT_MAX_ARTIFACTS_PER_RUN
    )
    this.maxPreviewBytes = positiveBound(options.maxPreviewBytes, DEFAULT_MAX_PREVIEW_BYTES)
    this.maxTextPreviewBytes = positiveBound(
      options.maxTextPreviewBytes,
      DEFAULT_MAX_TEXT_PREVIEW_BYTES
    )
    this.maxRunBytes = positiveBound(options.maxRunBytes, DEFAULT_MAX_RUN_BYTES)
    this.maxTotalBytes = positiveBound(options.maxTotalBytes, DEFAULT_MAX_TOTAL_BYTES)
    this.ttlMs = Math.min(positiveBound(options.ttlMs, DEFAULT_TTL_MS), MAX_TTL_MS)
  }

  async openSession(): Promise<BrowserArtifactOutputSession> {
    await this.ensureInitialized()
    this.assertUsable()
    const id = randomUUID()
    const directory = join(this.sessionsDirectory, id)
    try {
      await mkdir(directory, { mode: 0o700 })
      await chmod(directory, 0o700)
      // `shutdown()` installs its fence before waiting for filesystem cleanup. Re-check after the
      // final directory await so an open racing shutdown cannot republish a stale SessionRecord or
      // leave a freshly-created private directory behind.
      this.assertUsable()
      const record: SessionRecord = { closed: false, directory, id, reservations: new Set() }
      this.sessions.set(id, record)
      return {
        outputDirectory: directory,
        reserveFile: (input) => this.reserveInSession(record, input),
        adoptFile: (input) => this.adoptInSession(record, input),
        close: () => this.closeSession(record)
      }
    } catch (error) {
      await rm(directory, { recursive: true, force: true }).catch(() => undefined)
      if (error instanceof BrowserArtifactBrokerError) throw error
      throw new BrowserArtifactBrokerError(
        this.closed ? 'browser.artifact.closed' : 'browser.artifact.unavailable'
      )
    }
  }

  async storeBytes(
    input: BrowserArtifactCreateInput & { bytes: Uint8Array }
  ): Promise<BrowserArtifactReference> {
    if (!(input.bytes instanceof Uint8Array)) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    if (input.bytes.byteLength === 0) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    if (input.bytes.byteLength > this.maxArtifactBytes) {
      throw new BrowserArtifactBrokerError('browser.artifact.too_large')
    }
    const createInput = validateCreateInput(input)
    this.assertUsable()
    this.assertOwnerLive(createInput.owner)
    // This is an early resource guard, not the publication linearization point. Reservation and
    // commit repeat the count/byte checks so concurrent writers still cannot over-admit.
    this.assertReservationCapacity(createInput.owner.runId)
    this.assertCommitCapacity(createInput.owner.runId, input.bytes.byteLength)
    const session = await this.openSession()
    try {
      const reservation = await session.reserveFile(createInput)
      await writeFile(reservation.managedPath, input.bytes, { flag: 'wx', mode: 0o600 })
      return await reservation.commit()
    } finally {
      await session.close()
    }
  }

  async storeText(
    input: BrowserArtifactCreateInput & { text: string }
  ): Promise<BrowserArtifactReference> {
    if (typeof input.text !== 'string') {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    return this.storeBytes({ ...input, bytes: Buffer.from(input.text, 'utf8') })
  }

  /**
   * Main/Core-only absolute file for publishing an `image-artifact://` readPath.
   * Never include this path in MCP, Renderer, or model payloads.
   */
  hostOwnedAbsolutePath(reference: BrowserArtifactReference): string | undefined {
    const record = this.artifacts.get(reference.artifactId)
    if (!record || record.reference.kind !== 'image') return undefined
    if (!sameReference(reference, record.reference)) return undefined
    return record.path
  }

  /** Reads only bounded preview-capable content; large/file-only Artifacts never cross IPC. */
  async readPreview(referenceValue: unknown): Promise<{
    artifact: BrowserArtifactReference
    bytes: Uint8Array
  }> {
    await this.ensureInitialized()
    this.assertUsable()
    const reference = parseBrowserArtifactReference(referenceValue)
    await this.sweepExpired()
    const record = this.artifacts.get(reference.artifactId)
    if (!record) throw new BrowserArtifactBrokerError('browser.artifact.not_found')
    if (!sameReference(reference, record.reference)) {
      throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
    }
    if (record.reference.preview === 'none') {
      throw new BrowserArtifactBrokerError('browser.artifact.preview_unavailable')
    }
    const previewLimit =
      record.reference.preview === 'text' ? this.maxTextPreviewBytes : this.maxPreviewBytes
    if (record.reference.sizeBytes > previewLimit) {
      throw new BrowserArtifactBrokerError('browser.artifact.preview_unavailable')
    }
    try {
      const metadata = await validatedRegularFile(record.path, this.objectsDirectory)
      if (metadata.size !== record.reference.sizeBytes) {
        throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
      }
      const bytes = await readFile(record.path)
      if (hashBytes(bytes) !== record.sha256) {
        throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
      }
      return { artifact: structuredClone(record.reference), bytes: Uint8Array.from(bytes) }
    } catch (error) {
      if (error instanceof BrowserArtifactBrokerError) throw error
      throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
    }
  }

  /**
   * Copies an exact frozen Artifact reference to a path selected by Main's native save dialog.
   *
   * The destination is never accepted over Renderer IPC and this method returns only a safe
   * basename. Publication uses a 0600 random sibling followed by one synchronous rename, so a
   * revocation/shutdown fence cannot interleave with the final filesystem side effect.
   */
  async exportArtifact(
    referenceValue: unknown,
    destinationValue: unknown
  ): Promise<{ displayName: string }> {
    await this.ensureInitialized()
    this.assertUsable()
    const reference = parseBrowserArtifactReference(referenceValue)
    const destination = await resolveExportDestination(destinationValue, this.rootDirectory)
    return this.withMutationLock(async () => {
      let temporaryPath: string | undefined
      try {
        const record = await this.requireExportableRecord(reference)
        const bytes = await readVerifiedArtifactBytes(
          record.path,
          this.objectsDirectory,
          record.reference.sizeBytes,
          record.sha256
        )
        await assertExportDirectoryIdentity(destination)
        await assertReplaceableExportLeaf(destination.path)

        temporaryPath = join(
          destination.directory,
          `.${destination.displayName}.${randomUUID()}.tmp`
        )
        const handle = await open(
          temporaryPath,
          constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | (constants.O_NOFOLLOW ?? 0),
          0o600
        )
        try {
          await handle.writeFile(bytes)
          await handle.sync()
          const written = await handle.stat()
          if (!written.isFile() || written.nlink !== 1 || written.size !== bytes.byteLength) {
            throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
          }
        } finally {
          await handle.close()
        }
        await chmod(temporaryPath, 0o600)
        if ((await hashFile(temporaryPath)) !== record.sha256) {
          throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
        }

        await this.beforeExportPublish()
        await this.requireExportableRecord(reference)
        await assertExportDirectoryIdentity(destination)
        await assertReplaceableExportLeaf(destination.path)
        const staged = await lstat(temporaryPath)
        if (
          !staged.isFile() ||
          staged.isSymbolicLink() ||
          staged.nlink !== 1 ||
          staged.size !== record.reference.sizeBytes
        ) {
          throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
        }
        if ((await hashFile(temporaryPath)) !== record.sha256) {
          throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
        }

        // No await is permitted between this final fence and publication. `renameSync` replaces a
        // raced leaf atomically rather than following it, and the staged inode already has 0600.
        this.beforeExportFinalPublish()
        this.assertExportRecordLive(reference, record)
        renameSync(temporaryPath, destination.path)
        temporaryPath = undefined
        return { displayName: destination.displayName }
      } catch (error) {
        if (error instanceof BrowserArtifactBrokerError) throw error
        throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
      } finally {
        if (temporaryPath) await rm(temporaryPath, { force: true }).catch(() => undefined)
      }
    })
  }

  async releaseRun(runId: string): Promise<void> {
    this.finalizedRuns.add(runId)
    this.releasedRuns.add(runId)
    try {
      await this.releaseWhere((owner) => owner.runId === runId)
    } finally {
      this.releasedRuns.delete(runId)
    }
  }

  /**
   * Ends mutation authority for a run while retaining committed references until their TTL.
   * Renderer previews therefore remain usable after the Agent emits its terminal event.
   */
  async finalizeRun(runId: string): Promise<void> {
    if (!safeIdentity(runId) || this.closed) return
    await this.ensureInitialized()
    this.finalizedRuns.add(runId)
    await this.withMutationLock(async () => {
      for (const [id, reservation] of [...this.reservations]) {
        if (reservation.input.owner.runId === runId) {
          await this.discardReservation(id, reservation)
        }
      }
    })
  }

  async releaseCapability(activationId: string): Promise<void> {
    this.revokedActivations.add(activationId)
    await this.releaseWhere((owner) => owner.activationId === activationId)
  }

  /** Revokes one Tool call before cleaning any late or already-published unreferenced output. */
  async releaseToolCall(input: { runId: string; toolCallId: string }): Promise<void> {
    if (!safeIdentity(input.runId) || !safeIdentity(input.toolCallId)) return
    this.closedToolCalls.add(keyForToolCall(input.runId, input.toolCallId))
    await this.releaseWhere(
      (owner) => owner.runId === input.runId && owner.toolCallId === input.toolCallId
    )
  }

  /** Cancels target-owned writes but retains committed run Artifacts for activity previews. */
  async releaseSurface(input: { surfaceId: string; generation: number }): Promise<void> {
    const surfaceId = parseBrowserSurfaceId(input.surfaceId)
    if (!Number.isSafeInteger(input.generation) || input.generation <= 0) return
    this.closedSurfaces.add(keyForSurface(surfaceId, input.generation))
    await this.ensureInitialized()
    await this.withMutationLock(async () => {
      for (const [id, reservation] of [...this.reservations]) {
        if (
          reservation.input.owner.surfaceId === surfaceId &&
          reservation.input.owner.generation === input.generation
        ) {
          await this.discardReservation(id, reservation)
        }
      }
    })
  }

  async sweepExpired(): Promise<number> {
    if (this.closed) return 0
    const now = this.clock.now()
    const expired = [...this.artifacts.values()].filter(
      (record) => record.reference.expiresAt <= now
    )
    if (expired.length === 0) {
      this.armExpirationTimer()
      return 0
    }
    await this.withMutationLock(async () => {
      for (const record of expired) await this.deleteRecord(record)
    })
    this.armExpirationTimer()
    return expired.length
  }

  snapshot(): {
    artifacts: number
    bytes: number
    reservations: number
    sessions: number
    runs: number
  } {
    return {
      artifacts: this.artifacts.size,
      bytes: this.totalBytes,
      reservations: this.reservations.size,
      sessions: this.sessions.size,
      runs: this.runUsage.size
    }
  }

  async shutdown(): Promise<void> {
    if (this.closed) return
    this.closed = true
    if (this.expirationTimer) this.clock.clearTimeout(this.expirationTimer)
    this.expirationTimer = undefined
    await this.initializePromise?.catch(() => undefined)
    await this.withMutationLock(async () => {
      for (const session of this.sessions.values()) session.closed = true
      this.sessions.clear()
      this.reservations.clear()
      this.artifacts.clear()
      this.closedSurfaces.clear()
      this.closedToolCalls.clear()
      this.finalizedRuns.clear()
      this.releasedRuns.clear()
      this.revokedActivations.clear()
      this.runUsage.clear()
      this.totalBytes = 0
      await rm(this.rootDirectory, { recursive: true, force: true }).catch(() => undefined)
    })
  }

  private async ensureInitialized(): Promise<void> {
    this.assertUsable()
    this.initializePromise ??= (async () => {
      await rm(this.rootDirectory, { recursive: true, force: true })
      await mkdir(this.objectsDirectory, { recursive: true, mode: 0o700 })
      await mkdir(this.sessionsDirectory, { recursive: true, mode: 0o700 })
      await Promise.all([
        chmod(this.rootDirectory, 0o700),
        chmod(this.objectsDirectory, 0o700),
        chmod(this.sessionsDirectory, 0o700)
      ])
    })()
    try {
      await this.initializePromise
    } catch {
      this.initializePromise = undefined
      throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
    }
  }

  private async reserveInSession(
    session: SessionRecord,
    rawInput: BrowserArtifactCreateInput
  ): Promise<BrowserArtifactReservation> {
    await this.ensureInitialized()
    this.assertSession(session)
    if (this.reservations.size >= MAX_PENDING_RESERVATIONS) {
      throw new BrowserArtifactBrokerError('browser.artifact.capacity')
    }
    const input = validateCreateInput(rawInput)
    this.assertOwnerLive(input.owner)
    this.assertReservationCapacity(input.owner.runId)
    const id = randomUUID()
    const displayName = safeSuggestedFileName(input.suggestedFileName)
    const fileName = `${id}-${displayName}`
    const managedPath = join(session.directory, fileName)
    const record: ReservationRecord = {
      input: { ...input, suggestedFileName: displayName },
      managedPath,
      fileName,
      session,
      state: 'pending'
    }
    this.reservations.set(id, record)
    session.reservations.add(id)
    return {
      managedPath,
      fileName,
      updateMetadata: (metadata) => this.updateReservationMetadata(id, record, metadata),
      commit: () => this.commitReservation(id, record),
      discard: () => this.discardReservation(id, record)
    }
  }

  private updateReservationMetadata(
    id: string,
    reservation: ReservationRecord,
    metadata: { mimeType: string; suggestedFileName: string }
  ): void {
    if (reservation.state !== 'pending' || this.reservations.get(id) !== reservation) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    const suggestedFileName = safeSuggestedFileName(metadata.suggestedFileName)
    validateKindMime(reservation.input.kind, metadata.mimeType)
    reservation.input = {
      ...reservation.input,
      mimeType: metadata.mimeType,
      suggestedFileName
    }
  }

  private async adoptInSession(
    session: SessionRecord,
    rawInput: BrowserArtifactCreateInput & { fileName: string }
  ): Promise<BrowserArtifactReference> {
    this.assertSession(session)
    const fileName = safeSuggestedFileName(rawInput.fileName)
    const reservation = await this.reserveInSession(session, rawInput)
    const sourcePath = join(session.directory, fileName)
    if (sourcePath === reservation.managedPath) return reservation.commit()
    try {
      await rename(sourcePath, reservation.managedPath)
      return await reservation.commit()
    } catch (error) {
      await reservation.discard()
      if (error instanceof BrowserArtifactBrokerError) throw error
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
  }

  private async commitReservation(
    id: string,
    reservation: ReservationRecord
  ): Promise<BrowserArtifactReference> {
    if (reservation.state !== 'pending' || this.reservations.get(id) !== reservation) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    reservation.state = 'committing'
    let publishedPath: string | undefined
    try {
      return await this.withMutationLock(async () => {
        this.assertUsable()
        this.assertSession(reservation.session)
        this.assertOwnerLive(reservation.input.owner)
        const metadata = await validatedRegularFile(
          reservation.managedPath,
          reservation.session.directory
        )
        if (metadata.size <= 0) {
          throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
        }
        if (metadata.size > this.maxArtifactBytes) {
          throw new BrowserArtifactBrokerError('browser.artifact.too_large')
        }
        validateKindMime(reservation.input.kind, reservation.input.mimeType)
        await validateTextualFile(
          reservation.managedPath,
          reservation.input.kind,
          metadata.size,
          this.maxArtifactBytes
        )
        this.assertCommitCapacity(reservation.input.owner.runId, metadata.size)
        const sha256 = await hashFile(reservation.managedPath)
        const unchanged = await lstat(reservation.managedPath)
        if (
          unchanged.dev !== metadata.dev ||
          unchanged.ino !== metadata.ino ||
          unchanged.size !== metadata.size ||
          unchanged.isSymbolicLink() ||
          !unchanged.isFile()
        ) {
          throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
        }
        const artifactUuid = randomUUID()
        const artifactId = `browser-artifact:${artifactUuid}`
        const destination = join(this.objectsDirectory, artifactUuid)
        await chmod(reservation.managedPath, 0o600)
        await rename(reservation.managedPath, destination)
        publishedPath = destination
        await chmod(destination, 0o600)
        // Revocation/finalization fences are installed synchronously outside the mutation queue.
        // Pause only after the final mutating filesystem operation, then re-check both authority
        // and the exact inode immediately before the synchronous publication step. This closes the
        // rename-to-record window without allowing a late commit to escape a run/capability/tool/
        // surface shutdown fence.
        await this.beforePublish()
        const published = await lstat(destination)
        if (
          published.dev !== metadata.dev ||
          published.ino !== metadata.ino ||
          published.size !== metadata.size ||
          published.isSymbolicLink() ||
          !published.isFile()
        ) {
          throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
        }
        this.assertUsable()
        this.assertSession(reservation.session)
        this.assertOwnerLive(reservation.input.owner)
        this.assertCommitCapacity(reservation.input.owner.runId, metadata.size)
        const createdAt = this.clock.now()
        const reference: BrowserArtifactReference = {
          schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
          artifactId,
          kind: reservation.input.kind,
          displayName: reservation.input.suggestedFileName,
          mimeType: reservation.input.mimeType,
          sizeBytes: metadata.size,
          createdAt,
          expiresAt: createdAt + this.ttlMs,
          lifecycle: 'run',
          owner: 'browser_automation',
          preview: previewFor(
            reservation.input.kind,
            metadata.size,
            this.maxPreviewBytes,
            this.maxTextPreviewBytes,
            reservation.input.allowPreview !== false
          )
        }
        const record: ArtifactRecord = {
          reference,
          owner: reservation.input.owner,
          path: destination,
          sha256
        }
        this.artifacts.set(artifactId, record)
        const usage = this.runUsage.get(record.owner.runId) ?? { bytes: 0, count: 0 }
        usage.bytes += metadata.size
        usage.count += 1
        this.runUsage.set(record.owner.runId, usage)
        this.totalBytes += metadata.size
        this.settleReservation(id, reservation)
        this.armExpirationTimer()
        return structuredClone(reference)
      })
    } catch (error) {
      if (publishedPath) await rm(publishedPath, { force: true }).catch(() => undefined)
      await this.discardReservation(id, reservation)
      if (error instanceof BrowserArtifactBrokerError) throw error
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
  }

  private async discardReservation(id: string, reservation: ReservationRecord): Promise<void> {
    if (reservation.state === 'settled') return
    this.settleReservation(id, reservation)
    await rm(reservation.managedPath, { force: true }).catch(() => undefined)
  }

  private settleReservation(id: string, reservation: ReservationRecord): void {
    reservation.state = 'settled'
    if (this.reservations.get(id) === reservation) this.reservations.delete(id)
    reservation.session.reservations.delete(id)
  }

  private async closeSession(session: SessionRecord): Promise<void> {
    if (session.closed) return
    session.closed = true
    this.sessions.delete(session.id)
    const reservations = [...session.reservations]
    await Promise.all(
      reservations.map(async (id) => {
        const reservation = this.reservations.get(id)
        if (reservation) await this.discardReservation(id, reservation)
      })
    )
    await rm(session.directory, { recursive: true, force: true }).catch(() => undefined)
  }

  private async releaseWhere(predicate: (owner: BrowserArtifactOwner) => boolean): Promise<void> {
    if (this.closed) return
    await this.ensureInitialized()
    await this.withMutationLock(async () => {
      for (const record of [...this.artifacts.values()]) {
        if (predicate(record.owner)) await this.deleteRecord(record)
      }
      for (const [id, reservation] of [...this.reservations]) {
        if (predicate(reservation.input.owner)) {
          await this.discardReservation(id, reservation)
        }
      }
    })
    this.armExpirationTimer()
  }

  private async deleteRecord(record: ArtifactRecord): Promise<void> {
    if (this.artifacts.get(record.reference.artifactId) !== record) return
    this.artifacts.delete(record.reference.artifactId)
    this.totalBytes = Math.max(0, this.totalBytes - record.reference.sizeBytes)
    const usage = this.runUsage.get(record.owner.runId)
    if (usage) {
      usage.bytes = Math.max(0, usage.bytes - record.reference.sizeBytes)
      usage.count = Math.max(0, usage.count - 1)
      if (usage.bytes === 0 && usage.count === 0) this.runUsage.delete(record.owner.runId)
    }
    await rm(record.path, { force: true }).catch(() => undefined)
  }

  private assertReservationCapacity(runId: string): void {
    if (this.finalizedRuns.has(runId)) {
      throw new BrowserArtifactBrokerError('browser.artifact.closed')
    }
    const pendingForRun = [...this.reservations.values()].filter(
      (reservation) => reservation.input.owner.runId === runId
    ).length
    const usage = this.runUsage.get(runId)
    if (
      this.artifacts.size + this.reservations.size >= this.maxArtifacts ||
      (usage?.count ?? 0) + pendingForRun >= this.maxArtifactsPerRun
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.capacity')
    }
  }

  private assertOwnerLive(owner: BrowserArtifactOwner): void {
    if (
      this.finalizedRuns.has(owner.runId) ||
      this.revokedActivations.has(owner.activationId) ||
      this.closedSurfaces.has(keyForSurface(owner.surfaceId, owner.generation)) ||
      this.closedToolCalls.has(keyForToolCall(owner.runId, owner.toolCallId))
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.closed')
    }
  }

  private assertCommitCapacity(runId: string, size: number): void {
    const usage = this.runUsage.get(runId)
    if (
      this.artifacts.size >= this.maxArtifacts ||
      (usage?.count ?? 0) >= this.maxArtifactsPerRun
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.capacity')
    }
    if (
      (usage?.bytes ?? 0) + size > this.maxRunBytes ||
      this.totalBytes + size > this.maxTotalBytes
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.too_large')
    }
  }

  private async requireExportableRecord(
    reference: BrowserArtifactReference
  ): Promise<ArtifactRecord> {
    this.assertUsable()
    const record = this.artifacts.get(reference.artifactId)
    if (!record) throw new BrowserArtifactBrokerError('browser.artifact.not_found')
    if (!sameReference(reference, record.reference)) {
      throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
    }
    if (record.reference.expiresAt <= this.clock.now()) {
      await this.deleteRecord(record)
      throw new BrowserArtifactBrokerError('browser.artifact.expired')
    }
    this.assertExportAuthority(record.owner)
    return record
  }

  private assertExportAuthority(owner: BrowserArtifactOwner): void {
    if (
      this.releasedRuns.has(owner.runId) ||
      this.revokedActivations.has(owner.activationId) ||
      this.closedToolCalls.has(keyForToolCall(owner.runId, owner.toolCallId))
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.closed')
    }
  }

  /** Exact synchronous authority check immediately adjacent to the atomic export rename. */
  private assertExportRecordLive(
    reference: BrowserArtifactReference,
    record: ArtifactRecord
  ): void {
    this.assertUsable()
    if (
      this.artifacts.get(reference.artifactId) !== record ||
      !sameReference(reference, record.reference)
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
    }
    if (record.reference.expiresAt <= this.clock.now()) {
      throw new BrowserArtifactBrokerError('browser.artifact.expired')
    }
    this.assertExportAuthority(record.owner)
  }

  private armExpirationTimer(): void {
    if (this.closed) return
    if (this.expirationTimer) this.clock.clearTimeout(this.expirationTimer)
    this.expirationTimer = undefined
    let nextExpiry = Number.POSITIVE_INFINITY
    for (const record of this.artifacts.values()) {
      nextExpiry = Math.min(nextExpiry, record.reference.expiresAt)
    }
    if (!Number.isFinite(nextExpiry)) return
    const delay = Math.max(1, nextExpiry - this.clock.now())
    this.expirationTimer = this.clock.setTimeout(() => {
      this.expirationTimer = undefined
      void this.sweepExpired().catch(() => undefined)
    }, delay)
    this.expirationTimer.unref?.()
  }

  private async withMutationLock<T>(operation: () => Promise<T>): Promise<T> {
    const previous = this.mutationTail
    let release!: () => void
    this.mutationTail = new Promise<void>((resolveLock) => {
      release = resolveLock
    })
    await previous.catch(() => undefined)
    try {
      return await operation()
    } finally {
      release()
    }
  }

  private assertSession(session: SessionRecord): void {
    this.assertUsable()
    if (session.closed || this.sessions.get(session.id) !== session) {
      throw new BrowserArtifactBrokerError('browser.artifact.closed')
    }
  }

  private assertUsable(): void {
    if (this.closed) throw new BrowserArtifactBrokerError('browser.artifact.closed')
  }
}

function validateCreateInput(input: BrowserArtifactCreateInput): BrowserArtifactCreateInput {
  if (!input || typeof input !== 'object') {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
  }
  const owner = input.owner
  if (
    !owner ||
    owner.capabilityId !== 'browser_automation' ||
    !safeIdentity(owner.runId) ||
    !safeIdentity(owner.activationId) ||
    !safeIdentity(owner.toolCallId) ||
    !Number.isSafeInteger(owner.generation) ||
    owner.generation <= 0
  ) {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
  }
  parseBrowserSurfaceId(owner.surfaceId)
  if (input.allowPreview !== undefined && typeof input.allowPreview !== 'boolean') {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
  }
  const mimeType = input.mimeType.trim().toLowerCase()
  validateKindMime(input.kind, mimeType)
  return {
    owner: { ...owner },
    kind: input.kind,
    mimeType,
    suggestedFileName: safeSuggestedFileName(input.suggestedFileName),
    ...(input.allowPreview === undefined ? {} : { allowPreview: input.allowPreview })
  }
}

function safeIdentity(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.length >= 1 &&
    value.length <= MAX_IDENTITY_LENGTH &&
    !hasAsciiControl(value)
  )
}

export function safeSuggestedFileName(value: unknown): string {
  if (typeof value !== 'string') {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_name')
  }
  const normalized = value.normalize('NFKC').trim()
  if (
    normalized.length < 1 ||
    normalized.length > 128 ||
    normalized === '.' ||
    normalized === '..' ||
    basename(normalized) !== normalized ||
    hasUnsafeFileNameCharacter(normalized) ||
    normalized.startsWith('.') ||
    normalized.endsWith('.') ||
    normalized.endsWith(' ')
  ) {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_name')
  }
  return normalized
}

function hasAsciiControl(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code <= 0x1f || code === 0x7f) return true
  }
  return false
}

function hasUnsafeFileNameCharacter(value: string): boolean {
  if (hasAsciiControl(value)) return true
  for (const character of value) {
    if ('<>:"/\\|?*'.includes(character)) return true
  }
  return false
}

function validateKindMime(kind: BrowserArtifactKind, rawMimeType: string): void {
  const mimeType = rawMimeType.trim().toLowerCase()
  const allowed = MIME_TYPES_BY_KIND[kind]
  if (!allowed?.has(mimeType)) {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_type')
  }
}

const MIME_TYPES_BY_KIND: Readonly<Record<BrowserArtifactKind, ReadonlySet<string>>> = {
  image: new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']),
  text: new Set(['text/plain']),
  json: new Set(['application/json']),
  pdf: new Set(['application/pdf']),
  trace: new Set(['application/zip']),
  video: new Set(['video/webm', 'video/mp4']),
  download: new Set([
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
  ]),
  snapshot: new Set(['text/plain', 'application/json']),
  console: new Set(['text/plain', 'application/json']),
  network: new Set(['text/plain', 'application/json'])
}

function previewFor(
  kind: BrowserArtifactKind,
  size: number,
  maxPreviewBytes: number,
  maxTextPreviewBytes: number,
  allowPreview: boolean
): BrowserArtifactPreview {
  if (!allowPreview) return 'none'
  if (kind === 'image' && size <= maxPreviewBytes) return 'image'
  if (
    ['text', 'json', 'snapshot', 'console', 'network'].includes(kind) &&
    size <= maxTextPreviewBytes
  ) {
    return 'text'
  }
  return 'none'
}

async function validateTextualFile(
  path: string,
  kind: BrowserArtifactKind,
  size: number,
  maxBytes: number
): Promise<void> {
  if (!['text', 'json', 'snapshot', 'console', 'network'].includes(kind)) return
  if (size > maxBytes) throw new BrowserArtifactBrokerError('browser.artifact.too_large')
  const bytes = await readFile(path)
  let text: string
  try {
    text = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  } catch {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
  }
  if (kind === 'json') {
    try {
      JSON.parse(text)
    } catch {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
  }
}

async function validatedRegularFile(
  path: string,
  expectedRoot: string
): Promise<{ dev: bigint | number; ino: bigint | number; size: number }> {
  try {
    const root = await realpath(expectedRoot)
    const candidate = await realpath(path)
    const relativePath = relative(root, candidate)
    if (!relativePath || relativePath.startsWith('..') || isAbsolute(relativePath)) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    const metadata = await lstat(path)
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    const handle = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0))
    try {
      const opened = await handle.stat()
      if (
        !opened.isFile() ||
        opened.nlink !== 1 ||
        opened.dev !== metadata.dev ||
        opened.ino !== metadata.ino
      ) {
        throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
      }
    } finally {
      await handle.close()
    }
    return { dev: metadata.dev, ino: metadata.ino, size: metadata.size }
  } catch (error) {
    if (error instanceof BrowserArtifactBrokerError) throw error
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
  }
}

async function hashFile(path: string): Promise<string> {
  const handle = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0))
  const hash = createHash('sha256')
  try {
    const stream = handle.createReadStream({ autoClose: false })
    for await (const chunk of stream) hash.update(chunk as Buffer)
    return hash.digest('hex')
  } finally {
    await handle.close()
  }
}

async function readVerifiedArtifactBytes(
  path: string,
  expectedRoot: string,
  expectedSize: number,
  expectedSha256: string
): Promise<Uint8Array> {
  const validated = await validatedRegularFile(path, expectedRoot)
  if (validated.size !== expectedSize) {
    throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
  }
  const handle = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0))
  try {
    const before = await handle.stat()
    if (
      !before.isFile() ||
      before.nlink !== 1 ||
      before.dev !== validated.dev ||
      before.ino !== validated.ino ||
      before.size !== expectedSize
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
    }
    const bytes = await handle.readFile()
    const after = await handle.stat()
    if (
      after.dev !== before.dev ||
      after.ino !== before.ino ||
      after.nlink !== 1 ||
      after.size !== before.size ||
      bytes.byteLength !== expectedSize ||
      hashBytes(bytes) !== expectedSha256
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.identity_mismatch')
    }
    return Uint8Array.from(bytes)
  } finally {
    await handle.close()
  }
}

interface BrowserArtifactExportDestination {
  directory: string
  directoryDev: bigint | number
  directoryIno: bigint | number
  displayName: string
  path: string
}

async function resolveExportDestination(
  value: unknown,
  managedRoot: string
): Promise<BrowserArtifactExportDestination> {
  if (typeof value !== 'string' || value.length > 4_096 || hasAsciiControl(value)) {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_name')
  }
  const lexicalPath = resolve(value)
  if (!isAbsolute(value) || value !== lexicalPath) {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_name')
  }
  const rawDisplayName = basename(lexicalPath)
  const displayName = safeSuggestedFileName(rawDisplayName)
  if (displayName !== rawDisplayName) {
    throw new BrowserArtifactBrokerError('browser.artifact.invalid_name')
  }
  try {
    const [directory, privateRoot] = await Promise.all([
      realpath(dirname(lexicalPath)),
      realpath(managedRoot)
    ])
    const directoryMetadata = await lstat(directory)
    if (!directoryMetadata.isDirectory() || directoryMetadata.isSymbolicLink()) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    const destinationPath = join(directory, displayName)
    const privateRelative = relative(privateRoot, destinationPath)
    if (
      privateRelative === '' ||
      (!privateRelative.startsWith('..') && !isAbsolute(privateRelative))
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
    return {
      directory,
      directoryDev: directoryMetadata.dev,
      directoryIno: directoryMetadata.ino,
      displayName,
      path: destinationPath
    }
  } catch (error) {
    if (error instanceof BrowserArtifactBrokerError) throw error
    throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
  }
}

async function assertExportDirectoryIdentity(
  destination: BrowserArtifactExportDestination
): Promise<void> {
  try {
    const metadata = await lstat(destination.directory)
    if (
      !metadata.isDirectory() ||
      metadata.isSymbolicLink() ||
      metadata.dev !== destination.directoryDev ||
      metadata.ino !== destination.directoryIno
    ) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
  } catch (error) {
    if (error instanceof BrowserArtifactBrokerError) throw error
    throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
  }
}

async function assertReplaceableExportLeaf(path: string): Promise<void> {
  try {
    const metadata = await lstat(path)
    // Existing regular files may be replaced after Electron's native overwrite confirmation.
    // Symlinks and special files are never followed or replaced.
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new BrowserArtifactBrokerError('browser.artifact.invalid_file')
    }
  } catch (error) {
    if (isFileSystemError(error, 'ENOENT')) return
    if (error instanceof BrowserArtifactBrokerError) throw error
    throw new BrowserArtifactBrokerError('browser.artifact.unavailable')
  }
}

function hashBytes(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex')
}

function isFileSystemError(error: unknown, code: string): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error as { code?: unknown }).code === code
  )
}

function sameReference(left: BrowserArtifactReference, right: BrowserArtifactReference): boolean {
  return (
    left.schemaVersion === right.schemaVersion &&
    left.artifactId === right.artifactId &&
    left.kind === right.kind &&
    left.displayName === right.displayName &&
    left.mimeType === right.mimeType &&
    left.sizeBytes === right.sizeBytes &&
    left.createdAt === right.createdAt &&
    left.expiresAt === right.expiresAt &&
    left.lifecycle === right.lifecycle &&
    left.owner === right.owner &&
    left.preview === right.preview
  )
}

function positiveBound(value: number | undefined, fallback: number): number {
  return Number.isSafeInteger(value) && (value ?? 0) > 0 ? (value as number) : fallback
}

function keyForSurface(surfaceId: string, generation: number): string {
  return `${surfaceId}\u0000${generation}`
}

function keyForToolCall(runId: string, toolCallId: string): string {
  return `${runId}\u0000${toolCallId}`
}
