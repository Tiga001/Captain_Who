export const BROWSER_WEBVIEW_PARTITION = 'persist:mycopilot-browser'

export const BROWSER_SURFACE_SCHEMA_VERSION = 1 as const

const BROWSER_SURFACE_BOOTSTRAP_PREFIX = 'about:blank#mycopilot-browser-surface='
const SURFACE_ID_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,255}$/
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
  viewport?: { height: number; width: number }
}

export interface BrowserSurfaceReadyOutput {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  accepted: true
  surfaceId: string
}

export interface BrowserSurfaceSelectedInput {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  surfaceId: string
}

export interface BrowserSurfaceSelectedOutput {
  schemaVersion: typeof BROWSER_SURFACE_SCHEMA_VERSION
  accepted: true
  surfaceId: string
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

  expectOnlyKeys(record, ['schemaVersion', 'kind', 'requestId', 'surfaceId'])
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    kind,
    requestId: expectRequestId(record.requestId),
    surfaceId: parseBrowserSurfaceId(record.surfaceId)
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
  expectOnlyKeys(record, ['schemaVersion', 'requestId', 'surfaceId', 'viewport'])
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
  expectOnlyKeys(record, ['schemaVersion', 'accepted', 'surfaceId'])
  if (record.accepted !== true) throw new Error('Invalid browser surface ready output')
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    accepted: true,
    surfaceId: parseBrowserSurfaceId(record.surfaceId)
  }
}

export function parseBrowserSurfaceSelectedInput(value: unknown): BrowserSurfaceSelectedInput {
  const record = expectRecord(value, 'browser surface selected input')
  expectOnlyKeys(record, ['schemaVersion', 'surfaceId'])
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    surfaceId: parseBrowserSurfaceId(record.surfaceId)
  }
}

export function parseBrowserSurfaceSelectedOutput(value: unknown): BrowserSurfaceSelectedOutput {
  const record = expectRecord(value, 'browser surface selected output')
  expectOnlyKeys(record, ['schemaVersion', 'accepted', 'surfaceId'])
  if (record.accepted !== true) throw new Error('Invalid browser surface selected output')
  return {
    schemaVersion: expectSchemaVersion(record.schemaVersion),
    accepted: true,
    surfaceId: parseBrowserSurfaceId(record.surfaceId)
  }
}

export function parseBrowserSurfaceId(value: unknown): string {
  if (typeof value !== 'string' || !SURFACE_ID_PATTERN.test(value)) {
    throw new Error('Invalid browser surface identity')
  }
  return value
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

function expectEnum<T extends string>(value: unknown, allowed: readonly T[]): T {
  if (typeof value !== 'string' || !allowed.includes(value as T)) {
    throw new Error('Invalid browser surface command kind')
  }
  return value as T
}
