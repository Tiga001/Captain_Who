import type {
  AgentBrowserAddressClass,
  AgentBrowserRiskApproval,
  AgentBrowserRiskKind,
  AgentBrowserReviewedToolName,
  AgentBrowserRiskTrigger,
  AgentBuiltinCapabilityActivationApproval,
  AgentBuiltinMcpToolApproval,
  AgentBuiltinMcpToolRiskKind,
  AgentProposedAction
} from '../agent'
import { parseMcpBuiltinCapabilityId } from '../mcp/parsers'
import {
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import {
  MAX_RENDERER_DATE_UNIX_SECONDS,
  SAFE_CODE_PATTERN,
  expectBoundedNonEmptyString,
  expectDisplayText,
  expectModelToolCallId,
  expectOpaqueRunId,
  expectSafeCode,
  expectUuidV4,
  expectVersionedSha256Digest,
  hasAnyControl,
  parseNullableHttpOrigin
} from './shared'
export const BUILTIN_CAPABILITY_APPROVAL_TTL_SECONDS = 15 * 60
export const BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS = 15 * 60
export const BROWSER_RISK_APPROVAL_TTL_SECONDS = 15 * 60
export const BROWSER_ADDRESS_CLASSES = [
  'public',
  'loopback',
  'private',
  'link_local',
  'cloud_metadata',
  'unresolved'
] as const satisfies readonly AgentBrowserAddressClass[]
export const BROWSER_RISK_KINDS = [
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
] as const satisfies readonly AgentBrowserRiskKind[]
export const BROWSER_RISK_TRIGGERS = [
  'tool_argument',
  'main_frame',
  'redirect',
  'new_window',
  'subresource',
  'upload',
  'download'
] as const satisfies readonly AgentBrowserRiskTrigger[]
export const BROWSER_REVIEWED_TOOL_NAMES = [
  'browser_navigate',
  'browser_snapshot',
  'browser_find',
  'browser_click',
  'browser_type',
  'browser_fill_form',
  'browser_press_key',
  'browser_tabs',
  'browser_wait_for',
  'browser_close'
] as const satisfies readonly AgentBrowserReviewedToolName[]
export const BUILTIN_MCP_TOOL_RISK_KINDS = [
  'file_read',
  'file_write',
  'file_upload',
  'file_download',
  'cookie_read',
  'cookie_write',
  'local_storage_read',
  'local_storage_write',
  'session_storage_read',
  'session_storage_write',
  'storage_state_import',
  'storage_state_export',
  'network_sensitive_read',
  'page_script_execution',
  'unsafe_code_execution'
] as const satisfies readonly AgentBuiltinMcpToolRiskKind[]
export const BUILTIN_MCP_APPROVAL_TOOL_NAMES = [
  'browser_cookie_clear',
  'browser_cookie_delete',
  'browser_cookie_get',
  'browser_cookie_list',
  'browser_cookie_set',
  'browser_drop',
  'browser_evaluate',
  'browser_file_upload',
  'browser_localstorage_clear',
  'browser_localstorage_delete',
  'browser_localstorage_get',
  'browser_localstorage_list',
  'browser_localstorage_set',
  'browser_network_request',
  'browser_sessionstorage_clear',
  'browser_sessionstorage_delete',
  'browser_sessionstorage_get',
  'browser_sessionstorage_list',
  'browser_sessionstorage_set',
  'browser_set_storage_state',
  'browser_storage_state'
] as const

export function parseAgentBuiltinCapabilityActivationProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'builtin_capability_activation' }> {
  const context = 'built-in capability activation proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'builtin_capability_activation') {
    throw invalidProtocolValue(context, 'type must be builtin_capability_activation')
  }
  return {
    type: 'builtin_capability_activation',
    approval: parseAgentBuiltinCapabilityActivationApproval(record.approval)
  }
}

