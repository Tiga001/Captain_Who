import { describe, expect, it } from 'vitest'
import { parsePastedTextMetadata } from './attachments'
import { parseConversationTraceAttachments } from './agentParsers/eventPayloads'

describe('pasted text source metadata', () => {
  it('preserves bounded Unicode preview and UTF-16 count across guidance parsing', () => {
    const pastedText = { preview: '  中文🙂\n', characterCount: 8_000 }
    const parsed = parseConversationTraceAttachments(
      [
        {
          id: 'paste',
          kind: 'file',
          name: 'pasted-text.txt',
          mimeType: 'text/plain',
          sizeBytes: 12_000,
          pastedText
        }
      ],
      'attachments'
    )
    expect(parsed[0]?.pastedText).toEqual(pastedText)
    expect(parsePastedTextMetadata(undefined)).toBeUndefined()
    expect(parsePastedTextMetadata({ preview: '🙂'.repeat(80), characterCount: 160 })).toEqual({
      preview: '🙂'.repeat(80),
      characterCount: 160
    })
  })

  it('rejects forged metadata bounds and unknown fields', () => {
    for (const value of [
      null,
      {},
      { preview: 'a', characterCount: 0 },
      { preview: 'a'.repeat(81), characterCount: 81 },
      { preview: '🙂', characterCount: 1 },
      { preview: 'x', characterCount: 2.5 },
      { preview: 'x', characterCount: 2, path: '/tmp/file' }
    ]) {
      expect(() => parsePastedTextMetadata(value)).toThrow(/pastedText/)
    }
  })
})
