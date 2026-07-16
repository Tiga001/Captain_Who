import { describe, expect, it } from 'vitest'
import { resolveGitReviewSyntaxTokens } from '../syntaxHighlighting/GitReviewSyntaxHighlightProvider'
import type { GitReviewHighlightResult, GitReviewSyntaxHighlightState } from '../syntaxHighlighting'

const RESULT: GitReviewHighlightResult = {
  cacheKey: 'snapshot:file:new',
  language: 'typescript',
  lines: [
    {
      line: 0,
      tokens: [
        {
          color: 'var(--git-review-syntax-keyword)',
          content: 'const',
          end: 5,
          start: 0
        },
        { content: ' value', end: 11, start: 5 }
      ]
    }
  ],
  mode: 'highlighted'
}

describe('resolveGitReviewSyntaxTokens', () => {
  it('returns a complete, line-addressed token sequence', () => {
    const state: GitReviewSyntaxHighlightState = { result: RESULT, status: 'ready' }

    expect(resolveGitReviewSyntaxTokens(state, 1, 'const value')).toBe(RESULT.lines[0]?.tokens)
  })

  it.each([
    [{ status: 'idle' } as const, 1, 'const value'],
    [{ status: 'loading' } as const, 1, 'const value'],
    [{ result: { ...RESULT, mode: 'plain' as const }, status: 'ready' } as const, 1, 'const value'],
    [{ result: RESULT, status: 'ready' } as const, undefined, 'const value'],
    [{ result: RESULT, status: 'ready' } as const, 1, 'stale value'],
    [{ result: RESULT, status: 'ready' } as const, 2, 'const value']
  ] satisfies Array<[GitReviewSyntaxHighlightState, number | undefined, string]>)(
    'falls back to plain text for non-ready, stale or invalid input %#',
    (state, lineNumber, content) => {
      expect(resolveGitReviewSyntaxTokens(state, lineNumber, content)).toBeUndefined()
    }
  )
})
