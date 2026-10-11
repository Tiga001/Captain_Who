import { describe, expect, it } from 'vitest'
import { parseStorageReadToolFileContext } from './storageReadToolFileContext'

const context = {
  conversationId: 'conversation',
  projectId: null,
  assistantMessageId: 'assistant',
  messageFound: true,
  runId: 'run',
  call: { id: 'call', tool: 'read_file', path: 'notes.txt', filePath: null },
  callIdentity: { type: 'builtin', toolName: 'read_file' },
  resultPath: null
}

describe('read tool context wire boundary', () => {
  it('preserves exact path proof and explicit invalid identity instead of turning it into legacy proof', () => {
    expect(parseStorageReadToolFileContext(context)).toEqual(context)
    expect(parseStorageReadToolFileContext(null)).toBeNull()
    expect(
      parseStorageReadToolFileContext({ ...context, callIdentity: { type: null, toolName: null } })
        ?.callIdentity
    ).toEqual({ type: null, toolName: null })
  })
  it('rejects malformed narrow receipts', () => {
    for (const value of [
      undefined,
      {},
      [],
      { ...context, messageFound: 1 },
      { ...context, call: {} },
      { ...context, callIdentity: {} },
      { ...context, resultPath: {} }
    ]) {
      expect(() => parseStorageReadToolFileContext(value)).toThrow('Invalid read tool file context')
    }
  })
})
