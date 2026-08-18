import {
  MCP_MANAGEMENT_LIMITS,
  MCP_MANAGEMENT_SCHEMA_VERSION,
  type McpApprovalModeView,
  type McpBuiltinCapabilityId,
  type McpBuiltinCapabilityListInput,
  type McpBuiltinCapabilityListItem,
  type McpBuiltinCapabilityListOutput,
  type McpBuiltinCapabilityMutationOutput,
  type McpBuiltinCapabilitySetAllowedInput,
  type McpCapabilitySnapshotView,
  type McpCatalogCompletenessView,
  type McpCatalogToolsPageInput,
  type McpCatalogToolsPageOutput,
  type McpChangedNotification,
  type McpLaunchAuthorizationCommitInput,
  type McpLaunchAuthorizationPreview,
  type McpLaunchAuthorizationResult,
  type McpManagementErrorData,
  type McpProtocolSnapshotView,
  type McpSafeErrorView,
  type McpServerCreateInput,
  type McpServerDetailsOutput,
  type McpServerDetailsView,
  type McpServerIdInput,
  type McpServerListInput,
  type McpServerListItem,
  type McpServerListOutput,
  type McpServerMutationInput,
  type McpServerMutationPrecondition,
  type McpServerUpdateInput,
  type McpToolSummaryView
} from './contracts'
import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue
} from '../skills/validation'

const SERVER_STATES = [
  'disabled',
  'starting',
  'discovering',
  'ready',
  'stopping',
  'error',
  'backoff',
  'degraded'
] as const
const CATALOG_COMPLETENESS = ['complete', 'partial', 'stale', 'failed'] as const
const MANAGEMENT_OPERATIONS = [
  'listBuiltinCapabilities',
  'setBuiltinCapabilityAllowed',
  'list',
  'get',
  'add',
  'update',
  'delete',
  'prepareLaunchAuthorization',
  'commitLaunchAuthorization',
  'enable',
  'disable',
  'start',
  'stop',
  'restart',
  'status',
  'listTools',
  'refreshCatalog'
] as const
const MANAGEMENT_ERROR_CODES = [
  'invalidInput',
  'notFound',
  'conflict',
  'authorizationRequired',
  'authorizationStale',
  'policyDenied',
  'invalidState',
  'serverError',
  'timeout',
  'cleanupIncomplete',
  'internalSafeError'
] as const
const MANAGEMENT_RECOVERIES = [
  'fixInput',
  'refresh',
  'requestLaunchAuthorization',
  'retry',
  'stopAndRetry',
  'doNotRetry'
] as const
const CHANGED_KINDS = [
  'added',
  'updated',
  'deleted',
  'authorizationChanged',
  'enablementChanged',
  'stateChanged',
  'catalogChanged',
  'resyncRequired'
] as const

const DIGEST_PATTERN = /^[0-9a-f]{64}$/
const DIGEST_PREFIX_PATTERN = /^[0-9a-f]{12}$/
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const UUID_V4_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const SAFE_IDENTIFIER_PATTERN = /^[a-zA-Z][a-zA-Z0-9_.-]{0,127}$/

export function parseMcpServerListInput(value: unknown): McpServerListInput {
  const context = 'MCP server list request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return { schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION }
}

export function parseMcpBuiltinCapabilityListInput(value: unknown): McpBuiltinCapabilityListInput {
  const context = 'built-in MCP capability list request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return { schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION }
}

/** Canonical Host-owned capability identity shared by management and Agent contracts. */
export function parseMcpBuiltinCapabilityId(
  value: unknown,
  context = 'built-in MCP capability id'
): McpBuiltinCapabilityId {
  return expectEnum(value, ['browser_automation'] as const, context)
}

export function parseMcpBuiltinCapabilitySetAllowedInput(
  value: unknown
): McpBuiltinCapabilitySetAllowedInput {
  const context = 'built-in MCP capability policy request'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'capabilityId', 'allowed', 'expectedPolicyRevision'] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
    allowed: expectBoolean(record.allowed, `${context}.allowed`),
    expectedPolicyRevision: expectSafeInteger(
      record.expectedPolicyRevision,
      `${context}.expectedPolicyRevision`,
      0
    )
  }
}

