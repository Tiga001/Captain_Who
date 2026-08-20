import { createHmac, randomBytes, randomUUID } from 'node:crypto'

import type {
  ManagedPlaywrightAuthorizationContext,
  ManagedPlaywrightPrepareSensitiveToolInput,
  ManagedPlaywrightSensitiveBindingScope
} from '@mycopilot/protocol'

const DEFAULT_MAX_BINDINGS = 128
const MAX_BINDING_TTL_MS = 15 * 60 * 1_000
const MAX_CLOCK_SKEW_MS = 5_000
const DEFAULT_SWEEP_INTERVAL_MS = 30_000

export interface ManagedPlaywrightSensitiveTargetIdentity {
  readonly surfaceId: string
  readonly generation: number
  readonly navigationEpoch: number
  readonly origin: string
}

export interface ManagedPlaywrightSensitiveDispatchFence {
  finish(): void
}

export interface ManagedPlaywrightPreparedSensitiveTargetBinding {
  readonly bindingId: string
  readonly targetBindingDigest: string
  readonly origin: string | null
  readonly createdAtMs: number
  readonly expiresAtMs: number
  readonly fileBasenames: readonly string[]
  readonly fileRevisionDigest: string | null
}

export interface ManagedPlaywrightSensitiveTargetBindingLease {
  readonly target?: ManagedPlaywrightSensitiveTargetIdentity
  readonly preparedFileHandles?: readonly string[]
  markDispatched(): void
  finish(): void
}

export interface ManagedPlaywrightPreparedFileAuthority {
  readonly handles: readonly string[]
  readonly basenames: readonly string[]
  readonly fileRevisionDigest: string
}

export interface ManagedPlaywrightSensitiveTargetBindingStore {
  acquire(
    authorization: ManagedPlaywrightAuthorizationContext
  ): ManagedPlaywrightSensitiveTargetBindingLease
  releaseRun(runId: string): number
  releaseToolCall(input: { runId: string; callId: string }): number
}

export type ManagedPlaywrightSensitiveTargetBindingErrorCode =
  'missing' | 'drifted' | 'expired' | 'origin_drifted' | 'reused' | 'busy' | 'surface_unavailable'

export class ManagedPlaywrightSensitiveTargetBindingError extends Error {
  readonly name = 'ManagedPlaywrightSensitiveTargetBindingError'

  constructor(readonly code: ManagedPlaywrightSensitiveTargetBindingErrorCode) {
    super(`mcp.builtin_playwright.sensitive_target_binding_${code}`)
  }
}

export interface ManagedPlaywrightSensitiveTargetBindingBrokerOptions {
  beginDispatchFence: (
    target: ManagedPlaywrightSensitiveTargetIdentity
  ) => ManagedPlaywrightSensitiveDispatchFence
  getActiveTarget: () => ManagedPlaywrightSensitiveTargetIdentity | null
  maxBindings?: number
  now?: () => number
  releasePreparedFiles?: (owner: { runId: string; callId: string }) => void
  secret?: Uint8Array
  sweepIntervalMs?: number
}

interface BindingRecord extends ManagedPlaywrightPreparedSensitiveTargetBinding {
  readonly activationId: string
  readonly argumentsDigest: string
  readonly bindingRequestId: string
  readonly callId: string
  readonly capabilityId: 'browser_automation'
  readonly grantExpiresAtMs: number
  readonly manifestDigest: string
  readonly policyRevision: number
  readonly runId: string
  readonly bindingScope: ManagedPlaywrightSensitiveBindingScope
  readonly target?: ManagedPlaywrightSensitiveTargetIdentity
  readonly toolName: string
  readonly preparedFileHandles: readonly string[]
}

interface ActiveBindingLease {
  dispatched: boolean
  fence?: ManagedPlaywrightSensitiveDispatchFence
  finished: boolean
  readonly record: BindingRecord
}

/**
 * Process-owned proposal-time authority for one exact managed Browser Surface generation.
 *
 * The public projection contains only an opaque UUID, a keyed digest and a credential-free
 * origin. Surface IDs and generations never leave Main. Preparation does not reveal/create a
 * Surface, attach Playwright, connect MCP, or dispatch a browser command.
 */
