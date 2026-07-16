import { describe, expect, it } from 'vitest'
import { GIT_REVIEW_SYNTAX_LANGUAGE_IDS } from '../syntax/fileLanguageRegistry'
import {
  getGitReviewSyntaxLanguageLoader,
  GIT_REVIEW_SYNTAX_LANGUAGE_LOADERS
} from '../syntaxHighlighting/gitReviewSyntaxLanguageLoaders'

describe('Git review syntax language loaders', () => {
  it('covers every canonical product language except the fail-closed text fallback', () => {
    const expected = GIT_REVIEW_SYNTAX_LANGUAGE_IDS.filter((language) => language !== 'text').sort()
    const actual = Object.keys(GIT_REVIEW_SYNTAX_LANGUAGE_LOADERS).sort()

    expect(actual).toEqual(expected)
    for (const language of expected) {
      expect(getGitReviewSyntaxLanguageLoader(language)).toBe(
        GIT_REVIEW_SYNTAX_LANGUAGE_LOADERS[language]
      )
    }
  })

  it('does not expose text, arbitrary aliases or unknown grammars as dynamic imports', () => {
    expect(getGitReviewSyntaxLanguageLoader('text')).toBeUndefined()
    expect(getGitReviewSyntaxLanguageLoader('ts')).toBeUndefined()
    expect(getGitReviewSyntaxLanguageLoader('definitely-unknown')).toBeUndefined()
    expect(getGitReviewSyntaxLanguageLoader('__proto__')).toBeUndefined()
  })
})
