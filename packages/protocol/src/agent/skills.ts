import type { AgentApprovalStatus } from './core'

export type AgentSkillMaterializationResultStatus =
  'applied' | 'already_applied' | 'conflict' | 'rejected' | 'failed'

export interface AgentSkillMaterializationRequest {
  id: string
  sourceUri: string
  sourcePrefix: string | null
  destination: string
  approvalStatus: AgentApprovalStatus
  reason: string | null
}

export interface AgentSkillMaterializationResult {
  status: AgentSkillMaterializationResultStatus
  sourceUri: string
  sourcePrefix?: string
  destination: string
  sourceRevision: string
  fileCount: number
  byteCount: number
  planDigest?: string
  error?: string
  message?: string
}

export type AgentSkillScriptInterpreter = 'python3'

export type AgentSkillScriptPreflightStatus =
  'ready' | 'missing_dependencies' | 'unsupported' | 'conflict'

export type AgentSkillDependencyKind = 'python_distribution' | 'command'

export type AgentSkillDependencyStatus = 'available' | 'missing'

export interface AgentSkillScriptRequirements {
  pythonDistributions?: string[]
  commands?: string[]
}

export interface AgentSkillDependencyCheck {
  kind: AgentSkillDependencyKind
  name: string
  status: AgentSkillDependencyStatus
  version?: string
}

export interface AgentSkillScriptPreflightReport {
  status: AgentSkillScriptPreflightStatus
  interpreter: AgentSkillScriptInterpreter
  interpreterVersion?: string
  dependencies?: AgentSkillDependencyCheck[]
  runtimeFingerprint: string
  errorCode?: string
  message?: string
}

export type AgentSkillScriptSourceKind = 'workspace' | 'bundled' | 'installed'

export type AgentSkillScriptTrust = 'untrusted' | 'user_approved' | 'application'

/** Host-derived evidence; execution revalidates it against the active resource session. */
export interface AgentSkillScriptSourceProof {
  sourceId: string
  sourceKind: AgentSkillScriptSourceKind
  trust: AgentSkillScriptTrust
}

export interface AgentSkillScriptRequest {
  id: string
  scriptUri: string
  skillId: string
  skillRevision: string
  resourcePath: string
  resourceDigest: string
  source: AgentSkillScriptSourceProof
  interpreter: AgentSkillScriptInterpreter
  args: string[]
  requirements: AgentSkillScriptRequirements
  preflight: AgentSkillScriptPreflightReport
  timeoutMs: number | null
  approvalStatus: AgentApprovalStatus
  reason: string | null
}

export interface AgentSkillScriptResult {
  scriptUri: string
  skillId: string
  skillRevision: string
  resourceDigest: string
  preflight: AgentSkillScriptPreflightReport
  exitCode?: number
  stdout: string
  stderr: string
  timedOut: boolean
  cancelled: boolean
  durationMs: number
  stdoutTruncated: boolean
  stderrTruncated: boolean
  errorCode?: string
  error?: string
}

export interface AgentSkillInstallationResourceSummary {
  total: number
  references: number
  assets: number
  scripts: number
  bytes: number
}

export interface AgentSkillInstallationWarning {
  code: string
  message: string
  requiresAcknowledgement: boolean
}

export interface AgentSkillInstallationPreview {
  name: string
  description: string
  sourceSummary: unknown
  resolvedRevision: string
  fileCount: number
  totalBytes: number
  resourceSummary: AgentSkillInstallationResourceSummary
  containsScripts: boolean
  warnings: AgentSkillInstallationWarning[]
  compatibility: string
  operation: string
  impact: string
}

export interface AgentSkillInstallationRequest {
  schemaVersion: number
  id: string
  installRef: string
  preview: AgentSkillInstallationPreview
  approvalStatus: AgentApprovalStatus
  expiresAt: number
}
