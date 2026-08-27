import { createHash } from 'node:crypto'

import rawPolicyManifest from '../../../crates/core-server/resources/playwright-browser-manifest-v1.json'
import rawUpstreamCatalog from '../../../crates/core-server/resources/playwright-upstream-catalog-0.0.79.json'

export const MANAGED_PLAYWRIGHT_CAPABILITIES = [
  'config',
  'core',
  'network',
  'pdf',
  'storage',
  'testing',
  'vision',
  'devtools'
] as const

export const MANAGED_PLAYWRIGHT_TOOL_CAPABILITIES = [
  'config',
  'core',
  'core-input',
  'core-navigation',
  'core-tabs',
  'devtools',
  'network',
  'pdf',
  'storage',
  'testing',
  'vision'
] as const

export type ManagedPlaywrightToolCapability = (typeof MANAGED_PLAYWRIGHT_TOOL_CAPABILITIES)[number]

export type ManagedPlaywrightHandlingMode =
  | 'pass_through'
  | 'host_adapted'
  | 'approval_required'
  | 'artifact_managed'
  | 'sandboxed'
  | 'unsupported'

export interface ManagedPlaywrightCatalogTool {
  name: string
  capability?: ManagedPlaywrightToolCapability
  description?: string
  inputSchema: Record<string, unknown>
  annotations?: Record<string, unknown>
}

export interface ManagedPlaywrightReviewedTool {
  readonly rawName: string
  readonly modelName: string
  readonly description: string
  readonly handlingMode: ManagedPlaywrightHandlingMode
  readonly exposed: boolean
  readonly safety: 'read_only' | 'destructive'
  readonly inputSchema: Readonly<Record<string, unknown>>
  readonly schemaDigest: string
  readonly upstreamSchemaDigest: string
  readonly reasonCode: string
  readonly constraints: readonly string[]
}

export interface ManagedPlaywrightCatalogLock {
  readonly packageName: '@playwright/mcp'
  readonly packageVersion: '0.0.79'
  readonly playwrightVersion: '1.63.0-alpha-2026-08-05'
  readonly capabilities: readonly string[]
  readonly catalogDigest: string
  readonly tools: readonly ManagedPlaywrightCatalogTool[]
}

export interface ManagedPlaywrightPolicyManifest {
  readonly schemaVersion: 2
  readonly packageName: '@playwright/mcp'
  readonly packageVersion: '0.0.79'
  readonly managedMcpId: 'builtin.browser_automation.mcp'
  readonly capabilityId: 'browser_automation'
  readonly manifestVersion: string
  readonly upstreamCatalogDigest: string
  readonly policyDigest: string
  readonly exposedToolCount: number
  readonly tools: readonly ManagedPlaywrightReviewedTool[]
}

export interface ManagedPlaywrightCatalogConformanceReport {
  readonly schemaVersion: 1
  readonly packageName: '@playwright/mcp'
  readonly packageVersion: '0.0.79'
  readonly status: 'exact'
  readonly upstreamToolCount: number
  readonly exposedToolCount: number
  readonly upstreamCatalogDigest: string
  readonly policyDigest: string
}

export interface ManagedPlaywrightHostOverlay {
  addCallReason: true
  removeProperties?: readonly string[]
  propertyOverrides?: Readonly<Record<string, Readonly<Record<string, unknown>>>>
}

interface ParsedPolicyTool {
  rawName: string
  modelName: string
  handlingMode: ManagedPlaywrightHandlingMode
  exposed: boolean
  upstreamSchemaDigest: string
  hostOverlay: ManagedPlaywrightHostOverlay
  hostOverlayDigest: string
  hostInputSchemaDigest: string
  reasonCode: string
  constraints: string[]
}

const HOST_CALL_REASON_SCHEMA = Object.freeze({
  type: 'string',
  description: 'Brief reason for using this browser tool.',
  minLength: 1,
  maxLength: 512
})

const HOST_TOOL_DESCRIPTIONS: Readonly<Record<string, string>> = Object.freeze({
  browser_click:
    'Perform a click on the current page. If the click starts a browser download, the result reports download_started with a stable download ID; use browser_wait_for with a short time to observe progress.',
  browser_get_config:
    "Get the managed browser's resolved Host configuration and the current task's path-free download progress.",
  browser_wait_for:
    'Wait for text to appear or disappear or for a specified time. The result also reports current-task browser download progress, so use a short time to poll an active download.'
})

