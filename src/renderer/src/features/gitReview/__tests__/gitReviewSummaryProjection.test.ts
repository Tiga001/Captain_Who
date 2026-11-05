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
      { id: 'image', path: 'assets/preview.png', status: 'added', contentKind: 'binary' },
      {
        id: 'deleted',
        path: 'src/legacy.ts',
        stats: { additions: 0, deletions: 5 },
        status: 'deleted'
      },
      { id: 'office', path: 'report.docx', status: 'added', contentKind: 'binary' }
    ],
    context: {},
    repositoryId: 'repository-1',
    target: { kind: 'unstaged' },
    snapshotId: 'snapshot-1',
    stats: {
      additions: 12,
      deletions: 8,
      fileCount: 4,
      lineCountsComplete: true
    },
    truncated: false
  }
}

describe('projectGitReviewSummary', () => {
  it('hides only confirmed binary files and recomputes the totals', () => {
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

  it('keeps unknown and text files when statistics fail, without reporting complete totals', () => {
    const source = summary()
    source.stats.lineCountsComplete = false
    source.files.push(
      { id: 'unknown', path: 'proof.md', status: 'untracked' },
      { id: 'text', path: 'large.txt', status: 'added', contentKind: 'text' }
    )
    const projected = projectGitReviewSummary(source)
    expect(projected.files.map((file) => file.id)).toEqual(['source', 'deleted', 'unknown', 'text'])
    expect(projected.stats).toEqual({
      additions: 12,
      deletions: 8,
      fileCount: 4,
      lineCountsComplete: false
    })
    expect(
      projectGitReviewSummary(source, { includeFilesWithoutStats: true }).stats.lineCountsComplete
    ).toBe(false)
  })

  it('does not claim completeness for missing statistics or a truncated file list', () => {
    const source = summary()
    source.files.push({ id: 'unread', path: 'unknown.md', status: 'untracked' })
    expect(projectGitReviewSummary(source).stats.lineCountsComplete).toBe(false)
    source.files.pop()
    source.truncated = true
    expect(projectGitReviewSummary(source).stats.lineCountsComplete).toBe(false)
  })
})
