export type GitRepositoryInspectionState = 'ready' | 'notRepository' | 'unsupported' | 'unavailable'

export interface GitRepositoryInspectInput {
  projectId: string
}

export interface GitRepositoryInspection {
  projectId: string
  state: GitRepositoryInspectionState
  repositoryId?: string
  message?: string
}

export type GitReviewScope = 'unstaged' | 'staged' | 'lastTurn'

export type GitReviewFileStatus =
  'modified' | 'added' | 'deleted' | 'renamed' | 'copied' | 'untracked' | 'conflicted'

export interface GitReviewSummaryInput {
  conversationId?: string
  projectId: string
  scope: GitReviewScope
}

export interface GitReviewFile {
  id: string
  path: string
  previousPath?: string
  status: GitReviewFileStatus
  stats?: GitReviewFileStats
}

export interface GitReviewFileStats {
  additions: number
  deletions: number
}

export interface GitReviewStats {
  fileCount: number
  additions: number
  deletions: number
  lineCountsComplete: boolean
}

export interface GitReviewSummary {
  repositoryId: string
  snapshotId: string
  scope: GitReviewScope
  stats: GitReviewStats
  files: GitReviewFile[]
  truncated: boolean
}

export interface GitTurnDiffSummariesInput {
  conversationId: string
  projectId: string
  assistantMessageIds: string[]
}

export interface GitTurnDiffSummaryFile {
  path: string
  status: Extract<GitReviewFileStatus, 'modified' | 'added' | 'deleted'>
  stats?: GitReviewFileStats
}

/**
 * A read-only projection of one durable agent turn diff. The backend derives this
 * from the turn's first before-state and final after-state; it is not persisted
 * as a second summary and must not be reconstructed from individual tool patches.
 */
export interface GitTurnDiffSummary {
  assistantMessageId: string
  stats: GitReviewStats
  files: GitTurnDiffSummaryFile[]
  truncated: boolean
}

export interface GitTurnDiffSummaries {
  conversationId: string
  summaries: GitTurnDiffSummary[]
}

export interface GitReviewFileDiffInput {
  snapshotId: string
  fileId: string
}

export type GitReviewFileDiffStatus = 'ready' | 'binary' | 'tooLarge' | 'snapshotExpired'

export interface GitReviewFileDiff {
  snapshotId: string
  fileId: string
  status: GitReviewFileDiffStatus
  patch?: string
}

export interface GitReviewFileContentInput {
  snapshotId: string
  fileId: string
}

export type GitReviewFileContentStatus =
  'ready' | 'binary' | 'tooLarge' | 'unsupported' | 'snapshotExpired'

export interface GitReviewFileContent {
  snapshotId: string
  fileId: string
  status: GitReviewFileContentStatus
  beforeText: string | null
  afterText: string | null
}

export type GitReviewFileMutationAction = 'stage' | 'unstage' | 'restore'

export interface GitReviewFileMutationInput {
  snapshotId: string
  fileId: string
  action: GitReviewFileMutationAction
}

export type GitReviewFileMutationStatus = 'applied' | 'snapshotExpired'

export interface GitReviewFileMutation {
  snapshotId: string
  fileId: string
  action: GitReviewFileMutationAction
  status: GitReviewFileMutationStatus
}
