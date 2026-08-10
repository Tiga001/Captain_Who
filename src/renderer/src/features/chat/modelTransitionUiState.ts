import type {
  AgentProviderTransitionOperation,
  AgentProviderTransitionPreflightOutput
} from '@mycopilot/protocol'

export type ModelTransitionConfirmation = Extract<
  AgentProviderTransitionPreflightOutput,
  { decision: 'requires_compaction' }
>

export interface ModelTransitionUiStore {
  confirmations: Record<string, ModelTransitionConfirmation | undefined>
  operations: Record<string, AgentProviderTransitionOperation[] | undefined>
}

export type ModelTransitionUiAction =
  | { type: 'confirmation_requested'; preflight: ModelTransitionConfirmation }
  | { type: 'confirmation_cancelled'; conversationId: string; transitionToken: string }
  | {
      type: 'operations_loaded'
      conversationId: string
      operations: AgentProviderTransitionOperation[]
    }
  | { type: 'operation_received'; operation: AgentProviderTransitionOperation }
  | { type: 'conversation_cleared'; conversationId: string }

export const initialModelTransitionUiStore: ModelTransitionUiStore = {
  confirmations: {},
  operations: {}
}

const MAX_RENDERER_TRANSITION_OPERATIONS = 50

function mergeOperations(
  existing: readonly AgentProviderTransitionOperation[],
  incoming: readonly AgentProviderTransitionOperation[]
): AgentProviderTransitionOperation[] {
  const merged = new Map(existing.map((operation) => [operation.operationId, operation]))

  for (const operation of incoming) {
    const current = merged.get(operation.operationId)
    // Once Host has reported a terminal receipt, delayed loads or duplicate notifications cannot
    // rewrite it. Operation IDs are opaque identities, not ordering or causality signals.
    if (current && current.status !== 'running') continue
    merged.set(operation.operationId, operation)
  }

  return [...merged.values()]
    .sort((left, right) => left.startedAt - right.startedAt)
    .slice(-MAX_RENDERER_TRANSITION_OPERATIONS)
}

export function reduceModelTransitionUiStore(
  state: ModelTransitionUiStore,
  action: ModelTransitionUiAction
): ModelTransitionUiStore {
  if (action.type === 'confirmation_requested') {
    const { conversationId } = action.preflight
    if (modelTransitionLocksComposer(state, conversationId)) return state
    return {
      ...state,
      confirmations: { ...state.confirmations, [conversationId]: action.preflight }
    }
  }

  if (action.type === 'confirmation_cancelled') {
    const current = state.confirmations[action.conversationId]
    if (!current || current.transitionToken !== action.transitionToken) return state
    return {
      ...state,
      confirmations: { ...state.confirmations, [action.conversationId]: undefined }
    }
  }

  if (action.type === 'operations_loaded') {
    return {
      ...state,
      operations: {
        ...state.operations,
        [action.conversationId]: mergeOperations(
          state.operations[action.conversationId] ?? [],
          action.operations
        )
      }
    }
  }

  if (action.type === 'operation_received') {
    const { conversationId } = action.operation
    const confirmation = state.confirmations[conversationId]
    return {
      confirmations:
        confirmation?.targetModelId === action.operation.targetModelId
          ? { ...state.confirmations, [conversationId]: undefined }
          : state.confirmations,
      operations: {
        ...state.operations,
        [conversationId]: mergeOperations(state.operations[conversationId] ?? [], [
          action.operation
        ])
      }
    }
  }

  if (action.type === 'conversation_cleared') {
    return {
      confirmations: { ...state.confirmations, [action.conversationId]: undefined },
      operations: { ...state.operations, [action.conversationId]: undefined }
    }
  }

  return state
}

export function modelTransitionLocksComposer(
  state: ModelTransitionUiStore,
  conversationId: string
): boolean {
  return (state.operations[conversationId] ?? []).some(
    (operation) => operation.status === 'running'
  )
}

export function selectRenderableModelTransitionOperations(
  state: ModelTransitionUiStore,
  conversationId: string
): AgentProviderTransitionOperation[] {
  const operations = state.operations[conversationId] ?? []
  const latest = operations.at(-1)

  return operations.filter((operation) => {
    if (operation.status === 'completed') return Boolean(operation.coveredThroughMessageId)
    return operation.operationId === latest?.operationId
  })
}
