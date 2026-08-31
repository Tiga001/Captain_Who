import type {
  AgentMcpArgumentSummary,
  AgentMcpServerScope,
  AgentMcpToolApproval,
  AgentMcpToolApprovalSummary,
  AgentMcpToolInvocationIdentity,
  AgentMcpToolProvenance,
  AgentToolIdentity,
  AgentToolCall
} from '../agent'
import { parseMcpBuiltinCapabilityId } from '../mcp/parsers'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import {
  expectBoundedNonEmptyString,
  expectBuiltinCapabilityModelName,
  expectBuiltinCapabilityPackageIdentity,
  expectBuiltinCapabilityStableId,
  expectDigest,
  expectDisplayText,
  expectModelToolCallId,
  expectOpaqueRunId,
  expectServerDisplayName,
  expectUuid,
  expectUuidV4,
  expectVersionedSha256Digest
} from './shared'
export const MCP_APPROVAL_TTL_MS = 15 * 60 * 1000

/** Strict parser for durable Tool provenance crossing the Host-to-Renderer boundary. */
export function parseAgentToolIdentityForHost(value: unknown): AgentToolIdentity {
  const context = 'Agent Tool identity'
  const record = expectRecord(value, context)
  switch (record.type) {
    case 'builtin':
      expectOnlyKeys(record, ['type', 'toolName'] as const, context)
      return {
        type: 'builtin',
        toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 256)
      }
    case 'runtime_extension':
      expectOnlyKeys(record, ['type', 'extensionId', 'toolName'] as const, context)
      return {
        type: 'runtime_extension',
        extensionId: expectBoundedNonEmptyString(record.extensionId, `${context}.extensionId`, 256),
        toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 256)
      }
    case 'builtin_capability':
      expectOnlyKeys(
        record,
        [
          'type',
          'capabilityId',
          'managedMcpId',
          'packageName',
          'packageVersion',
          'upstreamCatalogDigest',
          'policyDigest',
          'manifestDigest',
          'toolId',
          'rawName',
          'modelName',
          'upstreamSchemaDigest',
          'hostOverlayDigest',
          'hostInputSchemaDigest'
        ] as const,
        context
      )
      return {
        type: 'builtin_capability',
        capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
        managedMcpId: expectBuiltinCapabilityStableId(
          record.managedMcpId,
          `${context}.managedMcpId`
        ),
        packageName: expectBuiltinCapabilityPackageIdentity(
          record.packageName,
          `${context}.packageName`,
          256
        ),
        packageVersion: expectBuiltinCapabilityPackageIdentity(
          record.packageVersion,
          `${context}.packageVersion`,
          128
        ),
        upstreamCatalogDigest: expectVersionedSha256Digest(
          record.upstreamCatalogDigest,
          `${context}.upstreamCatalogDigest`
        ),
        policyDigest: expectVersionedSha256Digest(record.policyDigest, `${context}.policyDigest`),
        manifestDigest: expectVersionedSha256Digest(
          record.manifestDigest,
          `${context}.manifestDigest`
        ),
        toolId: expectBuiltinCapabilityStableId(record.toolId, `${context}.toolId`),
        rawName: expectBuiltinCapabilityStableId(record.rawName, `${context}.rawName`),
        modelName: expectBuiltinCapabilityModelName(record.modelName, `${context}.modelName`),
        upstreamSchemaDigest: expectVersionedSha256Digest(
          record.upstreamSchemaDigest,
          `${context}.upstreamSchemaDigest`
        ),
        hostOverlayDigest: expectVersionedSha256Digest(
          record.hostOverlayDigest,
          `${context}.hostOverlayDigest`
        ),
        hostInputSchemaDigest: expectVersionedSha256Digest(
          record.hostInputSchemaDigest,
          `${context}.hostInputSchemaDigest`
        )
      }
    case 'mcp':
      expectOnlyKeys(record, ['type', 'provenance'] as const, context)
      return {
        type: 'mcp',
        provenance: parseProvenance(record.provenance, `${context}.provenance`)
      }
    case 'unregistered':
      expectOnlyKeys(record, ['type', 'toolName'] as const, context)
      return {
        type: 'unregistered',
        toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 256)
      }
    default:
      throw invalidProtocolValue(context, 'type must be a supported Tool identity')
  }
}

