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

export type GitReviewScope = 'unstaged' | 'staged'

export type GitReviewFileStatus =
  'modified' | 'added' | 'deleted' | 'renamed' | 'copied' | 'untracked' | 'conflicted'

export interface GitReviewSummaryInput {
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
