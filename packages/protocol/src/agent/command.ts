import type { AgentApprovalStatus } from './core'

export const AGENT_COMMAND_SESSIONS_LIST_METHOD = 'agent.commandSessions.list'

export const AGENT_COMMAND_SESSIONS_GET_METHOD = 'agent.commandSessions.get'

/**
 * Controls whether run_command requires a human click.
 * Approval and command safety are separate inputs. In guarded mode, auto_approve applies only to
 * commands the policy permits automatically; high-impact commands may still require explicit user
 * approval.
 */
export type AgentCommandPermission = 'require_approval' | 'auto_approve'

/**
 * Controls how broadly an authorized command may execute.
 * - guarded: auto-run low-risk commands and route high-impact commands to explicit approval.
 * - full_access: auto-run high-impact commands except operations that are always denied.
 *
 * This is independent from command approval: approval decides who authorizes a command, while
 * commandSafety decides which policy applies after authorization.
 */
export type AgentCommandSafetyPolicy = 'guarded' | 'full_access'

export type AgentCommandOutputStream = 'stdout' | 'stderr'

export const AGENT_COMMAND_SESSION_SCHEMA_VERSION = 2

/** Process Session state is independent from Agent Run and approval state. */
export type AgentCommandSessionStatus =
  'starting' | 'running' | 'exited' | 'interrupted' | 'timed_out' | 'failed' | 'outcome_unknown'

/** Bounded Renderer-safe projection; it contains no process handles, environment, or raw output. */
export interface AgentCommandSessionSnapshot {
  schemaVersion: typeof AGENT_COMMAND_SESSION_SCHEMA_VERSION
  sessionId: string
  conversationId: string
  assistantMessageId: string
  originRunId: string
  callId: string
  projectId?: string
  command: string
  cwd: string
  commandDigest: string
  status: AgentCommandSessionStatus
  startedAt: number
  endedAt?: number
  exitCode?: number
  latestSequence: number
  outputTruncated: boolean
  outputs?: AgentCommandPublishedOutput[]
  /** Bounded Host observation attached only after terminal command settlement. */
  artifactObservation?: AgentCommandArtifactObservation
  archiveRef?: string
}

export interface AgentCommandSessionOutputChunk {
  sequence: number
  stream: AgentCommandOutputStream
  output: string
}

/** Maximum chunk count accepted across the Rust/TypeScript command transcript boundary. */
export const AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS = 2048

export interface AgentCommandSessionTranscript {
  requestedAfterSequence: number
  firstAvailableSequence?: number
  latestSequence: number
  truncatedBefore: boolean
  outputCaptureTruncated: boolean
  chunks: AgentCommandSessionOutputChunk[]
}

export interface AgentCommandSessionListInput {
  conversationId: string
}

export interface AgentCommandSessionListOutput {
  sessions: AgentCommandSessionSnapshot[]
}

export interface AgentCommandSessionGetInput {
  conversationId: string
  sessionId: string
  afterSequence?: number
  maxBytes?: number
}

export interface AgentCommandSessionGetOutput {
  session: AgentCommandSessionSnapshot
  transcript: AgentCommandSessionTranscript
}

export type AgentCommandRiskLevel =
  'read_only' | 'writes_workspace' | 'network' | 'destructive' | 'unknown'

export type AgentCommandPolicyDecision = 'allow' | 'require_explicit_approval' | 'deny'

export type AgentCommandRiskClass =
  | 'read_only'
  | 'safe_workspace_write'
  | 'direct_write'
  | 'network'
  | 'package_management'
  | 'high_impact'
  | 'unknown'
  | 'catastrophic'
  | 'unsupported'
  | 'external_read'

export interface AgentCommandPolicyFinding {
  segmentIndex: number
  program: string
  risk: AgentCommandRiskClass
  /** Stable discriminator for UI routing and telemetry. */
  code: string
  /** Human-readable diagnostic; callers must not branch on this text. */
  reason: string
}

export interface AgentCommandPolicyEvaluation {
  decision: AgentCommandPolicyDecision
  /** Stable summary discriminator for UI routing and telemetry. */
  code: string
  /** Human-readable diagnostic; callers must not branch on this text. */
  reason: string
  riskLevel: AgentCommandRiskLevel
  findings: AgentCommandPolicyFinding[]
}

export type AgentCommandPublishedOutputKind = 'image' | 'document'

export interface AgentCommandPublishedOutput {
  name: string
  kind: AgentCommandPublishedOutputKind
  readPath: string
  mimeType: string
  sizeBytes: number
  sha256: string
  width?: number
  height?: number
}

export interface AgentCommandExecutionResult {
  outputs?: AgentCommandPublishedOutput[]
  command: string
  cwd: string
  exitCode?: number
  stdout: string
  stderr: string
  timedOut: boolean
  cancelled: boolean
  durationMs: number
  stdoutTruncated: boolean
  stderrTruncated: boolean
  error?: string
  /** Present when execution was stopped by the authoritative backend command policy. */
  policyEvaluation?: AgentCommandPolicyEvaluation
  /** Best-effort file effects observed around this exact command execution. */
  artifactObservation?: AgentCommandArtifactObservation
  /** Host-owned runtime resolution or structured preflight failure evidence. */
  runtime?: AgentCommandRuntimeResolution
}

export type AgentCommandArtifactObservationKind = 'office'

export const AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION = 3

export interface AgentCommandArtifactObservationRequest {
  kinds: AgentCommandArtifactObservationKind[]
  /** Expected Office output files, resolved relative to command cwd. This does not grant access. */
  expectedOutputs?: string[]
  /** Extra files or directories to observe beyond the automatically included workspace. */
  additionalRoots?: string[]
}

export type AgentCommandArtifactObservationStatus = 'complete' | 'partial' | 'failed'

