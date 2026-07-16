import type { GitReviewFileContent } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { buildGitDiffDocument, parseGitPatch } from '../diff'
import { buildGitReviewSyntaxSources } from '../syntax/buildGitReviewSyntaxSources'

function documentFromPatch(patch: string) {
  const parsed = parseGitPatch(patch)
  if (!parsed.ok) throw new Error(parsed.error.message)
  const document = buildGitDiffDocument(parsed.value)
  if (!document.ok) throw new Error(document.error.message)
  return document.value
}

describe('buildGitReviewSyntaxSources', () => {
  it('prefers exact full snapshots for stateful tokenization', () => {
    const document = documentFromPatch('@@ -2 +2 @@\n-old\n+new')
    const content: GitReviewFileContent = {
      afterText: 'const value = `\nnew\n`',
      beforeText: 'const value = `\nold\n`',
      fileId: 'file',
      snapshotId: 'snapshot',
      status: 'ready'
    }

    expect(buildGitReviewSyntaxSources(document, content)).toEqual({
      newSource: { code: content.afterText, fidelity: 'full' },
      oldSource: { code: content.beforeText, fidelity: 'full' }
    })
  })

  it('projects compact hunks onto their real old and new line numbers', () => {
    const document = documentFromPatch(
      ['@@ -2,2 +2,2 @@', ' same', '-old', '+new', '@@ -6 +6 @@', '-before', '+after'].join('\n')
    )

    const sources = buildGitReviewSyntaxSources(document)
    expect(sources.oldSource.fidelity).toBe('patch')
    expect(sources.newSource.fidelity).toBe('patch')
    expect(sources.oldSource.code.split('\n')).toEqual(['', 'same', 'old', '', '', 'before'])
    expect(sources.newSource.code.split('\n')).toEqual(['', 'same', 'new', '', '', 'after'])
  })

  it('uses a full side independently when the opposite side does not exist', () => {
    const document = documentFromPatch('@@ -0,0 +1,2 @@\n+one\n+two')
    const content: GitReviewFileContent = {
      afterText: 'one\ntwo',
      beforeText: null,
      fileId: 'file',
      snapshotId: 'snapshot',
      status: 'ready'
    }

    expect(buildGitReviewSyntaxSources(document, content)).toEqual({
      newSource: { code: 'one\ntwo', fidelity: 'full' },
      oldSource: { code: '', fidelity: 'patch' }
    })
  })

  it('never includes no-newline metadata in tokenizer input', () => {
    const document = documentFromPatch('@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new')
    const sources = buildGitReviewSyntaxSources(document)

    expect(sources.oldSource.code).toBe('old')
    expect(sources.newSource.code).toBe('new')
  })
})
