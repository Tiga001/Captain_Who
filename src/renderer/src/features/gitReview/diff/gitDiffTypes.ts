export type GitDiffLineKind = 'context' | 'addition' | 'deletion' | 'meta'

export interface GitDiffRange {
  /** One-based line number, except a zero-count range names the preceding line. */
  start: number
  count: number
}

export interface GitDiffLine {
  content: string
  kind: GitDiffLineKind
  newLineNumber?: number
  oldLineNumber?: number
}

export interface GitDiffHunk {
  id: string
  lines: readonly GitDiffLine[]
  newRange: GitDiffRange
  oldRange: GitDiffRange
  rawHeader: string
}

export interface ParsedGitPatch {
  hunks: readonly GitDiffHunk[]
}

export type GitDiffGapPosition = 'leading' | 'between' | 'trailing'

export interface GitDiffGapSection {
  /** Hydrated unchanged lines. Compact documents intentionally omit this field. */
  lines?: readonly GitDiffLine[]
  id: string
  kind: 'gap'
  lineCount: number
  newStartOffset: number
  oldStartOffset: number
  position: GitDiffGapPosition
}

export interface GitDiffHunkSection {
  hunk: GitDiffHunk
  id: string
  kind: 'hunk'
}

export type GitDiffSection = GitDiffGapSection | GitDiffHunkSection

export interface GitDiffDocument {
  hunks: readonly GitDiffHunk[]
  isPartial: boolean
  newLineCount?: number
  oldLineCount?: number
  sections: readonly GitDiffSection[]
}

export type GitDiffDomainErrorCode =
  | 'invalid-hunk-header'
  | 'invalid-hunk-line'
  | 'invalid-range'
  | 'hunk-line-count-mismatch'
  | 'overlapping-hunks'
  | 'inconsistent-gap'
  | 'invalid-compact-document'
  | 'range-out-of-bounds'
  | 'hunk-content-mismatch'
  | 'unchanged-content-mismatch'

export interface GitDiffDomainError {
  code: GitDiffDomainErrorCode
  /** Safe diagnostic metadata only; source line contents are never included. */
  hunkId?: string
  lineNumber?: number
  message: string
  side?: 'old' | 'new' | 'both'
}

export type GitDiffResult<T> = { ok: true; value: T } | { error: GitDiffDomainError; ok: false }

export function gitDiffSuccess<T>(value: T): GitDiffResult<T> {
  return { ok: true, value }
}

export function gitDiffFailure<T = never>(
  code: GitDiffDomainErrorCode,
  message: string,
  metadata: Omit<GitDiffDomainError, 'code' | 'message'> = {}
): GitDiffResult<T> {
  return { error: { code, message, ...metadata }, ok: false }
}
