import type { GitReviewFileMutationAction, GitReviewTarget } from '@mycopilot/protocol'

export interface GitReviewTargetCapabilities {
  mutation: Extract<GitReviewFileMutationAction, 'stage' | 'unstage'> | null
  restore: boolean
}

const READ_ONLY_CAPABILITIES: GitReviewTargetCapabilities = Object.freeze({
  mutation: null,
  restore: false
})

const CAPABILITIES: Record<GitReviewTarget['kind'], GitReviewTargetCapabilities> = {
  branch: READ_ONLY_CAPABILITIES,
  commit: READ_ONLY_CAPABILITIES,
  lastTurn: READ_ONLY_CAPABILITIES,
  staged: Object.freeze({ mutation: 'unstage', restore: false }),
  uncommitted: READ_ONLY_CAPABILITIES,
  unstaged: Object.freeze({ mutation: 'stage', restore: true })
}

export function getGitReviewTargetCapabilities(
  kind: GitReviewTarget['kind']
): GitReviewTargetCapabilities {
  return CAPABILITIES[kind]
}
