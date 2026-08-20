export const BROWSER_WEBVIEW_PARTITION = 'persist:mycopilot-browser'

export const BROWSER_SURFACE_SCHEMA_VERSION = 1 as const

const BROWSER_SURFACE_BOOTSTRAP_PREFIX = 'about:blank#mycopilot-browser-surface='
const SURFACE_ID_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,255}$/
const SURFACE_INSTANCE_ID_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9_-]{15,127}$/
const REQUEST_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

export interface BrowserSurfaceEnsureAttachedCommand {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  kind: 'ensureAttached'
  requestId: string
  surfaceId: string
}

export interface BrowserSurfaceCreateCommand {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  kind: 'createSurface'
  requestId: string
  surfaceId: string
  activate: boolean
}

export interface BrowserSurfaceSelectCommand {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  kind: 'selectSurface'
  requestId: string
  surfaceId: string
}

export interface BrowserSurfaceResizeCommand {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  kind: 'resizeSurface'
  requestId: string
  surfaceId: string
  width: number
  height: number
}

export interface BrowserSurfaceCloseCommand {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  kind: 'closeSurface'
  requestId: string
  surfaceId: string
  /** Main-generated exact retired incarnation. Optional only for legacy payload parsing. */
  surfaceInstanceId?: string
}

export type BrowserSurfaceCommand =
  | BrowserSurfaceEnsureAttachedCommand
  | BrowserSurfaceCreateCommand
  | BrowserSurfaceSelectCommand
  | BrowserSurfaceResizeCommand
  | BrowserSurfaceCloseCommand

export interface BrowserSurfaceReadyInput {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  requestId: string
  surfaceId: string
  /**
   * Main-generated binding for the exact Renderer webview. Optional only for legacy payload
   * parsing; a live pending command is not accepted without the matching identity.
   */
  surfaceInstanceId?: string
  viewport?: { height: number; width: number }
}

interface BrowserSurfaceReadyOutputBase {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  requestId: string
  surfaceId: string
  /** Main-owned identity when the exact registered guest is known. */
  surfaceInstanceId?: string
  retryable: boolean
}

export interface BrowserSurfaceReadyAppliedOutput extends BrowserSurfaceReadyOutputBase {
  accepted: true
  status: 'applied'
  reason: 'surface_ready'
  retryable: false
}

export interface BrowserSurfaceReadyAlreadyReadyOutput extends BrowserSurfaceReadyOutputBase {
  accepted: false
  status: 'noop'
  reason: 'already_ready'
  retryable: false
}

export interface BrowserSurfaceReadyNotRegisteredOutput extends BrowserSurfaceReadyOutputBase {
  accepted: false
  status: 'noop'
  reason: 'not_registered'
  retryable: true
}

export type BrowserSurfaceReadyNoopOutput =
  BrowserSurfaceReadyAlreadyReadyOutput | BrowserSurfaceReadyNotRegisteredOutput

export interface BrowserSurfaceReadyLifecycleStaleOutput extends BrowserSurfaceReadyOutputBase {
  accepted: false
  status: 'stale'
  reason: 'request_expired' | 'request_superseded' | 'request_cancelled' | 'target_closed'
  retryable: false
}

export interface BrowserSurfaceReadyInstanceMismatchOutput extends BrowserSurfaceReadyOutputBase {
  accepted: false
  status: 'stale'
  reason: 'instance_mismatch'
  retryable: true
}

export type BrowserSurfaceReadyStaleOutput =
  BrowserSurfaceReadyLifecycleStaleOutput | BrowserSurfaceReadyInstanceMismatchOutput

export type BrowserSurfaceReadyOutput =
  BrowserSurfaceReadyAppliedOutput | BrowserSurfaceReadyNoopOutput | BrowserSurfaceReadyStaleOutput

export interface BrowserSurfaceSelectedInput {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  /** `null` clears the trusted UI selection. A non-null surface without an instance is a probe. */
  surfaceId: string | null
  /** Main-generated opaque binding for one exact registered guest incarnation. */
  surfaceInstanceId: string | null
  /** Renderer-owned, strictly increasing sequence. Main decides whether it is authoritative. */
  selectionRevision: number
}

interface BrowserSurfaceSelectedOutputBase {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  surfaceId: string | null
  surfaceInstanceId: string | null
  selectionRevision: number
  authoritativeRevision: number
  retryable: boolean
}

export interface BrowserSurfaceSelectionAppliedOutput extends BrowserSurfaceSelectedOutputBase {
  status: 'applied'
  reason: 'selection_applied'
  retryable: false
}

