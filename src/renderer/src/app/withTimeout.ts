export const START_TURN_TIMEOUT_MS = 30_000

export class TurnAcceptanceTimeoutError extends Error {
  constructor(message = 'Timed out waiting for the host to accept the turn.') {
    super(message)
    this.name = 'TurnAcceptanceTimeoutError'
  }
}

// Safety-net timeout for "waiting for the Host to accept the turn".
// Hitting it is an indeterminate failure: the host may or may not have accepted the turn.
export function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  message?: string
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false

    const timer = setTimeout(() => {
      settled = true
      reject(new TurnAcceptanceTimeoutError(message))
    }, timeoutMs)

    // Always attach both callbacks so a late rejection from the original promise is
    // consumed here and can never surface as an unhandledRejection.
    promise.then(
      (value) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        resolve(value)
      },
      (error) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        reject(error)
      }
    )
  })
}