const SHA256_PATTERN = /^sha256:[a-f0-9]{64}$/
const TOOL_NAME_PATTERN = /^[A-Za-z0-9_-]{1,64}$/
export const MANAGED_PLAYWRIGHT_CATALOG_LOCK = parseUpstreamCatalog(rawUpstreamCatalog)
export const MANAGED_PLAYWRIGHT_POLICY_MANIFEST = parsePolicyManifest(
  rawPolicyManifest,
  MANAGED_PLAYWRIGHT_CATALOG_LOCK
)
export const MANAGED_PLAYWRIGHT_REVIEWED_TOOLS = MANAGED_PLAYWRIGHT_POLICY_MANIFEST.tools
export const MANAGED_PLAYWRIGHT_EXPOSED_TOOLS = Object.freeze(
  MANAGED_PLAYWRIGHT_REVIEWED_TOOLS.filter((tool) => tool.exposed)
)

const reviewedByRawName = new Map(
  MANAGED_PLAYWRIGHT_REVIEWED_TOOLS.map((tool) => [tool.rawName, tool] as const)
)

export function managedPlaywrightReviewedTool(
  rawName: string
): ManagedPlaywrightReviewedTool | undefined {
  return reviewedByRawName.get(rawName)
}

/** Produces a bounded, machine-readable release report only after full live-Catalog validation. */
export function managedPlaywrightCatalogConformanceReport(
  tools: readonly ManagedPlaywrightCatalogTool[]
): ManagedPlaywrightCatalogConformanceReport {
  const indexed = validateAndIndexOfficialPlaywrightCatalog(tools)
  return Object.freeze({
    schemaVersion: 1,
    packageName: '@playwright/mcp',
    packageVersion: '0.0.79',
    status: 'exact',
    upstreamToolCount: indexed.size,
    exposedToolCount: MANAGED_PLAYWRIGHT_POLICY_MANIFEST.exposedToolCount,
    upstreamCatalogDigest: MANAGED_PLAYWRIGHT_CATALOG_LOCK.catalogDigest,
    policyDigest: MANAGED_PLAYWRIGHT_POLICY_MANIFEST.policyDigest
  })
}

/** Validates the live fixed package against the committed full 69-tool Catalog lock. */
export function validateAndIndexOfficialPlaywrightCatalog(
  tools: readonly ManagedPlaywrightCatalogTool[]
): ReadonlyMap<string, ManagedPlaywrightCatalogTool> {
  const locked = new Map(MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => [tool.name, tool]))
  if (tools.length !== locked.size) throw new Error('catalog_drift')
  const indexed = new Map<string, ManagedPlaywrightCatalogTool>()
  for (const tool of tools) {
    const expected = locked.get(tool.name)
    if (
      !expected ||
      !hasExactOwnKeys(tool, ['name', 'description', 'inputSchema', 'annotations']) ||
      indexed.has(tool.name) ||
      tool.description !== expected.description ||
      digestJson(tool.inputSchema) !== digestJson(expected.inputSchema) ||
      digestJson(tool.annotations ?? null) !== digestJson(expected.annotations ?? null)
    ) {
      throw new Error('catalog_drift')
    }
    indexed.set(tool.name, tool)
  }
  if ([...locked.keys()].some((name) => !indexed.has(name))) throw new Error('catalog_drift')
  return indexed
}

export function modelSchemaForOfficialPlaywrightTool(
  upstreamSchema: Readonly<Record<string, unknown>>,
  overlay: ManagedPlaywrightHostOverlay
): Record<string, unknown> {
  const schema = structuredClone(upstreamSchema) as Record<string, unknown>
  const properties = expectRecord(schema.properties, 'upstream tool properties')
  if (
    schema.type !== 'object' ||
    schema.additionalProperties !== false ||
    Object.prototype.hasOwnProperty.call(properties, 'call_reason')
  ) {
    throw new Error('catalog_drift')
  }

  const upstreamRequired = Array.isArray(schema.required)
    ? schema.required.filter((value): value is string => typeof value === 'string')
    : []
  const removed = new Set<string>()
  for (const name of overlay.removeProperties ?? []) {
    if (
      removed.has(name) ||
      upstreamRequired.includes(name) ||
      !Object.prototype.hasOwnProperty.call(properties, name)
    ) {
      throw new Error('catalog_drift')
    }
    removed.add(name)
    delete properties[name]
  }
  for (const [name, override] of Object.entries(overlay.propertyOverrides ?? {})) {
    const property = expectRecord(properties[name], `upstream tool property ${name}`)
    validateHostPropertyOverride(property, override)
    properties[name] = deepMergeRecord(property, override)
  }

  properties.call_reason = structuredClone(HOST_CALL_REASON_SCHEMA)
  schema.required = [...new Set([...upstreamRequired, 'call_reason'])]
  return schema
}

