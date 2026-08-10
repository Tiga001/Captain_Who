import { useCallback, useEffect, useReducer, useRef } from 'react'
import type {
  AgentProviderTransitionOperation,
  AgentProviderTransitionPreflightOutput,
  AgentProviderTransitionReason
} from '@mycopilot/protocol'
import {
  getProviderTransitionStatus,
  onProviderTransition,
  preflightProviderTransition,
  startProviderTransition
} from '../agent/agentClient'
import {
  initialModelTransitionUiStore,
  reduceModelTransitionUiStore
} from '../chat/modelTransitionUiState'

const PROVIDER_TRANSITION_RECONCILIATION_BACKOFF_MS = [
  0, 250, 500, 1_000, 2_000, 4_000, 8_000, 15_000, 30_000, 30_000
] as const
const PROVIDER_TRANSITION_RECONCILIATION_STEADY_DELAY_MS = 30_000

type TerminalProviderTransitionOperation = Extract<
  AgentProviderTransitionOperation,
  { status: 'completed' | 'failed' }
>

interface ActiveProviderTransitionAttempt {
  targetModelId: string
  operationId: string
  controller?: AbortController
  terminal?: TerminalProviderTransitionOperation
}

function providerTransitionOperationKey(operation: AgentProviderTransitionOperation): string {
  return `${operation.conversationId}\u0000${operation.operationId}`
}

function waitForProviderTransitionReconciliation(
  delayMs: number,
  signal: AbortSignal
): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false)
  return new Promise((resolve) => {
    let settled = false
    const settle = (ready: boolean) => {
      if (settled) return
      settled = true
      clearTimeout(timeoutId)
      signal.removeEventListener('abort', onAbort)
      resolve(ready)
    }
    const onAbort = () => settle(false)
    const timeoutId = setTimeout(() => settle(true), delayMs)
    signal.addEventListener('abort', onAbort, { once: true })
    if (signal.aborted) onAbort()
  })
}

type TransitionRequestOutcome =
  | {
      status: 'completed'
      operation: Extract<AgentProviderTransitionOperation, { status: 'completed' }>
    }
  | { status: 'confirmation_required' }
  | {
      status: 'running'
      /** Missing only when both idempotent invocation replies were lost; exact polling continues. */
      operation?: Extract<AgentProviderTransitionOperation, { status: 'running' }>
    }
  | { status: 'blocked' | 'failed' | 'superseded' }

interface UseProviderTransitionOptions {
  onBlocked: (reason: AgentProviderTransitionReason) => void
  onOperationCompleted: (
    operation: Extract<AgentProviderTransitionOperation, { status: 'completed' }>
  ) => void
  onOperationFailed?: (
    operation: Extract<AgentProviderTransitionOperation, { status: 'failed' }>
  ) => void
  onRequestError: () => void
}

