import type { GitReviewSummary } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { projectGitReviewSummary } from '../gitReviewSummaryProjection'

function summary(): GitReviewSummary {
  return {
    files: [
      {
        id: 'source',
        path: 'src/main.ts',
        stats: { additions: 12, deletions: 3 },
        status: 'modified'
      },
      { id: 'image', path: 'assets/preview.png', status: 'added' },
      {
        id: 'deleted',
        path: 'src/legacy.ts',
        stats: { additions: 0, deletions: 5 },
        status: 'deleted'
      },
      { id: 'office', path: 'report.docx', status: 'added' }
    ],
    repositoryId: 'repository-1',
    scope: 'unstaged',
    snapshotId: 'snapshot-1',
    stats: {
      additions: 12,
      deletions: 8,
      fileCount: 4,
      lineCountsComplete: false
    },
    truncated: false
  }
}

describe('projectGitReviewSummary', () => {
  it('keeps only files with concrete line counts and recomputes the totals', () => {
    const source = summary()
    const projected = projectGitReviewSummary(source)

    expect(projected.files.map((file) => file.id)).toEqual(['source', 'deleted'])
    expect(projected.stats).toEqual({
      additions: 12,
      deletions: 8,
      fileCount: 2,
      lineCountsComplete: true
    })
    expect(source.files).toHaveLength(4)
  })

  it('preserves an already complete summary by reference', () => {
    const source = summary()
    source.files = source.files.filter((file) => file.stats !== undefined)
    source.stats = {
      additions: 12,
      deletions: 8,
      fileCount: 2,
      lineCountsComplete: true
    }

    expect(projectGitReviewSummary(source)).toBe(source)
  })

  it('can retain every file while totaling only files with concrete line counts', () => {
    const source = summary()
    const projected = projectGitReviewSummary(source, { includeFilesWithoutStats: true })

    expect(projected.files).toEqual(source.files)
    expect(projected.stats).toEqual({
      additions: 12,
      deletions: 8,
      fileCount: 4,
      lineCountsComplete: true
    })
  })
})
