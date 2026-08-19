export const MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION = 1 as const
export const MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD =
  'mcp.builtinPlaywright.command' as const
export const MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD = 'mcp.builtinPlaywright.cancel' as const
export const MANAGED_PLAYWRIGHT_COMPLETE_METHOD = 'mcp.builtinPlaywright.complete' as const
export const BROWSER_RISK_AUTHORIZE_METHOD = 'mcp.browserRisk.authorize' as const
export const BROWSER_RISK_CANCEL_METHOD = 'mcp.browserRisk.cancel' as const
export const BROWSER_RISK_PROTOCOL_SCHEMA_VERSION = 1 as const

export interface ManagedPlaywrightAuthorizationContext {
  runId: string
  capabilityId: 'browser_automation'
  activationId: string
  manifestDigest: string
  policyRevision: number
  grantExpiresAtMs: number
  invocationId: string
  callId: string
  triggerToolName: string
  callReason: string
}

export type ManagedPlaywrightCommand =
  | { type: 'connect' }
  | { type: 'list_tools'; cursor: string | null }
  | {
      type: 'call_tool'
      name: string
      arguments: unknown
      timeoutMs: number
      authorizationContext: ManagedPlaywrightAuthorizationContext
    }
  | { type: 'close' }

export interface ManagedPlaywrightCommandNotification {
  schemaVersion: typeof MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION
  requestId: string
  serverId: string
  deadlineMs: number
  command: ManagedPlaywrightCommand
}

export type ManagedPlaywrightCancelReason = 'cancelled' | 'timeout' | 'shutdown'

export interface ManagedPlaywrightCancelNotification {
  schemaVersion: typeof MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION
  requestId: string
  reason: ManagedPlaywrightCancelReason
}

export type ManagedPlaywrightDispatchCertainty =
  'definitely_not_dispatched' | 'possibly_dispatched' | 'response_received'

export type ManagedPlaywrightBridgeErrorCode =
  | 'cancelled'
  | 'timeout'
  | 'closed'
  | 'busy'
  | 'surface_unavailable'
  | 'target_closed'
  | 'tool_not_reviewed'
  | 'invalid_arguments'
  | 'catalog_drift'
  | 'output_too_large'
  | 'protocol_error'
  | 'outcome_unknown'
  | 'internal_safe_error'

export type ManagedPlaywrightCompletionOutcome =
  | { type: 'connected'; protocol: unknown }
  | { type: 'tools_listed'; page: unknown }
  | { type: 'tool_called'; result: unknown }
  | { type: 'closed' }
  | {
      type: 'error'
      code: ManagedPlaywrightBridgeErrorCode
      dispatchCertainty: ManagedPlaywrightDispatchCertainty
    }

export interface ManagedPlaywrightCompletionInput {
  schemaVersion: typeof MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION
  requestId: string
  outcome: ManagedPlaywrightCompletionOutcome
}

export interface ManagedPlaywrightCompletionOutput {
  schemaVersion: typeof MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION
  accepted: boolean
}

export type BrowserRiskKind =
  | 'insecure_http'
  | 'localhost'
  | 'loopback'
  | 'private_network'
  | 'link_local'
  | 'cloud_metadata'
  | 'non_standard_port'
  | 'url_userinfo'
  | 'dns_private_resolution'
  | 'risk_escalation'
  | 'new_window'
  | 'file_upload'
  | 'file_download'
  | 'local_service_request'

export type BrowserResolvedAddressClass =
  'public' | 'loopback' | 'private' | 'link_local' | 'cloud_metadata' | 'unresolved'

export type BrowserRiskTrigger =
  'tool_argument' | 'main_frame' | 'redirect' | 'new_window' | 'subresource' | 'upload' | 'download'

export type BrowserRiskDispatchCertainty = 'definitely_not_dispatched' | 'possibly_dispatched'

export interface BrowserRiskDestinationInput {
  normalizedUrl: string
  origin: string
  scheme: 'http' | 'https'
  asciiHost: string
  effectivePort: number
  addressClass: BrowserResolvedAddressClass
  /** Main/Core-only HMAC. This field must never be forwarded to Agent or Renderer. */
  resolutionFingerprint: string
  /** Main/Core-only exact target/action HMAC. Never forwarded to Agent or Renderer. */
  targetFingerprint: string
}

