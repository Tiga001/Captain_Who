import type { GitReviewFileStatus } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { getGitReviewFileShape, resolveGitReviewDiffLayout } from '../gitReviewDiffLayout'

describe('git review diff layout', () => {
  it.each([
    ['added', 'new-only', 'single-new'],
    ['untracked', 'new-only', 'single-new'],
    ['deleted', 'old-only', 'single-old'],
    ['modified', 'two-sided', 'split'],
    ['renamed', 'two-sided', 'split'],
    ['copied', 'two-sided', 'split'],
    ['conflicted', 'two-sided', 'split']
  ] as const)('maps %s through %s to %s in split mode', (status, expectedShape, expectedLayout) => {
    const shape = getGitReviewFileShape(status)
    expect(shape).toBe(expectedShape)
    expect(resolveGitReviewDiffLayout('split', shape)).toBe(expectedLayout)
  })

  it.each<GitReviewFileStatus>([
    'added',
    'untracked',
    'deleted',
    'modified',
    'renamed',
    'copied',
    'conflicted'
  ])('keeps %s in the requested unified stream', (status) => {
    expect(resolveGitReviewDiffLayout('unified', getGitReviewFileShape(status))).toBe('unified')
  })
})