export function parseAgentMcpToolApproval(value: unknown): AgentMcpToolApproval {
  const context = 'MCP Tool approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'identity',
      'call',
      'summary',
      'approvalMode',
      'payloadPersistence',
      'createdAt',
      'expiresAt'
    ] as const,
    context
  )
  const identity = parseInvocationIdentity(record.identity, `${context}.identity`)
  const call = parseMcpToolCall(record.call, `${context}.call`)
  const summary = parseApprovalSummary(record.summary, `${context}.summary`)
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (expiresAt <= createdAt || expiresAt - createdAt > MCP_APPROVAL_TTL_MS) {
    throw invalidProtocolValue(context, 'expiry must be within the fixed 15 minute approval TTL')
  }
  if (call.id !== identity.callId || call.tool !== identity.provenance.modelToolName) {
    throw invalidProtocolValue(context, 'safe call projection must match invocation identity')
  }
  if (
    summary.serverId !== identity.provenance.serverId ||
    summary.rawToolName !== identity.provenance.rawToolName ||
    summary.modelToolName !== identity.provenance.modelToolName ||
    !sameScope(summary.scope, identity.provenance.scope)
  ) {
    throw invalidProtocolValue(context, 'summary must match frozen provenance')
  }
  const approvalMode = expectEnum(
    record.approvalMode,
    ['prompt', 'auto', 'deny'] as const,
    `${context}.approvalMode`
  )
  if (
    !(
      (approvalMode === 'prompt' && call.approvalStatus === 'required') ||
      (approvalMode === 'auto' && call.approvalStatus === 'approved')
    ) ||
    call.reason !== null
  ) {
    throw invalidProtocolValue(context, 'approval mode and call status must remain consistent')
  }
  return {
    identity,
    call,
    summary,
    approvalMode,
    payloadPersistence: expectEnum(
      record.payloadPersistence,
      ['process_only', 'durable_authenticated_envelope'] as const,
      `${context}.payloadPersistence`
    ),
    createdAt,
    expiresAt
  }
}

export function parseInvocationIdentity(
  value: unknown,
  context: string
): AgentMcpToolInvocationIdentity {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['actionId', 'invocationId', 'runId', 'callId', 'provenance'] as const,
    context
  )
  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const invocationId = expectUuidV4(record.invocationId, `${context}.invocationId`)
  if (actionId === invocationId) {
    throw invalidProtocolValue(context, 'actionId and invocationId must be distinct')
  }
  return {
    actionId,
    invocationId,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    provenance: parseProvenance(record.provenance, `${context}.provenance`)
  }
}

export function parseProvenance(value: unknown, context: string): AgentMcpToolProvenance {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'serverId',
      'scope',
      'rawToolName',
      'modelToolName',
      'configEpoch',
      'registryRevision',
      'configDigest',
      'catalogGeneration',
      'catalogDigest',
      'catalogSchemaDigest',
      'schemaDigest',
      'schemaNormalizerVersion'
    ] as const,
    context
  )
  return {
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    scope: parseScope(record.scope, `${context}.scope`),
    rawToolName: expectDisplayText(record.rawToolName, `${context}.rawToolName`, 1024),
    modelToolName: expectDisplayText(record.modelToolName, `${context}.modelToolName`, 64),
    configEpoch: expectUuidV4(record.configEpoch, `${context}.configEpoch`),
    registryRevision: expectSafeInteger(record.registryRevision, `${context}.registryRevision`, 0),
    configDigest: expectDigest(record.configDigest, `${context}.configDigest`),
    catalogGeneration: expectSafeInteger(
      record.catalogGeneration,
      `${context}.catalogGeneration`,
      0
    ),
    catalogDigest: expectDigest(record.catalogDigest, `${context}.catalogDigest`),
    catalogSchemaDigest: expectDigest(record.catalogSchemaDigest, `${context}.catalogSchemaDigest`),
    schemaDigest: expectDigest(record.schemaDigest, `${context}.schemaDigest`),
    schemaNormalizerVersion: expectSafeInteger(
      record.schemaNormalizerVersion,
      `${context}.schemaNormalizerVersion`,
      1
    )
  }
}