export interface BrowserRiskAuthorizeInput {
  schemaVersion: typeof BROWSER_RISK_PROTOCOL_SCHEMA_VERSION
  requestId: string
  parentRequestId: string | null
  authorizationContext: ManagedPlaywrightAuthorizationContext
  destination: BrowserRiskDestinationInput
  riskKinds: BrowserRiskKind[]
  trigger: BrowserRiskTrigger
  dispatchCertainty: BrowserRiskDispatchCertainty
  createdAtMs: number
  expiresAtMs: number
}

export type BrowserRiskAuthorizationDecision =
  | 'approved'
  | 'rejected'
  | 'cancelled'
  | 'expired'
  | 'policy_denied'
  | 'unsupported_host_boundary'
  | 'outcome_unknown'

export interface BrowserRiskAuthorizeOutput {
  schemaVersion: typeof BROWSER_RISK_PROTOCOL_SCHEMA_VERSION
  decision: BrowserRiskAuthorizationDecision
  grantId: string | null
  reason: string | null
}

export interface BrowserRiskCancelInput {
  schemaVersion: typeof BROWSER_RISK_PROTOCOL_SCHEMA_VERSION
  requestId: string
}

export interface BrowserRiskCancelOutput {
  schemaVersion: typeof BROWSER_RISK_PROTOCOL_SCHEMA_VERSION
  accepted: boolean
}

const REQUEST_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const SERVER_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const TOOL_NAME = /^[A-Za-z0-9_-]{1,128}$/
const BROWSER_TOOL_NAME = /^[A-Za-z0-9_-]{1,64}$/
const SHA256_DIGEST = /^sha256:[0-9a-f]{64}$/
const HMAC_SHA256_DIGEST = /^hmac-sha256:[0-9a-f]{64}$/
const ASCII_HOST = /^[A-Za-z0-9.:[\]-]{1,253}$/
const MAX_ARGUMENT_BYTES = 64 * 1024
const BROWSER_RISK_KINDS = [
  'insecure_http',
  'localhost',
  'loopback',
  'private_network',
  'link_local',
  'cloud_metadata',
  'non_standard_port',
  'url_userinfo',
  'dns_private_resolution',
  'risk_escalation',
  'new_window',
  'file_upload',
  'file_download',
  'local_service_request'
] as const satisfies readonly BrowserRiskKind[]
const BROWSER_ADDRESS_CLASSES = [
  'public',
  'loopback',
  'private',
  'link_local',
  'cloud_metadata',
  'unresolved'
] as const satisfies readonly BrowserResolvedAddressClass[]
const BROWSER_RISK_TRIGGERS = [
  'tool_argument',
  'main_frame',
  'redirect',
  'new_window',
  'subresource',
  'upload',
  'download'
] as const satisfies readonly BrowserRiskTrigger[]
const BROWSER_RISK_CERTAINTIES = [
  'definitely_not_dispatched',
  'possibly_dispatched'
] as const satisfies readonly BrowserRiskDispatchCertainty[]
const BROWSER_RISK_DECISIONS = [
  'approved',
  'rejected',
  'cancelled',
  'expired',
  'policy_denied',
  'unsupported_host_boundary',
  'outcome_unknown'
] as const satisfies readonly BrowserRiskAuthorizationDecision[]

export function parseManagedPlaywrightCommandNotification(
  value: unknown
): ManagedPlaywrightCommandNotification {
  const record = exactRecord(value, [
    'schemaVersion',
    'requestId',
    'serverId',
    'deadlineMs',
    'command'
  ])
  expectSchemaVersion(record.schemaVersion)
  const requestId = expectMatchingString(record.requestId, REQUEST_ID)
  const serverId = expectMatchingString(record.serverId, SERVER_ID)
  const deadlineMs = expectSafeInteger(record.deadlineMs, 1, Number.MAX_SAFE_INTEGER)
  return {
    schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
    requestId,
    serverId,
    deadlineMs,
    command: parseCommand(record.command)
  }
}

export function parseManagedPlaywrightCancelNotification(
  value: unknown
): ManagedPlaywrightCancelNotification {
  const record = exactRecord(value, ['schemaVersion', 'requestId', 'reason'])
  expectSchemaVersion(record.schemaVersion)
  const reason = expectEnum(record.reason, ['cancelled', 'timeout', 'shutdown'] as const)
  return {
    schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
    requestId: expectMatchingString(record.requestId, REQUEST_ID),
    reason
  }
}

