export const GIT_REVIEW_SYNTAX_COLORS = [
  'var(--git-review-syntax-foreground)',
  'var(--git-review-syntax-comment)',
  'var(--git-review-syntax-string)',
  'var(--git-review-syntax-regexp)',
  'var(--git-review-syntax-number)',
  'var(--git-review-syntax-keyword)',
  'var(--git-review-syntax-function)',
  'var(--git-review-syntax-type)',
  'var(--git-review-syntax-variable)',
  'var(--git-review-syntax-constant)',
  'var(--git-review-syntax-tag)',
  'var(--git-review-syntax-attribute)',
  'var(--git-review-syntax-invalid)'
] as const

export type GitReviewSyntaxColor = (typeof GIT_REVIEW_SYNTAX_COLORS)[number]

const TRUSTED_SYNTAX_COLORS = new Set<string>(GIT_REVIEW_SYNTAX_COLORS)

export interface GitReviewHighlightToken {
  /** Token content; HTML is never accepted at this boundary. */
  content: string
  /** Zero-based UTF-16 column, inclusive. */
  start: number
  /** Zero-based UTF-16 column, exclusive. */
  end: number
  /** A CSS variable from the worker-owned, trusted theme. */
  color?: GitReviewSyntaxColor
}

export interface GitReviewHighlightLine {
  /** Zero-based source line index. */
  line: number
  tokens: readonly GitReviewHighlightToken[]
}

export type GitReviewHighlightFallbackReason =
  | 'empty'
  | 'plain-language'
  | 'unsupported-language'
  | 'content-budget'
  | 'line-count-budget'
  | 'line-length-budget'
  | 'worker-error'

export interface GitReviewHighlightResult {
  cacheKey: string
  language: string
  lines: readonly GitReviewHighlightLine[]
  mode: 'highlighted' | 'plain'
  /** Present when all or part of the document deliberately used plain text. */
  reason?: GitReviewHighlightFallbackReason
}

export interface GitReviewHighlightInput {
  /** Stable file/revision identity supplied by the caller. */
  cacheKey: string
  code: string
  /** A canonical Shiki language id or alias. Unknown ids safely fall back to text. */
  language: string
}

export interface GitReviewHighlightBudget {
  maxContentCharacters: number
  maxLines: number
  /** Lines at or above this UTF-16 length are left un-tokenized. */
  tokenizeMaxLineLength: number
  /** Shiki's defensive per-line tokenizer time limit. */
  tokenizeTimeLimitMs: number
}

export const DEFAULT_GIT_REVIEW_HIGHLIGHT_BUDGET: Readonly<GitReviewHighlightBudget> =
  Object.freeze({
    maxContentCharacters: 200_000,
    maxLines: 5_000,
    tokenizeMaxLineLength: 1_000,
    tokenizeTimeLimitMs: 100
  })

export interface GitReviewHighlightWorkerInput extends GitReviewHighlightInput {
  budget: GitReviewHighlightBudget
}

export interface GitReviewHighlightWorkerRequest {
  generation: number
  input: GitReviewHighlightWorkerInput
  requestId: number
  type: 'highlight'
}

export interface GitReviewHighlightWorkerCancelRequest {
  generation: number
  requestId: number
  type: 'cancel'
}

export interface GitReviewHighlightWorkerDisposeRequest {
  generation: number
  type: 'dispose'
}

export type GitReviewHighlightWorkerMessage =
  | GitReviewHighlightWorkerRequest
  | GitReviewHighlightWorkerCancelRequest
  | GitReviewHighlightWorkerDisposeRequest

export interface GitReviewHighlightWorkerResultMessage {
  generation: number
  requestId: number
  result: GitReviewHighlightResult
  type: 'result'
}

export interface GitReviewHighlightWorkerErrorMessage {
  generation: number
  requestId: number
  type: 'error'
}

export type GitReviewHighlightWorkerResponse =
  | GitReviewHighlightWorkerResultMessage
  | GitReviewHighlightWorkerErrorMessage

export interface GitReviewHighlightBudgetAssessment {
  hasOverlongLine: boolean
  wholeDocumentFallback?: Extract<
    GitReviewHighlightFallbackReason,
    'content-budget' | 'line-count-budget'
  >
}

