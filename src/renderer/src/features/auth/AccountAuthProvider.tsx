import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import type { AuthState } from '@mycopilot/host-api'
import { hostClient } from '../../host/hostClient'
import { AccountAuthContext } from './AccountAuthContext'

const initialState: AuthState = {
  revision: -1,
  status: 'checking',
  profile: null,
  error: null,
  remembered: false
}

export function AccountAuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>(initialState)
  const stateRef = useRef(state)
  const [loginRequested, setLoginRequested] = useState(false)
  useEffect(() => {
    let mounted = true
    const accept = (next: AuthState): void => {
      if (!mounted || next.revision < stateRef.current.revision) return
      stateRef.current = next
      setState(next)
      if (next.status === 'signedIn') setLoginRequested(false)
    }
    const unsubscribe = hostClient.auth.onStateChanged(accept)
    void hostClient.auth
      .getState()
      .then(accept)
      .catch(() => {
        if (mounted && stateRef.current.revision < 0)
          accept({ ...initialState, status: 'error', error: 'unknown' })
      })
    let lastFocusRefresh = 0
    const onFocus = (): void => {
      if (stateRef.current.status !== 'signedIn' || Date.now() - lastFocusRefresh < 60_000) return
      lastFocusRefresh = Date.now()
      void hostClient.auth.refreshProfile().catch(() => undefined)
    }
    window.addEventListener('focus', onFocus)
    return () => {
      mounted = false
      unsubscribe()
      window.removeEventListener('focus', onFocus)
    }
  }, [])
  const requestLogin = useCallback(() => setLoginRequested(true), [])
  const dismissLogin = useCallback(() => setLoginRequested(false), [])
  const canStartTurn = useCallback(() => stateRef.current.status === 'signedIn', [])
  const logout = useCallback(async () => {
    stateRef.current = { ...stateRef.current, status: 'signedOut', profile: null }
    setState(stateRef.current)
    setLoginRequested(false)
    try {
      return await hostClient.auth.logout()
    } catch {
      return { ok: false as const, error: 'unknown' as const }
    }
  }, [])
  const value = useMemo(
    () => ({ state, loginRequested, requestLogin, dismissLogin, canStartTurn, logout }),
    [state, loginRequested, requestLogin, dismissLogin, canStartTurn, logout]
  )
  return <AccountAuthContext.Provider value={value}>{children}</AccountAuthContext.Provider>
}
