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
  /** Complete source coverage for highlighted mode; intentionally empty for plain fallbacks. */
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
  GitReviewHighlightWorkerResultMessage | GitReviewHighlightWorkerErrorMessage

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
    lines: [],
    mode: 'plain',
    reason
  }
}

/** Defends the renderer boundary even though the worker is packaged with the application. */
export function isValidGitReviewHighlightResult(
  result: unknown,
  input: GitReviewHighlightInput
): result is GitReviewHighlightResult {
  if (!result || typeof result !== 'object') return false
  const candidate = result as Partial<GitReviewHighlightResult> & Record<string, unknown>
  if (
    candidate.cacheKey !== input.cacheKey ||
    candidate.language !== normalizeGitReviewLanguage(input.language) ||
    (candidate.mode !== 'highlighted' && candidate.mode !== 'plain') ||
    !Array.isArray(candidate.lines) ||
    (candidate.reason !== undefined && !isGitReviewHighlightFallbackReason(candidate.reason)) ||
    ('html' in candidate && candidate.html !== undefined)
  ) {
    return false
  }

  if (candidate.mode === 'plain') return candidate.lines.length === 0

  const sourceLines = input.code.split('\n')
  if (candidate.lines.length !== sourceLines.length) return false

  return candidate.lines.every((line: unknown, lineIndex: number) => {
    if (!line || typeof line !== 'object') return false
    const candidateLine = line as Partial<GitReviewHighlightLine> & Record<string, unknown>
    const source = sourceLines[lineIndex]
    if (
      source === undefined ||
      candidateLine.line !== lineIndex ||
      !Array.isArray(candidateLine.tokens) ||
      ('html' in candidateLine && candidateLine.html !== undefined)
    ) {
      return false
    }
    if (source.length === 0) return candidateLine.tokens.length === 0

    let nextColumn = 0
    for (const token of candidateLine.tokens) {
      if (!token || typeof token !== 'object') return false
      const candidateToken = token as Partial<GitReviewHighlightToken> & Record<string, unknown>
      if (
        typeof candidateToken.content !== 'string' ||
        candidateToken.start !== nextColumn ||
        typeof candidateToken.end !== 'number' ||
        candidateToken.end !== candidateToken.start + candidateToken.content.length ||
        candidateToken.end > source.length ||
        source.slice(candidateToken.start, candidateToken.end) !== candidateToken.content ||
        (candidateToken.color !== undefined &&
          (typeof candidateToken.color !== 'string' ||
            !isGitReviewSyntaxColor(candidateToken.color))) ||
        ('html' in candidateToken && candidateToken.html !== undefined)
      ) {
        return false
      }
      nextColumn = candidateToken.end
    }
    return nextColumn === source.length
  })
}

export function isGitReviewSyntaxColor(color: string): color is GitReviewSyntaxColor {
  return TRUSTED_SYNTAX_COLORS.has(color)
}

function isGitReviewHighlightFallbackReason(
  reason: unknown
): reason is GitReviewHighlightFallbackReason {
  return (
    reason === 'empty' ||
    reason === 'plain-language' ||
    reason === 'unsupported-language' ||
    reason === 'content-budget' ||
    reason === 'line-count-budget' ||
    reason === 'line-length-budget' ||
    reason === 'worker-error'
  )
}

function positiveInteger(value: number | undefined, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 1
    ? Math.floor(value)
    : fallback
}
