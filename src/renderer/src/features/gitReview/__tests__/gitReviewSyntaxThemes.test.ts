import { describe, expect, it } from 'vitest'
import { frontendThemes, type FrontendThemeId } from '../../../config/frontendTheme'
import {
  getGitReviewSyntaxThemeStyle,
  syntaxPaletteIdByFrontendThemeId
} from '../syntaxHighlighting/gitReviewSyntaxThemes'

const EXPECTED_VARIABLES = [
  '--git-review-syntax-attribute',
  '--git-review-syntax-background',
  '--git-review-syntax-comment',
  '--git-review-syntax-constant',
  '--git-review-syntax-foreground',
  '--git-review-syntax-function',
  '--git-review-syntax-invalid',
  '--git-review-syntax-keyword',
  '--git-review-syntax-number',
  '--git-review-syntax-regexp',
  '--git-review-syntax-string',
  '--git-review-syntax-tag',
  '--git-review-syntax-type',
  '--git-review-syntax-variable'
].sort()

describe('Git review syntax themes', () => {
  it('maps every registered frontend theme exactly once', () => {
    expect(Object.keys(syntaxPaletteIdByFrontendThemeId).sort()).toEqual(
      Object.keys(frontendThemes).sort()
    )
  })

  it.each(Object.keys(frontendThemes) as FrontendThemeId[])(
    'defines only foreground token variables for %s',
    (themeId) => {
      const style = getGitReviewSyntaxThemeStyle(themeId) as Record<string, string>

      expect(Object.keys(style).sort()).toEqual(EXPECTED_VARIABLES)
      expect(style['--git-review-syntax-background']).toBe('transparent')
      expect(style['--git-review-syntax-invalid']).toBe('var(--mc-color-text-danger)')
      for (const [name, value] of Object.entries(style)) {
        expect(value, name).toMatch(/^(?:#[0-9A-F]{6}|transparent|var\(--mc-color-text-danger\))$/)
      }
    }
  )

  it('uses the locally verified Codex palettes for the classic themes', () => {
    const light = getGitReviewSyntaxThemeStyle('classic-light') as Record<string, string>
    const dark = getGitReviewSyntaxThemeStyle('classic-dark') as Record<string, string>

    expect(light).toMatchObject({
      '--git-review-syntax-comment': '#666666',
      '--git-review-syntax-foreground': '#0D0D0D',
      '--git-review-syntax-keyword': '#D53538',
      '--git-review-syntax-string': '#008809'
    })
    expect(dark).toMatchObject({
      '--git-review-syntax-comment': '#999999',
      '--git-review-syntax-foreground': '#FCFCFC',
      '--git-review-syntax-keyword': '#F67576',
      '--git-review-syntax-string': '#85DF7B'
    })
    expect(light).not.toEqual(dark)
  })

  it('returns stable style objects so theme-independent token trees do not churn', () => {
    expect(getGitReviewSyntaxThemeStyle('github-dark')).toBe(
      getGitReviewSyntaxThemeStyle('github-dark')
    )
  })
})
