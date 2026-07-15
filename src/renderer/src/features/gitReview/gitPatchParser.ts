export type GitDiffLineKind = 'context' | 'addition' | 'deletion' | 'meta'

export interface GitDiffLine {
  content: string
  kind: GitDiffLineKind
  newLineNumber?: number
  oldLineNumber?: number
}

export interface GitDiffHunk {
  header: string
  lines: GitDiffLine[]
}

export interface ParsedGitPatch {
  hunks: GitDiffHunk[]
}

export interface GitSplitDiffRow {
  left?: GitDiffLine
  right?: GitDiffLine
}

const HUNK_HEADER_PATTERN = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/

export function parseGitPatch(patch: string): ParsedGitPatch {
  const hunks: GitDiffHunk[] = []
  let currentHunk: GitDiffHunk | null = null
  let oldLineNumber = 0
  let newLineNumber = 0

  for (const rawLine of patch.split(/\r?\n/)) {
    const headerMatch = rawLine.match(HUNK_HEADER_PATTERN)
    if (headerMatch) {
      oldLineNumber = Number(headerMatch[1])
      newLineNumber = Number(headerMatch[3])
      currentHunk = { header: rawLine, lines: [] }
      hunks.push(currentHunk)
      continue
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
    }
  }

  return { hunks }
}

export function buildSplitDiffRows(hunk: GitDiffHunk): GitSplitDiffRow[] {
  const rows: GitSplitDiffRow[] = []
  let index = 0

  while (index < hunk.lines.length) {
    const line = hunk.lines[index]
    if (!line) break
    if (line.kind === 'context') {
      rows.push({ left: line, right: line })
      index += 1
      continue
    }
    if (line.kind === 'meta') {
      rows.push({ left: line, right: line })
      index += 1
      continue
    }

    const deletions: GitDiffLine[] = []
    const additions: GitDiffLine[] = []
    const metadata: GitDiffLine[] = []
    while (index < hunk.lines.length) {
      const changedLine = hunk.lines[index]
      if (!changedLine || changedLine.kind === 'context') break
      if (changedLine.kind === 'deletion') deletions.push(changedLine)
      if (changedLine.kind === 'addition') additions.push(changedLine)
      if (changedLine.kind === 'meta') metadata.push(changedLine)
      index += 1
    }
    const rowCount = Math.max(deletions.length, additions.length)
    for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
      rows.push({ left: deletions[rowIndex], right: additions[rowIndex] })
    }
    for (const metaLine of metadata) {
      rows.push({ left: metaLine, right: metaLine })
    }
  }

  return rows
}