export function parseApprovalSummary(value: unknown, context: string): AgentMcpToolApprovalSummary {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'serverId',
      'serverDisplayName',
      'scope',
      'rawToolName',
      'modelToolName',
      'displayReason',
      'arguments',
      'risk',
      'external'
    ] as const,
    context
  )
  const external = expectBoolean(record.external, `${context}.external`)
  if (!external) {
    throw invalidProtocolValue(context, 'external must be true')
  }
  if (!Object.hasOwn(record, 'displayReason')) {
    throw invalidProtocolValue(context, 'displayReason is required')
  }
  return {
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    serverDisplayName: expectServerDisplayName(
      record.serverDisplayName,
      `${context}.serverDisplayName`
    ),
    scope: parseScope(record.scope, `${context}.scope`),
    rawToolName: expectDisplayText(record.rawToolName, `${context}.rawToolName`, 1024),
    modelToolName: expectDisplayText(record.modelToolName, `${context}.modelToolName`, 64),
    displayReason:
      record.displayReason === null
        ? null
        : expectDisplayText(record.displayReason, `${context}.displayReason`, 512),
    arguments: parseArgumentSummary(record.arguments, `${context}.arguments`),
    risk: expectEnum(
      record.risk,
      [
        'unknown',
        'read_only_claimed',
        'side_effects_possible',
        'destructive_claimed',
        'open_world_claimed'
      ] as const,
      `${context}.risk`
    ),
    external
  }
}

export function parseArgumentSummary(value: unknown, context: string): AgentMcpArgumentSummary {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'encodedBytes',
      'topLevelPropertyCount',
      'stringValueCount',
      'numberValueCount',
      'booleanValueCount',
      'nullValueCount',
      'objectValueCount',
      'arrayValueCount',
      'maxDepth',
      'truncated'
    ] as const,
    context
  )
  return {
    encodedBytes: expectSafeInteger(record.encodedBytes, `${context}.encodedBytes`, 0),
    topLevelPropertyCount: expectSafeInteger(
      record.topLevelPropertyCount,
      `${context}.topLevelPropertyCount`,
      0
    ),
    stringValueCount: expectSafeInteger(record.stringValueCount, `${context}.stringValueCount`, 0),
    numberValueCount: expectSafeInteger(record.numberValueCount, `${context}.numberValueCount`, 0),
    booleanValueCount: expectSafeInteger(
      record.booleanValueCount,
      `${context}.booleanValueCount`,
      0
    ),
    nullValueCount: expectSafeInteger(record.nullValueCount, `${context}.nullValueCount`, 0),
    objectValueCount: expectSafeInteger(record.objectValueCount, `${context}.objectValueCount`, 0),
    arrayValueCount: expectSafeInteger(record.arrayValueCount, `${context}.arrayValueCount`, 0),
    maxDepth: expectSafeInteger(record.maxDepth, `${context}.maxDepth`, 0),
    truncated: expectBoolean(record.truncated, `${context}.truncated`)
  }
}

export function parseMcpToolCall(value: unknown, context: string): AgentToolCall {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['id', 'tool', 'args', 'approvalStatus', 'reason'] as const, context)
  const args = expectRecord(record.args, `${context}.args`)
  expectOnlyKeys(args, [] as const, `${context}.args`)
  if (!Object.hasOwn(record, 'reason')) {
    throw invalidProtocolValue(context, 'reason is required')
  }
  const reason =
    record.reason === null ? null : expectDisplayText(record.reason, `${context}.reason`, 1024)
  return {
    id: expectModelToolCallId(record.id, `${context}.id`),
    tool: expectBoundedNonEmptyString(record.tool, `${context}.tool`, 64),
    args: {},
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    ),
    reason
  }
}

export function parseScope(value: unknown, context: string): AgentMcpServerScope {
  const record = expectRecord(value, context)
  switch (record.type) {
    case 'builtin':
    case 'user':
    case 'managed':
      expectOnlyKeys(record, ['type'] as const, context)
      return { type: record.type }
    case 'project':
      expectOnlyKeys(record, ['type', 'projectId'] as const, context)
      return {
        type: 'project',
        projectId: expectBoundedNonEmptyString(record.projectId, `${context}.projectId`, 256)
      }
    case 'plugin':
      expectOnlyKeys(record, ['type', 'pluginId'] as const, context)
      return {
        type: 'plugin',
        pluginId: expectBoundedNonEmptyString(record.pluginId, `${context}.pluginId`, 256)
      }
    default:
      throw invalidProtocolValue(context, `unknown type ${String(record.type)}`)
  }
}

export function sameScope(left: AgentMcpServerScope, right: AgentMcpServerScope): boolean {
  if (left.type !== right.type) return false
  if (left.type === 'project' && right.type === 'project') {
    return left.projectId === right.projectId
  }
  if (left.type === 'plugin' && right.type === 'plugin') {
    return left.pluginId === right.pluginId
  }
  return true
}