export function normalizeGitReviewLanguage(language: string): string {
  return language.trim().toLowerCase() || 'text'
}

export function normalizeGitReviewHighlightBudget(
  budget: Partial<GitReviewHighlightBudget> = {}
): GitReviewHighlightBudget {
  return {
    maxContentCharacters: positiveInteger(
      budget.maxContentCharacters,
      DEFAULT_GIT_REVIEW_HIGHLIGHT_BUDGET.maxContentCharacters
    ),
    maxLines: positiveInteger(budget.maxLines, DEFAULT_GIT_REVIEW_HIGHLIGHT_BUDGET.maxLines),
    tokenizeMaxLineLength: positiveInteger(
      budget.tokenizeMaxLineLength,
      DEFAULT_GIT_REVIEW_HIGHLIGHT_BUDGET.tokenizeMaxLineLength
    ),
    tokenizeTimeLimitMs: positiveInteger(
      budget.tokenizeTimeLimitMs,
      DEFAULT_GIT_REVIEW_HIGHLIGHT_BUDGET.tokenizeTimeLimitMs
    )
  }
}

export function assessGitReviewHighlightBudget(
  code: string,
  budget: GitReviewHighlightBudget
): GitReviewHighlightBudgetAssessment {
  if (code.length > budget.maxContentCharacters) {
    return { hasOverlongLine: false, wholeDocumentFallback: 'content-budget' }
  }

  let currentLineLength = 0
  let hasOverlongLine = false
  let lineCount = 1

  for (let index = 0; index < code.length; index += 1) {
    if (code.charCodeAt(index) === 10) {
      hasOverlongLine ||= currentLineLength >= budget.tokenizeMaxLineLength
      currentLineLength = 0
      lineCount += 1
      if (lineCount > budget.maxLines) {
        return { hasOverlongLine, wholeDocumentFallback: 'line-count-budget' }
      }
    } else {
      currentLineLength += 1
    }
  }

  hasOverlongLine ||= currentLineLength >= budget.tokenizeMaxLineLength
  return { hasOverlongLine }
}

export function createPlainGitReviewHighlightResult(
  input: GitReviewHighlightInput,
  reason: GitReviewHighlightFallbackReason
): GitReviewHighlightResult {
  const language = normalizeGitReviewLanguage(input.language)
  return {
    cacheKey: input.cacheKey,
    language,
    lines: input.code.split('\n').map((content, line) => ({
      line,
      tokens:
        content.length === 0
          ? []
          : [
              {
                content,
                end: content.length,
                start: 0
              }
            ]
    })),
    mode: 'plain',
    reason
  }
}

/** Defends the renderer boundary even though the worker is packaged with the application. */
export function isValidGitReviewHighlightResult(
  result: GitReviewHighlightResult,
  input: GitReviewHighlightInput
): boolean {
  if (
    result.cacheKey !== input.cacheKey ||
    result.language !== normalizeGitReviewLanguage(input.language) ||
    (result.mode !== 'highlighted' && result.mode !== 'plain')
  ) {
    return false
  }

  const sourceLines = input.code.split('\n')
  if (result.lines.length !== sourceLines.length) return false

  return result.lines.every((line, lineIndex) => {
    const source = sourceLines[lineIndex]
    if (source === undefined || line.line !== lineIndex) return false
    if (source.length === 0) return line.tokens.length === 0

    let nextColumn = 0
    for (const token of line.tokens) {
      if (
        token.start !== nextColumn ||
        token.end !== token.start + token.content.length ||
        token.end > source.length ||
        source.slice(token.start, token.end) !== token.content ||
        (token.color !== undefined && !isGitReviewSyntaxColor(token.color))
      ) {
        return false
      }
      nextColumn = token.end
    }
    return nextColumn === source.length
  })
}

export function isGitReviewSyntaxColor(color: string): color is GitReviewSyntaxColor {
  return TRUSTED_SYNTAX_COLORS.has(color)
}

function positiveInteger(value: number | undefined, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 1
    ? Math.floor(value)
    : fallback
}
