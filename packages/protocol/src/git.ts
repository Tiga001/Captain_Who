export type GitRepositoryInspectionState = 'ready' | 'notRepository' | 'unsupported' | 'unavailable'

export interface GitRepositoryInspectInput {
  projectId: string
}

export interface GitRepositorySourceInspection {
  folderId: string
  alias: string
  role: 'primary' | 'auxiliary'
  state: GitRepositoryInspectionState
  repositoryId?: string
  message?: string
}

export type GitReviewSource = { kind: 'folder'; folderId: string } | { kind: 'all' }

export interface GitRepositoryInspection {
  projectId: string
  state: GitRepositoryInspectionState
  repositoryId?: string
  message?: string
  folders: GitRepositorySourceInspection[]
  defaultFolderId?: string
}

export type GitReviewTarget =
  | { kind: 'lastTurn'; conversationId: string }
  | { kind: 'uncommitted' }
  | { kind: 'unstaged' }
  | { kind: 'staged' }
  | { kind: 'commit'; commitSha: string }
  | { kind: 'branch'; baseRef: string }

export type GitReviewBranchKind = 'local' | 'remote'

export interface GitReviewBranch {
  name: string
  ref: string
  kind: GitReviewBranchKind
  isDefault: boolean
}

export interface GitReviewRepositoryContextInput {
  folderId?: string
  projectId: string
}

export interface GitReviewRepositoryContext {
  repositoryId: string
  currentBranch?: string
  headSha?: string
  defaultBaseRef?: string
  branches: GitReviewBranch[]
  truncated: boolean
}

export interface GitReviewCommit {
  sha: string
  parents: string[]
  subject: string
  message: string
  committedAt: string
  stats: GitReviewStats
}

export interface GitReviewCommitListInput {
  folderId?: string
  projectId: string
}

export interface GitReviewCommitList {
  repositoryId: string
  commits: GitReviewCommit[]
  truncated: boolean
}

export type GitReviewFileStatus =
  'modified' | 'added' | 'deleted' | 'renamed' | 'copied' | 'untracked' | 'conflicted'

export interface GitReviewSummaryInput {
  source?: GitReviewSource
  projectId: string
  target: GitReviewTarget
}

export interface GitReviewContext {
  currentBranch?: string
  headSha?: string
  baseRef?: string
  baseSha?: string
  mergeBaseSha?: string
  commit?: GitReviewCommit
}

export interface GitReviewFile {
  sourceFolderId?: string
  sourceAlias?: string
  /** Host-qualified path for workspace opening/copying, frozen when assistantMessageId is set. */
  workspacePath?: string
  id: string
  /** Slash-separated path relative to the selected project root. */
  path: string
  /** Previous path relative to the selected project root, when it remains inside that project. */
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
  source?: GitReviewSource
  assistantMessageId?: string
  message?: string
  repositoryId: string
  snapshotId: string
  target: GitReviewTarget
  context: GitReviewContext
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
