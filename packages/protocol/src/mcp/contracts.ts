export const MCP_MANAGEMENT_SCHEMA_VERSION = 1 as const
export const MCP_MANAGEMENT_ERROR_CODE = -32030 as const

export const MCP_SERVER_LIST_METHOD = 'mcp.server.list' as const
export const MCP_SERVER_GET_METHOD = 'mcp.server.get' as const
export const MCP_SERVER_ADD_METHOD = 'mcp.server.add' as const
export const MCP_SERVER_UPDATE_METHOD = 'mcp.server.update' as const
export const MCP_SERVER_DELETE_METHOD = 'mcp.server.delete' as const
export const MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD =
  'mcp.server.authorizeLaunch.prepare' as const
export const MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD =
  'mcp.server.authorizeLaunch.commit' as const
export const MCP_SERVER_ENABLE_METHOD = 'mcp.server.enable' as const
export const MCP_SERVER_DISABLE_METHOD = 'mcp.server.disable' as const
export const MCP_SERVER_START_METHOD = 'mcp.server.start' as const
export const MCP_SERVER_STOP_METHOD = 'mcp.server.stop' as const
export const MCP_SERVER_RESTART_METHOD = 'mcp.server.restart' as const
export const MCP_SERVER_STATUS_METHOD = 'mcp.server.status' as const
export const MCP_CATALOG_TOOLS_METHOD = 'mcp.catalog.tools' as const
export const MCP_CATALOG_REFRESH_METHOD = 'mcp.catalog.refresh' as const
export const MCP_CHANGED_NOTIFICATION_METHOD = 'mcp.changed' as const
export const MCP_BUILTIN_CAPABILITY_LIST_METHOD = 'mcp.builtinCapability.list' as const
export const MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD = 'mcp.builtinCapability.setAllowed' as const

/** Public limits used for early UX validation. The trusted Host enforces the same or stricter. */
export const MCP_MANAGEMENT_LIMITS = {
  displayNameBytes: 256,
  pathBytes: 4096,
  arguments: 256,
  argumentBytes: 16 * 1024,
  argumentsTotalBytes: 64 * 1024,
  toolPageSize: 100,
  cursorBytes: 4096,
  safeTextBytes: 4096,
  toolDescriptionBytes: 1024,
  extensionCount: 64,
  extensionBytes: 256
} as const

export type McpTransportKind = 'stdio'
export type McpServerScopeView = 'user'
export type McpServerSourceView = 'userManual'
export type McpTrustView = 'untrusted' | 'userApproved'
export type McpApprovalModeView = 'prompt' | 'auto' | 'deny'
export type McpBuiltinCapabilityId = 'browser_automation'
export type McpManagementEntryKind = 'builtinCapability' | 'externalServer'
export type McpLaunchAuthorizationState = 'required' | 'authorized' | 'stale'
export type McpServerStateView =
  'disabled' | 'starting' | 'discovering' | 'ready' | 'stopping' | 'error' | 'backoff' | 'degraded'

export type McpCatalogCompletenessView = 'complete' | 'partial' | 'stale' | 'failed'

export interface McpSafeErrorView {
  code: string
  message: string
}

export interface McpCapabilitySnapshotView {
  tools: boolean
  resources: boolean
  prompts: boolean
  logging: boolean
  completion: boolean
}

export interface McpProtocolSnapshotView {
  protocolVersion: string
  lifecycle: 'discover' | 'initializeFallback' | 'unknown'
}

export interface McpServerStatusView {
  serverId: string
  state: McpServerStateView
  enabled: boolean
  launchAuthorizationState: McpLaunchAuthorizationState
  catalogGeneration: number
  catalogCompleteness: McpCatalogCompletenessView
  toolCount: number
  activeCallCount: number
  lastError?: McpSafeErrorView
  updatedAtMs: number
}

/**
 * Bounded, secret-free list projection. It intentionally omits executable, argv and cwd so a
 * settings list cannot accidentally become a launch-spec logging surface.
 */
export interface McpServerListItem extends McpServerStatusView {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  displayName: string
  scope: McpServerScopeView
  source: McpServerSourceView
  transport: McpTransportKind
  trust: McpTrustView
  approvalMode: McpApprovalModeView
  registryRevision: number
  configEpoch: string
  configDigest: string
}

/** Detail projection for the explicit edit screen. Environment and credentials are absent. */
export interface McpServerDetailsView extends McpServerListItem {
  executable: string
  arguments: string[]
  cwd: string
  protocol?: McpProtocolSnapshotView
  capabilities?: McpCapabilitySnapshotView
  createdAtMs: number
}

export interface McpServerListInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
}

export interface McpServerListOutput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  registryRevision: number
  servers: McpServerListItem[]
}

export interface McpBuiltinCapabilityListInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
}

export interface McpBuiltinCapabilitySetAllowedInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  capabilityId: McpBuiltinCapabilityId
  allowed: boolean
  expectedPolicyRevision: number
}