export interface BrowserSurfaceSelectionNoopOutput extends BrowserSurfaceSelectedOutputBase {
  status: 'noop'
  reason: 'selection_unchanged' | 'instance_required' | 'not_registered'
}

export interface BrowserSurfaceSelectionStaleOutput extends BrowserSurfaceSelectedOutputBase {
  status: 'stale'
  reason: 'stale_revision' | 'instance_mismatch' | 'surface_closing'
}

export type BrowserSurfaceSelectedOutput =
  | BrowserSurfaceSelectionAppliedOutput
  | BrowserSurfaceSelectionNoopOutput
  | BrowserSurfaceSelectionStaleOutput

/**
 * Builds the inert, renderer-visible bootstrap URL used to bind a webview DOM surface to its
 * Main-owned guest target. It contains no authority or secret and is replaced by the first real
 * navigation.
 */
export function createBrowserSurfaceBootstrapUrl(surfaceId: string): string {
  return `${BROWSER_SURFACE_BOOTSTRAP_PREFIX}${encodeURIComponent(parseBrowserSurfaceId(surfaceId))}`
}

/** Main-only registration helper. A malformed bootstrap URL never identifies a managed target. */
export function parseBrowserSurfaceBootstrapUrl(value: string): string | null {
  if (!value.startsWith(BROWSER_SURFACE_BOOTSTRAP_PREFIX)) return null
  try {
    return parseBrowserSurfaceId(
      decodeURIComponent(value.slice(BROWSER_SURFACE_BOOTSTRAP_PREFIX.length))
    )
  } catch {
    return null
  }
}

export function parseBrowserSurfaceCommand(value: unknown): BrowserSurfaceCommand {
  const record = expectRecord(value, 'browser surface command')
  const kind = expectEnum(record.kind, [
    'ensureAttached',
    'createSurface',
    'selectSurface',
    'resizeSurface',
    'closeSurface'
  ] as const)
  if (kind === 'resizeSurface') {
    expectOnlyKeys(record, ['schemaVersion', 'kind', 'requestId', 'surfaceId', 'width', 'height'])
    return {
      schemaVersion: expectSchemaVersion(record.schemaVersion),
      kind,
      requestId: expectRequestId(record.requestId),
      surfaceId: parseBrowserSurfaceId(record.surfaceId),
      width: expectViewportDimension(record.width, 'width'),
      height: expectViewportDimension(record.height, 'height')
    }
  }
  if (kind === 'createSurface') {
    expectOnlyKeys(record, ['schemaVersion', 'kind', 'requestId', 'surfaceId', 'activate'])
    if (typeof record.activate !== 'boolean') {
      throw new Error('Invalid browser surface activation mode')
    }
    return {
      schemaVersion: expectSchemaVersion(record.schemaVersion),
      kind,
      requestId: expectRequestId(record.requestId),
      surfaceId: parseBrowserSurfaceId(record.surfaceId),
      activate: record.activate
    }
  }
  if (kind !== 'closeSurface') {
    expectOnlyKeys(record, ['schemaVersion', 'kind', 'requestId', 'surfaceId'])
    return {
      schemaVersion: expectSchemaVersion(record.schemaVersion),
      kind,
      requestId: expectRequestId(record.requestId),
      surfaceId: parseBrowserSurfaceId(record.surfaceId)
    }
  }

  expectOnlyKeys(record, ['schemaVersion', 'kind', 'requestId', 'surfaceId', 'surfaceInstanceId'])
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    kind,
    requestId: expectRequestId(record.requestId),
    surfaceId: parseBrowserSurfaceId(record.surfaceId),
    ...(record.surfaceInstanceId === undefined
      ? {}
      : { surfaceInstanceId: parseBrowserSurfaceInstanceId(record.surfaceInstanceId) })
  }
}

function expectViewportDimension(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 240 || (value as number) > 4_096) {
    throw new Error(`Invalid browser surface ${name}`)
  }
  return value as number
}

export function parseBrowserSurfaceReadyInput(value: unknown): BrowserSurfaceReadyInput {
  const record = expectRecord(value, 'browser surface ready input')
  expectOnlyKeys(record, [
    'schemaVersion',
    'requestId',
    'surfaceId',
    'surfaceInstanceId',
    'viewport'
  ])
  let viewport: BrowserSurfaceReadyInput['viewport']
  if (record.viewport !== undefined) {
    const candidate = expectRecord(record.viewport, 'browser surface viewport')
    expectOnlyKeys(candidate, ['width', 'height'])
    viewport = {
      width: expectMeasuredViewportDimension(candidate.width, 'width'),
      height: expectMeasuredViewportDimension(candidate.height, 'height')
    }
  }
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    requestId: expectRequestId(record.requestId),
    surfaceId: parseBrowserSurfaceId(record.surfaceId),
    ...(record.surfaceInstanceId === undefined
      ? {}
      : { surfaceInstanceId: parseBrowserSurfaceInstanceId(record.surfaceInstanceId) }),
    ...(viewport ? { viewport } : {})
  }
}