export function parseAgentBuiltinCapabilityActivationApproval(
  value: unknown
): AgentBuiltinCapabilityActivationApproval {
  const context = 'built-in capability activation approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'actionId',
      'activationId',
      'runId',
      'callId',
      'capabilityId',
      'displayName',
      'reason',
      'manifestDigest',
      'policyRevision',
      'createdAt',
      'expiresAt',
      'approvalStatus'
    ] as const,
    context
  )
  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const activationId = expectUuidV4(record.activationId, `${context}.activationId`)
  if (actionId === activationId) {
    throw invalidProtocolValue(context, 'actionId and activationId must be distinct')
  }
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (createdAt > MAX_RENDERER_DATE_UNIX_SECONDS || expiresAt > MAX_RENDERER_DATE_UNIX_SECONDS) {
    throw invalidProtocolValue(context, 'timestamps exceed the Renderer-safe date range')
  }
  if (expiresAt - createdAt !== BUILTIN_CAPABILITY_APPROVAL_TTL_SECONDS) {
    throw invalidProtocolValue(context, 'expiry must equal the fixed 15 minute approval TTL')
  }
  const reason = expectDisplayText(record.reason, `${context}.reason`, 4096)
  if (!reason.trim()) {
    throw invalidProtocolValue(context, 'reason must not be blank')
  }
  const displayName = expectDisplayText(record.displayName, `${context}.displayName`, 256)
  if (!displayName.trim()) {
    throw invalidProtocolValue(context, 'displayName must not be blank')
  }
  return {
    actionId,
    activationId,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
    displayName,
    reason,
    manifestDigest: expectVersionedSha256Digest(record.manifestDigest, `${context}.manifestDigest`),
    policyRevision: expectSafeInteger(record.policyRevision, `${context}.policyRevision`, 0),
    createdAt,
    expiresAt,
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    )
  }
}

export function parseAgentBuiltinMcpToolApprovalProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'builtin_mcp_tool_approval' }> {
  const context = 'built-in MCP Tool approval proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'builtin_mcp_tool_approval') {
    throw invalidProtocolValue(context, 'type must be builtin_mcp_tool_approval')
  }
  return {
    type: 'builtin_mcp_tool_approval',
    approval: parseAgentBuiltinMcpToolApproval(record.approval)
  }
}

