import { createHash } from 'node:crypto'

import rawManifest from '../../../crates/core-server/resources/playwright-browser-manifest-v1.json'

export const MANAGED_PLAYWRIGHT_PACKAGE_NAME = '@playwright/mcp'
export const MANAGED_PLAYWRIGHT_PACKAGE_VERSION = '0.0.79'
export const MANAGED_PLAYWRIGHT_MANIFEST_SCHEMA_VERSION = 1
// Stable Host-owned routing identity. This is deliberately unrelated to the user-visible name and
// cannot be supplied by Renderer or by the managed Server.
export const MANAGED_PLAYWRIGHT_SERVER_ID = 'b77b3d54-b7c6-4ead-9cbd-3b9fe50d5311'

export type ManagedPlaywrightToolSafety = 'read_only' | 'destructive'

export interface ManagedPlaywrightToolManifestEntry {
  readonly rawName: string
  readonly modelName: string
  readonly description: string
  readonly safety: ManagedPlaywrightToolSafety
  readonly inputSchema: Readonly<Record<string, unknown>>
  readonly schemaDigest: string
}

export interface ManagedPlaywrightManifest {
  readonly schemaVersion: 1
  readonly packageName: typeof MANAGED_PLAYWRIGHT_PACKAGE_NAME
  readonly packageVersion: typeof MANAGED_PLAYWRIGHT_PACKAGE_VERSION
  readonly managedMcpId: 'builtin.browser_automation.mcp'
  readonly capabilityId: 'browser_automation'
  readonly manifestVersion: string
  readonly tools: readonly ManagedPlaywrightToolManifestEntry[]
}

const MAX_MANIFEST_TOOLS = 32
const MAX_SCHEMA_BYTES = 256 * 1024
const SHA256_PATTERN = /^sha256:[a-f0-9]{64}$/
const TOOL_NAME_PATTERN = /^[A-Za-z0-9_-]{1,64}$/

export const MANAGED_PLAYWRIGHT_MANIFEST: ManagedPlaywrightManifest =
  parseManagedPlaywrightManifest(rawManifest)

const toolsByRawName = new Map(
  MANAGED_PLAYWRIGHT_MANIFEST.tools.map((tool) => [tool.rawName, tool] as const)
)

export function managedPlaywrightTool(
  rawName: string
): ManagedPlaywrightToolManifestEntry | undefined {
  return toolsByRawName.get(rawName)
}

export function parseManagedPlaywrightManifest(value: unknown): ManagedPlaywrightManifest {
  const record = expectRecord(value, 'managed Playwright manifest')
  expectExactKeys(record, [
    'schemaVersion',
    'packageName',
    'packageVersion',
    'managedMcpId',
    'capabilityId',
    'manifestVersion',
    'tools'
  ])
  if (
    record.schemaVersion !== MANAGED_PLAYWRIGHT_MANIFEST_SCHEMA_VERSION ||
    record.packageName !== MANAGED_PLAYWRIGHT_PACKAGE_NAME ||
    record.packageVersion !== MANAGED_PLAYWRIGHT_PACKAGE_VERSION ||
    record.managedMcpId !== 'builtin.browser_automation.mcp' ||
    record.capabilityId !== 'browser_automation' ||
    typeof record.manifestVersion !== 'string' ||
    record.manifestVersion.length === 0 ||
    record.manifestVersion.length > 128 ||
    !Array.isArray(record.tools) ||
    record.tools.length === 0 ||
    record.tools.length > MAX_MANIFEST_TOOLS
  ) {
    throw new Error('Managed Playwright manifest identity is invalid')
  }

  const rawNames = new Set<string>()
  const modelNames = new Set<string>()
  const tools = record.tools.map((tool, index) => {
    const parsed = parseTool(tool, index)
    if (!insertUnique(rawNames, parsed.rawName) || !insertUnique(modelNames, parsed.modelName)) {
      throw new Error('Managed Playwright manifest contains duplicate tools')
    }
    return parsed
  })
  return Object.freeze({
    schemaVersion: 1,
    packageName: MANAGED_PLAYWRIGHT_PACKAGE_NAME,
    packageVersion: MANAGED_PLAYWRIGHT_PACKAGE_VERSION,
    managedMcpId: 'builtin.browser_automation.mcp',
    capabilityId: 'browser_automation',
    manifestVersion: record.manifestVersion,
    tools: Object.freeze(tools)
  })
}

function parseTool(value: unknown, index: number): ManagedPlaywrightToolManifestEntry {
  const context = `managed Playwright manifest.tools[${index}]`
  const record = expectRecord(value, context)
  expectExactKeys(record, [
    'rawName',
    'modelName',
    'description',
    'safety',
    'inputSchema',
    'schemaDigest'
  ])
  if (
    typeof record.rawName !== 'string' ||
    !TOOL_NAME_PATTERN.test(record.rawName) ||
    typeof record.modelName !== 'string' ||
    !TOOL_NAME_PATTERN.test(record.modelName) ||
    record.rawName !== record.modelName ||
    typeof record.description !== 'string' ||
    record.description.length === 0 ||
    record.description.length > 1024 ||
    (record.safety !== 'read_only' && record.safety !== 'destructive') ||
    typeof record.schemaDigest !== 'string' ||
    !SHA256_PATTERN.test(record.schemaDigest)
  ) {
    throw new Error(`${context} identity is invalid`)
  }
  const schema = expectRecord(record.inputSchema, `${context}.inputSchema`)
  if (
    schema.type !== 'object' ||
    schema.additionalProperties !== false ||
    Buffer.byteLength(JSON.stringify(schema), 'utf8') > MAX_SCHEMA_BYTES
  ) {
    throw new Error(`${context}.inputSchema is not a bounded closed object schema`)
  }
  const digest = digestJson(schema)
  if (record.schemaDigest !== digest) {
    throw new Error(`${context}.schemaDigest does not match the reviewed schema`)
  }
  return Object.freeze({
    rawName: record.rawName,
    modelName: record.modelName,
    description: record.description,
    safety: record.safety,
    inputSchema: deepFreeze(schema),
    schemaDigest: record.schemaDigest
  })
}

function digestJson(value: unknown): string {
  return `sha256:${createHash('sha256')
    .update(JSON.stringify(canonicalJson(value)))
    .digest('hex')}`
}

function canonicalJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalJson)
  if (value !== null && typeof value === 'object') {
    const record = value as Record<string, unknown>
    return Object.fromEntries(
      Object.keys(record)
        .sort()
        .map((key) => [key, canonicalJson(record[key])])
    )
  }
  return value
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === 'object') {
    Object.freeze(value)
    for (const child of Object.values(value as Record<string, unknown>)) deepFreeze(child)
  }
  return value
}

function expectRecord(value: unknown, context: string): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${context} must be an object`)
  }
  return value as Record<string, unknown>
}

function expectExactKeys(record: Record<string, unknown>, expected: readonly string[]): void {
  const actual = Object.keys(record).sort()
  const wanted = [...expected].sort()
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new Error('Managed Playwright manifest contains unknown or missing fields')
  }
}

function insertUnique<T>(set: Set<T>, value: T): boolean {
  if (set.has(value)) return false
  set.add(value)
  return true
}
