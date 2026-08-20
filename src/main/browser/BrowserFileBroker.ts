import { createHash, randomUUID } from 'node:crypto'
import { constants } from 'node:fs'
import { chmod, lstat, mkdir, open, realpath, rename, rm, stat, unlink } from 'node:fs/promises'
import { basename, dirname, extname, isAbsolute, join, resolve } from 'node:path'
import { parseBrowserSurfaceId } from '@mycopilot/protocol'

const DEFAULT_TTL_MS = 15 * 60 * 1_000
const MAX_TTL_MS = 24 * 60 * 60 * 1_000
const DEFAULT_MAX_FILE_BYTES = 64 * 1024 * 1024
const DEFAULT_MAX_RUN_BYTES = 128 * 1024 * 1024
const DEFAULT_MAX_FILES = 16
const DEFAULT_MAX_HANDLES = 64
const FILE_HANDLE =
  /^browser-file:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

export type BrowserFileBrokerErrorCode =
  | 'browser.file.closed'
  | 'browser.file.cancelled'
  | 'browser.file.invalid_handle'
  | 'browser.file.identity_mismatch'
  | 'browser.file.changed'
  | 'browser.file.expired'
  | 'browser.file.invalid_file'
  | 'browser.file.symlink_forbidden'
  | 'browser.file.too_large'
  | 'browser.file.capacity'
  | 'browser.file.selection_failed'

export class BrowserFileBrokerError extends Error {
  readonly name = 'BrowserFileBrokerError'

  constructor(readonly code: BrowserFileBrokerErrorCode) {
    super(code)
  }
}

export interface BrowserFileOwner {
  runId: string
  activationId: string
  capabilityId: 'browser_automation'
  toolCallId: string
}

/** Safe projection. Source and private staging paths never leave Main. */
export interface BrowserFileReference {
  schemaVersion: 1
  handle: string
  displayName: string
  mimeType: string
  sizeBytes: number
  createdAt: number
  expiresAt: number
}

export interface BrowserFileSelectionRequest {
  multiple: boolean
  maxFiles: number
  suggestedNames: readonly string[]
}

export interface BrowserFileSelectionProvider {
  selectFiles(request: BrowserFileSelectionRequest): Promise<readonly string[] | null>
}

export interface BrowserFileBrokerClock {
  now(): number
}

export interface BrowserFileBrokerOptions {
  rootDirectory: string
  selectionProvider: BrowserFileSelectionProvider
  /** Synchronous final-consume hook used only to prove the no-await TTL authority boundary. */
  beforeConsumeFinal?: () => void
  clock?: BrowserFileBrokerClock
  ttlMs?: number
  maxFileBytes?: number
  maxRunBytes?: number
  maxFilesPerSelection?: number
  maxHandles?: number
}

interface FileRevision {
  dev: bigint
  ino: bigint
  size: bigint
  mtimeNs: bigint
  ctimeNs: bigint
}

interface BrowserFileRecord {
  reference: BrowserFileReference
  owner: BrowserFileOwner
  sourcePath: string
  privatePath: string
  revision: FileRevision
  revisionDigest: string
  claimedByToolCallId?: string
}

interface BrowserFileRetainedRecord {
  dispatched: boolean
  dispose: () => Promise<void>
  expiresAt: number
  fileCount: number
  generation: number
  id: string
  owner: BrowserFileOwner
  sizeBytes: number
  surfaceId: string
}

interface BrowserFileSelectionFence {
  owner: BrowserFileOwner
  lifecycleEpoch: number
  runEpoch: number
  activationEpoch: number
  toolCallEpoch: number
}

export interface BrowserFileResolution {
  /** Main-only immutable private copy. Never serialize this object. */
  paths: readonly string[]
  references: readonly BrowserFileReference[]
  /** Main-only audit identity. Approval scope is independently derived from opaque handles. */
  fileRevisionDigest: string
}

export interface BrowserFileReadLease extends BrowserFileResolution {
  /** Idempotently destroys the one-time private copies. */
  finish(): Promise<void>
}

export interface BrowserFileRetainedLease {
  /** Marks that Chromium may now hold the file path; later Tool cancellation cannot revoke it. */
  markDispatched(): void
  /** Releases only a definitely-not-dispatched retention. Dispatched files remain Broker-owned. */
  finish(): Promise<void>
}

/**
 * Host-owned local-file authority for sensitive browser tools.
 *
 * A native picker is the only source of new authority. The selected file is opened with
 * O_NOFOLLOW, bounded, copied through the already-open descriptor into a private 0700 directory,
 * and represented outside Main only by an opaque handle and basename. Before use, the original
 * identity/revision is checked again; after that check the official MCP reads the immutable private
 * copy, closing the path-replacement TOCTOU window.
 */
