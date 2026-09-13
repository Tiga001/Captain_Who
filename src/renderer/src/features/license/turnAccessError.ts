export type TurnAccessErrorCode =
  'ACCOUNT_LOGIN_REQUIRED' | 'ACCOUNT_LICENSE_REQUIRED' | 'ACCOUNT_LICENSE_UNAVAILABLE'

// Invocation errors may be wrapped by feature clients. Accept only exact stable codes,
// never substring matches in arbitrary error text, before rolling back an unadmitted turn.
export function getTurnAccessErrorCode(error: unknown): TurnAccessErrorCode | null {
  let current = error
  for (let depth = 0; depth < 5 && current && typeof current === 'object'; depth += 1) {
    const record = current as { code?: unknown; message?: unknown; data?: unknown; cause?: unknown }
    const data = record.data as { code?: unknown } | undefined
    for (const code of [record.code, data?.code, record.message]) {
      if (
        code === 'ACCOUNT_LOGIN_REQUIRED' ||
        code === 'ACCOUNT_LICENSE_REQUIRED' ||
        code === 'ACCOUNT_LICENSE_UNAVAILABLE'
      )
        return code
    }
    current = record.cause
  }
  return null
}
