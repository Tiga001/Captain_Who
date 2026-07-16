import { afterAll, describe, expect, it } from 'vitest'
import {
  assessGitReviewHighlightBudget,
  createPlainGitReviewHighlightResult,
  isValidGitReviewHighlightResult,
  normalizeGitReviewHighlightBudget,
  type GitReviewHighlightWorkerInput
} from '../syntaxHighlighting/gitReviewSyntaxHighlightTypes'
import {
  disposeGitReviewSyntaxHighlighter,
  highlightGitReviewCodeInWorker
} from '../syntaxHighlighting/gitReviewSyntaxHighlightWorkerCore'

const TEST_BUDGET = normalizeGitReviewHighlightBudget({
  maxContentCharacters: 10_000,
  maxLines: 100,
  tokenizeMaxLineLength: 1_000,
  tokenizeTimeLimitMs: 100
})

afterAll(async () => {
  await disposeGitReviewSyntaxHighlighter()
})

describe('git review syntax highlight protocol', () => {
  it('assesses document, line-count and single-line budgets without tokenizing', () => {
    expect(
      assessGitReviewHighlightBudget(
        '1234',
        normalizeGitReviewHighlightBudget({ maxContentCharacters: 3 })
      )
    ).toMatchObject({ wholeDocumentFallback: 'content-budget' })
    expect(
      assessGitReviewHighlightBudget(
        'one\ntwo\nthree',
        normalizeGitReviewHighlightBudget({ maxLines: 2 })
      )
    ).toMatchObject({ wholeDocumentFallback: 'line-count-budget' })
    expect(
      assessGitReviewHighlightBudget(
        '1234\nok',
        normalizeGitReviewHighlightBudget({ tokenizeMaxLineLength: 4 })
      )
    ).toEqual({ hasOverlongLine: true })
  })

  it('uses UTF-16 token columns and rejects HTML or untrusted colors at the boundary', () => {
    const input = { cacheKey: 'unicode', code: 'a😀b', language: 'text' }
    const result = {
      cacheKey: input.cacheKey,
      language: input.language,
      lines: [{ line: 0, tokens: [{ content: 'a😀b', end: 4, start: 0 }] }],
      mode: 'highlighted' as const
    }
    expect(result.lines[0]?.tokens).toEqual([{ content: 'a😀b', end: 4, start: 0 }])
    expect(isValidGitReviewHighlightResult(result, input)).toBe(true)

    const withHtml = structuredClone(result) as unknown as {
      lines: Array<{ tokens: Array<Record<string, unknown>> }>
    }
    const htmlToken = withHtml.lines[0]?.tokens[0]
    if (htmlToken) htmlToken.html = '<script />'
    expect(isValidGitReviewHighlightResult(withHtml, input)).toBe(false)

    const withColor = structuredClone(result) as unknown as {
      lines: Array<{ tokens: Array<Record<string, unknown>> }>
    }
    const coloredToken = withColor.lines[0]?.tokens[0]
    if (coloredToken) coloredToken.color = 'var(--user-controlled-color)'
    expect(isValidGitReviewHighlightResult(withColor, input)).toBe(false)
  })

  it('keeps every plain fallback payload constant-size', () => {
    const input = {
      cacheKey: 'oversized',
      code: Array.from({ length: 100_000 }, () => 'content').join('\n'),
      language: 'typescript'
    }
    const result = createPlainGitReviewHighlightResult(input, 'line-count-budget')

    expect(result.lines).toEqual([])
    expect(isValidGitReviewHighlightResult(result, input)).toBe(true)
  })
})

describe('git review Shiki worker core', () => {
  it('tokenizes a complete TypeScript source with trusted semantic CSS variables', async () => {
    const input = workerInput(
      'const answer: number = 42\n/* first\n * second */\nconsole.log(answer)',
      'typescript'
    )
    const result = await highlightGitReviewCodeInWorker(input)

    expect(result.mode).toBe('highlighted')
    expect(isValidGitReviewHighlightResult(result, input)).toBe(true)
    expect(
      result.lines
        .flatMap((line) => line.tokens)
        .some((token) => token.color?.startsWith('var(--git-review-syntax-'))
    ).toBe(true)
    expect(
      result.lines
        .slice(1, 3)
        .flatMap((line) => line.tokens)
        .some((token) => token.color === 'var(--git-review-syntax-comment)')
    ).toBe(true)
  })

  it('supports canonical product languages and safely degrades inputs outside that contract', async () => {
    await expect(
      highlightGitReviewCodeInWorker(workerInput('const value = 1', 'typescript'))
    ).resolves.toMatchObject({ language: 'typescript', mode: 'highlighted' })
    await expect(
      highlightGitReviewCodeInWorker(workerInput('const value = 1', 'ts'))
    ).resolves.toMatchObject({ mode: 'plain', reason: 'unsupported-language' })
    await expect(
      highlightGitReviewCodeInWorker(workerInput('opaque content', 'definitely-unknown'))
    ).resolves.toMatchObject({ mode: 'plain', reason: 'unsupported-language' })
  })

  it('leaves an overlong line plain while retaining highlighted mode for the document', async () => {
    const input = workerInput('const farTooLong = 1\nconst ok = 2', 'typescript', {
      tokenizeMaxLineLength: 10
    })
    const result = await highlightGitReviewCodeInWorker(input)

    expect(result).toMatchObject({ mode: 'highlighted', reason: 'line-length-budget' })
    expect(isValidGitReviewHighlightResult(result, input)).toBe(true)
  })

  it('does not initialize a grammar when the whole document exceeds its budget', async () => {
    await expect(
      highlightGitReviewCodeInWorker(
        workerInput('content', 'typescript', { maxContentCharacters: 3 })
      )
    ).resolves.toMatchObject({ mode: 'plain', reason: 'content-budget' })
  })
})

function workerInput(
  code: string,
  language: string,
  budget: Partial<GitReviewHighlightWorkerInput['budget']> = {}
): GitReviewHighlightWorkerInput {
  return {
    budget: { ...TEST_BUDGET, ...budget },
    cacheKey: `${language}:${code.length}`,
    code,
    language
  }
}
