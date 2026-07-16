import type { GitReviewFileStatus } from '@mycopilot/protocol'
import type { GitReviewViewMode } from './gitReviewViewMode'

/** Describes which repository blobs exist for a file, independently of its requested presentation. */
export type GitReviewFileShape = 'old-only' | 'new-only' | 'two-sided'

/** The concrete renderer selected after combining user preference with repository file shape. */
export type GitReviewDiffLayout = 'unified' | 'split' | 'single-old' | 'single-new'

export function getGitReviewFileShape(status: GitReviewFileStatus): GitReviewFileShape {
  switch (status) {
    case 'deleted':
      return 'old-only'
    case 'added':
    case 'untracked':
      return 'new-only'
    case 'conflicted':
    case 'copied':
    case 'modified':
    case 'renamed':
      return 'two-sided'
  }
}

/**
 * A unified diff is always a single stream. In split mode, repository-level one-sided files use
 * the full width while two-sided files preserve comparison alignment even for one-way hunks.
 */
export function resolveGitReviewDiffLayout(
  viewMode: GitReviewViewMode,
  fileShape: GitReviewFileShape
): GitReviewDiffLayout {
  if (viewMode === 'unified') return 'unified'
  if (fileShape === 'old-only') return 'single-old'
  if (fileShape === 'new-only') return 'single-new'
  return 'split'
}
