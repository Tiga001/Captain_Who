import { describe, expect, it } from 'vitest'
import {
  parseAgentProviderTransitionNotification,
  parseAgentProviderTransitionOperation,
  parseAgentProviderTransitionPreflightInput,
  parseAgentProviderTransitionPreflightOutput,
  parseAgentProviderTransitionStartInput,
  parseAgentProviderTransitionStatusOutput
} from './providerTransition'

const runningOperation = {
  schemaVersion: 1,
  operationId: 'provider-transition-1',
  conversationId: 'conversation-1',
  targetModelId: 'generic-model',
  sourceModelDisplayName: '旧模型',
  targetModelDisplayName: '新模型',
  coveredThroughMessageId: 'assistant-1',
  status: 'running',
  startedAt: 10
} as const

describe('Provider transition protocol', () => {
  it('separates ordinary send readiness from model transition authority', () => {
    const ready = {
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      decision: 'ready_for_send',
      reason: 'same_protocol'
    }
    expect(parseAgentProviderTransitionPreflightOutput(ready)).toEqual(ready)
    for (const authority of [{ operationId: 'operation-1' }, { transitionToken: 'token-1' }]) {
      expect(() => parseAgentProviderTransitionPreflightOutput({ ...ready, ...authority })).toThrow(
        /must not carry transition authority/
      )
    }
    expect(() =>
      parseAgentProviderTransitionPreflightOutput({ ...ready, reason: 'api_provider_changed' })
    ).toThrow(/invalid for ready_for_send/)
    expect(() => parseAgentProviderTransitionStartInput(ready)).toThrow()
    for (const forSend of [true, false]) {
      const input = { conversationId: 'conversation-1', targetModelId: 'generic-model', forSend }
      expect(parseAgentProviderTransitionPreflightInput(input)).toEqual(input)
    }
    expect(() =>
      parseAgentProviderTransitionPreflightInput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        forSend: 'true'
      })
    ).toThrow(/forSend/)
  })

  it('strictly parses preflight authority without projecting runtime capabilities', () => {
    expect(
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId: 'provider-transition-1',
        transitionToken: 'opaque-token'
      })
    ).toEqual({
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      decision: 'requires_compaction',
      reason: 'api_provider_changed',
      operationId: 'provider-transition-1',
      transitionToken: 'opaque-token'
    })

    expect(() =>
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        decision: 'requires_compaction',
        reason: 'api_provider_changed',
        operationId: 'provider-transition-1',
        transitionToken: 'opaque-token',
        capabilities: { privateReplay: true }
      })
    ).toThrow(/unexpected field capabilities/)

    expect(() =>
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        currentModelId: 'current-model',
        targetModelId: 'generic-model',
        decision: 'compatible',
        reason: 'same_protocol',
        operationId: 'provider-transition-1',
        transitionToken: 'opaque-token'
      })
    ).toThrow(/unexpected field currentModelId/)
  })

  it('enforces decision, reason and transition-token combinations', () => {
    expect(() =>
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        decision: 'blocked',
        reason: 'active_run',
        operationId: 'must-not-cross',
        transitionToken: undefined
      })
    ).toThrow(/must not carry operationId/)

    expect(() =>
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        decision: 'blocked',
        reason: 'active_run',
        transitionToken: 'must-not-cross'
      })
    ).toThrow(/must not carry transitionToken/)

    expect(() =>
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        decision: 'compatible',
        reason: 'api_provider_changed',
        operationId: 'provider-transition-1',
        transitionToken: 'opaque-token'
      })
    ).toThrow(/invalid for compatible/)

    expect(() =>
      parseAgentProviderTransitionPreflightOutput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        decision: 'compatible',
        reason: 'same_protocol',
        transitionToken: 'opaque-token'
      })
    ).toThrow(/operationId/)
  })

  it('strictly parses preflight and start inputs', () => {
    expect(
      parseAgentProviderTransitionPreflightInput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model'
      })
    ).toEqual({ conversationId: 'conversation-1', targetModelId: 'generic-model' })
    expect(
      parseAgentProviderTransitionStartInput({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        transitionToken: 'opaque-token'
      })
    ).toEqual({
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      transitionToken: 'opaque-token'
    })
  })

  it('parses running and completed operations with only a safe Timeline anchor', () => {
    expect(parseAgentProviderTransitionOperation(runningOperation)).toEqual(runningOperation)
    expect(parseAgentProviderTransitionNotification(runningOperation)).toEqual(runningOperation)

    expect(() =>
      parseAgentProviderTransitionOperation({
        ...runningOperation,
        targetModelDisplayName: undefined
      })
    ).toThrow(/sourceModelDisplayName and targetModelDisplayName/)

    const completed = {
      ...runningOperation,
      status: 'completed',
      modelId: 'generic-model',
      summaryId: 'summary-1',
      completedAt: 20,
      conversationUpdatedAt: 21
    } as const
    expect(parseAgentProviderTransitionOperation(completed)).toEqual(completed)

    expect(() =>
      parseAgentProviderTransitionOperation({
        ...completed,
        conversationUpdatedAt: undefined
      })
    ).toThrow(/conversationUpdatedAt/)

    expect(() =>
      parseAgentProviderTransitionOperation({
        ...completed,
        continuation: 'private-provider-state'
      })
    ).toThrow(/unexpected field continuation/)
  })

  it('accepts only bounded safe asynchronous failure codes', () => {
    const failed = {
      ...runningOperation,
      status: 'failed',
      error: {
        code: 'provider_transition_generation_failed',
        message: 'History compression failed.',
        recovery: 'retry'
      },
      completedAt: 20
    } as const
    expect(parseAgentProviderTransitionOperation(failed)).toEqual(failed)

    expect(() =>
      parseAgentProviderTransitionOperation({
        ...failed,
        error: { ...failed.error, code: 'provider_raw_error' }
      })
    ).toThrow(/unexpected value provider_raw_error/)
  })

  it('returns a bounded, duplicate-free operation history for reload recovery', () => {
    expect(
      parseAgentProviderTransitionStatusOutput({
        operations: [
          runningOperation,
          {
            ...runningOperation,
            operationId: 'provider-transition-2',
            status: 'completed',
            modelId: 'generic-model',
            summaryId: 'summary-2',
            completedAt: 20,
            conversationUpdatedAt: 21
          }
        ]
      }).operations
    ).toHaveLength(2)

    expect(() =>
      parseAgentProviderTransitionStatusOutput({ operations: [runningOperation, runningOperation] })
    ).toThrow(/operationId values must be unique/)

    expect(() =>
      parseAgentProviderTransitionStatusOutput({
        operations: [
          runningOperation,
          { ...runningOperation, operationId: 'provider-transition-later', startedAt: 11 }
        ]
      })
    ).toThrow(/startedAt descending/)
  })
})
