import type { McpBuiltinCapabilityId } from '../mcp/contracts'
import type { AgentCommandActionProjection } from './command'
import type { AgentToolCall } from './conversation'
import type { AgentApprovalStatus, AgentMcpToolApproval } from './core'
import type { AgentFileChangeProposal } from './fileChange'
import type { AgentOfficeOperationRequest } from './office'
import type {
  AgentSkillInstallationRequest,
  AgentSkillMaterializationRequest,
  AgentSkillScriptRequest
} from './skills'

/**
 * Controls whether execution of Host-authenticated built-in Skills and capabilities requires a
 * human click. auto_approve only skips the prompt; it does not widen read, write, command, path,
 * manifest, revision, digest, or runtime safety authority.
 */
export type AgentBuiltinExecutionPermission = 'require_approval' | 'auto_approve'

/**
 * Renderer-safe projection of one exact request to activate a Host-owned capability.
 *
 * The Host deliberately excludes managed Server, transport, executable, credential, manifest
 * contents and reviewed Tool lists. Renderer must treat every string as untrusted plain text.
 */
export interface AgentBuiltinCapabilityActivationApproval {
  actionId: string
  activationId: string
  runId: string
  callId: string
  capabilityId: McpBuiltinCapabilityId
  displayName: string
  reason: string
  manifestDigest: string
  policyRevision: number
  /** Unix timestamp in seconds. */
  createdAt: number
  /** Unix timestamp in seconds. */
  expiresAt: number
  approvalStatus: AgentApprovalStatus
}

/** Host-classified address class for one frozen browser destination. */
export type AgentBrowserAddressClass =
  'public' | 'loopback' | 'private' | 'link_local' | 'cloud_metadata' | 'unresolved'

/** Risks that can require a task-scoped decision without weakening the Host boundary. */
export type AgentBrowserRiskKind =
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

export type AgentBrowserRiskTrigger =
  'tool_argument' | 'main_frame' | 'redirect' | 'new_window' | 'subresource' | 'upload' | 'download'

export type AgentBrowserReviewedToolName =
  | 'browser_navigate'
  | 'browser_snapshot'
  | 'browser_find'
  | 'browser_click'
  | 'browser_type'
  | 'browser_fill_form'
  | 'browser_press_key'
  | 'browser_tabs'
  | 'browser_wait_for'
  | 'browser_close'

/** Renderer-safe identity for the exact destination frozen by the Host. */
export interface AgentBrowserDestinationIdentity {
  normalizedUrl: string
  origin: string
  scheme: 'http' | 'https'
  asciiHost: string
  effectivePort: number
  addressClass: AgentBrowserAddressClass
}

/**
 * Renderer-safe projection of one exact browser boundary decision.
 *
 * Capability grants, request headers, credentials, cookies, target/debugger identifiers and raw
 * network data are deliberately absent. Every string remains untrusted plain text.
 */
export interface AgentBrowserRiskApproval {
  schemaVersion: 1
  actionId: string
  riskApprovalId: string
  runId: string
  callId: string
  capabilityId: McpBuiltinCapabilityId
  capabilityActivationId: string
  displayName: string
  reason: string
  destination: AgentBrowserDestinationIdentity
  trigger: AgentBrowserRiskTrigger
  triggerToolName: AgentBrowserReviewedToolName
  riskKinds: AgentBrowserRiskKind[]
  manifestDigest: string
  policyRevision: number
  /** Unix timestamp in seconds. */
  createdAt: number
  /** Unix timestamp in seconds. */
  expiresAt: number
  approvalStatus: AgentApprovalStatus
}

export type AgentBuiltinMcpToolRiskKind =
  | 'file_read'
  | 'file_write'
  | 'file_upload'
  | 'file_download'
  | 'cookie_read'
  | 'cookie_write'
  | 'local_storage_read'
  | 'local_storage_write'
  | 'session_storage_read'
  | 'session_storage_write'
  | 'storage_state_import'
  | 'storage_state_export'
  | 'network_sensitive_read'
  | 'page_script_execution'
  | 'unsafe_code_execution'

export interface AgentBuiltinMcpToolResourceSummary {
  scope: string
  displayName: string
  fileBasenames: string[]
  origin: string | null
}

export interface AgentBuiltinMcpToolApprovalIdentity {
  actionId: string
  approvalId: string
  runId: string
  callId: string
  capabilityId: McpBuiltinCapabilityId
  capabilityActivationId: string
  managedMcpId: string
  packageName: string
  packageVersion: string
  upstreamCatalogDigest: string
  manifestDigest: string
  policyDigest: string
  policyRevision: number
  toolId: string
  rawName: string
  modelName: string
  upstreamSchemaDigest: string
  hostOverlayDigest: string
  hostInputSchemaDigest: string
  argumentsDigest: string
  resourceScopeDigest: string
  origin: string | null
}

/** Renderer-safe, value-free projection of one exact built-in MCP sensitive Tool decision. */
export interface AgentBuiltinMcpToolApproval {
  schemaVersion: 1
  identity: AgentBuiltinMcpToolApprovalIdentity
  capabilityDisplayName: string
  toolDisplayName: string
  callReason: string
  operationCategory: string
  resourceSummary: AgentBuiltinMcpToolResourceSummary
  riskKinds: AgentBuiltinMcpToolRiskKind[]
  /** Unix timestamp in seconds. */
  createdAt: number
  /** Unix timestamp in seconds. */
  expiresAt: number
  approvalStatus: AgentApprovalStatus
}

export type AgentProposedAction =
  | { type: 'tool_call'; call: AgentToolCall }
  | { type: 'mcp_tool_call'; approval: AgentMcpToolApproval }
  | {
      type: 'builtin_capability_activation'
      approval: AgentBuiltinCapabilityActivationApproval
    }
  | { type: 'builtin_mcp_tool_approval'; approval: AgentBuiltinMcpToolApproval }
  | { type: 'browser_risk_approval'; approval: AgentBrowserRiskApproval }
  | { type: 'file_change'; fileChange: AgentFileChangeProposal }
  | { type: 'command'; command: AgentCommandActionProjection }
  | { type: 'skill_materialization'; materialization: AgentSkillMaterializationRequest }
  | { type: 'skill_script'; script: AgentSkillScriptRequest }
  | { type: 'office_operation'; officeOperation: AgentOfficeOperationRequest }
  | { type: 'skill_installation'; installation: AgentSkillInstallationRequest }