export function parseMcpServerIdInput(value: unknown): McpServerIdInput {
  const context = 'MCP server identity request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'serverId'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: expectUuid(record.serverId, `${context}.serverId`)
  }
}

export function parseMcpServerCreateInput(value: unknown): McpServerCreateInput {
  const context = 'MCP server create request'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'displayName',
      'transport',
      'executable',
      'arguments',
      'cwd',
      'approvalMode'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    displayName: expectDisplayName(record.displayName, `${context}.displayName`),
    transport: expectEnum(record.transport, ['stdio'] as const, `${context}.transport`),
    executable: expectAbsolutePath(record.executable, `${context}.executable`),
    arguments: expectArguments(record.arguments, `${context}.arguments`),
    cwd: expectAbsolutePath(record.cwd, `${context}.cwd`),
    approvalMode: parseApprovalMode(record.approvalMode, `${context}.approvalMode`)
  }
}

export function parseMcpServerUpdateInput(value: unknown): McpServerUpdateInput {
  const context = 'MCP server update request'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'serverId',
      'precondition',
      'displayName',
      'transport',
      'executable',
      'arguments',
      'cwd',
      'approvalMode'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    precondition: parseMcpServerMutationPrecondition(record.precondition),
    displayName: expectDisplayName(record.displayName, `${context}.displayName`),
    transport: expectEnum(record.transport, ['stdio'] as const, `${context}.transport`),
    executable: expectAbsolutePath(record.executable, `${context}.executable`),
    arguments: expectArguments(record.arguments, `${context}.arguments`),
    cwd: expectAbsolutePath(record.cwd, `${context}.cwd`),
    approvalMode: parseApprovalMode(record.approvalMode, `${context}.approvalMode`)
  }
}

export function parseMcpServerMutationInput(value: unknown): McpServerMutationInput {
  const context = 'MCP server mutation request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'serverId', 'precondition'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    precondition: parseMcpServerMutationPrecondition(record.precondition)
  }
}

export function parseMcpLaunchAuthorizationCommitInput(
  value: unknown
): McpLaunchAuthorizationCommitInput {
  const context = 'MCP launch authorization commit request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'authorizationId', 'precondition'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    authorizationId: expectUuidV4(record.authorizationId, `${context}.authorizationId`),
    precondition: parseMcpServerMutationPrecondition(record.precondition)
  }
}

export function parseMcpCatalogToolsPageInput(value: unknown): McpCatalogToolsPageInput {
  const context = 'MCP Catalog tools request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'serverId', 'cursor', 'limit'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const cursor =
    record.cursor === undefined
      ? undefined
      : expectBoundedString(record.cursor, `${context}.cursor`, MCP_MANAGEMENT_LIMITS.cursorBytes)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    ...(cursor === undefined ? {} : { cursor }),
    limit: expectSafeIntegerInRange(
      record.limit,
      `${context}.limit`,
      1,
      MCP_MANAGEMENT_LIMITS.toolPageSize
    )
  }
}

export function parseMcpServerListOutput(value: unknown): McpServerListOutput {
  const context = 'MCP server list response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'registryRevision', 'servers'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const servers = expectArray(record.servers, `${context}.servers`)
  if (servers.length > 1024) {
    throw invalidProtocolValue(context, 'server count exceeded 1024')
  }
  const registryRevision = expectSafeInteger(
    record.registryRevision,
    `${context}.registryRevision`,
    0
  )
  const parsed = servers.map((server, index) =>
    parseMcpServerListItem(server, `${context}.servers[${index}]`)
  )
  if (new Set(parsed.map((server) => server.serverId)).size !== parsed.length) {
    throw invalidProtocolValue(context, 'serverId values must be unique')
  }
  if (parsed.some((server) => server.registryRevision > registryRevision)) {
    throw invalidProtocolValue(context, 'server registryRevision must not exceed response revision')
  }
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    registryRevision,
    servers: parsed
  }
}

