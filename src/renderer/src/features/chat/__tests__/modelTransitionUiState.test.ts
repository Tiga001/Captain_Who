import type { AgentProviderTransitionOperation } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import {
  initialModelTransitionUiStore,
  modelTransitionLocksComposer,
  reduceModelTransitionUiStore,
  selectRenderableModelTransitionOperations
} from '../modelTransitionUiState'

const confirmation = {
  conversationId: 'conversation-1',
  targetModelId: 'deepseek-model',
  decision: 'requires_compaction' as const,
  reason: 'api_provider_changed' as const,
  operationId: 'operation-1',
  transitionToken: 'transition-token-1'
}

function operation(
  status: AgentProviderTransitionOperation['status'],
  overrides: Partial<AgentProviderTransitionOperation> = {}
): AgentProviderTransitionOperation {
  const base = {
    schemaVersion: 1 as const,
    operationId: 'operation-1',
    conversationId: 'conversation-1',
    targetModelId: 'deepseek-model',
    coveredThroughMessageId: 'assistant-2',
    startedAt: 10,
    ...overrides
  }
  if (status === 'running') return { ...base, status }
  if (status === 'completed') {
    return {
      ...base,
      status,
      modelId: base.targetModelId,
      summaryId: 'summary-1',
      completedAt: 20,
      conversationUpdatedAt: 21
    }
  }
  return {
    ...base,
    status,
    error: {
      code: 'provider_transition_generation_failed',
      message: 'The transition could not be completed.',
      recovery: 'retry'
    },
    completedAt: 20
  }
}

describe('model transition UI reducer', () => {
  it('cancels only the matching Host-signed confirmation', () => {
    const confirming = reduceModelTransitionUiStore(initialModelTransitionUiStore, {
      type: 'confirmation_requested',
      preflight: confirmation
    })

    expect(
      reduceModelTransitionUiStore(confirming, {
        type: 'confirmation_cancelled',
        conversationId: 'conversation-1',
        transitionToken: 'stale-token'
      })
    ).toEqual(confirming)
    expect(
      reduceModelTransitionUiStore(confirming, {
        type: 'confirmation_cancelled',
        conversationId: 'conversation-1',
        transitionToken: 'transition-token-1'
      }).confirmations['conversation-1']
    ).toBeUndefined()
  })

  it('locks the composer only while a Host operation is running', () => {
    const running = reduceModelTransitionUiStore(initialModelTransitionUiStore, {
      type: 'operation_received',
      operation: operation('running')
    })
    const failed = reduceModelTransitionUiStore(running, {
      type: 'operation_received',
      operation: operation('failed')
    })

    expect(modelTransitionLocksComposer(running, 'conversation-1')).toBe(true)
    expect(modelTransitionLocksComposer(failed, 'conversation-1')).toBe(false)
    expect(failed.operations['conversation-1']?.[0]).toMatchObject({
      status: 'failed',
      targetModelId: 'deepseek-model'
    })
  })

  it('does not regress a terminal notification when a stale running load arrives', () => {
    const completed = reduceModelTransitionUiStore(initialModelTransitionUiStore, {
      type: 'operation_received',
      operation: operation('completed')
    })
    const afterStaleLoad = reduceModelTransitionUiStore(completed, {
      type: 'operations_loaded',
      conversationId: 'conversation-1',
      operations: [operation('running')]
    })

    expect(afterStaleLoad.operations['conversation-1']?.[0]?.status).toBe('completed')
  })

  it('renders durable compaction boundaries but hides compatible direct commits', () => {
    const compacted = operation('completed')
    const compatible = operation('completed', {
      operationId: 'operation-2',
      coveredThroughMessageId: undefined,
      startedAt: 30
    })
    const loaded = reduceModelTransitionUiStore(initialModelTransitionUiStore, {
      type: 'operations_loaded',
      conversationId: 'conversation-1',
      operations: [compacted, compatible]
    })

    expect(selectRenderableModelTransitionOperations(loaded, 'conversation-1')).toEqual([compacted])
  })

  it('keeps only the latest failed receipt alongside successful history boundaries', () => {
    const completed = operation('completed')
    const oldFailure = operation('failed', { operationId: 'operation-z', startedAt: 30 })
    const latestFailure = operation('failed', { operationId: 'operation-a', startedAt: 30 })
    const loaded = reduceModelTransitionUiStore(initialModelTransitionUiStore, {
      type: 'operations_loaded',
      conversationId: 'conversation-1',
      operations: [completed, oldFailure, latestFailure]
    })

    expect(
      selectRenderableModelTransitionOperations(loaded, 'conversation-1').map(
        (candidate) => candidate.operationId
      )
    ).toEqual(['operation-1', 'operation-a'])
  })
})