export interface McpBuiltinCapabilityListItem {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  kind: Extract<McpManagementEntryKind, 'builtinCapability'>
  capabilityId: McpBuiltinCapabilityId
  displayName: string
  description: string
  userAllowed: boolean
  policyVersion: number
  policyRevision: number
}

export interface McpBuiltinCapabilityListOutput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  revision: number
  capabilities: McpBuiltinCapabilityListItem[]
}

export interface McpBuiltinCapabilityMutationOutput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  revision: number
  capability: McpBuiltinCapabilityListItem
}

export interface McpServerIdInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  serverId: string
}

export interface McpServerCreateInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  displayName: string
  transport: McpTransportKind
  executable: string
  arguments: string[]
  cwd: string
  approvalMode: McpApprovalModeView
}

export interface McpServerMutationPrecondition {
  expectedRegistryRevision: number
  expectedConfigEpoch: string
  expectedConfigDigest: string
}

export interface McpServerUpdateInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  serverId: string
  precondition: McpServerMutationPrecondition
  displayName: string
  transport: McpTransportKind
  executable: string
  arguments: string[]
  cwd: string
  approvalMode: McpApprovalModeView
}

export interface McpServerMutationInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  serverId: string
  precondition: McpServerMutationPrecondition
}

export interface McpServerDetailsOutput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  registryRevision: number
  server: McpServerDetailsView
}

export type McpServerGetOutput = McpServerDetailsOutput
export type McpServerMutationOutput = McpServerDetailsOutput

export interface McpLaunchAuthorizationPreview {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  authorizationId: string
  expiresAtMs: number
  serverId: string
  displayName: string
  executable: string
  arguments: string[]
  cwd: string
  launchSpecDigest: string
  precondition: McpServerMutationPrecondition
}

export type McpLaunchAuthorizationPrepareOutput = McpLaunchAuthorizationPreview

export interface McpLaunchAuthorizationCommitInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  authorizationId: string
  precondition: McpServerMutationPrecondition
}

export interface McpLaunchAuthorizationResult {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  authorized: boolean
  server: McpServerDetailsView
}

export interface McpCatalogToolsPageInput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  serverId: string
  cursor?: string
  limit: number
}

export interface McpToolSummaryView {
  serverId: string
  rawName: string
  modelName: string
  routable: boolean
  disabled: boolean
  schemaDigestPrefix: string
  description: string
  descriptionTruncated: boolean
  diagnosticCodes: string[]
  catalogGeneration: number
  catalogCompleteness: McpCatalogCompletenessView
}

export interface McpCatalogToolsPageOutput {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  serverId: string
  catalogGeneration: number
  catalogCompleteness: McpCatalogCompletenessView
  tools: McpToolSummaryView[]
  nextCursor?: string
}

export type McpManagementOperation =
  | 'listBuiltinCapabilities'
  | 'setBuiltinCapabilityAllowed'
  | 'list'
  | 'get'
  | 'add'
  | 'update'
  | 'delete'
  | 'prepareLaunchAuthorization'
  | 'commitLaunchAuthorization'
  | 'enable'
  | 'disable'
  | 'start'
  | 'stop'
  | 'restart'
  | 'status'
  | 'listTools'
  | 'refreshCatalog'

export type McpManagementErrorCode =
  | 'invalidInput'
  | 'notFound'
  | 'conflict'
  | 'authorizationRequired'
  | 'authorizationStale'
  | 'policyDenied'
  | 'invalidState'
  | 'serverError'
  | 'timeout'
  | 'cleanupIncomplete'
  | 'internalSafeError'

export type McpManagementRecovery =
  'fixInput' | 'refresh' | 'requestLaunchAuthorization' | 'retry' | 'stopAndRetry' | 'doNotRetry'

export interface McpManagementErrorData {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  type: 'mcpManagement'
  operation: McpManagementOperation
  code: McpManagementErrorCode
  recovery: McpManagementRecovery
  message: string
  serverId?: string
  currentRegistryRevision?: number
}

export type McpChangedKind =
  | 'added'
  | 'updated'
  | 'deleted'
  | 'authorizationChanged'
  | 'enablementChanged'
  | 'stateChanged'
  | 'catalogChanged'
  | 'resyncRequired'

interface McpChangedNotificationBase {
  schemaVersion: typeof MCP_MANAGEMENT_SCHEMA_VERSION
  /** UUIDv4 generated once for the lifetime of one core-server process. */
  sourceEpoch: string
  sequence: number
  registryRevision: number
}

export type McpChangedNotification =
  | (McpChangedNotificationBase & {
      kind: 'stateChanged'
      serverId: string
      state: McpServerStateView
    })
  | (McpChangedNotificationBase & {
      kind: Exclude<McpChangedKind, 'stateChanged' | 'resyncRequired'>
      serverId: string
      state?: never
    })
  | (McpChangedNotificationBase & {
      /** Loss was detected; consumers must discard incremental state and perform a full refresh. */
      kind: 'resyncRequired'
      serverId?: never
      state?: never
    })