export function parseMcpBuiltinCapabilityListOutput(
  value: unknown
): McpBuiltinCapabilityListOutput {
  const context = 'built-in MCP capability list response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'revision', 'capabilities'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const revision = expectSafeInteger(record.revision, `${context}.revision`, 0)
  const capabilities = expectArray(record.capabilities, `${context}.capabilities`)
  if (capabilities.length > 64) {
    throw invalidProtocolValue(context, 'capability count exceeded 64')
  }
  const parsed = capabilities.map((capability, index) =>
    parseMcpBuiltinCapabilityListItem(capability, `${context}.capabilities[${index}]`)
  )
  if (new Set(parsed.map((capability) => capability.capabilityId)).size !== parsed.length) {
    throw invalidProtocolValue(context, 'capabilityId values must be unique')
  }
  if (parsed.some((capability) => capability.policyRevision > revision)) {
    throw invalidProtocolValue(context, 'policyRevision must not exceed response revision')
  }
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    revision,
    capabilities: parsed
  }
}

export function parseMcpBuiltinCapabilityMutationOutput(
  value: unknown
): McpBuiltinCapabilityMutationOutput {
  const context = 'built-in MCP capability mutation response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'revision', 'capability'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const revision = expectSafeInteger(record.revision, `${context}.revision`, 0)
  const capability = parseMcpBuiltinCapabilityListItem(record.capability, `${context}.capability`)
  if (capability.policyRevision > revision) {
    throw invalidProtocolValue(context, 'policyRevision must not exceed response revision')
  }
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    revision,
    capability
  }
}

export function parseMcpServerDetailsOutput(value: unknown): McpServerDetailsOutput {
  const context = 'MCP server details response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'registryRevision', 'server'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const registryRevision = expectSafeInteger(
    record.registryRevision,
    `${context}.registryRevision`,
    0
  )
  const server = parseMcpServerDetailsView(record.server, `${context}.server`)
  if (server.registryRevision !== registryRevision) {
    throw invalidProtocolValue(context, 'server registryRevision must match response revision')
  }
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    registryRevision,
    server
  }
}

export const parseMcpServerGetOutput = parseMcpServerDetailsOutput
export const parseMcpServerMutationOutput = parseMcpServerDetailsOutput

export function parseMcpLaunchAuthorizationPreview(value: unknown): McpLaunchAuthorizationPreview {
  const context = 'MCP launch authorization preview'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'authorizationId',
      'expiresAtMs',
      'serverId',
      'displayName',
      'executable',
      'arguments',
      'cwd',
      'launchSpecDigest',
      'precondition'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    authorizationId: expectUuidV4(record.authorizationId, `${context}.authorizationId`),
    expiresAtMs: expectSafeInteger(record.expiresAtMs, `${context}.expiresAtMs`, 1),
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    displayName: expectDisplayName(record.displayName, `${context}.displayName`),
    executable: expectAbsolutePath(record.executable, `${context}.executable`),
    arguments: expectArguments(record.arguments, `${context}.arguments`),
    cwd: expectAbsolutePath(record.cwd, `${context}.cwd`),
    launchSpecDigest: expectDigest(record.launchSpecDigest, `${context}.launchSpecDigest`),
    precondition: parseMcpServerMutationPrecondition(record.precondition)
  }
}

export const parseMcpLaunchAuthorizationPrepareOutput = parseMcpLaunchAuthorizationPreview

export function parseMcpLaunchAuthorizationResult(value: unknown): McpLaunchAuthorizationResult {
  const context = 'MCP launch authorization result'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'authorized', 'server'] as const, context)
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    authorized: expectBoolean(record.authorized, `${context}.authorized`),
    server: parseMcpServerDetailsView(record.server, `${context}.server`)
  }
}

export function parseMcpCatalogToolsPageOutput(value: unknown): McpCatalogToolsPageOutput {
  const context = 'MCP Catalog tools response'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'serverId',
      'catalogGeneration',
      'catalogCompleteness',
      'tools',
      'nextCursor'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const tools = expectArray(record.tools, `${context}.tools`)
  if (tools.length > MCP_MANAGEMENT_LIMITS.toolPageSize) {
    throw invalidProtocolValue(context, `tool page exceeded ${MCP_MANAGEMENT_LIMITS.toolPageSize}`)
  }
  const parsed = tools.map((tool, index) =>
    parseMcpToolSummaryView(tool, `${context}.tools[${index}]`)
  )
  const serverId = expectUuid(record.serverId, `${context}.serverId`)
  const catalogGeneration = expectSafeInteger(
    record.catalogGeneration,
    `${context}.catalogGeneration`,
    0
  )
  const catalogCompleteness = parseCatalogCompleteness(
    record.catalogCompleteness,
    `${context}.catalogCompleteness`
  )
  if (
    parsed.some(
      (tool) =>
        tool.serverId !== serverId ||
        tool.catalogGeneration !== catalogGeneration ||
        tool.catalogCompleteness !== catalogCompleteness
    )
  ) {
    throw invalidProtocolValue(context, 'tool provenance must match page provenance')
  }
  const nextCursor =
    record.nextCursor === undefined
      ? undefined
      : expectBoundedString(
          record.nextCursor,
          `${context}.nextCursor`,
          MCP_MANAGEMENT_LIMITS.cursorBytes
        )
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId,
    catalogGeneration,
    catalogCompleteness,
    tools: parsed,
    ...(nextCursor === undefined ? {} : { nextCursor })
  }
}

