import { useEffect, useRef, useState } from 'react'
import type { AuthActionResult, AuthErrorCode } from '@mycopilot/host-api'
import { Eye, EyeOff } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { hostClient } from '../../host/hostClient'
import { useAccountAuth } from './AccountAuthContext'
import './AccountLoginForm.css'

export function AccountLoginForm({ canDismiss = false }: { canDismiss?: boolean }) {
  const auth = useAccountAuth()
  const { t } = useFrontendConfig()
  const [mode, setMode] = useState<'password' | 'code'>('password')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [code, setCode] = useState('')
  const [sentEmail, setSentEmail] = useState('')
  const [resendAt, setResendAt] = useState(0)
  const [seconds, setSeconds] = useState(0)
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const [error, setError] = useState<AuthErrorCode | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    inputRef.current?.focus()
  }, [auth?.state.status])
  useEffect(() => {
    const tick = (): void => setSeconds(Math.max(0, Math.ceil((resendAt - Date.now()) / 1000)))
    tick()
    const timer = window.setInterval(tick, 1000)
    return () => window.clearInterval(timer)
  }, [resendAt])
  if (!auth) return null
  const run = async (operation: () => Promise<AuthActionResult>): Promise<void> => {
    if (busyRef.current) return
    busyRef.current = true
    setBusy(true)
    setError(null)
    try {
      const result = await operation()
      if (!result.ok) setError(result.error)
    } catch {
      setError('unknown')
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }
  const open = (page: 'register' | 'reset'): void => {
    void hostClient.auth.openWebsite(page).catch(() => setError('unknown'))
  }
  const visibleError = error ?? auth.state.error
  const normalizedEmail = email.trim().toLowerCase()
  const checking = auth.state.status === 'checking'
  return (
    <section className="account-login" aria-label={t('auth.title')} aria-busy={busy || checking}>
      <h1>{t('auth.title')}</h1>
      {checking ? (
        <p role="status">{t('auth.checking')}</p>
      ) : (
        <>
          <div className="account-login__modes" role="group" aria-label={t('auth.login')}>
            {(['password', 'code'] as const).map((value) => (
              <button
                key={value}
                type="button"
                aria-pressed={mode === value}
                disabled={busy}
                onClick={() => {
                  setMode(value)
                  setPassword('')
                  setCode('')
                  setError(null)
                }}
              >
                {t(value === 'password' ? 'auth.passwordMode' : 'auth.codeMode')}
              </button>
            ))}
          </div>
          <form
            onSubmit={(event) => {
              event.preventDefault()
              if (mode === 'password') {
                const submittedPassword = password
                void run(() =>
                  hostClient.auth.login({ email: normalizedEmail, password: submittedPassword })
                )
                setPassword('')
              } else {
                void run(() => hostClient.auth.verifyEmailCode({ email: normalizedEmail, code }))
                setCode('')
              }
            }}
          >
            <label>
              {t('auth.email')}
              <input
                ref={inputRef}
                type="email"
                autoComplete="username"
                required
                maxLength={254}
                value={email}
                disabled={busy}
                onChange={(event) => {
                  setEmail(event.target.value)
                  setCode('')
                  setSentEmail('')
                }}
              />
            </label>
            {mode === 'password' ? (
              <label>
                {t('auth.password')}
                <span className="account-login__password-field">
                  <input
                    type={showPassword ? 'text' : 'password'}
                    autoComplete="current-password"
                    required
                    maxLength={1024}
                    value={password}
                    disabled={busy}
                    onChange={(event) => setPassword(event.target.value)}
                  />
                  <button
                    className="account-login__password-toggle"
                    type="button"
                    aria-label={t(showPassword ? 'auth.hidePassword' : 'auth.showPassword')}
                    aria-pressed={showPassword}
                    disabled={busy}
                    onClick={() => setShowPassword((visible) => !visible)}
                  >
                    {showPassword ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
                  </button>
                </span>
              </label>
            ) : (
              <>
                <div className="account-login__code-row">
                  <label>
                    {t('auth.code')}
                    <input
                      inputMode="numeric"
                      autoComplete="one-time-code"
                      pattern="[0-9]{4,8}"
                      required
                      maxLength={8}
                      value={code}
                      disabled={busy}
                      onChange={(event) => setCode(event.target.value)}
                    />
                  </label>
                  <button
                    type="button"
                    disabled={
                      busy || seconds > 0 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(normalizedEmail)
                    }
                    onClick={() =>
                      void run(async () => {
                        const result = await hostClient.auth.sendEmailCode(normalizedEmail)
                        if (result.ok) {
                          setSentEmail(normalizedEmail)
                          setResendAt(Date.now() + 60_000)
                        }
                        return result
                      })
                    }
                  >
                    {seconds > 0 ? `${seconds}s` : t('auth.sendCode')}
                  </button>
                </div>
                {sentEmail === normalizedEmail && sentEmail ? (
                  <p role="status">{t('auth.codeSent')}</p>
                ) : null}
              </>
            )}
            <button
              className="account-login__submit"
              type="submit"
              disabled={busy || (mode === 'code' && (!sentEmail || sentEmail !== normalizedEmail))}
            >
              {busy ? t('auth.working') : t('auth.login')}
            </button>
          </form>
        </>
      )}
      {visibleError ? (
        <p className="account-login__error" role="alert">
          {t(`auth.error.${visibleError}`)}
        </p>
      ) : null}
      {!checking && (auth.state.status === 'error' || auth.state.error === 'network') ? (
        <button
          type="button"
          disabled={busy}
          onClick={() => void run(() => hostClient.auth.restoreSession())}
        >
          {t('auth.retry')}
        </button>
      ) : null}
      <div className="account-login__links">
        <button type="button" onClick={() => open('register')}>
          {t('auth.register')}
        </button>
        <button type="button" onClick={() => open('reset')}>
          {t('auth.reset')}
        </button>
      </div>
      {canDismiss ? (
        <button type="button" onClick={auth.dismissLogin}>
          {t('auth.cancel')}
        </button>
      ) : null}
    </section>
  )
}
