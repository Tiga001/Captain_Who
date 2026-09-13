// Renderer startup layer: keeps the workspace mounted behind an accessible startup surface.

import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import darkBrandMark from '../../../../../resources/brand-mark-dark.png'
import lightBrandMark from '../../../../../resources/brand-mark-light.png'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { isMacOS } from '../../lib/platform'
import { useAppStartupStatus } from './AppStartupContext'
import { StartupAmbientText } from './StartupAmbientText'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { AccountLoginForm } from '../auth/AccountLoginForm'
import './AppStartupScreen.css'

const MINIMUM_STARTUP_SCREEN_MS = 280
const STARTUP_TIMEOUT_MS = 60_000
const STARTUP_EXIT_MS = 180

export function AppStartupGate({ children }: { children: ReactNode }) {
  const startup = useAppStartupStatus()
  const auth = useAccountAuth()
  const [hasEnteredWorkspace, setHasEnteredWorkspace] = useState(false)
  const authBlocking = Boolean(
    auth && ((!hasEnteredWorkspace && auth.state.status !== 'signedIn') || auth.loginRequested)
  )
  const { resolvedColorScheme, t } = useFrontendConfig()
  const supportsNativeTranslucency = isMacOS()
  const [interactive, setInteractive] = useState(false)
  const [overlayMounted, setOverlayMounted] = useState(true)
  const [timedOut, setTimedOut] = useState(false)
  const startupAttempt = startup?.attempt

  useEffect(() => {
    if (authBlocking) {
      setInteractive(false)
      setOverlayMounted(true)
    }
  }, [authBlocking])

  useEffect(() => {
    if (startupAttempt === undefined) return
    setInteractive(false)
    setOverlayMounted(true)
    setTimedOut(false)
  }, [startupAttempt])

  useEffect(() => {
    if (!startup || startup.ready || startup.hasFailed) return
    const remaining = Math.max(0, STARTUP_TIMEOUT_MS - (Date.now() - startup.startedAt))
    const timeoutId = window.setTimeout(() => setTimedOut(true), remaining)
    return () => window.clearTimeout(timeoutId)
  }, [startup])

  useEffect(() => {
    if (!startup?.ready || authBlocking) return
    const remaining = Math.max(0, MINIMUM_STARTUP_SCREEN_MS - (Date.now() - startup.startedAt))
    let exitTimeoutId: number | undefined
    const readyTimeoutId = window.setTimeout(() => {
      setInteractive(true)
      setHasEnteredWorkspace(true)
      exitTimeoutId = window.setTimeout(() => setOverlayMounted(false), STARTUP_EXIT_MS)
    }, remaining)
    return () => {
      window.clearTimeout(readyTimeoutId)
      if (exitTimeoutId !== undefined) window.clearTimeout(exitTimeoutId)
    }
  }, [startup?.ready, startup?.startedAt, authBlocking])

  if (!startup) return children

  const showFailure = startup.hasFailed || timedOut

  return (
    <div
      className="app-startup-root"
      data-interactive={interactive && !authBlocking ? 'true' : 'false'}
    >
      <div
        className="app-startup-workspace"
        aria-hidden={!interactive || authBlocking}
        inert={!interactive || authBlocking}
      >
        {children}
      </div>

      {overlayMounted || authBlocking ? (
        <div
          className="app-startup-screen"
          data-exiting={interactive && !authBlocking ? 'true' : 'false'}
          data-native-translucency={supportsNativeTranslucency ? 'true' : undefined}
          role={authBlocking ? 'dialog' : showFailure ? 'alert' : 'status'}
          aria-modal={authBlocking || undefined}
          aria-label={authBlocking ? t('auth.title') : undefined}
          aria-live={authBlocking ? undefined : showFailure ? 'assertive' : 'polite'}
        >
          <div className="app-startup-screen__drag-region" aria-hidden="true" />
          <div className="app-startup-screen__content">
            <img
              className="app-startup-screen__icon"
              src={resolvedColorScheme === 'dark' ? darkBrandMark : lightBrandMark}
              alt=""
              aria-hidden="true"
            />
            {authBlocking ? (
              <AccountLoginForm canDismiss={hasEnteredWorkspace} />
            ) : showFailure ? (
              <div className="app-startup-screen__failure">
                <strong>{t('startup.failedTitle')}</strong>
                <span>{t('startup.failedDescription')}</span>
                <button type="button" onClick={startup.retry}>
                  {t('startup.retry')}
                </button>
              </div>
            ) : (
              <>
                <StartupAmbientText />
                <span className="app-startup-screen__sr-only">{t('startup.loading')}</span>
              </>
            )}
          </div>
        </div>
      ) : null}
    </div>
  )
}