export function digestJson(value: unknown): string {
  return `sha256:${createHash('sha256')
    .update(JSON.stringify(canonicalJson(value)))
    .digest('hex')}`
}

function parseUpstreamCatalog(value: unknown): ManagedPlaywrightCatalogLock {
  const record = expectRecord(value, 'managed Playwright upstream Catalog')
  expectExactKeys(record, [
    'schemaVersion',
    'packageName',
    'packageVersion',
    'playwrightVersion',
    'capabilities',
    'tools',
    'catalogDigest'
  ])
  if (
    record.schemaVersion !== 1 ||
    record.packageName !== '@playwright/mcp' ||
    record.packageVersion !== '0.0.79' ||
    record.playwrightVersion !== '1.63.0-alpha-2026-08-05' ||
    !Array.isArray(record.capabilities) ||
    !sameStringSet(record.capabilities, MANAGED_PLAYWRIGHT_CAPABILITIES) ||
    !Array.isArray(record.tools) ||
    record.tools.length !== 69 ||
    typeof record.catalogDigest !== 'string' ||
    !SHA256_PATTERN.test(record.catalogDigest)
  ) {
    throw new Error('Managed Playwright upstream Catalog identity is invalid')
  }
  const catalogWithoutDigest = { ...record }
  delete catalogWithoutDigest.catalogDigest
  if (digestJson(catalogWithoutDigest) !== record.catalogDigest) {
    throw new Error('Managed Playwright upstream Catalog digest is invalid')
  }

  const names = new Set<string>()
  const tools = record.tools.map((value, index) => {
    const context = `managed Playwright upstream Catalog.tools[${index}]`
    const tool = expectRecord(value, context)
    expectExactKeys(tool, [
      'rawName',
      'capability',
      'description',
      'inputSchema',
      'annotations',
      'schemaDigest'
    ])
    if (
      typeof tool.rawName !== 'string' ||
      !TOOL_NAME_PATTERN.test(tool.rawName) ||
      names.has(tool.rawName) ||
      typeof tool.capability !== 'string' ||
      !(MANAGED_PLAYWRIGHT_TOOL_CAPABILITIES as readonly string[]).includes(tool.capability) ||
      typeof tool.description !== 'string' ||
      tool.description.length === 0 ||
      tool.description.length > 4_096 ||
      typeof tool.schemaDigest !== 'string' ||
      !SHA256_PATTERN.test(tool.schemaDigest)
    ) {
      throw new Error(`${context} identity is invalid`)
    }
    names.add(tool.rawName)
    const inputSchema = expectRecord(tool.inputSchema, `${context}.inputSchema`)
    const annotations = expectRecord(tool.annotations, `${context}.annotations`)
    if (digestJson(inputSchema) !== tool.schemaDigest) {
      throw new Error(`${context}.schemaDigest is invalid`)
    }
    return Object.freeze({
      name: tool.rawName,
      capability: tool.capability as ManagedPlaywrightToolCapability,
      description: tool.description,
      inputSchema: deepFreeze(structuredClone(inputSchema)),
      annotations: deepFreeze(structuredClone(annotations))
    })
  })
  return Object.freeze({
    packageName: '@playwright/mcp',
    packageVersion: '0.0.79',
    playwrightVersion: '1.63.0-alpha-2026-08-05',
    capabilities: Object.freeze([...record.capabilities] as string[]),
    catalogDigest: record.catalogDigest,
    tools: Object.freeze(tools)
  })
}

