export type PendingActionDecisionOutcome =
  | { status: 'applied' }
  | { status: 'skipped'; reason: 'missing_run_id' | 'not_accepted' }
  | { status: 'failed'; error: unknown }

export interface PendingActionDecisionOptions<TResult> {
  runId: string | null | undefined
  invoke: (runId: string) => Promise<TResult>
  apply: (result: TResult) => void
  isAccepted?: (result: TResult) => boolean
  onError: (error: unknown) => void
  onMissingRunId?: () => void
  onNotAccepted?: (result: TResult) => void
}

/**
 * Keeps a pending action visible until the backend has authoritatively accepted the decision.
 * Rejected requests, negative acknowledgements, and incomplete action identities never mutate UI
 * state, so the user can retry the original action.
 */
export async function applyAuthoritativePendingActionDecision<TResult>({
  runId,
  invoke,
  apply,
  isAccepted,
  onError,
  onMissingRunId,
  onNotAccepted
}: PendingActionDecisionOptions<TResult>): Promise<PendingActionDecisionOutcome> {
  if (!runId || runId.trim().length === 0) {
    onMissingRunId?.()
    return { status: 'skipped', reason: 'missing_run_id' }
  }

  try {
    const result = await invoke(runId)
    if (isAccepted && !isAccepted(result)) {
      onNotAccepted?.(result)
      return { status: 'skipped', reason: 'not_accepted' }
    }

    apply(result)
    return { status: 'applied' }
  } catch (error) {
    onError(error)
    return { status: 'failed', error }
  }
}