export function parseMcpManagementErrorData(value: unknown): McpManagementErrorData {
  const context = 'MCP management error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'type',
      'operation',
      'code',
      'recovery',
      'message',
      'serverId',
      'currentRegistryRevision'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  if (record.type !== 'mcpManagement') {
    throw invalidProtocolValue(context, 'type must be mcpManagement')
  }
  const serverId =
    record.serverId === undefined ? undefined : expectUuid(record.serverId, `${context}.serverId`)
  const currentRegistryRevision =
    record.currentRegistryRevision === undefined
      ? undefined
      : expectSafeInteger(record.currentRegistryRevision, `${context}.currentRegistryRevision`, 0)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    type: 'mcpManagement',
    operation: expectEnum(record.operation, MANAGEMENT_OPERATIONS, `${context}.operation`),
    code: expectEnum(record.code, MANAGEMENT_ERROR_CODES, `${context}.code`),
    recovery: expectEnum(record.recovery, MANAGEMENT_RECOVERIES, `${context}.recovery`),
    message: expectDisplayText(
      record.message,
      `${context}.message`,
      MCP_MANAGEMENT_LIMITS.safeTextBytes,
      false
    ),
    ...(serverId === undefined ? {} : { serverId }),
    ...(currentRegistryRevision === undefined ? {} : { currentRegistryRevision })
  }
}

export function parseMcpChangedNotification(value: unknown): McpChangedNotification {
  const context = 'MCP changed notification'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'sourceEpoch',
      'sequence',
      'registryRevision',
      'kind',
      'serverId',
      'state'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const sourceEpoch = expectUuidV4(record.sourceEpoch, `${context}.sourceEpoch`)
  const sequence = expectSafeInteger(record.sequence, `${context}.sequence`, 0)
  const registryRevision = expectSafeInteger(
    record.registryRevision,
    `${context}.registryRevision`,
    0
  )
  const kind = expectEnum(record.kind, CHANGED_KINDS, `${context}.kind`)
  if (kind === 'resyncRequired') {
    if (record.serverId !== undefined || record.state !== undefined) {
      throw invalidProtocolValue(context, 'resyncRequired must not claim a Server or state')
    }
    return {
      schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
      sourceEpoch,
      sequence,
      registryRevision,
      kind
    }
  }
  const serverId = expectUuid(record.serverId, `${context}.serverId`)
  if (kind === 'stateChanged') {
    return {
      schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
      sourceEpoch,
      sequence,
      registryRevision,
      kind,
      serverId,
      state: expectEnum(record.state, SERVER_STATES, `${context}.state`)
    }
  }
  if (record.state !== undefined) {
    throw invalidProtocolValue(context, 'state is only valid for stateChanged')
  }
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    sourceEpoch,
    sequence,
    registryRevision,
    kind,
    serverId
  }
}

function parseMcpServerMutationPrecondition(value: unknown): McpServerMutationPrecondition {
  const context = 'MCP server mutation precondition'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['expectedRegistryRevision', 'expectedConfigEpoch', 'expectedConfigDigest'] as const,
    context
  )
  return {
    expectedRegistryRevision: expectSafeInteger(
      record.expectedRegistryRevision,
      `${context}.expectedRegistryRevision`,
      1
    ),
    expectedConfigEpoch: expectUuidV4(record.expectedConfigEpoch, `${context}.expectedConfigEpoch`),
    expectedConfigDigest: expectDigest(
      record.expectedConfigDigest,
      `${context}.expectedConfigDigest`
    )
  }
}

