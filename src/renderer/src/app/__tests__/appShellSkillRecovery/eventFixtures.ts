import type {
  AgentEvent,
  AgentProviderTransitionNotification,
  AgentProviderTransitionOperation
} from '@mycopilot/protocol'
import { testState } from './state'

export function emitAgentEvent(event: AgentEvent) {
  for (const listener of testState.agentEventListeners) listener(event)
}

export function emitProviderTransition(event: AgentProviderTransitionNotification) {
  for (const listener of testState.providerTransitionListeners) listener(event)
}

export function completedProviderTransition(
  targetModelId: string,
  overrides: Partial<
    Omit<Extract<AgentProviderTransitionOperation, { status: 'completed' }>, 'status'>
  > = {}
): Extract<AgentProviderTransitionOperation, { status: 'completed' }> {
  testState.providerTransitionSequence += 1
  return {
    schemaVersion: 1,
    operationId: `provider-transition-${testState.providerTransitionSequence}`,
    conversationId: 'conversation-a',
    targetModelId,
    startedAt: 10 + testState.providerTransitionSequence,
    completedAt: 20 + testState.providerTransitionSequence,
    conversationUpdatedAt: 30 + testState.providerTransitionSequence,
    ...overrides,
    status: 'completed',
    modelId: targetModelId
  }
}

export function runningProviderTransition(
  targetModelId: string,
  overrides: Partial<
    Omit<Extract<AgentProviderTransitionOperation, { status: 'running' }>, 'status'>
  > = {}
): Extract<AgentProviderTransitionOperation, { status: 'running' }> {
  testState.providerTransitionSequence += 1
  return {
    schemaVersion: 1,
    operationId: `provider-transition-${testState.providerTransitionSequence}`,
    conversationId: 'conversation-a',
    targetModelId,
    coveredThroughMessageId: 'assistant-old',
    startedAt: 10 + testState.providerTransitionSequence,
    ...overrides,
    status: 'running'
  }
}

export function failedProviderTransition(
  running: Extract<AgentProviderTransitionOperation, { status: 'running' }>
): Extract<AgentProviderTransitionOperation, { status: 'failed' }> {
  return {
    ...running,
    status: 'failed',
    completedAt: running.startedAt + 10,
    error: {
      code: 'provider_transition_generation_failed',
      message: 'The transition could not be completed.',
      recovery: 'retry'
    }
  }
}

export function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}