function parsePolicyManifest(
  value: unknown,
  catalog: ManagedPlaywrightCatalogLock
): ManagedPlaywrightPolicyManifest {
  const record = expectRecord(value, 'managed Playwright policy manifest')
  expectExactKeys(record, [
    'schemaVersion',
    'packageName',
    'packageVersion',
    'managedMcpId',
    'capabilityId',
    'manifestVersion',
    'upstreamCatalogDigest',
    'tools',
    'policyDigest',
    'exposedToolCount'
  ])
  const manifestWithoutDigest = { ...record }
  delete manifestWithoutDigest.policyDigest
  if (
    record.schemaVersion !== 2 ||
    record.packageName !== catalog.packageName ||
    record.packageVersion !== catalog.packageVersion ||
    record.managedMcpId !== 'builtin.browser_automation.mcp' ||
    record.capabilityId !== 'browser_automation' ||
    typeof record.manifestVersion !== 'string' ||
    record.manifestVersion.length === 0 ||
    record.manifestVersion.length > 128 ||
    record.upstreamCatalogDigest !== catalog.catalogDigest ||
    typeof record.policyDigest !== 'string' ||
    !SHA256_PATTERN.test(record.policyDigest) ||
    digestJson(manifestWithoutDigest) !== record.policyDigest ||
    !Number.isSafeInteger(record.exposedToolCount) ||
    (record.exposedToolCount as number) < 1 ||
    (record.exposedToolCount as number) > 128 ||
    !Array.isArray(record.tools) ||
    record.tools.length !== catalog.tools.length
  ) {
    throw new Error('Managed Playwright policy manifest identity is invalid')
  }

  const upstreamByName = new Map(catalog.tools.map((tool) => [tool.name, tool] as const))
  const seen = new Set<string>()
  const reviewed = record.tools.map((value, index) => {
    const parsed = parsePolicyTool(value, index)
    const upstream = upstreamByName.get(parsed.rawName)
    if (
      !upstream ||
      seen.has(parsed.rawName) ||
      parsed.modelName !== parsed.rawName ||
      parsed.upstreamSchemaDigest !== digestJson(upstream.inputSchema) ||
      parsed.hostOverlayDigest !== digestJson(parsed.hostOverlay)
    ) {
      throw new Error('Managed Playwright policy manifest does not match the upstream Catalog')
    }
    seen.add(parsed.rawName)
    const inputSchema = modelSchemaForOfficialPlaywrightTool(
      upstream.inputSchema,
      parsed.hostOverlay
    )
    if (digestJson(inputSchema) !== parsed.hostInputSchemaDigest) {
      throw new Error('Managed Playwright Host overlay digest is invalid')
    }
    const readOnly = upstream.annotations?.readOnlyHint === true
    return Object.freeze({
      rawName: parsed.rawName,
      modelName: parsed.modelName,
      description:
        parsed.rawName === 'browser_take_screenshot'
          ? "Take a screenshot of the current page. You can't perform actions based on the screenshot, use browser_snapshot for actions. A successful result includes readPath as an image-artifact://sha256/... URI; pass that exact value to read_image.path. Do not guess a workspace path, filename, displayName, or artifactId."
          : (HOST_TOOL_DESCRIPTIONS[parsed.rawName] ?? upstream.description ?? parsed.rawName),
      handlingMode: parsed.handlingMode,
      exposed: parsed.exposed,
      safety: readOnly ? ('read_only' as const) : ('destructive' as const),
      inputSchema: deepFreeze(inputSchema),
      schemaDigest: parsed.hostInputSchemaDigest,
      upstreamSchemaDigest: parsed.upstreamSchemaDigest,
      reasonCode: parsed.reasonCode,
      constraints: Object.freeze(parsed.constraints)
    })
  })
  if ([...upstreamByName.keys()].some((name) => !seen.has(name))) {
    throw new Error('Managed Playwright policy manifest is incomplete')
  }
  if (reviewed.filter((tool) => tool.exposed).length !== record.exposedToolCount) {
    throw new Error('Managed Playwright policy manifest exposed tool count is invalid')
  }
  const handlingCounts = Object.fromEntries(
    (
      [
        'pass_through',
        'host_adapted',
        'approval_required',
        'artifact_managed',
        'sandboxed',
        'unsupported'
      ] as const
    ).map((mode) => [mode, reviewed.filter((tool) => tool.handlingMode === mode).length])
  )
  if (
    handlingCounts.pass_through !== 25 ||
    handlingCounts.host_adapted !== 8 ||
    handlingCounts.approval_required !== 21 ||
    handlingCounts.artifact_managed !== 7 ||
    handlingCounts.sandboxed !== 1 ||
    handlingCounts.unsupported !== 7
  ) {
    throw new Error('Managed Playwright policy manifest handling classification is invalid')
  }
  return Object.freeze({
    schemaVersion: 2,
    packageName: '@playwright/mcp',
    packageVersion: '0.0.79',
    managedMcpId: 'builtin.browser_automation.mcp',
    capabilityId: 'browser_automation',
    manifestVersion: record.manifestVersion as string,
    upstreamCatalogDigest: record.upstreamCatalogDigest as string,
    policyDigest: record.policyDigest as string,
    exposedToolCount: record.exposedToolCount as number,
    tools: Object.freeze(reviewed)
  })
}

