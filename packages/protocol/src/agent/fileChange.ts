import type { AgentApprovalStatus } from './core'

export const AGENT_READ_FILE_CHANGE_METHOD = 'agent.readFileChange'

export const AGENT_GET_FILE_CHANGE_DIFF_METHOD = 'agent.getFileChangeDiff'

export const AGENT_GET_FILE_CHANGE_HISTORY_DIFF_METHOD = 'agent.getFileChangeHistoryDiff'

export const AGENT_FILE_CHANGE_SCHEMA_VERSION = 1 as const

export type AgentFileChangeOperation = 'create' | 'update' | 'delete'

export type AgentFileChangeUpdateStrategy = 'modify' | 'rewrite'

export type AgentFileChangeStatus =
  | 'drafting'
  | 'ready'
  | 'waiting_approval'
  | 'applying'
  | 'applied'
  | 'already_applied'
  | 'rejected'
  | 'conflict'
  | 'failed'
  | 'outcome_unknown'
  | 'aborted'
  | 'expired'

export type AgentFileChangeResultStatus =
  | 'applied'
  | 'already_applied'
  | 'failed'
  | 'conflict'
  | 'rejected'
  | 'outcome_unknown'
  | 'aborted'
  | 'expired'

export type AgentFileChangeOutcome = 'definitely_not_executed' | 'applied' | 'outcome_unknown'

/** Renderer-safe projection. Host-private FileChange execution authority is excluded. */
export interface AgentFileChangeProposal {
  schemaVersion: typeof AGENT_FILE_CHANGE_SCHEMA_VERSION
  id: string
  transactionId: string
  operation: AgentFileChangeOperation
  updateStrategy: AgentFileChangeUpdateStrategy | null
  filePath: string
  /** Complete authoritative Direct diff, or null when the Staged diff must be paged by id. */
  inlineDiff: AgentGitDiffSnapshot | null
  baseRevision: string | null
  summary: string | null
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  approvalStatus: AgentApprovalStatus
}

export interface AgentFileChangeSnapshot {
  schemaVersion: typeof AGENT_FILE_CHANGE_SCHEMA_VERSION
  transactionId: string
  conversationId: string
  projectId: string | null
  filePath: string
  operation: AgentFileChangeOperation
  updateStrategy: AgentFileChangeUpdateStrategy | null
  status: AgentFileChangeStatus
  baseRevision: string | null
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  mutationCount: number
  nextMutationIndex: number
  statsFinal: boolean
  summary: string | null
  createdAt: number
  updatedAt: number
}

export interface AgentFileChangePreview {
  schemaVersion: typeof AGENT_FILE_CHANGE_SCHEMA_VERSION
  previewId: string
  streamId: string
  attempt: number
  toolCallIndex: number
  toolCallId: string | null
  transactionId: string
  filePath: string
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  generatedBytes: number
  contentOffsetBytes: number
  contentDelta: string
  updatedAt: number
}

export interface AgentFileChangeResult {
  schemaVersion: typeof AGENT_FILE_CHANGE_SCHEMA_VERSION
  status: AgentFileChangeResultStatus
  outcome: AgentFileChangeOutcome
  transactionId: string
  operation: AgentFileChangeOperation
  updateStrategy: AgentFileChangeUpdateStrategy | null
  filePath: string
  additions: number
  deletions: number
  lineCount: number
  byteCount: number
  revision: string | null
  errorCode: string | null
  error: string | null
  message: string | null
}

export interface AgentFileChangeIdInput {
  transactionId: string
  /** Exact root authority for an authorized read-only child observer. */
  observerRootConversationId?: string
}

export interface AgentFileChangeReadInput extends AgentFileChangeIdInput {
  offset?: number
  maxChars?: number
}

export interface AgentFileChangeContentPage {
  fileChange: AgentFileChangeSnapshot
  content: string
  offset: number
  nextOffset: number | null
  truncated: boolean
}

export interface AgentFileChangeDiffInput extends AgentFileChangeIdInput {
  offset?: number
  maxChars?: number
}

export interface AgentFileChangeDiffPage {
  transactionId: string
  patch: string
  offset: number
  nextOffset: number | null
  truncated: boolean
}

/** Durable, terminal Apply Patch identity. No private audit payload crosses this contract. */
export interface AgentFileChangeHistoryDiffInput {
  conversationId: string
  assistantMessageId: string
  runId: string
  toolCallId: string
  /** Exact root authority for an authorized read-only child observer. */
  observerRootConversationId?: string
  offset?: number
  maxChars?: number
}

export interface AgentFileChangeHistoryDiffPage {
  conversationId: string
  assistantMessageId: string
  runId: string
  toolCallId: string
  patch: string
  offset: number
  nextOffset: number | null
  truncated: boolean
}

export interface AgentGitDiffSnapshot {
  patch: string
  truncated: boolean
}
