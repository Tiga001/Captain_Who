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
  /** Main-generated exact retired incarnation. */
  surfaceInstanceId: string
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
  /** Main-generated binding for the exact Renderer webview. */
  surfaceInstanceId: string
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

export const BROWSER_SURFACE_LOAD_ERROR_KINDS = [
  'offline',
  'dns',
  'connection_refused',
  'timeout',
  'certificate',
  'generic'
] as const

export type BrowserSurfaceLoadErrorKind = (typeof BROWSER_SURFACE_LOAD_ERROR_KINDS)[number]

export interface BrowserSurfacePublicLoadError {
  kind: BrowserSurfaceLoadErrorKind
  errorCode: number
  errorDescription: string
  failedUrl: string
  title: string
  heading: string
  summary: string
  suggestions: readonly string[]
}

export const BROWSER_SURFACE_CRASH_ERROR_KINDS = [
  'renderer_crashed',
  'renderer_unresponsive'
] as const

export type BrowserSurfaceCrashErrorKind = (typeof BROWSER_SURFACE_CRASH_ERROR_KINDS)[number]

export interface BrowserSurfacePublicCrashError {
  kind: BrowserSurfaceCrashErrorKind
  title: string
  heading: string
  summary: string
  actionLabel: string
}

export type BrowserSurfacePresentation = 'content' | 'error-page' | 'crash-page' | 'host-fallback'

/** Renderer-safe logical state. Internal implementation URLs never cross this boundary. */
export interface BrowserSurfaceState {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  surfaceId: string
  surfaceInstanceId: string
  stateRevision: number
  url: string | null
  title: string | null
  faviconUrl: string | null
  canGoBack: boolean
  canGoForward: boolean
  isLoading: boolean
  presentation: BrowserSurfacePresentation
  loadError: BrowserSurfacePublicLoadError | null
  crashError: BrowserSurfacePublicCrashError | null
}

export interface BrowserSurfaceStateInput {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  surfaceId: string
  surfaceInstanceId: string
}

export interface BrowserSurfaceActionInput extends BrowserSurfaceStateInput {
  action: 'navigate' | 'reload' | 'goBack' | 'goForward'
  /** Required only for `navigate`; forbidden for all other actions. */
  url?: string
}

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
    surfaceInstanceId: parseBrowserSurfaceInstanceId(record.surfaceInstanceId)
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
    surfaceInstanceId: parseBrowserSurfaceInstanceId(record.surfaceInstanceId),
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

export function parseBrowserSurfaceStateInput(value: unknown): BrowserSurfaceStateInput {
  const record = expectRecord(value, 'browser surface state input')
  expectOnlyKeys(record, ['schemaVersion', 'surfaceId', 'surfaceInstanceId'])
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    surfaceId: parseBrowserSurfaceId(record.surfaceId),
    surfaceInstanceId: parseBrowserSurfaceInstanceId(record.surfaceInstanceId)
  }
}

export function parseBrowserSurfaceActionInput(value: unknown): BrowserSurfaceActionInput {
  const record = expectRecord(value, 'browser surface action input')
  expectOnlyKeys(record, ['schemaVersion', 'surfaceId', 'surfaceInstanceId', 'action', 'url'])
  const action = expectEnum(
    record.action,
    ['navigate', 'reload', 'goBack', 'goForward'] as const,
    'browser surface action'
  )
  const base = parseBrowserSurfaceStateInput({
    schemaVersion: record.schemaVersion,
    surfaceId: record.surfaceId,
    surfaceInstanceId: record.surfaceInstanceId
  })
  if (action === 'navigate') {
    return { ...base, action, url: parseBrowserNavigationUrl(record.url) }
  }
  if (record.url !== undefined) {
    throw new Error('Only browser navigation actions may include a URL')
  }
  return { ...base, action }
}

