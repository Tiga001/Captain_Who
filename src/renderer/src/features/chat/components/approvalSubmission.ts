export type ApprovalSubmissionResult = boolean | void | Promise<boolean | void>

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    typeof (value as PromiseLike<unknown>).then === 'function'
  )
}

/** Re-enables an approval when the authoritative decision was not accepted. */
export function resetApprovalSubmissionOnFailure(
  submission: ApprovalSubmissionResult,
  resetSubmitting: () => void
): void {
  if (submission === false) {
    resetSubmitting()
    return
  }
  if (!isPromiseLike(submission)) return

  void Promise.resolve(submission).then(
    (accepted) => {
      if (accepted === false) resetSubmitting()
    },
    () => resetSubmitting()
  )
}
