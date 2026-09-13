import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode
} from 'react'
import type { LicenseState } from '@mycopilot/host-api'
import { hostClient } from '../../host/hostClient'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { LicenseContext } from './LicenseContext'
import { getTurnAccessErrorCode } from './turnAccessError'

const initialState: LicenseState = {
  revision: -1,
  status: 'checking',
  reason: null,
  expiresAt: null,
  verifiedAt: null,
  cacheValidUntil: null,
  error: null
}

export function LicenseProvider({ children }: { children: ReactNode }) {
  const auth = useAccountAuth()
  const { t } = useFrontendConfig()
  const [state, setState] = useState<LicenseState>(initialState)
  const stateRef = useRef(state)
  const authRef = useRef(auth)
  useLayoutEffect(() => {
    authRef.current = auth
  }, [auth])
  const mountedRef = useRef(false)
  const [noticeRequested, setNoticeRequested] = useState(false)
  useEffect(() => {
    if (auth?.state.status !== 'signedIn') setNoticeRequested(false)
  }, [auth?.state.status])
  const accept = useCallback((next: LicenseState) => {
    if (!mountedRef.current || next.revision < stateRef.current.revision) return
    stateRef.current = next
    setState(next)
    if (next.status === 'allowed' || next.status === 'signedOut') setNoticeRequested(false)
  }, [])
  useEffect(() => {
    mountedRef.current = true
    const unsubscribe = hostClient.license.onStateChanged(accept)
    void hostClient.license
      .getState()
      .then(accept)
      .catch(() => {
        accept({ ...initialState, status: 'unavailable', error: 'network' })
      })
    return () => {
      mountedRef.current = false
      unsubscribe()
    }
  }, [accept])
  const refresh = useCallback(async () => {
    const revision = stateRef.current.revision
    try {
      const next = await hostClient.license.refresh()
      accept(next)
      return next
    } catch {
      if (stateRef.current.revision !== revision) return stateRef.current
      const next: LicenseState = { ...stateRef.current, status: 'unavailable', error: 'network' }
      accept(next)
      return next
    }
  }, [accept])
  const canStartTurn = useCallback(() => {
    // Main owns server-adjusted expiry, broadcasts changes, and enforces the final turn guard.
    return authRef.current?.state.status === 'signedIn' && stateRef.current.status === 'allowed'
  }, [])
  const openManagement = useCallback(() => {
    void hostClient.license.openManagement().catch(() => setNoticeRequested(true))
  }, [])
  const requestAccess = useCallback(() => {
    if (authRef.current?.state.status !== 'signedIn') {
      authRef.current?.requestLogin()
      return
    }
    const current = stateRef.current
    if (current.status === 'denied' || current.error === 'notProvisioned') {
      openManagement()
    } else {
      setNoticeRequested(true)
    }
  }, [openManagement])
  const handleDenied = useCallback(
    (error: unknown) => {
      const code = getTurnAccessErrorCode(error)
      if (!code) return false
      if (code === 'ACCOUNT_LOGIN_REQUIRED' || authRef.current?.state.status !== 'signedIn') {
        authRef.current?.requestLogin()
      } else if (code === 'ACCOUNT_LICENSE_REQUIRED') {
        openManagement()
      } else {
        // An unavailable verification is not evidence that a license is missing.
        setNoticeRequested(true)
      }
      return true
    },
    [openManagement]
  )
  const value = useMemo(
    () => ({
      state:
        auth?.state.status === 'signedIn'
          ? state
          : { ...initialState, revision: state.revision, status: 'signedOut' as const },
      canStartTurn,
      requestAccess,
      handleDenied,
      refresh
    }),
    [auth?.state.status, state, canStartTurn, requestAccess, handleDenied, refresh]
  )
  return (
    <LicenseContext.Provider value={value}>
      {children}
      {noticeRequested && auth?.state.status === 'signedIn' ? (
        <ConfirmationDialog
          title={t('license.title')}
          description={t('license.verificationNeeded')}
          confirmLabel={t('license.retry')}
          cancelLabel={t('auth.cancel')}
          confirmVariant="primary"
          onCancel={() => setNoticeRequested(false)}
          onConfirm={async () => {
            const next = await refresh()
            if (next.status === 'denied' || next.error === 'notProvisioned') {
              setNoticeRequested(false)
              requestAccess()
            }
          }}
        />
      ) : null}
    </LicenseContext.Provider>
  )
}