function parsePolicyTool(value: unknown, index: number): ParsedPolicyTool {
  const context = `managed Playwright policy manifest.tools[${index}]`
  const record = expectRecord(value, context)
  expectExactKeys(record, [
    'rawName',
    'modelName',
    'handlingMode',
    'exposed',
    'upstreamSchemaDigest',
    'hostOverlay',
    'hostOverlayDigest',
    'hostInputSchemaDigest',
    'reasonCode',
    'constraints'
  ])
  const modes: readonly ManagedPlaywrightHandlingMode[] = [
    'pass_through',
    'host_adapted',
    'approval_required',
    'artifact_managed',
    'sandboxed',
    'unsupported'
  ]
  if (
    typeof record.rawName !== 'string' ||
    !TOOL_NAME_PATTERN.test(record.rawName) ||
    typeof record.modelName !== 'string' ||
    !TOOL_NAME_PATTERN.test(record.modelName) ||
    typeof record.handlingMode !== 'string' ||
    !modes.includes(record.handlingMode as ManagedPlaywrightHandlingMode) ||
    typeof record.exposed !== 'boolean' ||
    typeof record.upstreamSchemaDigest !== 'string' ||
    !SHA256_PATTERN.test(record.upstreamSchemaDigest) ||
    typeof record.hostOverlayDigest !== 'string' ||
    !SHA256_PATTERN.test(record.hostOverlayDigest) ||
    typeof record.hostInputSchemaDigest !== 'string' ||
    !SHA256_PATTERN.test(record.hostInputSchemaDigest) ||
    typeof record.reasonCode !== 'string' ||
    !/^[a-z0-9_]{1,128}$/.test(record.reasonCode) ||
    !Array.isArray(record.constraints) ||
    record.constraints.some(
      (constraint) => typeof constraint !== 'string' || !/^[a-z0-9_]{1,128}$/.test(constraint)
    )
  ) {
    throw new Error(`${context} identity is invalid`)
  }
  if (
    record.exposed &&
    record.handlingMode !== 'pass_through' &&
    record.handlingMode !== 'host_adapted' &&
    record.handlingMode !== 'approval_required' &&
    record.handlingMode !== 'artifact_managed'
  ) {
    throw new Error(`${context} cannot expose a tool before its handling boundary exists`)
  }
  const constraints = record.constraints as string[]
  const requiredHandlingConstraint: Record<ManagedPlaywrightHandlingMode, string> = {
    pass_through: 'managed_page_context_only',
    host_adapted: 'host_lifecycle_adapter_required',
    approval_required: 'task_scoped_tool_approval',
    artifact_managed: 'artifact_handle_only',
    sandboxed: 'isolated_process_required',
    unsupported: 'not_model_visible'
  }
  if (
    new Set(constraints).size !== constraints.length ||
    !constraints.includes('requires_call_reason') ||
    !constraints.includes(
      requiredHandlingConstraint[record.handlingMode as ManagedPlaywrightHandlingMode]
    )
  ) {
    throw new Error(`${context} constraints are invalid`)
  }
  const hostOverlay = parseHostOverlay(record.hostOverlay, `${context}.hostOverlay`)
  return {
    rawName: record.rawName,
    modelName: record.modelName,
    handlingMode: record.handlingMode as ManagedPlaywrightHandlingMode,
    exposed: record.exposed,
    upstreamSchemaDigest: record.upstreamSchemaDigest,
    hostOverlay,
    hostOverlayDigest: record.hostOverlayDigest,
    hostInputSchemaDigest: record.hostInputSchemaDigest,
    reasonCode: record.reasonCode,
    constraints: [...constraints]
  }
}

