import { describe, expect, it } from 'vitest'
import {
  assessGitDiffRenderBudget,
  isGitDiffHydrationTextWithinBudget,
  MAX_GIT_DIFF_HYDRATION_LINES,
  MAX_GIT_DIFF_RENDER_CHARACTERS,
  MAX_GIT_DIFF_RENDER_HUNKS,
  MAX_GIT_DIFF_RENDER_LINE_CHARACTERS,
  MAX_GIT_DIFF_RENDER_LINES
} from '../diff'

describe('git diff render budget', () => {
  it('counts LF, CRLF, and standalone CR lines without allocating a split array', () => {
    expect(assessGitDiffRenderBudget('@@ -1 +1 @@\r\n-old\r+new\n context')).toEqual({
      hunkCount: 1,
      lineCount: 4,
      ok: true
    })
  })

  it('rejects every independently bounded dimension', () => {
    expect(assessGitDiffRenderBudget('x'.repeat(MAX_GIT_DIFF_RENDER_CHARACTERS + 1))).toEqual({
      ok: false,
      reason: 'characters'
    })
    expect(assessGitDiffRenderBudget('x'.repeat(MAX_GIT_DIFF_RENDER_LINE_CHARACTERS + 1))).toEqual({
      ok: false,
      reason: 'lineCharacters'
    })
    expect(
      assessGitDiffRenderBudget(
        Array(MAX_GIT_DIFF_RENDER_LINES + 1)
          .fill(' x')
          .join('\n')
      )
    ).toEqual({ ok: false, reason: 'lines' })
    expect(
      assessGitDiffRenderBudget(
        Array.from(
          { length: MAX_GIT_DIFF_RENDER_HUNKS + 1 },
          (_, index) => `@@ -${index + 1} +${index + 1} @@`
        ).join('\n')
      )
    ).toEqual({ ok: false, reason: 'hunks' })
  })

  it('keeps optional full-content hydration behind its own lower memory ceiling', () => {
    expect(isGitDiffHydrationTextWithinBudget(null)).toBe(true)
    expect(isGitDiffHydrationTextWithinBudget('small\nfile')).toBe(true)
    expect(
      isGitDiffHydrationTextWithinBudget(
        Array(MAX_GIT_DIFF_HYDRATION_LINES + 1)
          .fill('line')
          .join('\n')
      )
    ).toBe(false)
  })
})