export class ManagedPlaywrightSensitiveTargetBindingBroker implements ManagedPlaywrightSensitiveTargetBindingStore {
  private readonly activeLeases = new Set<ActiveBindingLease>()
  private readonly beginDispatchFence: ManagedPlaywrightSensitiveTargetBindingBrokerOptions['beginDispatchFence']
  private readonly byRequestId = new Map<string, string>()
  private readonly getActiveTarget: () => ManagedPlaywrightSensitiveTargetIdentity | null
  private readonly maxBindings: number
  private readonly now: () => number
  private readonly releasePreparedFiles: NonNullable<
    ManagedPlaywrightSensitiveTargetBindingBrokerOptions['releasePreparedFiles']
  >
  private readonly records = new Map<string, BindingRecord>()
  private readonly secret: Uint8Array
  private readonly sweepIntervalMs: number
  private sweepTimer?: ReturnType<typeof setTimeout>
  private closed = false

  constructor(options: ManagedPlaywrightSensitiveTargetBindingBrokerOptions) {
    this.beginDispatchFence = options.beginDispatchFence
    this.getActiveTarget = options.getActiveTarget
    this.maxBindings = normalizeCapacity(options.maxBindings)
    this.now = options.now ?? Date.now
    this.releasePreparedFiles = options.releasePreparedFiles ?? (() => undefined)
    this.sweepIntervalMs = normalizeSweepInterval(options.sweepIntervalMs)
    this.secret = options.secret ? Uint8Array.from(options.secret) : randomBytes(32)
    if (this.secret.byteLength < 32) {
      throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
    }
  }

  prepare(
    input: ManagedPlaywrightPrepareSensitiveToolInput,
    preparedFiles?: ManagedPlaywrightPreparedFileAuthority
  ): ManagedPlaywrightPreparedSensitiveTargetBinding {
    const bindingScope = input.bindingScope
    this.assertOpen()
    const now = this.now()
    this.pruneExpired(now)
    const duplicateId = this.byRequestId.get(input.bindingRequestId)
    if (duplicateId) {
      const duplicate = this.records.get(duplicateId)
      if (duplicate && this.matchesPrepareInput(duplicate, input, preparedFiles, bindingScope)) {
        return project(duplicate)
      }
      if (duplicate) this.deleteRecord(duplicate)
      throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
    }
    validatePreparedFiles(input, preparedFiles)
    if (this.records.size >= this.maxBindings) {
      throw new ManagedPlaywrightSensitiveTargetBindingError('busy')
    }
    if (
      input.createdAtMs > now + MAX_CLOCK_SKEW_MS ||
      input.expiresAtMs <= now ||
      input.expiresAtMs > input.grantExpiresAtMs ||
      input.expiresAtMs - input.createdAtMs > MAX_BINDING_TTL_MS
    ) {
      throw new ManagedPlaywrightSensitiveTargetBindingError('expired')
    }
    const target =
      bindingScope === 'managed_surface' ? normalizeTarget(this.getActiveTarget()) : undefined
    if (bindingScope === 'managed_surface' && !target) {
      throw new ManagedPlaywrightSensitiveTargetBindingError('surface_unavailable')
    }
    const bindingId = randomUUID()
    const material = {
      schemaVersion: 4,
      bindingScope,
      bindingId,
      bindingRequestId: input.bindingRequestId,
      runId: input.runId,
      capabilityId: input.capabilityId,
      activationId: input.activationId,
      manifestDigest: input.manifestDigest,
      policyRevision: input.policyRevision,
      callId: input.callId,
      toolName: input.toolName,
      argumentsDigest: input.argumentsDigest,
      grantExpiresAtMs: input.grantExpiresAtMs,
      surfaceId: target?.surfaceId ?? null,
      generation: target?.generation ?? null,
      navigationEpoch: target?.navigationEpoch ?? null,
      origin: target?.origin ?? null,
      createdAtMs: input.createdAtMs,
      expiresAtMs: input.expiresAtMs,
      fileRevisionDigest: preparedFiles?.fileRevisionDigest ?? null,
      fileCount: preparedFiles?.handles.length ?? 0
    }
    const targetBindingDigest = `sha256:${createHmac('sha256', this.secret)
      .update(JSON.stringify(material))
      .digest('hex')}`
    const record: BindingRecord = {
      ...material,
      ...(target ? { target: Object.freeze({ ...target }) } : {}),
      targetBindingDigest,
      fileBasenames: Object.freeze([...(preparedFiles?.basenames ?? [])]),
      preparedFileHandles: Object.freeze([...(preparedFiles?.handles ?? [])])
    }
    this.records.set(bindingId, record)
    this.byRequestId.set(input.bindingRequestId, bindingId)
    this.ensureSweepTimer()
    return project(record)
  }

