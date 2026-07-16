import { describe, expect, it } from 'vitest'
import {
  buildGitDiffDocument,
  buildSplitDiffBlocks,
  getGitDiffExpandedSlices,
  hydrateGitDiffDocument,
  parseGitPatch,
  reduceGitDiffExpansion,
  type GitDiffDocument,
  type GitDiffHunk
} from '../diff'

function parsePatch(patch: string): GitDiffHunk[] {
  const result = parseGitPatch(patch)
  if (!result.ok) throw new Error(result.error.message)
  return [...result.value.hunks]
}

function compactDocument(patch: string): GitDiffDocument {
  const parsed = parseGitPatch(patch)
  if (!parsed.ok) throw new Error(parsed.error.message)
  const document = buildGitDiffDocument(parsed.value)
  if (!document.ok) throw new Error(document.error.message)
  return document.value
}

describe('parseGitPatch', () => {
  it('preserves ranges, defaulting an omitted count to one while keeping an explicit zero', () => {
    const result = parseGitPatch('@@ -4 +8,0 @@ function name\n-old')
    expect(result).toMatchObject({
      ok: true,
      value: {
        hunks: [
          {
            id: 'hunk:4,1:8,0',
            newRange: { count: 0, start: 8 },
            oldRange: { count: 1, start: 4 },
            rawHeader: '@@ -4 +8,0 @@ function name'
          }
        ]
      }
    })
  })

  it('rejects a body that does not consume its declared range', () => {
    const result = parseGitPatch('@@ -1,2 +1,1 @@\n-old\n+new')
    expect(result).toMatchObject({
      error: { code: 'hunk-line-count-mismatch' },
      ok: false
    })
  })
})

describe('buildGitDiffDocument', () => {
  const patch = [
    '@@ -3,1 +3,1 @@',
    '-old three',
    '+new three',
    '@@ -8,1 +8,1 @@',
    '-old eight',
    '+new eight'
  ].join('\n')

  it('derives stable leading and inter-hunk gaps while leaving trailing context unknown', () => {
    const first = compactDocument(patch)
    const second = compactDocument(patch)
    expect(first.sections.map((section) => section.kind)).toEqual(['gap', 'hunk', 'gap', 'hunk'])
    expect(first.sections[0]).toMatchObject({
      id: 'gap:leading:0:0:2',
      lineCount: 2,
      position: 'leading'
    })
    expect(first.sections[2]).toMatchObject({
      id: 'gap:between:3:3:4',
      lineCount: 4,
      position: 'between'
    })
    expect(first.sections.map((section) => section.id)).toEqual(
      second.sections.map((section) => section.id)
    )
  })

  it('rejects unequal omitted ranges instead of inventing an unchanged count', () => {
    const parsed = parseGitPatch('@@ -3,1 +4,1 @@\n-old\n+new')
    expect(parsed.ok).toBe(true)
    if (!parsed.ok) return
    expect(buildGitDiffDocument(parsed.value)).toMatchObject({
      error: { code: 'inconsistent-gap' },
      ok: false
    })
  })

  it('uses zero-count coordinates as between-line offsets', () => {
    const insertion = compactDocument('@@ -2,0 +3,1 @@\n+inserted')
    expect(insertion.sections[0]).toMatchObject({
      lineCount: 2,
      newStartOffset: 0,
      oldStartOffset: 0,
      position: 'leading'
    })
    expect(insertion.hunks[0]).toMatchObject({
      newRange: { count: 1, start: 3 },
      oldRange: { count: 0, start: 2 }
    })
  })
})

describe('buildSplitDiffBlocks', () => {
  it('groups an asymmetric tail into one continuous missing-side block', () => {
    const [hunk] = parsePatch(
      '@@ -1,2 +1,4 @@\n-old one\n-old two\n+new one\n+new two\n+new three\n+new four'
    )
    if (!hunk) throw new Error('Expected a hunk')
    const blocks = buildSplitDiffBlocks(hunk)
    expect(blocks).toHaveLength(2)
    expect(blocks[0]).toMatchObject({ kind: 'paired', rowSpan: 2, rows: [{}, {}] })
    expect(blocks[1]).toMatchObject({
      kind: 'one-sided',
      lines: [{ content: 'new three' }, { content: 'new four' }],
      missingSide: 'left',
      presentSide: 'right',
      rowSpan: 2
    })
  })

  it('represents a deletion-only run with one block and no undefined placeholder rows', () => {
    const [hunk] = parsePatch('@@ -1,3 +1,0 @@\n-one\n-two\n-three')
    if (!hunk) throw new Error('Expected a hunk')
    expect(buildSplitDiffBlocks(hunk)).toMatchObject([
      {
        kind: 'one-sided',
        lines: [{ content: 'one' }, { content: 'two' }, { content: 'three' }],
        missingSide: 'right',
        presentSide: 'left',
        rowSpan: 3
      }
    ])
  })
})

