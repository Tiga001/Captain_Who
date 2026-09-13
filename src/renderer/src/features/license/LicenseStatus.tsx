import { useState } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { useLicense } from './LicenseContext'
import '../auth/AccountLoginForm.css'
import './LicenseStatus.css'

export function LicenseStatus({ startup = false }: { startup?: boolean }) {
  const license = useLicense()
  const auth = useAccountAuth()
  const { t, language } = useFrontendConfig()
  const [busy, setBusy] = useState(false)
  if (!license) return null
  const { state } = license
  const expires = state.expiresAt ? new Date(state.expiresAt) : null
  const hasNoExpiry = state.status === 'allowed' && state.expiresAt === null
  const checking = state.status === 'checking' || busy
  return (
    <section
      className={startup ? 'account-login license-status' : 'settings-list-section license-status'}
      aria-labelledby={startup ? 'startup-license-title' : 'profile-license-title'}
      aria-busy={checking}
      data-setting-id={startup ? undefined : 'profile.license'}
    >
      {startup ? (
        <h1 id="startup-license-title">{t('license.title')}</h1>
      ) : (
        <h2 id="profile-license-title">{t('license.title')}</h2>
      )}
      <div className={startup ? undefined : 'settings-list'}>
        <div className={startup ? undefined : 'settings-list-row'}>
          {!startup ? <span>{t('license.status')}</span> : null}
          <span role="status">{t(`license.state.${state.status}`)}</span>
        </div>
        {state.status === 'denied' && state.reason && state.reason !== 'active' ? (
          <p role="alert">{t(`license.reason.${state.reason}`)}</p>
        ) : null}
        {state.error ? <p role="alert">{t(`license.error.${state.error}`)}</p> : null}
        {expires && Number.isFinite(expires.getTime()) ? (
          <div className={startup ? undefined : 'settings-list-row'}>
            <span>{t('license.expiresAt')}</span>
            <time dateTime={state.expiresAt!}>
              {expires.toLocaleString(language, { timeZone: 'Asia/Shanghai' })}
            </time>
          </div>
        ) : null}
        {hasNoExpiry ? (
          <div className={startup ? undefined : 'settings-list-row'}>
            <span>{t('license.validity')}</span>
            <span>{t('license.noExpiry')}</span>
          </div>
        ) : null}
        {state.status !== 'signedOut' ? (
          <div
            className={startup ? undefined : 'settings-list-row profile-settings-avatar-actions'}
          >
            {state.status === 'denied' || state.error === 'notProvisioned' ? (
              <button
                className={startup ? undefined : 'profile-settings-button'}
                type="button"
                onClick={license.requestAccess}
              >
                {t('license.manage')}
              </button>
            ) : null}
            <button
              className={startup ? undefined : 'profile-settings-button'}
              type="button"
              disabled={checking}
              onClick={async () => {
                setBusy(true)
                try {
                  await license.refresh()
                } finally {
                  setBusy(false)
                }
              }}
            >
              {t(checking ? 'license.state.checking' : 'license.retry')}
            </button>
          </div>
        ) : null}
      </div>
      {startup ? (
        <button type="button" onClick={() => void auth?.logout()}>
          {t('auth.logout')}
        </button>
      ) : null}
    </section>
  )
}