function expectMeasuredViewportDimension(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0 || (value as number) > 4_096) {
    throw new Error(`Invalid measured browser surface ${name}`)
  }
  return value as number
}

export function parseBrowserSurfaceReadyOutput(value: unknown): BrowserSurfaceReadyOutput {
  const record = expectRecord(value, 'browser surface ready output')
  expectOnlyKeys(record, [
    'schemaVersion',
    'accepted',
    'status',
    'reason',
    'retryable',
    'requestId',
    'surfaceId',
    'surfaceInstanceId'
  ])
  const schemaVersion = expectSchemaVersion(record.schemaVersion)
  const requestId = expectRequestId(record.requestId)
  const surfaceId = parseBrowserSurfaceId(record.surfaceId)
  const surfaceInstanceId =
    record.surfaceInstanceId === undefined
      ? undefined
      : parseBrowserSurfaceInstanceId(record.surfaceInstanceId)
  if (typeof record.retryable !== 'boolean') {
    throw new Error('Invalid browser surface ready retryability')
  }

  if (record.status === 'applied') {
    if (
      record.accepted !== true ||
      record.reason !== 'surface_ready' ||
      record.retryable !== false
    ) {
      throw new Error('Invalid browser surface ready applied output')
    }
    return {
      schemaVersion,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId,
      surfaceId,
      ...(surfaceInstanceId ? { surfaceInstanceId } : {})
    }
  }
  if (record.status === 'noop') {
    if (record.accepted !== false) {
      throw new Error('Invalid browser surface ready noop output')
    }
    if (record.reason === 'already_ready' && record.retryable === false) {
      return {
        schemaVersion,
        accepted: false,
        status: 'noop',
        reason: 'already_ready',
        retryable: false,
        requestId,
        surfaceId,
        ...(surfaceInstanceId ? { surfaceInstanceId } : {})
      }
    }
    if (record.reason === 'not_registered' && record.retryable === true) {
      return {
        schemaVersion,
        accepted: false,
        status: 'noop',
        reason: 'not_registered',
        retryable: true,
        requestId,
        surfaceId,
        ...(surfaceInstanceId ? { surfaceInstanceId } : {})
      }
    }
    throw new Error('Invalid browser surface ready noop output')
  }
  if (record.status !== 'stale' || record.accepted !== false) {
    throw new Error('Invalid browser surface ready output')
  }
  if (record.reason === 'instance_mismatch') {
    if (record.retryable !== true) {
      throw new Error('Invalid browser surface ready stale output')
    }
    return {
      schemaVersion,
      accepted: false,
      status: 'stale',
      reason: 'instance_mismatch',
      retryable: true,
      requestId,
      surfaceId,
      ...(surfaceInstanceId ? { surfaceInstanceId } : {})
    }
  }
  if (record.retryable !== false) {
    throw new Error('Invalid browser surface ready stale output')
  }
  const reason = expectEnum(record.reason, [
    'request_expired',
    'request_superseded',
    'request_cancelled',
    'target_closed'
  ] as const)
  return {
    schemaVersion,
    accepted: false,
    status: 'stale',
    reason,
    retryable: false,
    requestId,
    surfaceId,
    ...(surfaceInstanceId ? { surfaceInstanceId } : {})
  }
}

export function parseBrowserSurfaceSelectedInput(value: unknown): BrowserSurfaceSelectedInput {
  const record = expectRecord(value, 'browser surface selected input')
  expectOnlyKeys(record, ['schemaVersion', 'surfaceId', 'surfaceInstanceId', 'selectionRevision'])
  const surfaceId = parseNullableBrowserSurfaceId(record.surfaceId)
  const surfaceInstanceId = parseNullableBrowserSurfaceInstanceId(record.surfaceInstanceId)
  if (surfaceId === null && surfaceInstanceId !== null) {
    throw new Error('A cleared browser surface cannot carry an instance identity')
  }
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    surfaceId,
    surfaceInstanceId,
    selectionRevision: expectPositiveSelectionRevision(record.selectionRevision)
  }
}

