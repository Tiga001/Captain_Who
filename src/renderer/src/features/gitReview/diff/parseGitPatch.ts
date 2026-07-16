import {
  gitDiffFailure,
  gitDiffSuccess,
  type GitDiffHunk,
  type GitDiffLine,
  type GitDiffRange,
  type GitDiffResult,
  type ParsedGitPatch
} from './gitDiffTypes'

const HUNK_HEADER_PATTERN = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(?:.*)$/

interface MutableHunk {
  lines: GitDiffLine[]
  newRange: GitDiffRange
  oldRange: GitDiffRange
  rawHeader: string
}

function parseRange(startText: string, countText: string | undefined): GitDiffRange | null {
  const start = Number(startText)
  const count = countText === undefined ? 1 : Number(countText)
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(count)) return null
  if (start < 0 || count < 0 || (count > 0 && start === 0)) return null
  return { count, start }
}

function makeHunkId(oldRange: GitDiffRange, newRange: GitDiffRange): string {
  return `hunk:${oldRange.start},${oldRange.count}:${newRange.start},${newRange.count}`
}

function countHunkLines(lines: readonly GitDiffLine[]): { oldCount: number; newCount: number } {
  let oldCount = 0
  let newCount = 0
  for (const line of lines) {
    if (line.kind === 'context' || line.kind === 'deletion') oldCount += 1
    if (line.kind === 'context' || line.kind === 'addition') newCount += 1
  }
  return { newCount, oldCount }
}

function finalizeHunk(hunk: MutableHunk): GitDiffResult<GitDiffHunk> {
  const { newCount, oldCount } = countHunkLines(hunk.lines)
  const hunkId = makeHunkId(hunk.oldRange, hunk.newRange)
  if (oldCount !== hunk.oldRange.count || newCount !== hunk.newRange.count) {
    return gitDiffFailure(
      'hunk-line-count-mismatch',
      'The patch body does not consume the ranges declared by its hunk header.',
      { hunkId, side: 'both' }
    )
  }
  return gitDiffSuccess({
    id: hunkId,
    lines: hunk.lines,
    newRange: hunk.newRange,
    oldRange: hunk.oldRange,
    rawHeader: hunk.rawHeader
  })
}

/**
 * Parses a single-file unified patch into a range-preserving representation.
 * Invalid input is rejected instead of being partially rendered.
 */
export function parseGitPatch(patch: string): GitDiffResult<ParsedGitPatch> {
  const hunks: GitDiffHunk[] = []
  let currentHunk: MutableHunk | null = null
  let newLineNumber = 0
  let oldLineNumber = 0
  const rawLines = patch.split(/\r?\n/)

  for (let index = 0; index < rawLines.length; index += 1) {
    const rawLine = rawLines[index] ?? ''
    const headerMatch = rawLine.match(HUNK_HEADER_PATTERN)
    if (headerMatch) {
      if (currentHunk) {
        const finalized = finalizeHunk(currentHunk)
        if (!finalized.ok) return finalized
        hunks.push(finalized.value)
      }

      const oldRange = parseRange(headerMatch[1] ?? '', headerMatch[2])
      const newRange = parseRange(headerMatch[3] ?? '', headerMatch[4])
      if (!oldRange || !newRange) {
        return gitDiffFailure('invalid-range', 'The patch contains an invalid hunk range.')
      }
      currentHunk = { lines: [], newRange, oldRange, rawHeader: rawLine }
      oldLineNumber = oldRange.start
      newLineNumber = newRange.start
      continue
    }

    if (rawLine.startsWith('@@')) {
      return gitDiffFailure('invalid-hunk-header', 'The patch contains an unsupported hunk header.')
    }
    if (!currentHunk) continue

    if (rawLine.startsWith('+')) {
      currentHunk.lines.push({
        content: rawLine.slice(1),
        kind: 'addition',
        newLineNumber
      })
      newLineNumber += 1
      continue
    }
    if (rawLine.startsWith('-')) {
      currentHunk.lines.push({
        content: rawLine.slice(1),
        kind: 'deletion',
        oldLineNumber
      })
      oldLineNumber += 1
      continue
    }
    if (rawLine.startsWith(' ')) {
      currentHunk.lines.push({
        content: rawLine.slice(1),
        kind: 'context',
        newLineNumber,
        oldLineNumber
      })
      oldLineNumber += 1
      newLineNumber += 1
      continue
    }
    if (rawLine.startsWith('\\')) {
      currentHunk.lines.push({ content: rawLine, kind: 'meta' })
      continue
    }

    // A final split artifact is not part of the hunk. All other unprefixed lines
    // would make the line accounting ambiguous and are rejected.
    if (index === rawLines.length - 1 && rawLine === '') continue
    return gitDiffFailure(
      'invalid-hunk-line',
      'The patch contains an invalid unprefixed hunk line.'
    )
  }

  if (currentHunk) {
    const finalized = finalizeHunk(currentHunk)
    if (!finalized.ok) return finalized
    hunks.push(finalized.value)
  }

  return gitDiffSuccess({ hunks })
}