export function useProviderTransition({
  onBlocked,
  onOperationCompleted,
  onOperationFailed,
  onRequestError
}: UseProviderTransitionOptions) {
  const [store, dispatch] = useReducer(reduceModelTransitionUiStore, initialModelTransitionUiStore)
  const storeRef = useRef(store)
  const requestEpochsRef = useRef(new Map<string, number>())
  const completedCallbacksRef = useRef(new Set<string>())
  const activeAttemptsRef = useRef(new Map<string, ActiveProviderTransitionAttempt>())
  const onBlockedRef = useRef(onBlocked)
  const onOperationCompletedRef = useRef(onOperationCompleted)
  const onOperationFailedRef = useRef(onOperationFailed)
  const onRequestErrorRef = useRef(onRequestError)

  useEffect(() => {
    storeRef.current = store
  }, [store])

  useEffect(() => {
    onBlockedRef.current = onBlocked
    onOperationCompletedRef.current = onOperationCompleted
    onOperationFailedRef.current = onOperationFailed
    onRequestErrorRef.current = onRequestError
  }, [onBlocked, onOperationCompleted, onOperationFailed, onRequestError])

  const cancelActiveAttempt = useCallback((conversationId: string) => {
    const attempt = activeAttemptsRef.current.get(conversationId)
    if (!attempt) return
    attempt.controller?.abort()
    activeAttemptsRef.current.delete(conversationId)
  }, [])

  const settleActiveAttempt = useCallback(
    (
      attempt: ActiveProviderTransitionAttempt,
      operation: TerminalProviderTransitionOperation,
      projectOperation: boolean
    ): boolean => {
      const current = activeAttemptsRef.current.get(operation.conversationId)
      if (
        current !== attempt ||
        attempt.terminal ||
        attempt.operationId !== operation.operationId ||
        attempt.targetModelId !== operation.targetModelId
      ) {
        return false
      }

      attempt.terminal = operation
      attempt.controller?.abort()
      attempt.controller = undefined
      if (projectOperation) dispatch({ type: 'operation_received', operation })

      if (
        operation.status === 'completed' &&
        !completedCallbacksRef.current.has(providerTransitionOperationKey(operation))
      ) {
        completedCallbacksRef.current.add(providerTransitionOperationKey(operation))
        onOperationCompletedRef.current(operation)
      }
      if (operation.status === 'failed') onOperationFailedRef.current?.(operation)
      return true
    },
    []
  )

  const receiveNotification = useCallback(
    (operation: AgentProviderTransitionOperation) => {
      // Every valid Host notification is presentation state. Only the exact operation explicitly
      // bound to the current local start attempt may mutate the selected conversation model.
      dispatch({ type: 'operation_received', operation })
      if (operation.status === 'running') return

      const attempt = activeAttemptsRef.current.get(operation.conversationId)
      if (!attempt || attempt.targetModelId !== operation.targetModelId) return
      if (attempt.operationId !== operation.operationId) return
      settleActiveAttempt(attempt, operation, false)
    },
    [settleActiveAttempt]
  )

  useEffect(() => onProviderTransition(receiveNotification), [receiveNotification])

  useEffect(
    () => () => {
      for (const attempt of activeAttemptsRef.current.values()) {
        attempt.controller?.abort()
      }
      activeAttemptsRef.current.clear()
    },
    []
  )

  const loadStatus = useCallback(async (conversationId: string, reportError = true) => {
    try {
      const output = await getProviderTransitionStatus({ conversationId })
      dispatch({
        type: 'operations_loaded',
        conversationId,
        operations: output.operations
      })
    } catch {
      if (reportError) onRequestErrorRef.current()
    }
  }, [])

  const reconcileActiveAttempt = useCallback(
    (conversationId: string, attempt: ActiveProviderTransitionAttempt) => {
      const controller = new AbortController()
      attempt.controller?.abort()
      attempt.controller = controller

      void (async () => {
        for (let reconciliationIndex = 0; ; reconciliationIndex += 1) {
          const delayMs =
            PROVIDER_TRANSITION_RECONCILIATION_BACKOFF_MS[reconciliationIndex] ??
            PROVIDER_TRANSITION_RECONCILIATION_STEADY_DELAY_MS
          const shouldContinue = await waitForProviderTransitionReconciliation(
            delayMs,
            controller.signal
          )
          if (!shouldContinue) return
          if (activeAttemptsRef.current.get(conversationId) !== attempt) return

          try {
            const output = await getProviderTransitionStatus({
              conversationId,
              operationId: attempt.operationId
            })
            if (controller.signal.aborted) return
            if (activeAttemptsRef.current.get(conversationId) !== attempt) return
            const exact = output.operations.find(
              (operation) =>
                operation.operationId === attempt.operationId &&
                operation.conversationId === conversationId &&
                operation.targetModelId === attempt.targetModelId
            )
            if (!exact) continue
            if (exact.status === 'running') {
              dispatch({ type: 'operation_received', operation: exact })
              continue
            }
            settleActiveAttempt(attempt, exact, true)
            return
          } catch {
            // Notifications remain authoritative; exact polling tolerates transient status loss.
          }
        }
      })().finally(() => {
        if (attempt.controller === controller) attempt.controller = undefined
      })
    },
    [settleActiveAttempt]
  )

  const start = useCallback(
    async (
      preflight: Extract<
        AgentProviderTransitionPreflightOutput,
        { decision: 'compatible' | 'requires_compaction' }
      >
    ): Promise<TransitionRequestOutcome> => {
      cancelActiveAttempt(preflight.conversationId)
      const attempt: ActiveProviderTransitionAttempt = {
        targetModelId: preflight.targetModelId,
        operationId: preflight.operationId
      }
      activeAttemptsRef.current.set(preflight.conversationId, attempt)
      reconcileActiveAttempt(preflight.conversationId, attempt)
      const input = {
        conversationId: preflight.conversationId,
        targetModelId: preflight.targetModelId,
        transitionToken: preflight.transitionToken
      }
      let operation: AgentProviderTransitionOperation
      try {
        operation = await startProviderTransition(input)
      } catch {
        // A lost invocation reply may hide an already committed operation. The Host-signed token
        // makes repeating this exact start idempotent. The preflight-derived operation identity
        // keeps recovery scoped to this attempt rather than replaying historical status.
        if (activeAttemptsRef.current.get(preflight.conversationId) !== attempt) {
          return { status: 'superseded' }
        }
        const terminalBeforeRetry = activeAttemptsRef.current.get(
          preflight.conversationId
        )?.terminal
        if (terminalBeforeRetry) {
          return terminalBeforeRetry.status === 'completed'
            ? { status: 'completed', operation: terminalBeforeRetry }
            : { status: 'failed' }
        }
        try {
          operation = await startProviderTransition(input)
        } catch {
          if (activeAttemptsRef.current.get(preflight.conversationId) !== attempt) {
            return { status: 'superseded' }
          }
          const terminalAfterRetry = activeAttemptsRef.current.get(
            preflight.conversationId
          )?.terminal
          if (terminalAfterRetry) {
            return terminalAfterRetry.status === 'completed'
              ? { status: 'completed', operation: terminalAfterRetry }
              : { status: 'failed' }
          }
          // Both invocation replies may be lost after Host accepted the idempotent authority.
          // Keep exact-operation reconciliation alive and preserve the caller's pending message.
          return { status: 'running' }
        }
      }

      if (activeAttemptsRef.current.get(preflight.conversationId) !== attempt) {
        dispatch({ type: 'operation_received', operation })
        return { status: 'superseded' }
      }
      if (
        operation.conversationId !== preflight.conversationId ||
        operation.targetModelId !== preflight.targetModelId ||
        operation.operationId !== preflight.operationId
      ) {
        cancelActiveAttempt(preflight.conversationId)
        onRequestErrorRef.current()
        return { status: 'failed' }
      }

      dispatch({ type: 'operation_received', operation })

      if (attempt.terminal) {
        return attempt.terminal.status === 'completed'
          ? { status: 'completed', operation: attempt.terminal }
          : { status: 'failed' }
      }

      if (operation.status === 'completed') {
        settleActiveAttempt(attempt, operation, false)
        return { status: 'completed', operation }
      }
      if (operation.status === 'running') {
        return { status: 'running', operation }
      }
      settleActiveAttempt(attempt, operation, false)
      return { status: 'failed' }
    },
    [cancelActiveAttempt, reconcileActiveAttempt, settleActiveAttempt]
  )

  const request = useCallback(
    async (conversationId: string, targetModelId: string): Promise<TransitionRequestOutcome> => {
      const requestEpoch = (requestEpochsRef.current.get(conversationId) ?? 0) + 1
      requestEpochsRef.current.set(conversationId, requestEpoch)
      try {
        const preflight = await preflightProviderTransition({ conversationId, targetModelId })
        if (requestEpochsRef.current.get(conversationId) !== requestEpoch) {
          return { status: 'superseded' }
        }
        if (preflight.decision === 'blocked') {
          onBlockedRef.current(preflight.reason)
          return { status: 'blocked' }
        }
        if (preflight.decision === 'requires_compaction') {
          dispatch({ type: 'confirmation_requested', preflight })
          return { status: 'confirmation_required' }
        }
        return start(preflight)
      } catch {
        onRequestErrorRef.current()
        return { status: 'failed' }
      }
    },
    [start]
  )

  const cancelConfirmation = useCallback((conversationId: string) => {
    const confirmation = storeRef.current.confirmations[conversationId]
    if (!confirmation) return
    dispatch({
      type: 'confirmation_cancelled',
      conversationId,
      transitionToken: confirmation.transitionToken
    })
  }, [])

  const clearConversation = useCallback(
    (conversationId: string) => {
      cancelActiveAttempt(conversationId)
      requestEpochsRef.current.delete(conversationId)
      dispatch({ type: 'conversation_cleared', conversationId })
    },
    [cancelActiveAttempt]
  )

  const confirm = useCallback(
    async (conversationId: string): Promise<TransitionRequestOutcome> => {
      const confirmation = storeRef.current.confirmations[conversationId]
      if (!confirmation) return { status: 'superseded' }
      return start(confirmation)
    },
    [start]
  )

  const retry = useCallback(
    async (operation: AgentProviderTransitionOperation): Promise<TransitionRequestOutcome> => {
      try {
        const preflight = await preflightProviderTransition({
          conversationId: operation.conversationId,
          targetModelId: operation.targetModelId
        })
        if (preflight.decision === 'blocked') {
          onBlockedRef.current(preflight.reason)
          return { status: 'blocked' }
        }
        return start(preflight)
      } catch {
        onRequestErrorRef.current()
        return { status: 'failed' }
      }
    },
    [start]
  )

  return {
    cancelConfirmation,
    clearConversation,
    confirm,
    loadStatus,
    request,
    retry,
    store
  }
}
