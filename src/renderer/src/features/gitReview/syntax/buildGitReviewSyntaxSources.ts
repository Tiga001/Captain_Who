import type { GitReviewFileContent } from '@mycopilot/protocol'
import type { GitDiffDocument, GitDiffLine } from '../diff'

export type GitReviewSyntaxSourceFidelity = 'full' | 'patch'

export interface GitReviewSyntaxSource {
  readonly code: string
  readonly fidelity: GitReviewSyntaxSourceFidelity
}

export interface GitReviewSyntaxSources {
  readonly newSource: GitReviewSyntaxSource
  readonly oldSource: GitReviewSyntaxSource
}

type GitReviewSyntaxSide = 'new' | 'old'

// Git's read model already caps a side at 100,000 lines. Keep the renderer defensive in case a
// malformed or stale patch bypasses that boundary before allocating a line-addressable array.
const MAX_PATCH_SOURCE_LINE_NUMBER = 100_000

/**
 * Builds line-addressable old/new inputs for the tokenizer.
 *
 * Full snapshots are preferred because TextMate grammars carry state across lines. When full
 * content is unavailable, compact hunks are projected back onto their real line numbers and the
 * missing ranges become blank lines. That fallback preserves diff line addressing while making
 * its lower grammar fidelity explicit to callers.
 */
export function buildGitReviewSyntaxSources(
  document: GitDiffDocument,
  fileContent?: GitReviewFileContent
): GitReviewSyntaxSources {
  const hasFullContent = fileContent?.status === 'ready'
  return {
    newSource:
      hasFullContent && fileContent.afterText !== null
        ? { code: fileContent.afterText, fidelity: 'full' }
        : buildPatchSource(document, 'new'),
    oldSource:
      hasFullContent && fileContent.beforeText !== null
        ? { code: fileContent.beforeText, fidelity: 'full' }
        : buildPatchSource(document, 'old')
  }
}

function buildPatchSource(
  document: GitDiffDocument,
  side: GitReviewSyntaxSide
): GitReviewSyntaxSource {
  const contentByLine = new Map<number, string>()
  let maxLineNumber = 0

  for (const hunk of document.hunks) {
    for (const line of hunk.lines) {
      const lineNumber = getLineNumber(line, side)
      if (lineNumber === undefined) continue
      if (lineNumber < 1 || lineNumber > MAX_PATCH_SOURCE_LINE_NUMBER) {
        return { code: '', fidelity: 'patch' }
      }
      contentByLine.set(lineNumber, line.content)
      maxLineNumber = Math.max(maxLineNumber, lineNumber)
    }
  }

  if (maxLineNumber === 0) return { code: '', fidelity: 'patch' }
  const lines = Array<string>(maxLineNumber).fill('')
  for (const [lineNumber, content] of contentByLine) lines[lineNumber - 1] = content
  return { code: lines.join('\n'), fidelity: 'patch' }
}

function getLineNumber(line: GitDiffLine, side: GitReviewSyntaxSide): number | undefined {
  if (line.kind === 'meta') return undefined
  return side === 'old' ? line.oldLineNumber : line.newLineNumber
}
