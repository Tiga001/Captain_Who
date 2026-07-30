import {
  MCP_MANAGEMENT_SCHEMA_VERSION,
  type McpApprovalModeView,
  type McpServerCreateInput,
  type McpServerListItem,
  type McpServerMutationInput,
  type McpServerMutationPrecondition
} from '@mycopilot/protocol'

export interface McpServerDraft {
  approvalMode: McpApprovalModeView
  arguments: string[]
  cwd: string
  displayName: string
  executable: string
}

export function toMcpCreateInput(draft: McpServerDraft): McpServerCreateInput {
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    displayName: draft.displayName,
    transport: 'stdio',
    executable: draft.executable,
    arguments: [...draft.arguments],
    cwd: draft.cwd,
    approvalMode: draft.approvalMode
  }
}

export function toMcpPrecondition(server: McpServerListItem): McpServerMutationPrecondition {
  return {
    expectedRegistryRevision: server.registryRevision,
    expectedConfigEpoch: server.configEpoch,
    expectedConfigDigest: server.configDigest
  }
}

export function toMcpMutationInput(server: McpServerListItem): McpServerMutationInput {
  return {
    schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
    serverId: server.serverId,
    precondition: toMcpPrecondition(server)
  }
}