  acquire(
    authorization: ManagedPlaywrightAuthorizationContext
  ): ManagedPlaywrightSensitiveTargetBindingLease {
    this.assertOpen()
    const grant = authorization.builtinToolGrant
    if (!grant) throw new ManagedPlaywrightSensitiveTargetBindingError('missing')
    const now = this.now()
    this.pruneExpired(now)
    const record = this.records.get(grant.targetBindingId)
    if (!record) throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
    if (record.expiresAtMs <= now || grant.expiresAtMs <= now) {
      this.deleteRecord(record)
      throw new ManagedPlaywrightSensitiveTargetBindingError('expired')
    }
    if (
      record.targetBindingDigest !== grant.targetBindingDigest ||
      record.runId !== authorization.runId ||
      record.capabilityId !== authorization.capabilityId ||
      record.activationId !== authorization.activationId ||
      record.manifestDigest !== authorization.manifestDigest ||
      record.policyRevision !== authorization.policyRevision ||
      record.callId !== authorization.callId ||
      record.toolName !== authorization.triggerToolName ||
      record.argumentsDigest !== grant.argumentsDigest ||
      record.origin !== grant.origin ||
      record.grantExpiresAtMs !== authorization.grantExpiresAtMs ||
      record.expiresAtMs !== grant.expiresAtMs
    ) {
      this.deleteRecord(record)
      throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
    }
    this.assertCurrentTarget(record)
    if ([...this.activeLeases].some((candidate) => candidate.record === record)) {
      throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
    }
    const lease: ActiveBindingLease = { dispatched: false, finished: false, record }
    this.activeLeases.add(lease)
    return {
      ...(record.target ? { target: record.target } : {}),
      ...(record.preparedFileHandles.length > 0
        ? { preparedFileHandles: record.preparedFileHandles }
        : {}),
      markDispatched: () => {
        if (lease.finished || lease.dispatched || this.records.get(record.bindingId) !== record) {
          throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
        }
        if (record.expiresAtMs <= this.now()) {
          this.deleteRecord(record)
          throw new ManagedPlaywrightSensitiveTargetBindingError('expired')
        }
        this.assertCurrentTarget(record)
        let fence: ManagedPlaywrightSensitiveDispatchFence | undefined
        try {
          if (record.target) fence = this.beginDispatchFence(record.target)
          // Fence installation and this second exact read are synchronous. A navigation that
          // started before the lock changes the epoch/in-progress state and fails closed here;
          // requests that begin afterwards are cancelled by the fence until Tool terminal.
          this.assertCurrentTarget(record)
        } catch (error) {
          try {
            fence?.finish()
          } catch {
            // The original pre-dispatch drift is authoritative.
          }
          throw error instanceof ManagedPlaywrightSensitiveTargetBindingError
            ? error
            : new ManagedPlaywrightSensitiveTargetBindingError('origin_drifted')
        }
        lease.fence = fence
        lease.dispatched = true
        this.deleteRecord(record, false)
      },
      finish: () => this.finishLease(lease, true)
    }
  }

  release(input: {
    bindingId: string
    runId: string
    activationId: string
    callId: string
  }): boolean {
    const active = [...this.activeLeases].find(
      (candidate) => candidate.record.bindingId === input.bindingId
    )
    const record = active?.record ?? this.records.get(input.bindingId)
    if (!record) return false
    if (
      record.runId !== input.runId ||
      record.activationId !== input.activationId ||
      record.callId !== input.callId
    ) {
      return false
    }
    if (active) this.finishLease(active, false)
    else this.deleteRecord(record)
    return true
  }