describe('hydrateGitDiffDocument', () => {
  const patch = [
    '@@ -2,3 +2,3 @@',
    ' two',
    '-three',
    '+THREE',
    ' four',
    '@@ -7,3 +7,3 @@',
    ' seven',
    '-eight',
    '+EIGHT',
    ' nine'
  ].join('\n')
  const oldText = 'one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n'
  const newText = 'one\ntwo\nTHREE\nfour\nfive\nsix\nseven\nEIGHT\nnine\nten\n'

  it('validates every compact line and hydrates leading, middle and trailing gaps', () => {
    const hydrated = hydrateGitDiffDocument(compactDocument(patch), { newText, oldText })
    expect(hydrated.ok).toBe(true)
    if (!hydrated.ok) return
    expect(hydrated.value.isPartial).toBe(false)
    expect(hydrated.value.sections.map((section) => section.kind)).toEqual([
      'gap',
      'hunk',
      'gap',
      'hunk',
      'gap'
    ])
    expect(hydrated.value.sections[0]).toMatchObject({
      lineCount: 1,
      lines: [{ content: 'one', newLineNumber: 1, oldLineNumber: 1 }]
    })
    expect(hydrated.value.sections[2]).toMatchObject({
      lineCount: 2,
      lines: [{ content: 'five' }, { content: 'six' }]
    })
    expect(hydrated.value.sections[4]).toMatchObject({
      id: 'gap:trailing:9:9:1',
      lines: [{ content: 'ten' }],
      position: 'trailing'
    })
  })

  it('fails closed when omitted content changed after the compact diff was created', () => {
    const staleNewText = newText.replace('\nfive\n', '\nFIVE\n')
    expect(
      hydrateGitDiffDocument(compactDocument(patch), { newText: staleNewText, oldText })
    ).toMatchObject({
      error: { code: 'unchanged-content-mismatch' },
      ok: false
    })
  })

  it('fails closed when a compact changed line does not match the full content', () => {
    const staleOldText = oldText.replace('\nthree\n', '\nTHREE-ELSE\n')
    expect(
      hydrateGitDiffDocument(compactDocument(patch), { newText, oldText: staleOldText })
    ).toMatchObject({
      error: { code: 'hunk-content-mismatch', side: 'old' },
      ok: false
    })
  })

  it('rejects a compact document whose gap metadata was altered or removed', () => {
    const compact = compactDocument(patch)
    const tampered = { ...compact, sections: compact.sections.slice(1) }
    expect(hydrateGitDiffDocument(tampered, { newText, oldText })).toMatchObject({
      error: { code: 'invalid-compact-document' },
      ok: false
    })
  })

  it('hydrates an insertion after unchanged content using zero-count range semantics', () => {
    const compact = compactDocument('@@ -2,0 +3,1 @@\n+inserted')
    const hydrated = hydrateGitDiffDocument(compact, {
      newText: 'one\ntwo\ninserted\nthree\n',
      oldText: 'one\ntwo\nthree\n'
    })
    expect(hydrated.ok).toBe(true)
    if (!hydrated.ok) return
    expect(hydrated.value.sections).toMatchObject([
      { lineCount: 2, lines: [{ content: 'one' }, { content: 'two' }], position: 'leading' },
      { kind: 'hunk' },
      { lineCount: 1, lines: [{ content: 'three' }], position: 'trailing' }
    ])
  })
})

describe('reduceGitDiffExpansion', () => {
  it('expands by twenty from either boundary without overlap', () => {
    const gapId = 'gap:between:0:0:50'
    const down = reduceGitDiffExpansion(new Map(), {
      direction: 'down',
      gapId,
      lineCount: 50,
      type: 'expand'
    })
    expect(down.get(gapId)).toEqual({ fromEnd: 0, fromStart: 20 })
    const both = reduceGitDiffExpansion(down, {
      direction: 'both',
      gapId,
      lineCount: 50,
      type: 'expand'
    })
    expect(both.get(gapId)).toEqual({ fromEnd: 10, fromStart: 40 })
    expect(getGitDiffExpandedSlices(50, both.get(gapId))).toEqual({
      collapsedCount: 0,
      end: { end: 50, start: 40 },
      start: { end: 40, start: 0 }
    })
  })

  it('supports upward expansion, custom steps, collapse and reset immutably', () => {
    const initial = new Map()
    const expanded = reduceGitDiffExpansion(initial, {
      direction: 'up',
      gapId: 'gap',
      lineCount: 44,
      step: 7,
      type: 'expand'
    })
    expect(initial.size).toBe(0)
    expect(expanded.get('gap')).toEqual({ fromEnd: 7, fromStart: 0 })
    const collapsed = reduceGitDiffExpansion(expanded, { gapId: 'gap', type: 'collapse' })
    expect(collapsed.size).toBe(0)
    expect(reduceGitDiffExpansion(expanded, { type: 'reset' }).size).toBe(0)
  })

  it('opens both edges by twenty and clamps a short gap without duplicate ranges', () => {
    const expanded = reduceGitDiffExpansion(new Map(), {
      direction: 'both',
      gapId: 'short-gap',
      lineCount: 30,
      type: 'expand'
    })
    expect(expanded.get('short-gap')).toEqual({ fromEnd: 10, fromStart: 20 })
    expect(getGitDiffExpandedSlices(30, expanded.get('short-gap'))).toEqual({
      collapsedCount: 0,
      end: { end: 30, start: 20 },
      start: { end: 20, start: 0 }
    })
  })
})