export function parseManagedPlaywrightCompletionInput(
  value: unknown
): ManagedPlaywrightCompletionInput {
  const record = exactRecord(value, ['schemaVersion', 'requestId', 'outcome'])
  expectSchemaVersion(record.schemaVersion)
  return {
    schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
    requestId: expectMatchingString(record.requestId, REQUEST_ID),
    outcome: parseCompletionOutcome(record.outcome)
  }
}

export function parseManagedPlaywrightCompletionOutput(
  value: unknown
): ManagedPlaywrightCompletionOutput {
  const record = exactRecord(value, ['schemaVersion', 'accepted'])
  expectSchemaVersion(record.schemaVersion)
  if (typeof record.accepted !== 'boolean') throw new Error('Invalid managed Playwright completion')
  return {
    schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
    accepted: record.accepted
  }
}

export function parseBrowserRiskAuthorizeInput(value: unknown): BrowserRiskAuthorizeInput {
  const record = exactRecord(value, [
    'schemaVersion',
    'requestId',
    'parentRequestId',
    'authorizationContext',
    'destination',
    'riskKinds',
    'trigger',
    'dispatchCertainty',
    'createdAtMs',
    'expiresAtMs'
  ])
  if (record.schemaVersion !== BROWSER_RISK_PROTOCOL_SCHEMA_VERSION) {
    throw new Error('Unsupported browser risk schema version')
  }
  const createdAtMs = expectSafeInteger(record.createdAtMs, 1, Number.MAX_SAFE_INTEGER)
  const expiresAtMs = expectSafeInteger(
    record.expiresAtMs,
    createdAtMs + 1,
    Number.MAX_SAFE_INTEGER
  )
  if (expiresAtMs - createdAtMs > 15 * 60 * 1_000) {
    throw new Error('Invalid browser risk expiry')
  }
  const parentRequestId =
    record.parentRequestId === null
      ? null
      : expectMatchingString(record.parentRequestId, REQUEST_ID)
  return {
    schemaVersion: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
    requestId: expectMatchingString(record.requestId, REQUEST_ID),
    parentRequestId,
    authorizationContext: parseAuthorizationContext(record.authorizationContext),
    destination: parseBrowserRiskDestination(record.destination),
    riskKinds: expectUniqueEnumArray(record.riskKinds, BROWSER_RISK_KINDS),
    trigger: expectEnum(record.trigger, BROWSER_RISK_TRIGGERS),
    dispatchCertainty: expectEnum(record.dispatchCertainty, BROWSER_RISK_CERTAINTIES),
    createdAtMs,
    expiresAtMs
  }
}

export function parseBrowserRiskAuthorizeOutput(value: unknown): BrowserRiskAuthorizeOutput {
  const record = exactRecord(value, ['schemaVersion', 'decision', 'grantId', 'reason'])
  if (record.schemaVersion !== BROWSER_RISK_PROTOCOL_SCHEMA_VERSION) {
    throw new Error('Unsupported browser risk schema version')
  }
  const decision = expectEnum(record.decision, BROWSER_RISK_DECISIONS)
  const grantId = record.grantId === null ? null : expectMatchingString(record.grantId, SERVER_ID)
  const reason = record.reason === null ? null : expectSafeString(record.reason, 1, 1_024, false)
  if ((decision === 'approved') !== (grantId !== null)) {
    throw new Error('Invalid browser risk grant identity')
  }
  return {
    schemaVersion: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
    decision,
    grantId,
    reason
  }
}

export function parseBrowserRiskCancelInput(value: unknown): BrowserRiskCancelInput {
  const record = exactRecord(value, ['schemaVersion', 'requestId'])
  if (record.schemaVersion !== BROWSER_RISK_PROTOCOL_SCHEMA_VERSION) {
    throw new Error('Unsupported browser risk schema version')
  }
  return {
    schemaVersion: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
    requestId: expectMatchingString(record.requestId, REQUEST_ID)
  }
}

export function parseBrowserRiskCancelOutput(value: unknown): BrowserRiskCancelOutput {
  const record = exactRecord(value, ['schemaVersion', 'accepted'])
  if (record.schemaVersion !== BROWSER_RISK_PROTOCOL_SCHEMA_VERSION) {
    throw new Error('Unsupported browser risk schema version')
  }
  if (typeof record.accepted !== 'boolean') throw new Error('Invalid browser risk cancellation')
  return { schemaVersion: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION, accepted: record.accepted }
}

