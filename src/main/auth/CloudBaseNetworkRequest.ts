import type { AuthOptions } from '@cloudbase/js-sdk/oauth'
import { accountFetch, type AccountFetch } from './AccountNetwork'

/** CloudBase's supported OAuth transport hook, retaining its token and captcha handling. */
export function createCloudBaseNetworkRequest(
  fetch: AccountFetch = accountFetch
): NonNullable<AuthOptions['baseRequest']> {
  return async <T>(
    url: string,
    options: Parameters<NonNullable<AuthOptions['baseRequest']>>[1] = {}
  ): Promise<T> => {
    let payload: unknown
    try {
      const response = await fetch(url, {
        method: options.method || 'GET',
        headers: options.headers ?? undefined,
        body:
          options.body == null
            ? undefined
            : typeof options.body === 'string'
              ? options.body
              : JSON.stringify(options.body),
        signal: options.signal
      })
      payload = await response.json()
      if (!payload || typeof payload !== 'object') throw new Error('Invalid account response')
      if (!response.ok && !('error' in payload)) throw new Error('Account HTTP request failed')
    } catch {
      // Keep transport errors in the SDK's expected envelope without copying signed URLs,
      // credentials, response bodies or raw network diagnostics into the authentication log.
      throw { error: 'unreachable', error_description: 'Account network request failed' }
    }
    if ('error' in payload && payload.error) throw payload
    return payload as T
  }
}