  releaseByRequestId(bindingRequestId: string): boolean {
    const bindingId = this.byRequestId.get(bindingRequestId)
    if (!bindingId) return false
    const record = this.records.get(bindingId)
    if (!record) {
      this.byRequestId.delete(bindingRequestId)
      return false
    }
    this.deleteRecord(record)
    return true
  }

  releaseRun(runId: string): number {
    return this.releaseMatching((record) => record.runId === runId)
  }

  releaseCapability(activationId: string): number {
    return this.releaseMatching((record) => record.activationId === activationId)
  }

  releaseToolCall(input: { runId: string; callId: string }): number {
    return this.releaseMatching(
      (record) => record.runId === input.runId && record.callId === input.callId
    )
  }

  shutdown(): void {
    this.closed = true
    if (this.sweepTimer) clearTimeout(this.sweepTimer)
    this.sweepTimer = undefined
    for (const lease of [...this.activeLeases]) this.finishLease(lease, false)
    for (const record of [...this.records.values()]) this.deleteRecord(record)
    this.byRequestId.clear()
    this.secret.fill(0)
  }

  snapshot(): { bindings: number; requests: number } {
    this.pruneExpired(this.now())
    return { bindings: this.records.size, requests: this.byRequestId.size }
  }

  private assertCurrentTarget(record: BindingRecord): void {
    if (record.bindingScope === 'managed_browser_profile') {
      if (record.target || record.origin !== null) {
        this.deleteRecord(record)
        throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
      }
      return
    }
    if (!record.target) {
      this.deleteRecord(record)
      throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
    }
    const current = normalizeTarget(this.getActiveTarget())
    if (
      !current ||
      current.surfaceId !== record.target.surfaceId ||
      current.generation !== record.target.generation ||
      current.navigationEpoch !== record.target.navigationEpoch ||
      current.origin !== record.target.origin
    ) {
      this.deleteRecord(record)
      throw new ManagedPlaywrightSensitiveTargetBindingError('origin_drifted')
    }
  }

  private matchesPrepareInput(
    record: BindingRecord,
    input: ManagedPlaywrightPrepareSensitiveToolInput,
    preparedFiles: ManagedPlaywrightPreparedFileAuthority | undefined,
    bindingScope: ManagedPlaywrightSensitiveBindingScope
  ): boolean {
    return (
      record.bindingScope === bindingScope &&
      record.runId === input.runId &&
      record.capabilityId === input.capabilityId &&
      record.activationId === input.activationId &&
      record.manifestDigest === input.manifestDigest &&
      record.policyRevision === input.policyRevision &&
      record.grantExpiresAtMs === input.grantExpiresAtMs &&
      record.callId === input.callId &&
      record.toolName === input.toolName &&
      record.argumentsDigest === input.argumentsDigest &&
      record.createdAtMs === input.createdAtMs &&
      record.expiresAtMs === input.expiresAtMs &&
      record.fileRevisionDigest === (preparedFiles?.fileRevisionDigest ?? null) &&
      sameStrings(record.fileBasenames, preparedFiles?.basenames ?? []) &&
      sameStrings(record.preparedFileHandles, preparedFiles?.handles ?? [])
    )
  }

  private pruneExpired(now: number): void {
    for (const record of this.records.values()) {
      if (record.expiresAtMs <= now) this.deleteRecord(record)
    }
  }

  private releaseMatching(predicate: (record: BindingRecord) => boolean): number {
    let released = 0
    for (const lease of [...this.activeLeases]) {
      if (!predicate(lease.record)) continue
      this.finishLease(lease, false)
      released += 1
    }
    for (const record of this.records.values()) {
      if (!predicate(record)) continue
      this.deleteRecord(record)
      released += 1
    }
    return released
  }

  private finishLease(lease: ActiveBindingLease, validateFence: boolean): void {
    if (lease.finished) return
    lease.finished = true
    this.activeLeases.delete(lease)
    if (!lease.dispatched) this.deleteRecord(lease.record)
    try {
      lease.fence?.finish()
    } catch {
      if (validateFence) {
        throw new ManagedPlaywrightSensitiveTargetBindingError('origin_drifted')
      }
    }
  }

