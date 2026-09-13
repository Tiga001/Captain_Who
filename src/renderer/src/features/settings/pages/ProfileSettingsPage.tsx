import { useEffect, useState } from 'react'
import type { AuthErrorCode } from '@mycopilot/host-api'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { hostClient } from '../../../host/hostClient'
import { useAccountAuth } from '../../auth/AccountAuthContext'
import { AccountAvatar } from '../../auth/AccountAvatar'
import './ProfileSettingsPage.css'

export function ProfileSettingsPage() {
  const { t } = useFrontendConfig()
  const auth = useAccountAuth()
  const profile = auth?.state.profile
  const userId = profile?.userId
  const [error, setError] = useState<AuthErrorCode | null>(null)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    if (!userId) return
    let cancelled = false
    void hostClient.auth
      .refreshProfile()
      .then((result) => {
        if (!cancelled && !result.ok) setError(result.error)
      })
      .catch(() => {
        if (!cancelled) setError('unknown')
      })
    return () => {
      cancelled = true
    }
  }, [userId])
  return (
    <article className="settings-list-page profile-settings-page" data-setting-id="profile.account">
      <h1>{t('settings.page.profile')}</h1>
      <section className="profile-settings-hero" aria-label={t('auth.cloudProfile')}>
        <div
          className="profile-settings-avatar"
          aria-label={t('profile.avatar')}
          data-setting-id="profile.avatar"
        >
          <AccountAvatar src={profile?.avatarDataUrl} />
          <span className="app-startup-screen__sr-only">{t('profile.avatar')}</span>
        </div>
        <div className="profile-settings-hero__text">
          <h2>{profile?.displayName || t('auth.signedOut')}</h2>
          {profile?.email ? <p>{profile.email}</p> : null}
        </div>
      </section>
      <section className="settings-list-section" aria-labelledby="profile-account-heading">
        <h2 id="profile-account-heading">{t('auth.cloudProfile')}</h2>
        <div className="settings-list">
          <div className="settings-list-row" data-setting-id="profile.displayName">
            <span>{t('profile.displayName')}</span>
            <span>{profile?.displayName || '—'}</span>
          </div>
          <div className="settings-list-row" data-setting-id="profile.email">
            <span>{t('auth.email')}</span>
            <span>{profile?.email || '—'}</span>
          </div>
          {profile ? (
            <div className="settings-list-row profile-settings-avatar-actions">
              <button
                className="profile-settings-button"
                type="button"
                onClick={() => {
                  void hostClient.auth.openWebsite('profile').catch(() => setError('unknown'))
                }}
              >
                {t('auth.editProfile')}
              </button>
              <button
                className="profile-settings-button"
                type="button"
                disabled={busy}
                onClick={async () => {
                  setBusy(true)
                  setError(null)
                  try {
                    const result = await hostClient.auth.refreshProfile()
                    if (!result.ok) setError(result.error)
                  } catch {
                    setError('unknown')
                  } finally {
                    setBusy(false)
                  }
                }}
              >
                {t(busy ? 'auth.working' : 'auth.refresh')}
              </button>
            </div>
          ) : null}
        </div>
      </section>
      {error ? <p role="alert">{t(`auth.error.${error}`)}</p> : null}
      {profile && !auth?.state.remembered ? <p role="status">{t('auth.memoryOnly')}</p> : null}
    </article>
  )
}
