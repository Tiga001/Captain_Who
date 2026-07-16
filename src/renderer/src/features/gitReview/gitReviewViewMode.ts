export type GitReviewViewMode = 'unified' | 'split'

/** Returns the mode represented by the view-switch action, never the mode already on screen. */
export function getTargetGitReviewViewMode(currentMode: GitReviewViewMode): GitReviewViewMode {
  return currentMode === 'unified' ? 'split' : 'unified'
}
