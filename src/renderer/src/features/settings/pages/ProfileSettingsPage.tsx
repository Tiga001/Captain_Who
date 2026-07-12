import { useState } from 'react'
import { UserCircle } from 'lucide-react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import {
  getDefaultProfileDisplayName,
  getProfileDisplayName,
  getProfileHandle,
  getProfileInitials,
  normalizeProfileDisplayName
} from '../../profile/profileUtils'
import { selectProfileAvatar } from '../../storage/storageClient'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import './ProfileSettingsPage.css'

interface ProfileSettingsPageProps {
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  uiPreferences: UiPreferencesSnapshot
}

export function ProfileSettingsPage({
  onUiPreferencesChange,
  uiPreferences
}: ProfileSettingsPageProps) {
  const { language, t } = useFrontendConfig()
  const [avatarError, setAvatarError] = useState('')
  const [isRemoveAvatarConfirmationOpen, setIsRemoveAvatarConfirmationOpen] = useState(false)
  const displayName = getProfileDisplayName(uiPreferences, language)
  const handle = getProfileHandle(uiPreferences)
  const initials = getProfileInitials(displayName)
  const explicitDisplayName = normalizeProfileDisplayName(uiPreferences.profileDisplayName)
  const profileAvatarDataUrl = uiPreferences.profileAvatarDataUrl?.trim()
    ? uiPreferences.profileAvatarDataUrl
    : null
  const hasCustomAvatar = Boolean(profileAvatarDataUrl)

  const uploadAvatar = async () => {
    setAvatarError('')
    try {
      const avatarDataUrl = await selectProfileAvatar()
      if (!avatarDataUrl) return
      onUiPreferencesChange({ profileAvatarDataUrl: avatarDataUrl })
    } catch (error) {
      setAvatarError(error instanceof Error ? error.message : t('profile.avatarUploadFailed'))
    }
  }

  const removeAvatar = () => {
    if (!hasCustomAvatar) return
    setAvatarError('')
    onUiPreferencesChange({ profileAvatarDataUrl: null })
    setIsRemoveAvatarConfirmationOpen(false)
  }

  return (
    <article className="settings-list-page profile-settings-page">
      <h1>{t('settings.page.profile')}</h1>

      <section className="profile-settings-hero" aria-label={t('profile.account')}>
        <div className="profile-settings-avatar" aria-label={t('profile.avatar')}>
          {profileAvatarDataUrl ? (
            <img src={profileAvatarDataUrl} alt="" />
          ) : (
            <span>{initials}</span>
          )}
        </div>
        <div className="profile-settings-hero__text">
          <h2>{displayName}</h2>
          <p>@{handle}</p>
        </div>
      </section>

      <section className="settings-list-section" aria-labelledby="profile-account-heading">
        <h2 id="profile-account-heading">{t('profile.account')}</h2>
        <div className="settings-list">
          <div className="settings-list-row profile-settings-avatar-row">
            <div className="settings-list-row__text">
              <h3 className="settings-list-row__title">{t('profile.avatar')}</h3>
              {avatarError && <p className="profile-settings-error">{avatarError}</p>}
            </div>

            <div className="settings-list-row__control profile-settings-avatar-actions">
              <button className="profile-settings-button" type="button" onClick={uploadAvatar}>
                <UserCircle aria-hidden="true" />
                <span>{t('profile.uploadAvatar')}</span>
              </button>
              {hasCustomAvatar && (
                <button
                  className="profile-settings-button profile-settings-button--danger"
                  type="button"
                  onClick={() => setIsRemoveAvatarConfirmationOpen(true)}
                >
                  {t('profile.removeAvatar')}
                </button>
              )}
            </div>
          </div>

          <label className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('profile.displayName')}</span>
              <span className="settings-list-row__description">
                {t('profile.displayNameDescription')}
              </span>
            </span>

            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                value={explicitDisplayName}
                placeholder={getDefaultProfileDisplayName(language)}
                onChange={(event) =>
                  onUiPreferencesChange({ profileDisplayName: event.target.value })
                }
              />
            </span>
          </label>

          <label className="settings-list-row">
            <span className="settings-list-row__text">
              <span className="settings-list-row__title">{t('profile.handle')}</span>
              <span className="settings-list-row__description">
                {t('profile.handleDescription')}
              </span>
            </span>

            <span className="settings-list-row__control">
              <input
                className="settings-list-control"
                value={handle}
                placeholder={t('profile.handlePlaceholder')}
                onChange={(event) => onUiPreferencesChange({ profileHandle: event.target.value })}
              />
            </span>
          </label>
        </div>
      </section>

      {isRemoveAvatarConfirmationOpen && hasCustomAvatar && (
        <ConfirmationDialog
          title={t('profile.removeAvatarTitle')}
          cancelLabel={t('profile.cancelRemoveAvatar')}
          confirmLabel={t('profile.confirmRemoveAvatar')}
          onCancel={() => setIsRemoveAvatarConfirmationOpen(false)}
          onConfirm={removeAvatar}
        />
      )}
    </article>
  )
}
