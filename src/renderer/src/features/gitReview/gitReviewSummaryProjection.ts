import type { GitReviewFile, GitReviewFileStats, GitReviewSummary } from '@mycopilot/protocol'

export type GitReviewFileWithStats = GitReviewFile & { stats: GitReviewFileStats }

interface GitReviewSummaryProjectionOptions {
  includeFilesWithoutStats?: boolean
}

function isLineCount(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0
}

export function hasConcreteGitReviewStats(file: GitReviewFile): file is GitReviewFileWithStats {
  return (
    file.stats !== undefined &&
    isLineCount(file.stats.additions) &&
    isLineCount(file.stats.deletions)
  )
}

/** Builds the review UI's presentation summary without changing the underlying Git snapshot. */
export function projectGitReviewSummary(
  summary: GitReviewSummary,
  options: GitReviewSummaryProjectionOptions = {}
): GitReviewSummary {
  const filesWithStats = summary.files.filter(hasConcreteGitReviewStats)
  const files = options.includeFilesWithoutStats ? summary.files : filesWithStats
  const stats = filesWithStats.reduce<GitReviewFileStats>(
    (totals, file) => ({
      additions: totals.additions + file.stats.additions,
      deletions: totals.deletions + file.stats.deletions
    }),
    { additions: 0, deletions: 0 }
  )

  if (
    files.length === summary.files.length &&
    summary.stats.fileCount === files.length &&
    summary.stats.additions === stats.additions &&
    summary.stats.deletions === stats.deletions &&
    summary.stats.lineCountsComplete
  ) {
    return summary
  }

  return {
    ...summary,
    files,
    stats: {
      ...stats,
      fileCount: files.length,
      lineCountsComplete: true
    }
  }
}
