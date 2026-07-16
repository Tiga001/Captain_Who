import type { GitDiffHunk, GitDiffLine } from './gitDiffTypes'

export interface GitSplitDiffRow {
  left: GitDiffLine
  right: GitDiffLine
}

export interface GitSplitPairedBlock {
  id: string
  kind: 'paired'
  rowSpan: number
  rows: readonly GitSplitDiffRow[]
}

export interface GitSplitOneSidedBlock {
  id: string
  kind: 'one-sided'
  lines: readonly GitDiffLine[]
  missingSide: 'left' | 'right'
  presentSide: 'left' | 'right'
  rowSpan: number
}

export type GitSplitDiffBlock = GitSplitPairedBlock | GitSplitOneSidedBlock

function pushPairedBlock(
  blocks: GitSplitDiffBlock[],
  hunkId: string,
  sequence: number,
  rows: GitSplitDiffRow[]
): number {
  if (rows.length === 0) return sequence
  blocks.push({
    id: `${hunkId}:split:${sequence}:paired`,
    kind: 'paired',
    rowSpan: rows.length,
    rows
  })
  return sequence + 1
}

function pushOneSidedBlock(
  blocks: GitSplitDiffBlock[],
  hunkId: string,
  sequence: number,
  presentSide: 'left' | 'right',
  lines: GitDiffLine[]
): number {
  if (lines.length === 0) return sequence
  blocks.push({
    id: `${hunkId}:split:${sequence}:one-sided:${presentSide}`,
    kind: 'one-sided',
    lines,
    missingSide: presentSide === 'left' ? 'right' : 'left',
    presentSide,
    rowSpan: lines.length
  })
  return sequence + 1
}

/**
 * Produces structural split blocks. Consecutive unpaired lines are represented
 * by one one-sided block so the renderer can span them with one continuous buffer.
 */
export function buildSplitDiffBlocks(hunk: GitDiffHunk): readonly GitSplitDiffBlock[] {
  const blocks: GitSplitDiffBlock[] = []
  let index = 0
  let sequence = 0
  let ordinaryRows: GitSplitDiffRow[] = []

  const flushOrdinaryRows = (): void => {
    sequence = pushPairedBlock(blocks, hunk.id, sequence, ordinaryRows)
    ordinaryRows = []
  }

  while (index < hunk.lines.length) {
    const line = hunk.lines[index]
    if (!line) break
    if (line.kind === 'context' || line.kind === 'meta') {
      ordinaryRows.push({ left: line, right: line })
      index += 1
      continue
    }

    flushOrdinaryRows()
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

    const pairedCount = Math.min(deletions.length, additions.length)
    const pairedRows: GitSplitDiffRow[] = []
    for (let rowIndex = 0; rowIndex < pairedCount; rowIndex += 1) {
      const left = deletions[rowIndex]
      const right = additions[rowIndex]
      if (left && right) pairedRows.push({ left, right })
    }
    sequence = pushPairedBlock(blocks, hunk.id, sequence, pairedRows)
    sequence = pushOneSidedBlock(blocks, hunk.id, sequence, 'left', deletions.slice(pairedCount))
    sequence = pushOneSidedBlock(blocks, hunk.id, sequence, 'right', additions.slice(pairedCount))
    sequence = pushPairedBlock(
      blocks,
      hunk.id,
      sequence,
      metadata.map((metaLine) => ({ left: metaLine, right: metaLine }))
    )
  }

  flushOrdinaryRows()
  return blocks
}