export function parseAgentBuiltinMcpToolApproval(value: unknown): AgentBuiltinMcpToolApproval {
  const context = 'built-in MCP Tool approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'identity',
      'capabilityDisplayName',
      'toolDisplayName',
      'callReason',
      'operationCategory',
      'resourceSummary',
      'riskKinds',
      'createdAt',
      'expiresAt',
      'approvalStatus'
    ] as const,
    context
  )
  if (record.schemaVersion !== 1) {
    throw invalidProtocolValue(`${context}.schemaVersion`, 'expected 1')
  }
  const identityRecord = expectRecord(record.identity, `${context}.identity`)
  expectOnlyKeys(
    identityRecord,
    [
      'actionId',
      'approvalId',
      'runId',
      'callId',
      'capabilityId',
      'capabilityActivationId',
      'managedMcpId',
      'packageName',
      'packageVersion',
      'upstreamCatalogDigest',
      'manifestDigest',
      'policyDigest',
      'policyRevision',
      'toolId',
      'rawName',
      'modelName',
      'upstreamSchemaDigest',
      'hostOverlayDigest',
      'hostInputSchemaDigest',
      'argumentsDigest',
      'resourceScopeDigest',
      'origin'
    ] as const,
    `${context}.identity`
  )
  const actionId = expectUuidV4(identityRecord.actionId, `${context}.identity.actionId`)
  const approvalId = expectUuidV4(identityRecord.approvalId, `${context}.identity.approvalId`)
  const capabilityActivationId = expectUuidV4(
    identityRecord.capabilityActivationId,
    `${context}.identity.capabilityActivationId`
  )
  if (new Set([actionId, approvalId, capabilityActivationId]).size !== 3) {
    throw invalidProtocolValue(context, 'approval identities must be distinct')
  }
  const origin = parseNullableHttpOrigin(identityRecord.origin, `${context}.identity.origin`)
  const resourceRecord = expectRecord(record.resourceSummary, `${context}.resourceSummary`)
  expectOnlyKeys(
    resourceRecord,
    ['scope', 'displayName', 'fileBasenames', 'origin'] as const,
    `${context}.resourceSummary`
  )
  if (!Array.isArray(resourceRecord.fileBasenames) || resourceRecord.fileBasenames.length > 32) {
    throw invalidProtocolValue(`${context}.resourceSummary.fileBasenames`, 'invalid file list')
  }
  const fileBasenames = resourceRecord.fileBasenames.map((value, index) => {
    const basename = expectDisplayText(
      value,
      `${context}.resourceSummary.fileBasenames[${index}]`,
      256
    )
    if (!basename.trim() || basename.includes('/') || basename.includes('\\')) {
      throw invalidProtocolValue(
        `${context}.resourceSummary.fileBasenames[${index}]`,
        'must remain a basename'
      )
    }
    return basename
  })
  const resourceOrigin = parseNullableHttpOrigin(
    resourceRecord.origin,
    `${context}.resourceSummary.origin`
  )
  if (resourceOrigin !== origin) {
    throw invalidProtocolValue(context, 'resource and identity origins must match')
  }
  if (!Array.isArray(record.riskKinds) || record.riskKinds.length === 0) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must be a non-empty array')
  }
  const riskKinds = record.riskKinds.map((risk, index) =>
    expectEnum(risk, BUILTIN_MCP_TOOL_RISK_KINDS, `${context}.riskKinds[${index}]`)
  )
  if (
    new Set(riskKinds).size !== riskKinds.length ||
    riskKinds.some(
      (risk, index) =>
        index > 0 &&
        BUILTIN_MCP_TOOL_RISK_KINDS.indexOf(riskKinds[index - 1]) >=
          BUILTIN_MCP_TOOL_RISK_KINDS.indexOf(risk)
    )
  ) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must be unique and canonical')
  }
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (
    createdAt > MAX_RENDERER_DATE_UNIX_SECONDS ||
    expiresAt > MAX_RENDERER_DATE_UNIX_SECONDS ||
    expiresAt - createdAt !== BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS
  ) {
    throw invalidProtocolValue(context, 'invalid fixed approval TTL')
  }
  const rawName = expectBoundedNonEmptyString(
    identityRecord.rawName,
    `${context}.identity.rawName`,
    64
  )
  const modelName = expectBoundedNonEmptyString(
    identityRecord.modelName,
    `${context}.identity.modelName`,
    64
  )
  const toolId = expectBoundedNonEmptyString(
    identityRecord.toolId,
    `${context}.identity.toolId`,
    64
  )
  if (rawName !== modelName || rawName !== toolId || !SAFE_CODE_PATTERN.test(rawName)) {
    throw invalidProtocolValue(context, 'Tool identities must match the reviewed raw identity')
  }
  return {
    schemaVersion: 1,
    identity: {
      actionId,
      approvalId,
      runId: expectOpaqueRunId(identityRecord.runId, `${context}.identity.runId`),
      callId: expectModelToolCallId(identityRecord.callId, `${context}.identity.callId`),
      capabilityId: parseMcpBuiltinCapabilityId(
        identityRecord.capabilityId,
        `${context}.identity.capabilityId`
      ),
      capabilityActivationId,
      managedMcpId: expectBoundedNonEmptyString(
        identityRecord.managedMcpId,
        `${context}.identity.managedMcpId`,
        128
      ),
      packageName: expectDisplayText(
        identityRecord.packageName,
        `${context}.identity.packageName`,
        128
      ),
      packageVersion: expectDisplayText(
        identityRecord.packageVersion,
        `${context}.identity.packageVersion`,
        64
      ),
      upstreamCatalogDigest: expectVersionedSha256Digest(
        identityRecord.upstreamCatalogDigest,
        `${context}.identity.upstreamCatalogDigest`
      ),
      manifestDigest: expectVersionedSha256Digest(
        identityRecord.manifestDigest,
        `${context}.identity.manifestDigest`
      ),
      policyDigest: expectVersionedSha256Digest(
        identityRecord.policyDigest,
        `${context}.identity.policyDigest`
      ),
      policyRevision: expectSafeInteger(
        identityRecord.policyRevision,
        `${context}.identity.policyRevision`,
        1
      ),
      toolId,
      rawName,
      modelName,
      upstreamSchemaDigest: expectVersionedSha256Digest(
        identityRecord.upstreamSchemaDigest,
        `${context}.identity.upstreamSchemaDigest`
      ),
      hostOverlayDigest: expectVersionedSha256Digest(
        identityRecord.hostOverlayDigest,
        `${context}.identity.hostOverlayDigest`
      ),
      hostInputSchemaDigest: expectVersionedSha256Digest(
        identityRecord.hostInputSchemaDigest,
        `${context}.identity.hostInputSchemaDigest`
      ),
      argumentsDigest: expectVersionedSha256Digest(
        identityRecord.argumentsDigest,
        `${context}.identity.argumentsDigest`
      ),
      resourceScopeDigest: expectVersionedSha256Digest(
        identityRecord.resourceScopeDigest,
        `${context}.identity.resourceScopeDigest`
      ),
      origin
    },
    capabilityDisplayName: expectDisplayText(
      record.capabilityDisplayName,
      `${context}.capabilityDisplayName`,
      256
    ),
    toolDisplayName: expectDisplayText(record.toolDisplayName, `${context}.toolDisplayName`, 256),
    callReason: expectDisplayText(record.callReason, `${context}.callReason`, 4096),
    operationCategory: expectSafeCode(record.operationCategory, `${context}.operationCategory`),
    resourceSummary: {
      scope: expectSafeCode(resourceRecord.scope, `${context}.resourceSummary.scope`),
      displayName: expectDisplayText(
        resourceRecord.displayName,
        `${context}.resourceSummary.displayName`,
        512
      ),
      fileBasenames,
      origin: resourceOrigin
    },
    riskKinds,
    createdAt,
    expiresAt,
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    )
  }
}

export function parseAgentBrowserRiskProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'browser_risk_approval' }> {
  const context = 'browser risk proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'browser_risk_approval') {
    throw invalidProtocolValue(context, 'type must be browser_risk_approval')
  }
  return {
    type: 'browser_risk_approval',
    approval: parseAgentBrowserRiskApproval(record.approval)
  }
}

export function parseAgentBrowserRiskApproval(value: unknown): AgentBrowserRiskApproval {
  const context = 'browser risk approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'actionId',
      'riskApprovalId',
      'runId',
      'callId',
      'capabilityId',
      'capabilityActivationId',
      'displayName',
      'reason',
      'destination',
      'trigger',
      'triggerToolName',
      'riskKinds',
      'manifestDigest',
      'policyRevision',
      'createdAt',
      'expiresAt',
      'approvalStatus'
    ] as const,
    context
  )
  if (record.schemaVersion !== 1) {
    throw invalidProtocolValue(`${context}.schemaVersion`, 'expected 1')
  }

  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const riskApprovalId = expectUuidV4(record.riskApprovalId, `${context}.riskApprovalId`)
  const capabilityActivationId = expectUuidV4(
    record.capabilityActivationId,
    `${context}.capabilityActivationId`
  )
  if (
    actionId === riskApprovalId ||
    actionId === capabilityActivationId ||
    riskApprovalId === capabilityActivationId
  ) {
    throw invalidProtocolValue(
      context,
      'action, risk approval, and activation identities must differ'
    )
  }

  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (createdAt > MAX_RENDERER_DATE_UNIX_SECONDS || expiresAt > MAX_RENDERER_DATE_UNIX_SECONDS) {
    throw invalidProtocolValue(context, 'timestamps exceed the Renderer-safe date range')
  }
  if (expiresAt - createdAt !== BROWSER_RISK_APPROVAL_TTL_SECONDS) {
    throw invalidProtocolValue(context, 'expiry must equal the fixed 15 minute approval TTL')
  }

  const reason = expectDisplayText(record.reason, `${context}.reason`, 4096)
  const displayName = expectDisplayText(record.displayName, `${context}.displayName`, 256)
  if (!reason.trim()) throw invalidProtocolValue(context, 'reason must not be blank')
  if (!displayName.trim()) throw invalidProtocolValue(context, 'displayName must not be blank')

  if (!Array.isArray(record.riskKinds) || record.riskKinds.length === 0) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must be a non-empty array')
  }
  if (record.riskKinds.length > BROWSER_RISK_KINDS.length) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'exceeded the supported risk count')
  }
  const riskKinds = record.riskKinds.map((risk, index) =>
    expectEnum(risk, BROWSER_RISK_KINDS, `${context}.riskKinds[${index}]`)
  )
  if (new Set(riskKinds).size !== riskKinds.length) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must not contain duplicates')
  }

  return {
    schemaVersion: 1,
    actionId,
    riskApprovalId,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
    capabilityActivationId,
    displayName,
    reason,
    destination: parseAgentBrowserDestinationIdentity(record.destination, `${context}.destination`),
    trigger: expectEnum(record.trigger, BROWSER_RISK_TRIGGERS, `${context}.trigger`),
    triggerToolName: expectEnum(
      record.triggerToolName,
      BROWSER_REVIEWED_TOOL_NAMES,
      `${context}.triggerToolName`
    ),
    riskKinds,
    manifestDigest: expectVersionedSha256Digest(record.manifestDigest, `${context}.manifestDigest`),
    policyRevision: expectSafeInteger(record.policyRevision, `${context}.policyRevision`, 0),
    createdAt,
    expiresAt,
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    )
  }
}

