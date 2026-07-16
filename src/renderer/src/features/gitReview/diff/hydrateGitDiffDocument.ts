import {
  buildGitDiffDocument,
  createGitDiffGapSection,
  getGitDiffRangeEndOffset,
  getGitDiffRangeStartOffset
} from './buildGitDiffDocument'
import {
  gitDiffFailure,
  gitDiffSuccess,
  type GitDiffDocument,
  type GitDiffGapSection,
  type GitDiffHunk,
  type GitDiffLine,
  type GitDiffResult,
  type GitDiffSection
} from './gitDiffTypes'

export interface GitDiffFullContent {
  newText: string
  oldText: string
}

function splitFileLines(text: string): string[] {
  if (text.length === 0) return []
  const lines = text.split(/\r\n|\n|\r/)
  if (/(?:\r\n|\n|\r)$/.test(text)) lines.pop()
  return lines
}

function validateCompactDocument(document: GitDiffDocument): GitDiffResult<void> {
  if (!document.isPartial) {
    return gitDiffFailure(
      'invalid-compact-document',
      'Only a compact diff document can be hydrated.'
    )
  }
  const rebuilt = buildGitDiffDocument({ hunks: document.hunks })
  if (!rebuilt.ok) return rebuilt
  const expectedSections = rebuilt.value.sections
  if (expectedSections.length !== document.sections.length) {
    return gitDiffFailure(
      'invalid-compact-document',
      'The compact document sections do not match its hunks.'
    )
  }
  for (let index = 0; index < expectedSections.length; index += 1) {
    const expected = expectedSections[index]
    const actual = document.sections[index]
    if (!expected || !actual || expected.kind !== actual.kind || expected.id !== actual.id) {
      return gitDiffFailure(
        'invalid-compact-document',
        'The compact document sections do not match its hunks.'
      )
    }
    if (
      expected.kind === 'gap' &&
      actual.kind === 'gap' &&
      (expected.lineCount !== actual.lineCount ||
        expected.oldStartOffset !== actual.oldStartOffset ||
        expected.newStartOffset !== actual.newStartOffset ||
        expected.position !== actual.position)
    ) {
      return gitDiffFailure(
        'invalid-compact-document',
        'The compact document contains altered omitted-range metadata.'
      )
    }
  }
  return gitDiffSuccess(undefined)
}

function validateHunkAgainstFullContent(
  hunk: GitDiffHunk,
  oldLines: readonly string[],
  newLines: readonly string[]
): GitDiffResult<void> {
  let oldOffset = getGitDiffRangeStartOffset(hunk.oldRange)
  let newOffset = getGitDiffRangeStartOffset(hunk.newRange)
  const oldEnd = getGitDiffRangeEndOffset(hunk.oldRange)
  const newEnd = getGitDiffRangeEndOffset(hunk.newRange)
  if (oldOffset < 0 || newOffset < 0 || oldEnd > oldLines.length || newEnd > newLines.length) {
    return gitDiffFailure('range-out-of-bounds', 'A hunk range exceeds the full file content.', {
      hunkId: hunk.id,
      side: 'both'
    })
  }

  for (const line of hunk.lines) {
    if (line.kind === 'meta') continue
    if (line.kind === 'context') {
      if (
        oldLines[oldOffset] !== line.content ||
        newLines[newOffset] !== line.content ||
        line.oldLineNumber !== oldOffset + 1 ||
        line.newLineNumber !== newOffset + 1
      ) {
        return gitDiffFailure(
          'hunk-content-mismatch',
          'A context line does not match the supplied full file content.',
          { hunkId: hunk.id, lineNumber: oldOffset + 1, side: 'both' }
        )
      }
      oldOffset += 1
      newOffset += 1
      continue
    }
    if (line.kind === 'deletion') {
      if (oldLines[oldOffset] !== line.content || line.oldLineNumber !== oldOffset + 1) {
        return gitDiffFailure(
          'hunk-content-mismatch',
          'A deletion line does not match the supplied old file content.',
          { hunkId: hunk.id, lineNumber: oldOffset + 1, side: 'old' }
        )
      }
      oldOffset += 1
      continue
    }
    if (newLines[newOffset] !== line.content || line.newLineNumber !== newOffset + 1) {
      return gitDiffFailure(
        'hunk-content-mismatch',
        'An addition line does not match the supplied new file content.',
        { hunkId: hunk.id, lineNumber: newOffset + 1, side: 'new' }
      )
    }
    newOffset += 1
  }

  if (oldOffset !== oldEnd || newOffset !== newEnd) {
    return gitDiffFailure(
      'hunk-line-count-mismatch',
      'The hunk body does not consume its declared full-content ranges.',
      { hunkId: hunk.id, side: 'both' }
    )
  }
  return gitDiffSuccess(undefined)
}

