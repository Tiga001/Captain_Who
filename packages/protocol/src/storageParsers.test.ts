import { describe, expect, it } from 'vitest'
import {
  parseStorageForkConversationErrorData,
  parseStorageForkConversationRequest,
  parseStorageModelSettingsValidationErrorData
} from './storageParsers'

const valid = {
  type: 'conversation_fork',
  code: 'active_command_session',
  conversationId: 'conversation-1',
  activeSessionCount: 1
} as const

describe('storage protocol parsers', () => {
  it.each([
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' }
    },
    {
      requestId: 'request-2',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'provider_transition_boundary', operationId: 'operation-1' }
    }
  ] as const)('parses an explicit timeline fork point %#', (request) => {
    expect(parseStorageForkConversationRequest(request)).toEqual(request)
  })

  it.each([
    null,
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1', operationId: 'x' }
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'provider_transition_boundary', operationId: '' }
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'unknown', operationId: 'operation-1' }
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      throughAssistantMessageId: 'assistant-1'
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1'
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' },
      extra: true
    }
  ])('rejects an ambiguous or malformed timeline fork point %#', (request) => {
    expect(() => parseStorageForkConversationRequest(request)).toThrow(
      'Invalid storage fork conversation request'
    )
  })

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

describe('model settings validation error parser', () => {
  const duplicate = {
    kind: 'model_settings_validation',
    code: 'duplicate_model_id',
    modelId: 'deepseek-v4-flash'
  } as const

  it('accepts only the bounded duplicate-model recovery contract', () => {
    expect(parseStorageModelSettingsValidationErrorData(duplicate)).toEqual(duplicate)
    const maximumWidthId = '😀'.repeat(512)
    expect(
      parseStorageModelSettingsValidationErrorData({ ...duplicate, modelId: maximumWidthId })
    ).toEqual({ ...duplicate, modelId: maximumWidthId })
  })

  it.each([
    null,
    { ...duplicate, kind: 'database_error' },
    { ...duplicate, code: 'invalid_price' },
    { ...duplicate, modelId: '' },
    { ...duplicate, modelId: '😀'.repeat(513) },
    { ...duplicate, sql: 'private schema detail' }
  ])('rejects malformed, oversized, or expanded error data %#', (value) => {
    expect(() => parseStorageModelSettingsValidationErrorData(value)).toThrow(
      'Invalid storage model settings validation error data'
    )
  })
})
