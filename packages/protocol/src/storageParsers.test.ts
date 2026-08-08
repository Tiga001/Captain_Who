import { describe, expect, it } from 'vitest'
import { parseStorageForkConversationErrorData } from './storageParsers'

const valid = {
  type: 'conversation_fork',
  code: 'active_command_session',
  conversationId: 'conversation-1',
  activeSessionCount: 1
} as const

describe('storage protocol parsers', () => {
  it('parses the bounded active-command fork rejection', () => {
    expect(parseStorageForkConversationErrorData(valid)).toEqual(valid)
    const maximumWidthId = '😀'.repeat(512)
    expect(
      parseStorageForkConversationErrorData({ ...valid, conversationId: maximumWidthId })
    ).toEqual({ ...valid, conversationId: maximumWidthId })
  })

  it.each([
    null,
    { ...valid, type: 'database_error' },
    { ...valid, code: 'unknown' },
    { ...valid, conversationId: '' },
    { ...valid, conversationId: '😀'.repeat(513) },
    { ...valid, activeSessionCount: 0 },
    { ...valid, activeSessionCount: 513 },
    { ...valid, sql: 'private schema detail' }
  ])('rejects malformed or oversized recovery data %#', (value) => {
    expect(() => parseStorageForkConversationErrorData(value)).toThrow(
      'Invalid storage fork conversation error data'
    )
  })
})