function parseMcpBuiltinCapabilityListItem(
  value: unknown,
  context: string
): McpBuiltinCapabilityListItem {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'kind',
      'capabilityId',
      'displayName',
      'description',
      'userAllowed',
      'policyVersion',
      'policyRevision'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    kind: expectEnum(record.kind, ['builtinCapability'] as const, `${context}.kind`),
    capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
    displayName: expectDisplayText(
      record.displayName,
      `${context}.displayName`,
      MCP_MANAGEMENT_LIMITS.displayNameBytes,
      false
    ),
    description: expectDisplayText(
      record.description,
      `${context}.description`,
      MCP_MANAGEMENT_LIMITS.safeTextBytes,
      false
    ),
    userAllowed: expectBoolean(record.userAllowed, `${context}.userAllowed`),
    policyVersion: expectSafeInteger(record.policyVersion, `${context}.policyVersion`, 1),
    policyRevision: expectSafeInteger(record.policyRevision, `${context}.policyRevision`, 0)
  }
}

function parseMcpServerListItem(value: unknown, context: string): McpServerListItem {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'serverId',
      'displayName',
      'scope',
      'source',
      'transport',
      'enabled',
      'trust',
      'approvalMode',
      'launchAuthorizationState',
      'state',
      'registryRevision',
      'configEpoch',
      'configDigest',
      'catalogGeneration',
      'catalogCompleteness',
      'toolCount',
      'activeCallCount',
      'lastError',
      'updatedAtMs'
    ] as const,
    context
  )
  expectSchemaVersion(record, MCP_MANAGEMENT_SCHEMA_VERSION, context)
  const lastError =
    record.lastError === undefined
      ? undefined
      : parseSafeError(record.lastError, `${context}.lastError`)
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    displayName: expectDisplayName(record.displayName, `${context}.displayName`),
    scope: expectEnum(record.scope, ['user'] as const, `${context}.scope`),
    source: expectEnum(record.source, ['userManual'] as const, `${context}.source`),
    transport: expectEnum(record.transport, ['stdio'] as const, `${context}.transport`),
    enabled: expectBoolean(record.enabled, `${context}.enabled`),
    trust: expectEnum(record.trust, ['untrusted', 'userApproved'] as const, `${context}.trust`),
    approvalMode: parseApprovalMode(record.approvalMode, `${context}.approvalMode`),
    launchAuthorizationState: expectEnum(
      record.launchAuthorizationState,
      ['required', 'authorized', 'stale'] as const,
      `${context}.launchAuthorizationState`
    ),
    state: expectEnum(record.state, SERVER_STATES, `${context}.state`),
    registryRevision: expectSafeInteger(record.registryRevision, `${context}.registryRevision`, 0),
    configEpoch: expectUuidV4(record.configEpoch, `${context}.configEpoch`),
    configDigest: expectDigest(record.configDigest, `${context}.configDigest`),
    catalogGeneration: expectSafeInteger(
      record.catalogGeneration,
      `${context}.catalogGeneration`,
      0
    ),
    catalogCompleteness: parseCatalogCompleteness(
      record.catalogCompleteness,
      `${context}.catalogCompleteness`
    ),
    toolCount: expectSafeInteger(record.toolCount, `${context}.toolCount`, 0),
    activeCallCount: expectSafeInteger(record.activeCallCount, `${context}.activeCallCount`, 0),
    ...(lastError === undefined ? {} : { lastError }),
    updatedAtMs: expectSafeInteger(record.updatedAtMs, `${context}.updatedAtMs`, 0)
  }
}