function hydrateGap(
  gap: GitDiffGapSection,
  oldLines: readonly string[],
  newLines: readonly string[]
): GitDiffResult<GitDiffGapSection> {
  const hydratedLines: GitDiffLine[] = []
  for (let offset = 0; offset < gap.lineCount; offset += 1) {
    const oldOffset = gap.oldStartOffset + offset
    const newOffset = gap.newStartOffset + offset
    if (oldLines[oldOffset] !== newLines[newOffset]) {
      return gitDiffFailure(
        'unchanged-content-mismatch',
        'An omitted range differs between the old and new full file contents.',
        { lineNumber: oldOffset + 1, side: 'both' }
      )
    }
    const content = oldLines[oldOffset]
    if (content === undefined) {
      return gitDiffFailure(
        'range-out-of-bounds',
        'An omitted range exceeds the supplied full file content.',
        { lineNumber: oldOffset + 1, side: 'both' }
      )
    }
    hydratedLines.push({
      content,
      kind: 'context',
      newLineNumber: newOffset + 1,
      oldLineNumber: oldOffset + 1
    })
  }
  return gitDiffSuccess({ ...gap, lines: hydratedLines })
}

/**
 * Validates full old/new text against every compact hunk and omitted range.
 * No mixed document is returned when any range or line fails validation.
 */
export function hydrateGitDiffDocument(
  compactDocument: GitDiffDocument,
  content: GitDiffFullContent
): GitDiffResult<GitDiffDocument> {
  const compactValidation = validateCompactDocument(compactDocument)
  if (!compactValidation.ok) return compactValidation

  const oldLines = splitFileLines(content.oldText)
  const newLines = splitFileLines(content.newText)
  for (const hunk of compactDocument.hunks) {
    const validation = validateHunkAgainstFullContent(hunk, oldLines, newLines)
    if (!validation.ok) return validation
  }

  const sections: GitDiffSection[] = []
  for (const section of compactDocument.sections) {
    if (section.kind === 'hunk') {
      sections.push(section)
      continue
    }
    const hydratedGap = hydrateGap(section, oldLines, newLines)
    if (!hydratedGap.ok) return hydratedGap
    sections.push(hydratedGap.value)
  }

  const lastHunk = compactDocument.hunks.at(-1)
  const oldTrailingStart = lastHunk ? getGitDiffRangeEndOffset(lastHunk.oldRange) : 0
  const newTrailingStart = lastHunk ? getGitDiffRangeEndOffset(lastHunk.newRange) : 0
  const oldTrailingCount = oldLines.length - oldTrailingStart
  const newTrailingCount = newLines.length - newTrailingStart
  if (oldTrailingCount < 0 || newTrailingCount < 0) {
    return gitDiffFailure(
      'range-out-of-bounds',
      'The final hunk exceeds the supplied full file content.',
      { side: 'both' }
    )
  }
  if (oldTrailingCount !== newTrailingCount) {
    return gitDiffFailure(
      'inconsistent-gap',
      'The full contents have different omitted trailing line counts.',
      { side: 'both' }
    )
  }
  if (oldTrailingCount > 0) {
    const trailingGap = createGitDiffGapSection(
      'trailing',
      oldTrailingStart,
      newTrailingStart,
      oldTrailingCount
    )
    const hydratedTrailingGap = hydrateGap(trailingGap, oldLines, newLines)
    if (!hydratedTrailingGap.ok) return hydratedTrailingGap
    sections.push(hydratedTrailingGap.value)
  }

  return gitDiffSuccess({
    hunks: compactDocument.hunks,
    isPartial: false,
    newLineCount: newLines.length,
    oldLineCount: oldLines.length,
    sections
  })
}