function parseAuthorizationContext(value: unknown): ManagedPlaywrightAuthorizationContext {
  const record = exactRecord(value, [
    'runId',
    'capabilityId',
    'activationId',
    'manifestDigest',
    'policyRevision',
    'grantExpiresAtMs',
    'invocationId',
    'callId',
    'triggerToolName',
    'callReason'
  ])
  if (record.capabilityId !== 'browser_automation') {
    throw new Error('Invalid managed Playwright capability identity')
  }
  const grantExpiresAtMs = expectSafeInteger(record.grantExpiresAtMs, 1, Number.MAX_SAFE_INTEGER)
  if (grantExpiresAtMs % 1_000 !== 0) {
    throw new Error('Invalid managed Playwright grant expiry')
  }
  return {
    runId: expectSafeString(record.runId, 1, 256),
    capabilityId: 'browser_automation',
    activationId: expectMatchingString(record.activationId, REQUEST_ID),
    manifestDigest: expectMatchingString(record.manifestDigest, SHA256_DIGEST),
    policyRevision: expectSafeInteger(record.policyRevision, 1, Number.MAX_SAFE_INTEGER),
    grantExpiresAtMs,
    invocationId: expectMatchingString(record.invocationId, REQUEST_ID),
    callId: expectSafeString(record.callId, 1, 256),
    triggerToolName: expectMatchingString(record.triggerToolName, BROWSER_TOOL_NAME),
    callReason: expectSafeString(record.callReason, 1, 512)
  }
}

function parseBrowserRiskDestination(value: unknown): BrowserRiskDestinationInput {
  const record = exactRecord(value, [
    'normalizedUrl',
    'origin',
    'scheme',
    'asciiHost',
    'effectivePort',
    'addressClass',
    'resolutionFingerprint',
    'targetFingerprint'
  ])
  const normalizedUrl = expectSafeString(record.normalizedUrl, 1, 2_048)
  const origin = expectSafeString(record.origin, 1, 2_048)
  const scheme = expectEnum(record.scheme, ['http', 'https'] as const)
  const asciiHost = expectMatchingString(record.asciiHost, ASCII_HOST)
  const effectivePort = expectSafeInteger(record.effectivePort, 1, 65_535)
  let parsed: URL
  try {
    parsed = new URL(normalizedUrl)
  } catch {
    throw new Error('Invalid browser risk destination')
  }
  const parsedPort = parsed.port ? Number(parsed.port) : parsed.protocol === 'https:' ? 443 : 80
  if (
    parsed.protocol !== `${scheme}:` ||
    parsed.origin !== origin ||
    parsedPort !== effectivePort ||
    parsed.username !== '' ||
    parsed.password !== '' ||
    parsed.search !== '' ||
    parsed.hash !== ''
  ) {
    throw new Error('Invalid browser risk destination')
  }
  return {
    normalizedUrl,
    origin,
    scheme,
    asciiHost,
    effectivePort,
    addressClass: expectEnum(record.addressClass, BROWSER_ADDRESS_CLASSES),
    resolutionFingerprint: expectMatchingString(record.resolutionFingerprint, HMAC_SHA256_DIGEST),
    targetFingerprint: expectMatchingString(record.targetFingerprint, HMAC_SHA256_DIGEST)
  }
}

function parseCommand(value: unknown): ManagedPlaywrightCommand {
  const base = expectRecord(value)
  const type = base.type
  switch (type) {
    case 'connect':
    case 'close':
      exactKeys(base, ['type'])
      return { type }
    case 'list_tools': {
      exactKeys(base, ['type', 'cursor'])
      if (base.cursor !== null && (typeof base.cursor !== 'string' || base.cursor.length > 4096)) {
        throw new Error('Invalid managed Playwright cursor')
      }
      return { type, cursor: base.cursor }
    }
    case 'call_tool': {
      exactKeys(base, ['type', 'name', 'arguments', 'timeoutMs', 'authorizationContext'])
      const name = expectMatchingString(base.name, TOOL_NAME)
      const serialized = JSON.stringify(base.arguments)
      if (
        serialized === undefined ||
        new TextEncoder().encode(serialized).byteLength > MAX_ARGUMENT_BYTES
      ) {
        throw new Error('Invalid managed Playwright arguments')
      }
      return {
        type,
        name,
        arguments: base.arguments,
        timeoutMs: expectSafeInteger(base.timeoutMs, 1, 300_000),
        authorizationContext: parseAuthorizationContext(base.authorizationContext)
      }
    }
    default:
      throw new Error('Invalid managed Playwright command')
  }
}