function parseMcpServerDetailsView(value: unknown, context: string): McpServerDetailsView {
  const record = expectRecord(value, context)
  const detailKeys = [
    'executable',
    'arguments',
    'cwd',
    'protocol',
    'capabilities',
    'createdAtMs'
  ] as const
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'serverId',
      'displayName',
      'scope',
      'source',
      'transport',
      'enabled',
      'trust',
      'approvalMode',
      'launchAuthorizationState',
      'state',
      'registryRevision',
      'configEpoch',
      'configDigest',
      'catalogGeneration',
      'catalogCompleteness',
      'toolCount',
      'activeCallCount',
      'lastError',
      'updatedAtMs',
      ...detailKeys
    ] as const,
    context
  )
  const summary = parseMcpServerListItem(
    Object.fromEntries(
      Object.entries(record).filter(([key]) => !detailKeys.includes(key as never))
    ),
    context
  )
  const protocol =
    record.protocol === undefined
      ? undefined
      : parseProtocolSnapshot(record.protocol, `${context}.protocol`)
  const capabilities =
    record.capabilities === undefined
      ? undefined
      : parseCapabilities(record.capabilities, `${context}.capabilities`)
  return {
    ...summary,
    executable: expectAbsolutePath(record.executable, `${context}.executable`),
    arguments: expectArguments(record.arguments, `${context}.arguments`),
    cwd: expectAbsolutePath(record.cwd, `${context}.cwd`),
    ...(protocol === undefined ? {} : { protocol }),
    ...(capabilities === undefined ? {} : { capabilities }),
    createdAtMs: expectSafeInteger(record.createdAtMs, `${context}.createdAtMs`, 0)
  }
}

function parseSafeError(value: unknown, context: string): McpSafeErrorView {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['code', 'message'] as const, context)
  return {
    code: expectSafeIdentifier(record.code, `${context}.code`),
    message: expectDisplayText(
      record.message,
      `${context}.message`,
      MCP_MANAGEMENT_LIMITS.safeTextBytes,
      false
    )
  }
}

function parseCatalogCompleteness(value: unknown, context: string): McpCatalogCompletenessView {
  return expectEnum(value, CATALOG_COMPLETENESS, context)
}

function parseProtocolSnapshot(value: unknown, context: string): McpProtocolSnapshotView {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['protocolVersion', 'lifecycle'] as const, context)
  return {
    protocolVersion: expectDisplayText(record.protocolVersion, `${context}.protocolVersion`, 64),
    lifecycle: expectEnum(
      record.lifecycle,
      ['discover', 'initializeFallback', 'unknown'] as const,
      `${context}.lifecycle`
    )
  }
}

function parseCapabilities(value: unknown, context: string): McpCapabilitySnapshotView {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['tools', 'resources', 'prompts', 'logging', 'completion'] as const,
    context
  )
  return {
    tools: expectBoolean(record.tools, `${context}.tools`),
    resources: expectBoolean(record.resources, `${context}.resources`),
    prompts: expectBoolean(record.prompts, `${context}.prompts`),
    logging: expectBoolean(record.logging, `${context}.logging`),
    completion: expectBoolean(record.completion, `${context}.completion`)
  }
}

function parseMcpToolSummaryView(value: unknown, context: string): McpToolSummaryView {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'serverId',
      'rawName',
      'modelName',
      'routable',
      'disabled',
      'schemaDigestPrefix',
      'description',
      'descriptionTruncated',
      'diagnosticCodes',
      'catalogGeneration',
      'catalogCompleteness'
    ] as const,
    context
  )
  const diagnostics = expectArray(record.diagnosticCodes, `${context}.diagnosticCodes`)
  if (diagnostics.length > 32) {
    throw invalidProtocolValue(context, 'diagnostic code count exceeded 32')
  }
  const diagnosticCodes = diagnostics.map((diagnostic, index) =>
    expectSafeIdentifier(diagnostic, `${context}.diagnosticCodes[${index}]`)
  )
  if (new Set(diagnosticCodes).size !== diagnosticCodes.length) {
    throw invalidProtocolValue(context, 'diagnostic codes must be unique')
  }
  return {
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    rawName: expectDisplayText(record.rawName, `${context}.rawName`, 1024),
    modelName: expectDisplayText(record.modelName, `${context}.modelName`, 64),
    routable: expectBoolean(record.routable, `${context}.routable`),
    disabled: expectBoolean(record.disabled, `${context}.disabled`),
    schemaDigestPrefix: expectDigestPrefix(
      record.schemaDigestPrefix,
      `${context}.schemaDigestPrefix`
    ),
    description: expectDisplayText(
      record.description,
      `${context}.description`,
      MCP_MANAGEMENT_LIMITS.toolDescriptionBytes
    ),
    descriptionTruncated: expectBoolean(
      record.descriptionTruncated,
      `${context}.descriptionTruncated`
    ),
    diagnosticCodes,
    catalogGeneration: expectSafeInteger(
      record.catalogGeneration,
      `${context}.catalogGeneration`,
      0
    ),
    catalogCompleteness: parseCatalogCompleteness(
      record.catalogCompleteness,
      `${context}.catalogCompleteness`
    )
  }
}