export function parseAgentBrowserDestinationIdentity(
  value: unknown,
  context: string
): AgentBrowserRiskApproval['destination'] {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['normalizedUrl', 'origin', 'scheme', 'asciiHost', 'effectivePort', 'addressClass'] as const,
    context
  )
  const normalizedUrl = expectDisplayText(record.normalizedUrl, `${context}.normalizedUrl`, 2048)
  const origin = expectDisplayText(record.origin, `${context}.origin`, 512)
  const scheme = expectEnum(record.scheme, ['http', 'https'] as const, `${context}.scheme`)
  const asciiHost = expectBoundedNonEmptyString(record.asciiHost, `${context}.asciiHost`, 253)
  if (asciiHost !== asciiHost.toLowerCase() || hasAnyControl(asciiHost)) {
    throw invalidProtocolValue(`${context}.asciiHost`, 'must be canonical lower-case ASCII')
  }
  const effectivePort = expectSafeInteger(record.effectivePort, `${context}.effectivePort`, 1)
  if (effectivePort > 65_535) {
    throw invalidProtocolValue(`${context}.effectivePort`, 'must be a valid TCP port')
  }

  let parsed: URL
  try {
    parsed = new URL(normalizedUrl)
  } catch {
    throw invalidProtocolValue(`${context}.normalizedUrl`, 'must be an absolute URL')
  }
  const parsedHost = parsed.hostname.replace(/^\[|\]$/gu, '').toLowerCase()
  const parsedPort = parsed.port ? Number(parsed.port) : scheme === 'https' ? 443 : 80
  if (
    parsed.protocol !== `${scheme}:` ||
    parsedHost !== asciiHost ||
    parsedPort !== effectivePort ||
    parsed.origin !== origin ||
    parsed.username ||
    parsed.password ||
    parsed.search ||
    parsed.hash
  ) {
    throw invalidProtocolValue(context, 'normalized URL fields are inconsistent')
  }

  return {
    normalizedUrl,
    origin,
    scheme,
    asciiHost,
    effectivePort,
    addressClass: expectEnum(
      record.addressClass,
      BROWSER_ADDRESS_CLASSES,
      `${context}.addressClass`
    )
  }
}