export type AgentCommandArtifactObservationPhase = 'setup' | 'before' | 'after'

export type AgentCommandArtifactKind = 'document' | 'spreadsheet' | 'presentation'

export type AgentCommandArtifactScope = 'workspace' | 'external'

export type AgentCommandArtifactChangeKind =
  'created' | 'modified' | 'replaced' | 'deleted' | 'renamed'

export type AgentCommandExpectedArtifactOutcomeKind =
  | 'created'
  | 'modified'
  | 'replaced'
  | 'renamed'
  | 'unchanged'
  | 'missing'
  | 'unobserved'
  | 'invalid'

export type AgentCommandArtifactValidationStatus =
  'valid' | 'invalid' | 'not_applicable' | 'unchecked'

export interface AgentCommandArtifactValidation {
  status: AgentCommandArtifactValidationStatus
  code?: string
  message?: string
}

export interface AgentCommandArtifactMetadata {
  sizeBytes: number
  sha256?: string
  validation: AgentCommandArtifactValidation
}

export interface AgentCommandArtifactChange {
  kind: AgentCommandArtifactChangeKind
  artifactKind: AgentCommandArtifactKind
  path: string
  scope: AgentCommandArtifactScope
  previousPath?: string
  previousScope?: AgentCommandArtifactScope
  before?: AgentCommandArtifactMetadata
  after?: AgentCommandArtifactMetadata
}

export interface AgentCommandExpectedArtifactOutcome {
  requestedPath: string
  outcome: AgentCommandExpectedArtifactOutcomeKind
  path?: string
  scope?: AgentCommandArtifactScope
  artifactKind?: AgentCommandArtifactKind
  metadata?: AgentCommandArtifactMetadata
}

export interface AgentCommandArtifactSnapshotCoverage {
  rootsScanned: number
  directoryEntriesScanned: number
  officeFilesSeen: number
  filesHashed: number
  filesUnhashed: number
  bytesHashed: number
  symlinksSkipped: number
  excludedDirectories: number
  durationMs: number
  timeBudgetExceeded: boolean
  cancelled: boolean
  truncated: boolean
}

export interface AgentCommandArtifactObservationCoverage {
  workspaceIncluded: boolean
  expectedOutputCount: number
  additionalRootCount: number
  before: AgentCommandArtifactSnapshotCoverage
  after: AgentCommandArtifactSnapshotCoverage
}

export interface AgentCommandArtifactObservationWarning {
  phase: AgentCommandArtifactObservationPhase
  code: string
  path?: string
  message: string
}

export interface AgentCommandArtifactObservation {
  schemaVersion: number
  status: AgentCommandArtifactObservationStatus
  /** False only when both snapshots and the bounded change report are complete. */
  partial: boolean
  /** Stable backend reason codes explaining incomplete observation evidence. */
  stopReasons: string[]
  /** Office files considered across the before and after snapshots. */
  scanned: number
  /** Change records included in this observation. */
  returned: number
  /** Known change records omitted from the bounded report. */
  omitted: number
  coverage: AgentCommandArtifactObservationCoverage
  changes: AgentCommandArtifactChange[]
  changesTruncated: boolean
  changesOmitted: number
  expectedOutputs: AgentCommandExpectedArtifactOutcome[]
  warnings: AgentCommandArtifactObservationWarning[]
}

export type AgentCommandRuntimeKind = 'node' | 'python'

/**
 * Host-owned reproducible runtime identity persisted in approvals and evidence.
 * `pdf` is bound only by the trusted built-in PDF Skill and is intentionally absent from the
 * model-visible run_command.runtimeProfile enum.
 */
export type AgentCommandRuntimeProfile = 'documents' | 'spreadsheets' | 'presentations' | 'pdf'

export interface AgentCommandRuntimeResolvedPackage {
  name: string
  version: string
}

/** Approval-time runtime identity. Host-private executable and environment data are excluded. */
export interface AgentCommandRuntimeBinding {
  schemaVersion: number
  profile: AgentCommandRuntimeProfile
  profileRevision: string
  providerId: string
  bundleVersion: string
  bundleRevision: string
  kind: AgentCommandRuntimeKind
  runtimeVersion: string
  runtimeFingerprint: string
  resolvedPackages: AgentCommandRuntimeResolvedPackage[]
}

/** Public runtime evidence; private executable and component paths are intentionally absent. */
export interface AgentCommandRuntimeResolution {
  schemaVersion: number
  providerId: string
  profile?: AgentCommandRuntimeProfile
  profileRevision?: string
  bundleVersion?: string
  bundleRevision?: string
  kind: AgentCommandRuntimeKind
  runtimeVersion?: string
  runtimeFingerprint?: string
  resolvedPackages?: AgentCommandRuntimeResolvedPackage[]
  errorCode?: string
  recovery?: string
  message?: string
}

export type AgentFileInputRef =
  | { type: 'attachment'; readPath: string }
  | { type: 'workspace'; path: string }
  | { type: 'external'; path: string }
  | { type: 'generated_artifact'; uri: string; path: string }
  | { type: 'skill_resource'; uri: string }

export interface AgentFileInputSpec {
  mountPath: string
  source: AgentFileInputRef
}

export interface AgentFileInputBinding {
  schemaVersion: 1
  mountPath: string
  source: AgentFileInputRef
  sizeBytes: number
  sha256: string
}

/** Renderer-safe projection of a durable command action; Host authority is deliberately absent. */
export interface AgentCommandActionProjection {
  id: string
  command: string
  cwd: string | null
  timeoutMs: number | null
  approvalStatus: AgentApprovalStatus
  riskLevel: AgentCommandRiskLevel | null
  reason: string | null
  observe: AgentCommandArtifactObservationRequest | null
}