function parseApprovalMode(value: unknown, context: string): McpApprovalModeView {
  return expectEnum(value, ['prompt', 'auto', 'deny'] as const, context)
}

function expectDisplayName(value: unknown, context: string): string {
  const result = expectDisplayText(value, context, MCP_MANAGEMENT_LIMITS.displayNameBytes, false)
  if (result.trim().length === 0) {
    throw invalidProtocolValue(context, 'must not be blank')
  }
  if (/[\r\n\t]/.test(result)) {
    throw invalidProtocolValue(context, 'must be a single line')
  }
  return result
}

function expectAbsolutePath(value: unknown, context: string): string {
  const path = expectBoundedString(value, context, MCP_MANAGEMENT_LIMITS.pathBytes)
  if (path.length === 0) {
    throw invalidProtocolValue(context, 'must not be empty')
  }
  if (path.includes('\0')) {
    throw invalidProtocolValue(context, 'must not contain NUL')
  }
  if (!/^(?:\/|[a-zA-Z]:[\\/]|\\\\)/.test(path)) {
    throw invalidProtocolValue(context, 'must be an absolute path')
  }
  return path
}

function expectArguments(value: unknown, context: string): string[] {
  const argumentsValue = expectArray(value, context)
  if (argumentsValue.length > MCP_MANAGEMENT_LIMITS.arguments) {
    throw invalidProtocolValue(
      context,
      `argument count exceeded ${MCP_MANAGEMENT_LIMITS.arguments}`
    )
  }
  let totalBytes = 0
  const parsed = argumentsValue.map((argument, index) => {
    const result = expectBoundedString(
      argument,
      `${context}[${index}]`,
      MCP_MANAGEMENT_LIMITS.argumentBytes
    )
    if (result.includes('\0')) {
      throw invalidProtocolValue(`${context}[${index}]`, 'must not contain NUL')
    }
    totalBytes += utf8Bytes(result)
    return result
  })
  if (totalBytes > MCP_MANAGEMENT_LIMITS.argumentsTotalBytes) {
    throw invalidProtocolValue(
      context,
      `encoded arguments exceeded ${MCP_MANAGEMENT_LIMITS.argumentsTotalBytes} bytes`
    )
  }
  return parsed
}

function expectDisplayText(
  value: unknown,
  context: string,
  maxBytes: number,
  allowEmpty = true
): string {
  const result = expectBoundedString(value, context, maxBytes)
  if (!allowEmpty && result.length === 0) {
    throw invalidProtocolValue(context, 'must not be empty')
  }
  if (hasDisallowedDisplayControl(result)) {
    throw invalidProtocolValue(context, 'contains disallowed control characters')
  }
  return result
}

function hasDisallowedDisplayControl(value: string): boolean {
  return /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value)
}

function expectSafeIdentifier(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!SAFE_IDENTIFIER_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a bounded safe identifier')
  }
  return result
}

function expectBoundedString(value: unknown, context: string, maxBytes: number): string {
  const result = expectString(value, context)
  if (utf8Bytes(result) > maxBytes) {
    throw invalidProtocolValue(context, `exceeded ${maxBytes} UTF-8 bytes`)
  }
  return result
}

function expectUuid(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!UUID_PATTERN.test(result) || result === '00000000-0000-0000-0000-000000000000') {
    throw invalidProtocolValue(context, 'expected a canonical non-nil lower-case UUID')
  }
  return result
}

function expectUuidV4(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!UUID_V4_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a canonical lower-case UUIDv4')
  }
  return result
}

function expectDigest(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!DIGEST_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a lower-case SHA-256 digest')
  }
  return result
}

function expectDigestPrefix(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!DIGEST_PREFIX_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a 12-character lower-case digest prefix')
  }
  return result
}

function expectSafeIntegerInRange(
  value: unknown,
  context: string,
  minimum: number,
  maximum: number
): number {
  if (
    typeof value !== 'number' ||
    !Number.isSafeInteger(value) ||
    value < minimum ||
    value > maximum
  ) {
    throw invalidProtocolValue(context, `expected an integer between ${minimum} and ${maximum}`)
  }
  return value
}

function utf8Bytes(value: string): number {
  return new TextEncoder().encode(value).byteLength
}