  private deleteRecord(record: BindingRecord, releasePreparedFiles = true): void {
    if (this.records.get(record.bindingId) !== record) return
    this.records.delete(record.bindingId)
    if (this.byRequestId.get(record.bindingRequestId) === record.bindingId) {
      this.byRequestId.delete(record.bindingRequestId)
    }
    if (releasePreparedFiles && record.preparedFileHandles.length > 0) {
      this.releasePreparedFiles({ runId: record.runId, callId: record.callId })
    }
    if (this.records.size === 0 && this.sweepTimer) {
      clearTimeout(this.sweepTimer)
      this.sweepTimer = undefined
    }
  }

  private ensureSweepTimer(): void {
    if (this.closed || this.records.size === 0 || this.sweepTimer) return
    this.sweepTimer = setTimeout(() => {
      this.sweepTimer = undefined
      if (this.closed) return
      this.pruneExpired(this.now())
      this.ensureSweepTimer()
    }, this.sweepIntervalMs)
    this.sweepTimer.unref?.()
  }

  private assertOpen(): void {
    if (this.closed) throw new ManagedPlaywrightSensitiveTargetBindingError('surface_unavailable')
  }
}

function normalizeTarget(
  target: ManagedPlaywrightSensitiveTargetIdentity | null
): ManagedPlaywrightSensitiveTargetIdentity | null {
  if (
    !target ||
    !target.surfaceId ||
    !Number.isSafeInteger(target.generation) ||
    target.generation < 1
  ) {
    return null
  }
  if (!Number.isSafeInteger(target.navigationEpoch) || target.navigationEpoch < 1) return null
  try {
    const parsed = new URL(target.origin)
    if (
      !['http:', 'https:'].includes(parsed.protocol) ||
      parsed.origin !== target.origin ||
      parsed.username !== '' ||
      parsed.password !== '' ||
      parsed.pathname !== '/' ||
      parsed.search !== '' ||
      parsed.hash !== ''
    ) {
      return null
    }
  } catch {
    return null
  }
  return target
}

function project(record: BindingRecord): ManagedPlaywrightPreparedSensitiveTargetBinding {
  return {
    bindingId: record.bindingId,
    targetBindingDigest: record.targetBindingDigest,
    origin: record.origin,
    createdAtMs: record.createdAtMs,
    expiresAtMs: record.expiresAtMs,
    fileBasenames: [...record.fileBasenames],
    fileRevisionDigest: record.fileRevisionDigest
  }
}

function validatePreparedFiles(
  input: ManagedPlaywrightPrepareSensitiveToolInput,
  preparedFiles: ManagedPlaywrightPreparedFileAuthority | undefined
): void {
  if (input.filePreparation === null) {
    if (preparedFiles) throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
    return
  }
  if (
    !preparedFiles ||
    preparedFiles.handles.length < 1 ||
    preparedFiles.handles.length > 16 ||
    preparedFiles.handles.length !== preparedFiles.basenames.length ||
    !/^sha256:[0-9a-f]{64}$/u.test(preparedFiles.fileRevisionDigest) ||
    preparedFiles.handles.some(
      (handle) =>
        typeof handle !== 'string' ||
        !/^browser-file:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
          handle
        )
    ) ||
    preparedFiles.basenames.some(
      (name) =>
        typeof name !== 'string' ||
        name.length < 1 ||
        name.length > 255 ||
        name.includes('/') ||
        name.includes('\\') ||
        /[\u0000-\u001f\u007f]/u.test(name)
    )
  ) {
    throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
  }
}

function sameStrings(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index])
}

function normalizeCapacity(value: number | undefined): number {
  if (value === undefined) return DEFAULT_MAX_BINDINGS
  if (!Number.isSafeInteger(value) || value < 1 || value > DEFAULT_MAX_BINDINGS) {
    throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
  }
  return value
}

function normalizeSweepInterval(value: number | undefined): number {
  if (value === undefined) return DEFAULT_SWEEP_INTERVAL_MS
  if (!Number.isSafeInteger(value) || value < 10 || value > 60_000) {
    throw new ManagedPlaywrightSensitiveTargetBindingError('drifted')
  }
  return value
}
