import {
  gitDiffFailure,
  gitDiffSuccess,
  type GitDiffDocument,
  type GitDiffGapPosition,
  type GitDiffGapSection,
  type GitDiffHunk,
  type GitDiffRange,
  type GitDiffResult,
  type GitDiffSection,
  type ParsedGitPatch
} from './gitDiffTypes'

export function getGitDiffRangeStartOffset(range: GitDiffRange): number {
  return range.count === 0 ? range.start : range.start - 1
}

export function getGitDiffRangeEndOffset(range: GitDiffRange): number {
  return getGitDiffRangeStartOffset(range) + range.count
}

function isValidRange(range: GitDiffRange): boolean {
  return (
    Number.isSafeInteger(range.start) &&
    Number.isSafeInteger(range.count) &&
    range.start >= 0 &&
    range.count >= 0 &&
    (range.count === 0 || range.start > 0)
  )
}

export function makeGitDiffGapId(
  position: GitDiffGapPosition,
  oldStartOffset: number,
  newStartOffset: number,
  lineCount: number
): string {
  return `gap:${position}:${oldStartOffset}:${newStartOffset}:${lineCount}`
}

export function createGitDiffGapSection(
  position: GitDiffGapPosition,
  oldStartOffset: number,
  newStartOffset: number,
  lineCount: number
): GitDiffGapSection {
  return {
    id: makeGitDiffGapId(position, oldStartOffset, newStartOffset, lineCount),
    kind: 'gap',
    lineCount,
    newStartOffset,
    oldStartOffset,
    position
  }
}

function appendGap(
  sections: GitDiffSection[],
  position: GitDiffGapPosition,
  oldStartOffset: number,
  newStartOffset: number,
  oldCount: number,
  newCount: number
): GitDiffResult<void> {
  if (oldCount < 0 || newCount < 0) {
    return gitDiffFailure('overlapping-hunks', 'Patch hunks overlap or are out of order.')
  }
  if (oldCount !== newCount) {
    return gitDiffFailure(
      'inconsistent-gap',
      'The omitted old and new ranges do not describe the same unchanged line count.',
      { side: 'both' }
    )
  }
  if (oldCount > 0) {
    sections.push(createGitDiffGapSection(position, oldStartOffset, newStartOffset, oldCount))
  }
  return gitDiffSuccess(undefined)
}

/** Builds the compact document. A trailing gap can only be derived after hydration. */
export function buildGitDiffDocument(parsed: ParsedGitPatch): GitDiffResult<GitDiffDocument> {
  const sections: GitDiffSection[] = []
  let previousHunk: GitDiffHunk | undefined

  for (const hunk of parsed.hunks) {
    if (!isValidRange(hunk.oldRange) || !isValidRange(hunk.newRange)) {
      return gitDiffFailure('invalid-range', 'The patch contains an invalid hunk range.', {
        hunkId: hunk.id
      })
    }

    const oldStartOffset = getGitDiffRangeStartOffset(hunk.oldRange)
    const newStartOffset = getGitDiffRangeStartOffset(hunk.newRange)
    if (previousHunk) {
      const previousOldEnd = getGitDiffRangeEndOffset(previousHunk.oldRange)
      const previousNewEnd = getGitDiffRangeEndOffset(previousHunk.newRange)
      const gap = appendGap(
        sections,
        'between',
        previousOldEnd,
        previousNewEnd,
        oldStartOffset - previousOldEnd,
        newStartOffset - previousNewEnd
      )
      if (!gap.ok) return gap
    } else {
      const gap = appendGap(sections, 'leading', 0, 0, oldStartOffset, newStartOffset)
      if (!gap.ok) return gap
    }

    sections.push({ hunk, id: `section:${hunk.id}`, kind: 'hunk' })
    previousHunk = hunk
  }

  return gitDiffSuccess({ hunks: parsed.hunks, isPartial: true, sections })
}