function parseHostOverlay(value: unknown, context: string): ManagedPlaywrightHostOverlay {
  const record = expectRecord(value, context)
  const allowedKeys = ['addCallReason', 'removeProperties', 'propertyOverrides']
  if (Object.keys(record).some((key) => !allowedKeys.includes(key))) {
    throw new Error(`${context} contains unsupported fields`)
  }
  if (record.addCallReason !== true) throw new Error(`${context}.addCallReason must be true`)
  const removeProperties = record.removeProperties
  if (
    removeProperties !== undefined &&
    (!Array.isArray(removeProperties) ||
      removeProperties.some((name) => typeof name !== 'string' || !TOOL_NAME_PATTERN.test(name)))
  ) {
    throw new Error(`${context}.removeProperties is invalid`)
  }
  const rawPropertyOverrides =
    record.propertyOverrides === undefined
      ? undefined
      : expectRecord(record.propertyOverrides, `${context}.propertyOverrides`)
  let propertyOverrides: Record<string, Readonly<Record<string, unknown>>> | undefined
  if (rawPropertyOverrides) {
    propertyOverrides = {}
    for (const [name, override] of Object.entries(rawPropertyOverrides)) {
      if (!TOOL_NAME_PATTERN.test(name)) throw new Error(`${context}.propertyOverrides is invalid`)
      propertyOverrides[name] = structuredClone(
        expectRecord(override, `${context}.propertyOverrides.${name}`)
      )
    }
  }
  return deepFreeze({
    addCallReason: true,
    ...(removeProperties === undefined ? {} : { removeProperties: [...removeProperties] }),
    ...(propertyOverrides === undefined ? {} : { propertyOverrides })
  })
}

function deepMergeRecord(
  base: Record<string, unknown>,
  overlay: Readonly<Record<string, unknown>>
): Record<string, unknown> {
  const merged = structuredClone(base)
  for (const [key, value] of Object.entries(overlay)) {
    merged[key] =
      isRecord(merged[key]) && isRecord(value)
        ? deepMergeRecord(merged[key] as Record<string, unknown>, value)
        : structuredClone(value)
  }
  return merged
}

function validateHostPropertyOverride(
  upstream: Readonly<Record<string, unknown>>,
  override: Readonly<Record<string, unknown>>
): void {
  const keys = Object.keys(override)
  if (
    keys.length === 0 ||
    keys.some((key) => key !== 'description' && key !== 'enum' && key !== 'default')
  ) {
    throw new Error('catalog_drift')
  }
  if (
    override.description !== undefined &&
    (typeof override.description !== 'string' ||
      override.description.trim().length === 0 ||
      override.description.length > 4_096)
  ) {
    throw new Error('catalog_drift')
  }
  const upstreamEnum = Array.isArray(upstream.enum) ? upstream.enum : undefined
  const restrictedEnum = override.enum
  if (restrictedEnum !== undefined) {
    if (
      !upstreamEnum ||
      !Array.isArray(restrictedEnum) ||
      restrictedEnum.length === 0 ||
      restrictedEnum.some(
        (value, index) =>
          restrictedEnum
            .slice(0, index)
            .some((candidate) => digestJson(candidate) === digestJson(value)) ||
          !upstreamEnum.some((candidate) => digestJson(candidate) === digestJson(value))
      )
    ) {
      throw new Error('catalog_drift')
    }
  }
  if (override.default !== undefined) {
    const finalEnum = Array.isArray(restrictedEnum) ? restrictedEnum : upstreamEnum
    if (
      upstream.default === undefined ||
      digestJson(override.default) === digestJson(upstream.default) ||
      !finalEnum?.some((candidate) => digestJson(candidate) === digestJson(override.default))
    ) {
      throw new Error('catalog_drift')
    }
  }
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
  if (!isRecord(value)) throw new Error(`${context} must be an object`)
  return value
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function expectExactKeys(record: Record<string, unknown>, expected: readonly string[]): void {
  const actual = Object.keys(record).sort()
  const wanted = [...expected].sort()
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new Error('Managed Playwright Catalog contains unknown or missing fields')
  }
}

function hasExactOwnKeys(value: object, expected: readonly string[]): boolean {
  const actual = Object.keys(value).sort()
  const wanted = [...expected].sort()
  return actual.length === wanted.length && actual.every((key, index) => key === wanted[index])
}

function sameStringSet(value: unknown[], expected: readonly string[]): boolean {
  const actual = value.filter((entry): entry is string => typeof entry === 'string').sort()
  const wanted = [...expected].sort()
  return (
    actual.length === value.length &&
    actual.length === wanted.length &&
    actual.every((entry, index) => entry === wanted[index])
  )
}