export function parseBrowserSurfaceState(value: unknown): BrowserSurfaceState {
  const record = expectRecord(value, 'browser surface state')
  expectOnlyKeys(record, [
    'schemaVersion',
    'surfaceId',
    'surfaceInstanceId',
    'stateRevision',
    'url',
    'title',
    'faviconUrl',
    'canGoBack',
    'canGoForward',
    'isLoading',
    'presentation',
    'loadError',
    'crashError'
  ])
  const state: BrowserSurfaceState = {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    surfaceId: parseBrowserSurfaceId(record.surfaceId),
    surfaceInstanceId: parseBrowserSurfaceInstanceId(record.surfaceInstanceId),
    stateRevision: expectNonNegativeSelectionRevision(record.stateRevision),
    url: record.url === null ? null : parseBrowserNavigationUrl(record.url),
    title: parseNullableBoundedText(record.title, 'browser surface title', 256),
    faviconUrl: parseNullableHttpUrl(record.faviconUrl, 'browser surface favicon URL', 4_096),
    canGoBack: expectBoolean(record.canGoBack, 'browser surface back state'),
    canGoForward: expectBoolean(record.canGoForward, 'browser surface forward state'),
    isLoading: expectBoolean(record.isLoading, 'browser surface loading state'),
    presentation: expectEnum(
      record.presentation,
      ['content', 'error-page', 'crash-page', 'host-fallback'] as const,
      'browser surface presentation'
    ),
    loadError:
      record.loadError === null ? null : parseBrowserSurfacePublicLoadError(record.loadError),
    crashError:
      record.crashError === null ? null : parseBrowserSurfacePublicCrashError(record.crashError)
  }
  if (state.loadError && state.url !== state.loadError.failedUrl) {
    throw new Error('Browser surface failure URL does not match its logical URL')
  }
  if (state.loadError && state.crashError) {
    throw new Error('Browser surface cannot expose multiple failures')
  }
  if (state.presentation === 'content' && (state.loadError || state.crashError)) {
    throw new Error('Browser surface content presentation cannot expose a failure')
  }
  if (state.presentation === 'error-page' && !state.loadError) {
    throw new Error('Browser surface error page requires a load error')
  }
  if (state.presentation === 'crash-page' && !state.crashError) {
    throw new Error('Browser surface crash page requires a renderer failure')
  }
  if (state.presentation === 'host-fallback' && !state.loadError && !state.crashError) {
    throw new Error('Browser surface host fallback requires a failure')
  }
  return state
}

function parseBrowserSurfacePublicLoadError(value: unknown): BrowserSurfacePublicLoadError {
  const record = expectRecord(value, 'browser surface load error')
  expectOnlyKeys(record, [
    'kind',
    'errorCode',
    'errorDescription',
    'failedUrl',
    'title',
    'heading',
    'summary',
    'suggestions'
  ])
  if (!Number.isSafeInteger(record.errorCode)) {
    throw new Error('Invalid browser surface error code')
  }
  if (!Array.isArray(record.suggestions) || record.suggestions.length > 6) {
    throw new Error('Invalid browser surface error suggestions')
  }
  return {
    kind: expectEnum(
      record.kind,
      BROWSER_SURFACE_LOAD_ERROR_KINDS,
      'browser surface load error kind'
    ),
    errorCode: record.errorCode as number,
    errorDescription: parseBoundedText(
      record.errorDescription,
      'browser surface error description',
      128
    ),
    failedUrl: parseBrowserNavigationUrl(record.failedUrl),
    title: parseBoundedText(record.title, 'browser surface error title', 256),
    heading: parseBoundedText(record.heading, 'browser surface error heading', 256),
    summary: parseBoundedText(record.summary, 'browser surface error summary', 512),
    suggestions: record.suggestions.map((suggestion) =>
      parseBoundedText(suggestion, 'browser surface error suggestion', 256)
    )
  }
}

function parseBrowserSurfacePublicCrashError(value: unknown): BrowserSurfacePublicCrashError {
  const record = expectRecord(value, 'browser surface crash error')
  expectOnlyKeys(record, ['kind', 'title', 'heading', 'summary', 'actionLabel'])
  return {
    kind: expectEnum(
      record.kind,
      BROWSER_SURFACE_CRASH_ERROR_KINDS,
      'browser surface crash error kind'
    ),
    title: parseBoundedText(record.title, 'browser surface crash title', 256),
    heading: parseBoundedText(record.heading, 'browser surface crash heading', 256),
    summary: parseBoundedText(record.summary, 'browser surface crash summary', 512),
    actionLabel: parseBoundedText(record.actionLabel, 'browser surface crash action', 128)
  }
}

function parseBrowserNavigationUrl(value: unknown): string {
  return parseHttpUrl(value, 'browser navigation URL', 16_384)
}

function parseNullableHttpUrl(value: unknown, context: string, maxLength: number): string | null {
  return value === null ? null : parseHttpUrl(value, context, maxLength)
}

function parseHttpUrl(value: unknown, context: string, maxLength: number): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > maxLength) {
    throw new Error(`Invalid ${context}`)
  }
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new Error(`Invalid ${context}`)
  }
  if (
    !['http:', 'https:'].includes(parsed.protocol) ||
    parsed.username !== '' ||
    parsed.password !== ''
  ) {
    throw new Error(`Invalid ${context}`)
  }
  return parsed.toString()
}

function parseNullableBoundedText(
  value: unknown,
  context: string,
  maxLength: number
): string | null {
  return value === null ? null : parseBoundedText(value, context, maxLength)
}

function parseBoundedText(value: unknown, context: string, maxLength: number): string {
  if (
    typeof value !== 'string' ||
    value.length === 0 ||
    value.length > maxLength ||
    hasForbiddenControlCharacter(value)
  ) {
    throw new Error(`Invalid ${context}`)
  }
  return value
}

function hasForbiddenControlCharacter(value: string): boolean {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0) ?? 0
    return (codePoint < 32 && ![9, 10, 13].includes(codePoint)) || codePoint === 127
  })
}

function expectBoolean(value: unknown, context: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`Invalid ${context}`)
  return value
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