function parseCompletionOutcome(value: unknown): ManagedPlaywrightCompletionOutcome {
  const base = expectRecord(value)
  switch (base.type) {
    case 'connected':
      exactKeys(base, ['type', 'protocol'])
      return { type: 'connected', protocol: base.protocol }
    case 'tools_listed':
      exactKeys(base, ['type', 'page'])
      return { type: 'tools_listed', page: base.page }
    case 'tool_called':
      exactKeys(base, ['type', 'result'])
      return { type: 'tool_called', result: base.result }
    case 'closed':
      exactKeys(base, ['type'])
      return { type: 'closed' }
    case 'error': {
      exactKeys(base, ['type', 'code', 'dispatchCertainty'])
      const code = expectEnum(base.code, [
        'cancelled',
        'timeout',
        'closed',
        'busy',
        'surface_unavailable',
        'target_closed',
        'tool_not_reviewed',
        'invalid_arguments',
        'catalog_drift',
        'output_too_large',
        'protocol_error',
        'outcome_unknown',
        'internal_safe_error'
      ] as const)
      const dispatchCertainty = expectEnum(base.dispatchCertainty, [
        'definitely_not_dispatched',
        'possibly_dispatched',
        'response_received'
      ] as const)
      if (code === 'outcome_unknown' && dispatchCertainty !== 'possibly_dispatched') {
        throw new Error('Outcome unknown requires possibly-dispatched certainty')
      }
      return { type: 'error', code, dispatchCertainty }
    }
    default:
      throw new Error('Invalid managed Playwright completion outcome')
  }
}

function expectSchemaVersion(value: unknown): void {
  if (value !== MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION) {
    throw new Error('Unsupported managed Playwright bridge schema version')
  }
}

function exactRecord(value: unknown, keys: readonly string[]): Record<string, unknown> {
  const record = expectRecord(value)
  exactKeys(record, keys)
  return record
}

function expectRecord(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('Invalid managed Playwright bridge object')
  }
  return value as Record<string, unknown>
}

function exactKeys(record: Record<string, unknown>, keys: readonly string[]): void {
  const actual = Object.keys(record).sort()
  const expected = [...keys].sort()
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new Error('Unknown managed Playwright bridge field')
  }
}

function expectMatchingString(value: unknown, pattern: RegExp): string {
  if (typeof value !== 'string' || !pattern.test(value)) {
    throw new Error('Invalid managed Playwright bridge identity')
  }
  return value
}

function expectSafeString(
  value: unknown,
  minimumLength: number,
  maximumLength: number,
  rejectWhitespaceOnly = true
): string {
  if (
    typeof value !== 'string' ||
    value.length < minimumLength ||
    value.length > maximumLength ||
    containsControlCharacter(value) ||
    (rejectWhitespaceOnly && value.trim().length === 0)
  ) {
    throw new Error('Invalid managed Playwright safe string')
  }
  return value
}

function containsControlCharacter(value: string): boolean {
  for (const character of value) {
    const code = character.charCodeAt(0)
    if (code <= 0x1f || code === 0x7f) return true
  }
  return false
}

function expectUniqueEnumArray<const T extends readonly string[]>(
  value: unknown,
  values: T
): T[number][] {
  if (!Array.isArray(value) || value.length === 0 || value.length > values.length) {
    throw new Error('Invalid browser risk enum list')
  }
  const parsed = value.map((entry) => expectEnum(entry, values))
  if (new Set(parsed).size !== parsed.length) throw new Error('Invalid browser risk enum list')
  return parsed
}

function expectSafeInteger(value: unknown, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < minimum || (value as number) > maximum) {
    throw new Error('Invalid managed Playwright bridge number')
  }
  return value as number
}

function expectEnum<const T extends readonly string[]>(value: unknown, values: T): T[number] {
  if (typeof value !== 'string' || !values.includes(value)) {
    throw new Error('Invalid managed Playwright bridge enum')
  }
  return value as T[number]
}
