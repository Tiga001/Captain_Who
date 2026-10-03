import type { Session } from 'electron'

const ACCOUNT_NETWORK_TIMEOUT_MS = 15_000
export type AccountFetch = (url: string, options?: RequestInit) => Promise<Response>
type AccountNetworkSession = Pick<Session, 'fetch'>
let accountSession: Promise<AccountNetworkSession> | undefined

function getAccountSession(): Promise<AccountNetworkSession> {
  if (!accountSession) {
    const initializing = import('electron').then(async ({ session }) => {
      // No persist: prefix, cookies or shared browser state. Chromium resolves the current system
      // proxy/PAC for each destination; never snapshot a shell's HTTP(S)_PROXY into account traffic.
      const networkSession = session.fromPartition('captain-who-account', { cache: false })
      await networkSession.setProxy({ mode: 'system' })
      return networkSession
    })
    accountSession = initializing
    void initializing.catch(() => {
      if (accountSession === initializing) accountSession = undefined
    })
  }
  return accountSession
}

export function createAccountFetch(
  getSession: () => Promise<AccountNetworkSession> = getAccountSession
): AccountFetch {
  return async (url, options = {}) => {
    const target = new URL(url)
    if (target.protocol !== 'https:' || target.username || target.password) {
      throw new Error('Invalid account request URL')
    }
    const timeout = AbortSignal.timeout(ACCOUNT_NETWORK_TIMEOUT_MS)
    const signal = options.signal ? AbortSignal.any([options.signal, timeout]) : timeout
    signal.throwIfAborted()
    // Initialization is also bounded: cancellation must not wait for a stuck proxy configuration.
    let onAbort: () => void = () => undefined
    const aborted = new Promise<never>((_resolve, reject) => {
      onAbort = () => reject(signal.reason)
      signal.addEventListener('abort', onAbort, { once: true })
    })
    let networkSession: AccountNetworkSession
    try {
      networkSession = await Promise.race([getSession(), aborted])
    } finally {
      signal.removeEventListener('abort', onAbort)
    }
    signal.throwIfAborted()
    return networkSession.fetch(url, {
      ...options,
      signal,
      credentials: 'omit',
      redirect: 'error',
      cache: 'no-store'
    })
  }
}

export const accountFetch = createAccountFetch()
