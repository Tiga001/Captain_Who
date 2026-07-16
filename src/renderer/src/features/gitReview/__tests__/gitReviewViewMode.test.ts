import { describe, expect, it } from 'vitest'
import { getTargetGitReviewViewMode } from '../gitReviewViewMode'

describe('getTargetGitReviewViewMode', () => {
  it.each([
    ['unified', 'split'],
    ['split', 'unified']
  ] as const)('maps the visible %s mode to the %s action target', (currentMode, targetMode) => {
    expect(getTargetGitReviewViewMode(currentMode)).toBe(targetMode)
  })

  it('returns to the original mode after two switches', () => {
    const initialMode = 'unified' as const
    const nextMode = getTargetGitReviewViewMode(initialMode)

    expect(getTargetGitReviewViewMode(nextMode)).toBe(initialMode)
  })
})