export function parseBrowserSurfaceSelectedOutput(value: unknown): BrowserSurfaceSelectedOutput {
  const record = expectRecord(value, 'browser surface selected output')
  expectOnlyKeys(record, [
    'schemaVersion',
    'status',
    'surfaceId',
    'surfaceInstanceId',
    'selectionRevision',
    'authoritativeRevision',
    'reason',
    'retryable'
  ])
  const status = expectEnum(
    record.status,
    ['applied', 'noop', 'stale'] as const,
    'browser surface selection status'
  )
  const surfaceId = parseNullableBrowserSurfaceId(record.surfaceId)
  const surfaceInstanceId = parseNullableBrowserSurfaceInstanceId(record.surfaceInstanceId)
  if (surfaceId === null && surfaceInstanceId !== null) {
    throw new Error('A cleared browser surface cannot carry an instance identity')
  }
  if (typeof record.retryable !== 'boolean') {
    throw new Error('Invalid browser surface selection retryability')
  }
  const base = {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    surfaceId,
    surfaceInstanceId,
    selectionRevision: expectPositiveSelectionRevision(record.selectionRevision),
    authoritativeRevision: expectNonNegativeSelectionRevision(record.authoritativeRevision),
    retryable: record.retryable
  }
  if (status === 'applied') {
    if (
      record.reason !== 'selection_applied' ||
      record.retryable !== false ||
      (surfaceId !== null && surfaceInstanceId === null)
    ) {
      throw new Error('Invalid applied browser surface selection output')
    }
    return { ...base, status, reason: 'selection_applied', retryable: false }
  }
  if (status === 'noop') {
    const reason = expectEnum(
      record.reason,
      ['selection_unchanged', 'instance_required', 'not_registered'] as const,
      'browser surface selection reason'
    )
    if (
      (reason === 'selection_unchanged' && record.retryable) ||
      (reason === 'instance_required' &&
        (!record.retryable || surfaceId === null || surfaceInstanceId === null)) ||
      (reason === 'not_registered' &&
        (!record.retryable || surfaceId === null || surfaceInstanceId !== null))
    ) {
      throw new Error('Invalid no-op browser surface selection output')
    }
    return { ...base, status, reason }
  }
  const reason = expectEnum(
    record.reason,
    ['stale_revision', 'instance_mismatch', 'surface_closing'] as const,
    'browser surface selection reason'
  )
  return { ...base, status, reason }
}

export function parseBrowserSurfaceId(value: unknown): string {
  if (typeof value !== 'string' || !SURFACE_ID_PATTERN.test(value)) {
    throw new Error('Invalid browser surface identity')
  }
  return value
}

function parseNullableBrowserSurfaceId(value: unknown): string | null {
  return value === null ? null : parseBrowserSurfaceId(value)
}

export function parseBrowserSurfaceInstanceId(value: unknown): string {
  if (typeof value !== 'string' || !SURFACE_INSTANCE_ID_PATTERN.test(value)) {
    throw new Error('Invalid browser surface instance identity')
  }
  return value
}

function parseNullableBrowserSurfaceInstanceId(value: unknown): string | null {
  if (value === null) return null
  return parseBrowserSurfaceInstanceId(value)
}

function expectPositiveSelectionRevision(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 1) {
    throw new Error('Invalid browser surface selection revision')
  }
  return value as number
}

function expectNonNegativeSelectionRevision(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new Error('Invalid authoritative browser surface selection revision')
  }
  return value as number
}

function expectRequestId(value: unknown): string {
  if (typeof value !== 'string' || !REQUEST_ID_PATTERN.test(value)) {
    throw new Error('Invalid browser surface request identity')
  }
  return value
}

function expectSchemaVersion(value: unknown): typeof BROWSER_SURFACE_SCHEMA_VERSION {
  if (value !== BROWSER_SURFACE_SCHEMA_VERSION) {
    throw new Error('Unsupported browser surface schema version')
  }
  return BROWSER_SURFACE_SCHEMA_VERSION
}

function expectRecord(value: unknown, context: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`Invalid ${context}`)
  }
  return value as Record<string, unknown>
}

function expectOnlyKeys(record: Record<string, unknown>, allowed: readonly string[]): void {
  const allowedKeys = new Set(allowed)
  if (Object.keys(record).some((key) => !allowedKeys.has(key))) {
    throw new Error('Browser surface value contains unknown fields')
  }
}

function expectEnum<T extends string>(
  value: unknown,
  allowed: readonly T[],
  context = 'browser surface command kind'
): T {
  if (typeof value !== 'string' || !allowed.includes(value as T)) {
    throw new Error(`Invalid ${context}`)
  }
  return value as T
}