export class BrowserFileBroker {
  private readonly beforeConsumeFinal: () => void
  private readonly clock: BrowserFileBrokerClock
  private readonly maxFileBytes: number
  private readonly maxFilesPerSelection: number
  private readonly maxHandles: number
  private readonly maxRunBytes: number
  private readonly rootDirectory: string
  private readonly selectionProvider: BrowserFileSelectionProvider
  private readonly ttlMs: number
  private readonly records = new Map<string, BrowserFileRecord>()
  private readonly retained = new Map<string, BrowserFileRetainedRecord>()
  private readonly activeFreezes = new Set<Promise<BrowserFileRecord>>()
  private readonly activationEpochs = new Map<string, number>()
  private readonly runEpochs = new Map<string, number>()
  private readonly toolCallEpochs = new Map<string, number>()
  private initializing?: Promise<void>
  private shutdownOperation?: Promise<void>
  private closed = false
  private lifecycleEpoch = 0

  constructor(options: BrowserFileBrokerOptions) {
    const rootDirectory = resolve(options.rootDirectory)
    if (
      !rootDirectory ||
      rootDirectory === '/' ||
      basename(rootDirectory) !== 'browser-automation-files' ||
      dirname(rootDirectory) === rootDirectory
    ) {
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    this.rootDirectory = rootDirectory
    this.selectionProvider = options.selectionProvider
    this.beforeConsumeFinal = options.beforeConsumeFinal ?? (() => undefined)
    this.clock = options.clock ?? { now: Date.now }
    this.ttlMs = boundedInteger(options.ttlMs ?? DEFAULT_TTL_MS, 1_000, MAX_TTL_MS)
    this.maxFileBytes = boundedInteger(
      options.maxFileBytes ?? DEFAULT_MAX_FILE_BYTES,
      1,
      DEFAULT_MAX_FILE_BYTES
    )
    this.maxRunBytes = boundedInteger(
      options.maxRunBytes ?? DEFAULT_MAX_RUN_BYTES,
      this.maxFileBytes,
      DEFAULT_MAX_RUN_BYTES
    )
    this.maxFilesPerSelection = boundedInteger(
      options.maxFilesPerSelection ?? DEFAULT_MAX_FILES,
      1,
      DEFAULT_MAX_FILES
    )
    this.maxHandles = boundedInteger(options.maxHandles ?? DEFAULT_MAX_HANDLES, 1, 256)
  }

  async selectForRead(input: {
    owner: BrowserFileOwner
    multiple: boolean
    suggestedNames?: readonly string[]
  }): Promise<readonly BrowserFileReference[]> {
    this.assertOpen()
    const owner = validateOwner(input.owner)
    const fence = this.captureSelectionFence(owner)
    await this.initialize()
    this.assertSelectionFence(fence)
    await this.purgeExpired()
    this.assertSelectionFence(fence)
    if (this.records.size + this.retainedFileCount() >= this.maxHandles) {
      throw new BrowserFileBrokerError('browser.file.capacity')
    }
    const selected = await this.selectionProvider.selectFiles({
      multiple: input.multiple,
      maxFiles: input.multiple ? this.maxFilesPerSelection : 1,
      suggestedNames: safeSuggestedNames(input.suggestedNames)
    })
    this.assertSelectionFence(fence)
    if (!selected || selected.length === 0) return []
    return this.freezeReadPaths(
      owner,
      fence,
      selected,
      input.multiple ? this.maxFilesPerSelection : 1
    )
  }

  /**
   * Freezes paths already authorized by Core's workspace/attachment resolver.
   *
   * This entrypoint is process-internal: raw paths must never come from Renderer or model input
   * directly. Main still independently applies the same O_NOFOLLOW, revision, size and capacity
   * checks as the native picker path and publishes only opaque references.
   */
  async freezeResolvedForRead(input: {
    owner: BrowserFileOwner
    paths: readonly string[]
  }): Promise<readonly BrowserFileReference[]> {
    this.assertOpen()
    const owner = validateOwner(input.owner)
    if (
      !Array.isArray(input.paths) ||
      input.paths.length < 1 ||
      input.paths.length > this.maxFilesPerSelection ||
      input.paths.some(
        (path) =>
          typeof path !== 'string' ||
          path.length < 1 ||
          path.length > 8_192 ||
          !isAbsolute(path)
      )
    ) {
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    const fence = this.captureSelectionFence(owner)
    await this.initialize()
    this.assertSelectionFence(fence)
    await this.purgeExpired()
    this.assertSelectionFence(fence)
    return this.freezeReadPaths(owner, fence, input.paths, this.maxFilesPerSelection)
  }

  private async freezeReadPaths(
    owner: BrowserFileOwner,
    fence: BrowserFileSelectionFence,
    selected: readonly string[],
    maxFiles: number
  ): Promise<readonly BrowserFileReference[]> {
    if (
      selected.length < 1 ||
      selected.length > maxFiles ||
      selected.length + this.records.size + this.retainedFileCount() > this.maxHandles
    ) {
      throw new BrowserFileBrokerError('browser.file.capacity')
    }
    const created: BrowserFileRecord[] = []
    try {
      for (const selectedPath of selected) {
        this.assertSelectionFence(fence)
        if (
          typeof selectedPath !== 'string' ||
          selectedPath.length < 1 ||
          selectedPath.length > 8_192
        ) {
          throw new BrowserFileBrokerError('browser.file.invalid_file')
        }
        const record = await this.trackFreeze(this.freezeSelectedFile(owner, selectedPath, fence))
        created.push(record)
        this.assertSelectionFence(fence)
      }
      this.assertSelectionFence(fence)
      this.assertRunCapacity(
        owner.runId,
        created.reduce((sum, item) => sum + item.reference.sizeBytes, 0)
      )
      for (const record of created) {
        this.assertSelectionFence(fence)
        this.records.set(record.reference.handle, record)
      }
      return created.map((record) => structuredClone(record.reference))
    } catch (error) {
      await Promise.allSettled(created.map((record) => rm(record.privatePath, { force: true })))
      throw error
    }
  }

  async resolveForRead(input: {
    owner: BrowserFileOwner
    handles: readonly string[]
  }): Promise<BrowserFileResolution> {
    this.assertOpen()
    const owner = validateOwner(input.owner)
    const fence = this.captureSelectionFence(owner)
    await this.initialize()
    this.assertSelectionFence(fence)
    if (
      !Array.isArray(input.handles) ||
      input.handles.length < 1 ||
      input.handles.length > this.maxFilesPerSelection ||
      new Set(input.handles).size !== input.handles.length
    ) {
      throw new BrowserFileBrokerError('browser.file.invalid_handle')
    }
    const records: BrowserFileRecord[] = []
    for (const handle of input.handles) {
      if (typeof handle !== 'string' || !FILE_HANDLE.test(handle)) {
        throw new BrowserFileBrokerError('browser.file.invalid_handle')
      }
      const record = this.records.get(handle)
      if (!record) throw new BrowserFileBrokerError('browser.file.invalid_handle')
      if (!sameOwner(record.owner, owner)) {
        throw new BrowserFileBrokerError('browser.file.identity_mismatch')
      }
      if (
        record.claimedByToolCallId !== undefined &&
        record.claimedByToolCallId !== owner.toolCallId
      ) {
        throw new BrowserFileBrokerError('browser.file.identity_mismatch')
      }
      if (record.reference.expiresAt <= this.clock.now()) {
        await this.deleteRecord(record)
        throw new BrowserFileBrokerError('browser.file.expired')
      }
      records.push(record)
    }

    // Claim the complete handle set synchronously before the first filesystem await. This is the
    // linearization point for concurrent approved calls: a second call can no longer pass the
    // unclaimed check while this call is revalidating source revisions.
    for (const record of records) record.claimedByToolCallId ??= owner.toolCallId

    try {
      for (const record of records) {
        let revision: FileRevision
        try {
          revision = await readPathRevision(record.sourcePath)
          this.assertSelectionFence(fence)
        } catch {
          await this.deleteRecord(record)
          throw new BrowserFileBrokerError('browser.file.changed')
        }
        if (!sameRevision(revision, record.revision)) {
          await this.deleteRecord(record)
          throw new BrowserFileBrokerError('browser.file.changed')
        }
        await assertPrivateCopy(record.privatePath, record.reference.sizeBytes)
        this.assertSelectionFence(fence)
      }
      const now = this.clock.now()
      for (const record of records) {
        if (
          this.records.get(record.reference.handle) !== record ||
          !sameOwner(record.owner, owner) ||
          record.claimedByToolCallId !== owner.toolCallId
        ) {
          throw new BrowserFileBrokerError('browser.file.identity_mismatch')
        }
        if (record.reference.expiresAt <= now) {
          await this.deleteRecord(record)
          throw new BrowserFileBrokerError('browser.file.expired')
        }
      }
    } catch (error) {
      // A failed pre-dispatch verification must not strand otherwise-valid handles. The changed
      // record was already destroyed; remaining handles may be selected again by another exact
      // approved call if the caller chooses to retry from a definitely-not-dispatched result.
      for (const record of records) {
        if (
          this.records.get(record.reference.handle) === record &&
          record.claimedByToolCallId === owner.toolCallId
        ) {
          delete record.claimedByToolCallId
        }
      }
      throw error
    }
    return {
      paths: records.map((record) => record.privatePath),
      references: records.map((record) => structuredClone(record.reference)),
      fileRevisionDigest: digestJson(
        records.map((record) => ({
          handle: record.reference.handle,
          revision: record.revisionDigest,
          sizeBytes: record.reference.sizeBytes
        }))
      )
    }
  }

  /**
   * Atomically consumes task-scoped handles for one approved upstream dispatch. Handles disappear
   * before control returns, so concurrent or repeated calls cannot reuse the same file authority.
   */
  async consumeForRead(input: {
    owner: BrowserFileOwner
    handles: readonly string[]
  }): Promise<BrowserFileReadLease> {
    const owner = validateOwner(input.owner)
    const fence = this.captureSelectionFence(owner)
    const resolution = await this.resolveForRead({ ...input, owner })
    this.beforeConsumeFinal()
    this.assertSelectionFence(fence)
    const now = this.clock.now()
    const records = input.handles.map((handle) => this.records.get(handle))
    if (records.some((record) => !record)) {
      throw new BrowserFileBrokerError('browser.file.invalid_handle')
    }
    for (const record of records as BrowserFileRecord[]) {
      if (
        this.records.get(record.reference.handle) !== record ||
        !sameOwner(record.owner, owner) ||
        record.claimedByToolCallId !== owner.toolCallId
      ) {
        throw new BrowserFileBrokerError('browser.file.identity_mismatch')
      }
      if (record.reference.expiresAt <= now) {
        for (const candidate of records as BrowserFileRecord[]) {
          if (this.records.get(candidate.reference.handle) === candidate) {
            this.records.delete(candidate.reference.handle)
          }
        }
        await Promise.allSettled(
          (records as BrowserFileRecord[]).map((candidate) =>
            rm(candidate.privatePath, { force: true })
          )
        )
        throw new BrowserFileBrokerError('browser.file.expired')
      }
    }
    for (const handle of input.handles) this.records.delete(handle)
    let finished = false
    return {
      ...resolution,
      finish: async () => {
        if (finished) return
        finished = true
        await Promise.allSettled(
          (records as BrowserFileRecord[]).map((record) => rm(record.privatePath, { force: true }))
        )
      }
    }
  }

  /**
   * Transfers a Host-staged copy into task/surface lifetime after an opaque handle was consumed.
   * No path is retained here: the Host supplies one bounded idempotent disposer, and this Broker
   * owns when that disposer may run.
   */
  async retainConsumedFiles(input: {
    owner: BrowserFileOwner
    surfaceId: string
    generation: number
    references: readonly BrowserFileReference[]
    dispose: () => Promise<void>
  }): Promise<BrowserFileRetainedLease> {
    this.assertOpen()
    const owner = validateOwner(input.owner)
    const fence = this.captureSelectionFence(owner)
    let surfaceId: string
    try {
      surfaceId = parseBrowserSurfaceId(input.surfaceId)
    } catch {
      throw new BrowserFileBrokerError('browser.file.identity_mismatch')
    }
    if (!Number.isSafeInteger(input.generation) || input.generation < 1) {
      throw new BrowserFileBrokerError('browser.file.identity_mismatch')
    }
    if (
      !Array.isArray(input.references) ||
      input.references.length < 1 ||
      input.references.length > this.maxFilesPerSelection ||
      typeof input.dispose !== 'function'
    ) {
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    const references = input.references.map(validateRetainedReference)
    if (new Set(references.map((reference) => reference.handle)).size !== references.length) {
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    await this.purgeExpired()
    this.assertSelectionFence(fence)
    const fileCount = references.length
    const sizeBytes = references.reduce((sum, reference) => sum + reference.sizeBytes, 0)
    if (this.retainedFileCount() + this.records.size + fileCount > this.maxHandles) {
      throw new BrowserFileBrokerError('browser.file.capacity')
    }
    this.assertRunCapacity(owner.runId, sizeBytes)
    const record: BrowserFileRetainedRecord = {
      dispatched: false,
      dispose: onceAsync(input.dispose),
      expiresAt: Math.min(...references.map((reference) => reference.expiresAt)),
      fileCount,
      generation: input.generation,
      id: randomUUID(),
      owner,
      sizeBytes,
      surfaceId
    }
    this.retained.set(record.id, record)
    return {
      markDispatched: () => {
        if (this.retained.get(record.id) === record) record.dispatched = true
      },
      finish: async () => {
        if (this.retained.get(record.id) === record && !record.dispatched) {
          await this.releaseRetainedRecord(record)
        }
      }
    }
  }

  async releaseRun(runId: string): Promise<void> {
    incrementEpoch(this.runEpochs, runId)
    await Promise.all([
      this.releaseMatching((record) => record.owner.runId === runId),
      this.releaseRetainedMatching((record) => record.owner.runId === runId)
    ])
  }

  async releaseCapability(activationId: string): Promise<void> {
    incrementEpoch(this.activationEpochs, activationId)
    await Promise.all([
      this.releaseMatching((record) => record.owner.activationId === activationId),
      this.releaseRetainedMatching((record) => record.owner.activationId === activationId)
    ])
  }

  async releaseToolCall(owner: Pick<BrowserFileOwner, 'runId' | 'toolCallId'>): Promise<void> {
    incrementEpoch(this.toolCallEpochs, toolCallFenceKey(owner.runId, owner.toolCallId))
    await Promise.all([
      this.releaseMatching(
        (record) =>
          record.owner.runId === owner.runId &&
          (record.owner.toolCallId === owner.toolCallId ||
            record.claimedByToolCallId === owner.toolCallId)
      ),
      this.releaseRetainedMatching(
        (record) =>
          !record.dispatched &&
          record.owner.runId === owner.runId &&
          record.owner.toolCallId === owner.toolCallId
      )
    ])
  }

  async releaseSurface(input: { surfaceId: string; generation: number }): Promise<void> {
    let surfaceId: string
    try {
      surfaceId = parseBrowserSurfaceId(input.surfaceId)
    } catch {
      return
    }
    if (!Number.isSafeInteger(input.generation) || input.generation < 1) return
    await this.releaseRetainedMatching(
      (record) => record.surfaceId === surfaceId && record.generation === input.generation
    )
  }

  async purgeExpired(): Promise<void> {
    const now = this.clock.now()
    await Promise.all([
      this.releaseMatching((record) => record.reference.expiresAt <= now),
      this.releaseRetainedMatching((record) => !record.dispatched && record.expiresAt <= now)
    ])
  }

  snapshot(): {
    handles: number
    bytes: number
    retained: { leases: number; files: number; bytes: number }
  } {
    const retainedBytes = [...this.retained.values()].reduce(
      (sum, record) => sum + record.sizeBytes,
      0
    )
    return {
      handles: this.records.size,
      bytes:
        [...this.records.values()].reduce((sum, record) => sum + record.reference.sizeBytes, 0) +
        retainedBytes,
      retained: {
        leases: this.retained.size,
        files: this.retainedFileCount(),
        bytes: retainedBytes
      }
    }
  }

  /** Removes crash leftovers without ever following a Host-root symlink. */
  async initialize(): Promise<void> {
    this.assertOpen()
    if (this.initializing) return this.initializing
    const initializing = (async () => {
      const existing = await lstat(this.rootDirectory).catch(() => null)
      if (existing?.isSymbolicLink()) {
        await unlink(this.rootDirectory)
      } else if (existing) {
        await rm(this.rootDirectory, { recursive: true, force: true })
      }
      await mkdir(this.rootDirectory, { recursive: true, mode: 0o700 })
      await chmod(this.rootDirectory, 0o700)
    })()
    this.initializing = initializing
    try {
      await initializing
    } catch {
      this.initializing = undefined
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
  }

  async shutdown(): Promise<void> {
    if (this.shutdownOperation) return this.shutdownOperation
    this.closed = true
    this.lifecycleEpoch += 1
    this.records.clear()
    const retained = [...this.retained.values()]
    this.retained.clear()
    const shutdownOperation = (async () => {
      await this.initializing?.catch(() => undefined)
      while (this.activeFreezes.size > 0) {
        await Promise.allSettled([...this.activeFreezes])
      }
      await Promise.allSettled([
        ...retained.map((record) => record.dispose()),
        rm(this.rootDirectory, { recursive: true, force: true })
      ])
    })()
    this.shutdownOperation = shutdownOperation
    return shutdownOperation
  }

  private async freezeSelectedFile(
    owner: BrowserFileOwner,
    selectedPath: string,
    fence: BrowserFileSelectionFence
  ): Promise<BrowserFileRecord> {
    this.assertSelectionFence(fence)
    const sourcePath = resolve(selectedPath)
    const before = await lstat(sourcePath, { bigint: true }).catch(() => null)
    this.assertSelectionFence(fence)
    if (!before) {
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    if (before.isSymbolicLink()) {
      throw new BrowserFileBrokerError('browser.file.symlink_forbidden')
    }
    if (!before.isFile()) {
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    if (before.nlink !== 1n) {
      // A hard link can change through a second name after approval; reject it rather than claim a
      // revision guarantee we cannot enforce across arbitrary user directories.
      throw new BrowserFileBrokerError('browser.file.invalid_file')
    }
    if (before.size < 0n || before.size > BigInt(this.maxFileBytes)) {
      throw new BrowserFileBrokerError('browser.file.too_large')
    }
    const canonicalPath = await realpath(sourcePath).catch(() => null)
    this.assertSelectionFence(fence)
    if (!canonicalPath) throw new BrowserFileBrokerError('browser.file.invalid_file')

    await mkdir(this.rootDirectory, { recursive: true, mode: 0o700 })
    await chmod(this.rootDirectory, 0o700)
    this.assertSelectionFence(fence)
    const handle = `browser-file:${randomUUID()}`
    const safeName = safeFileName(basename(canonicalPath))
    const privatePath = join(
      this.rootDirectory,
      `${handle.slice('browser-file:'.length)}${extname(safeName)}`
    )
    const temporaryPath = `${privatePath}.partial`
    const source = await open(sourcePath, constants.O_RDONLY | platformNoFollowFlag()).catch(
      (error) => {
        throw mapOpenError(error)
      }
    )
    try {
      const opened = await source.stat({ bigint: true })
      this.assertSelectionFence(fence)
      const revision = revisionFromStat(opened)
      if (!sameRevision(revision, revisionFromStat(before))) {
        throw new BrowserFileBrokerError('browser.file.changed')
      }
      const output = await open(
        temporaryPath,
        constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | platformNoFollowFlag(),
        0o600
      )
      const hash = createHash('sha256')
      hash.update(
        [revision.dev, revision.ino, revision.size, revision.mtimeNs, revision.ctimeNs]
          .map(String)
          .join(':')
      )
      let copiedBytes = 0
      try {
        const chunk = Buffer.allocUnsafe(Math.min(64 * 1024, Math.max(1, Number(opened.size))))
        while (copiedBytes < Number(opened.size)) {
          this.assertSelectionFence(fence)
          const requested = Math.min(chunk.byteLength, Number(opened.size) - copiedBytes)
          const { bytesRead } = await source.read(chunk, 0, requested, null)
          this.assertSelectionFence(fence)
          if (bytesRead <= 0) throw new BrowserFileBrokerError('browser.file.changed')
          const bytes = chunk.subarray(0, bytesRead)
          let written = 0
          while (written < bytesRead) {
            const { bytesWritten } = await output.write(bytes, written, bytesRead - written, null)
            if (bytesWritten <= 0) throw new BrowserFileBrokerError('browser.file.invalid_file')
            written += bytesWritten
          }
          hash.update(bytes)
          copiedBytes += bytesRead
        }
        if ((await source.read(Buffer.allocUnsafe(1), 0, 1, null)).bytesRead !== 0) {
          throw new BrowserFileBrokerError('browser.file.changed')
        }
        await output.sync()
        this.assertSelectionFence(fence)
      } finally {
        await output.close()
      }
      const after = revisionFromStat(await source.stat({ bigint: true }))
      this.assertSelectionFence(fence)
      if (!sameRevision(after, revision) || copiedBytes !== Number(opened.size)) {
        throw new BrowserFileBrokerError('browser.file.changed')
      }
      await rename(temporaryPath, privatePath)
      await chmod(privatePath, 0o600)
      this.assertSelectionFence(fence)
      const createdAt = this.clock.now()
      const reference: BrowserFileReference = {
        schemaVersion: 1,
        handle,
        displayName: safeName,
        mimeType: mimeTypeForName(safeName),
        sizeBytes: copiedBytes,
        createdAt,
        expiresAt: createdAt + this.ttlMs
      }
      return {
        reference,
        owner,
        sourcePath,
        privatePath,
        revision,
        revisionDigest: `sha256:${hash.digest('hex')}`
      }
    } catch (error) {
      await Promise.allSettled([
        rm(temporaryPath, { force: true }),
        rm(privatePath, { force: true })
      ])
      throw error
    } finally {
      await source.close()
    }
  }

  private assertRunCapacity(runId: string, additionalBytes: number): void {
    const current =
      [...this.records.values()]
        .filter((record) => record.owner.runId === runId)
        .reduce((sum, record) => sum + record.reference.sizeBytes, 0) +
      [...this.retained.values()]
        .filter((record) => record.owner.runId === runId)
        .reduce((sum, record) => sum + record.sizeBytes, 0)
    if (current + additionalBytes > this.maxRunBytes) {
      throw new BrowserFileBrokerError('browser.file.capacity')
    }
  }

  private captureSelectionFence(owner: BrowserFileOwner): BrowserFileSelectionFence {
    return {
      owner,
      lifecycleEpoch: this.lifecycleEpoch,
      runEpoch: epochFor(this.runEpochs, owner.runId),
      activationEpoch: epochFor(this.activationEpochs, owner.activationId),
      toolCallEpoch: epochFor(this.toolCallEpochs, toolCallFenceKey(owner.runId, owner.toolCallId))
    }
  }

  private assertSelectionFence(fence: BrowserFileSelectionFence): void {
    if (this.closed || fence.lifecycleEpoch !== this.lifecycleEpoch) {
      throw new BrowserFileBrokerError('browser.file.closed')
    }
    if (
      fence.runEpoch !== epochFor(this.runEpochs, fence.owner.runId) ||
      fence.activationEpoch !== epochFor(this.activationEpochs, fence.owner.activationId) ||
      fence.toolCallEpoch !==
        epochFor(this.toolCallEpochs, toolCallFenceKey(fence.owner.runId, fence.owner.toolCallId))
    ) {
      throw new BrowserFileBrokerError('browser.file.cancelled')
    }
  }

  private async trackFreeze(operation: Promise<BrowserFileRecord>): Promise<BrowserFileRecord> {
    this.activeFreezes.add(operation)
    try {
      return await operation
    } finally {
      this.activeFreezes.delete(operation)
    }
  }

  private async releaseMatching(predicate: (record: BrowserFileRecord) => boolean): Promise<void> {
    const matching = [...this.records.values()].filter(predicate)
    for (const record of matching) this.records.delete(record.reference.handle)
    await Promise.allSettled(matching.map((record) => rm(record.privatePath, { force: true })))
  }

  private async releaseRetainedMatching(
    predicate: (record: BrowserFileRetainedRecord) => boolean
  ): Promise<void> {
    const matching = [...this.retained.values()].filter(predicate)
    for (const record of matching) this.retained.delete(record.id)
    await Promise.allSettled(matching.map((record) => record.dispose()))
  }

  private async releaseRetainedRecord(record: BrowserFileRetainedRecord): Promise<void> {
    if (this.retained.get(record.id) !== record) return
    this.retained.delete(record.id)
    await record.dispose().catch(() => undefined)
  }

  private retainedFileCount(): number {
    return [...this.retained.values()].reduce((sum, record) => sum + record.fileCount, 0)
  }

  private async deleteRecord(record: BrowserFileRecord): Promise<void> {
    this.records.delete(record.reference.handle)
    await rm(record.privatePath, { force: true }).catch(() => undefined)
  }

  private assertOpen(): void {
    if (this.closed) throw new BrowserFileBrokerError('browser.file.closed')
  }
}

function validateOwner(value: BrowserFileOwner): BrowserFileOwner {
  if (
    !value ||
    typeof value.runId !== 'string' ||
    value.runId.length < 1 ||
    value.runId.length > 256 ||
    typeof value.activationId !== 'string' ||
    value.activationId.length < 1 ||
    value.activationId.length > 128 ||
    value.capabilityId !== 'browser_automation' ||
    typeof value.toolCallId !== 'string' ||
    value.toolCallId.length < 1 ||
    value.toolCallId.length > 256
  ) {
    throw new BrowserFileBrokerError('browser.file.identity_mismatch')
  }
  return { ...value }
}

function validateRetainedReference(value: BrowserFileReference): BrowserFileReference {
  if (
    !value ||
    value.schemaVersion !== 1 ||
    typeof value.handle !== 'string' ||
    !FILE_HANDLE.test(value.handle) ||
    typeof value.displayName !== 'string' ||
    value.displayName.length < 1 ||
    value.displayName.length > 240 ||
    typeof value.mimeType !== 'string' ||
    value.mimeType.length < 1 ||
    value.mimeType.length > 256 ||
    !Number.isSafeInteger(value.sizeBytes) ||
    value.sizeBytes < 0 ||
    value.sizeBytes > DEFAULT_MAX_FILE_BYTES ||
    !Number.isSafeInteger(value.createdAt) ||
    !Number.isSafeInteger(value.expiresAt) ||
    value.expiresAt <= value.createdAt
  ) {
    throw new BrowserFileBrokerError('browser.file.invalid_file')
  }
  return structuredClone(value)
}

function onceAsync(operation: () => Promise<void>): () => Promise<void> {
  let pending: Promise<void> | undefined
  return () => (pending ??= Promise.resolve().then(operation))
}

function sameOwner(left: BrowserFileOwner, right: BrowserFileOwner): boolean {
  return (
    left.runId === right.runId &&
    left.activationId === right.activationId &&
    left.capabilityId === right.capabilityId
  )
}

function epochFor(epochs: ReadonlyMap<string, number>, key: string): number {
  return epochs.get(key) ?? 0
}

function incrementEpoch(epochs: Map<string, number>, key: string): void {
  epochs.set(key, epochFor(epochs, key) + 1)
}

function toolCallFenceKey(runId: string, toolCallId: string): string {
  return `${runId.length}:${runId}${toolCallId}`
}

function safeSuggestedNames(value: readonly string[] | undefined): readonly string[] {
  if (!value) return []
  return value.slice(0, DEFAULT_MAX_FILES).map((item) => safeFileName(basename(String(item))))
}

function safeFileName(value: string): string {
  const normalized = [...value.normalize('NFC')]
    .map((character) => {
      const code = character.charCodeAt(0)
      return code <= 0x1f ||
        code === 0x7f ||
        character === '/' ||
        character === '\\' ||
        character === ':'
        ? '_'
        : character
    })
    .join('')
    .replace(/^\.+/, '')
    .trim()
    .slice(0, 240)
  return normalized || 'selected-file'
}

function mimeTypeForName(name: string): string {
  switch (extname(name).toLowerCase()) {
    case '.json':
      return 'application/json'
    case '.pdf':
      return 'application/pdf'
    case '.png':
      return 'image/png'
    case '.jpg':
    case '.jpeg':
      return 'image/jpeg'
    case '.txt':
    case '.md':
    case '.csv':
      return 'text/plain'
    default:
      return 'application/octet-stream'
  }
}

async function readPathRevision(path: string): Promise<FileRevision> {
  const value = await lstat(path, { bigint: true })
  if (!value.isFile() || value.isSymbolicLink() || value.nlink !== 1n) {
    throw new BrowserFileBrokerError('browser.file.invalid_file')
  }
  return revisionFromStat(value)
}

function revisionFromStat(
  value: Awaited<ReturnType<typeof stat>> | Awaited<ReturnType<typeof lstat>>
): FileRevision {
  const record = value as unknown as {
    dev: bigint
    ino: bigint
    size: bigint
    mtimeNs: bigint
    ctimeNs: bigint
  }
  return {
    dev: record.dev,
    ino: record.ino,
    size: record.size,
    mtimeNs: record.mtimeNs,
    ctimeNs: record.ctimeNs
  }
}

function sameRevision(left: FileRevision, right: FileRevision): boolean {
  return (
    left.dev === right.dev &&
    left.ino === right.ino &&
    left.size === right.size &&
    left.mtimeNs === right.mtimeNs &&
    left.ctimeNs === right.ctimeNs
  )
}

function digestJson(value: unknown): string {
  return `sha256:${createHash('sha256').update(JSON.stringify(value)).digest('hex')}`
}

async function assertPrivateCopy(path: string, expectedSize: number): Promise<void> {
  const value = await lstat(path, { bigint: true }).catch(() => null)
  if (
    !value ||
    !value.isFile() ||
    value.isSymbolicLink() ||
    value.nlink !== 1n ||
    value.size !== BigInt(expectedSize)
  ) {
    throw new BrowserFileBrokerError('browser.file.changed')
  }
}

function platformNoFollowFlag(): number {
  return typeof constants.O_NOFOLLOW === 'number' ? constants.O_NOFOLLOW : 0
}

function mapOpenError(error: unknown): BrowserFileBrokerError {
  const code =
    typeof error === 'object' && error !== null && 'code' in error
      ? String((error as { code?: unknown }).code)
      : ''
  return new BrowserFileBrokerError(
    code === 'ELOOP' ? 'browser.file.symlink_forbidden' : 'browser.file.invalid_file'
  )
}

function boundedInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new BrowserFileBrokerError('browser.file.invalid_file')
  }
  return value
}
