import { renderSettingsNodes, settingLabel, settingDescription } from '../settingsDefinition'
import { profileSettingsNodes } from './ProfileSettingsPage.definition'
import { useState } from 'react'
import { UserCircle } from 'lucide-react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'
import {
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
  const { t } = useFrontendConfig()
  const [avatarError, setAvatarError] = useState('')
  const [isRemoveAvatarConfirmationOpen, setIsRemoveAvatarConfirmationOpen] = useState(false)
  const defaultDisplayName = t('profile.defaultDisplayName')
  const displayName = getProfileDisplayName(uiPreferences, defaultDisplayName)
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
      setAvatarError(getUserFacingErrorMessage(error, t, 'profile.avatarUploadFailed'))
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

      {renderSettingsNodes(profileSettingsNodes, (section) => (
        <section className="settings-list-section" aria-labelledby="profile-account-heading">
          <h2 id="profile-account-heading">{settingLabel(section, t)}</h2>
          <div className="settings-list">
            {renderSettingsNodes(section.children, (node) => {
              switch (node.id) {
                case 'profile.avatar':
                  return (
                    <div className="settings-list-row profile-settings-avatar-row">
                      <div className="settings-list-row__text">
                        <h3 className="settings-list-row__title">{settingLabel(node, t)}</h3>
                        {avatarError && <p className="profile-settings-error">{avatarError}</p>}
                      </div>

                      <div className="settings-list-row__control profile-settings-avatar-actions">
                        <button
                          className="profile-settings-button"
                          type="button"
                          onClick={uploadAvatar}
                        >
                          <UserCircle aria-hidden="true" />
                          <span>{t(node.terms[0])}</span>
                        </button>
                        {hasCustomAvatar && (
                          <button
                            className="profile-settings-button profile-settings-button--danger"
                            type="button"
                            onClick={() => setIsRemoveAvatarConfirmationOpen(true)}
                          >
                            {t(node.terms[1])}
                          </button>
                        )}
                      </div>
                    </div>
                  )
                case 'profile.displayName':
                  return (
                    <label className="settings-list-row">
                      <span className="settings-list-row__text">
                        <span className="settings-list-row__title">{settingLabel(node, t)}</span>
                        <span className="settings-list-row__description">
                          {settingDescription(node, t)}
                        </span>
                      </span>

                      <span className="settings-list-row__control">
                        <input
                          className="settings-list-control"
                          value={explicitDisplayName}
                          placeholder={defaultDisplayName}
                          onChange={(event) =>
                            onUiPreferencesChange({ profileDisplayName: event.target.value })
                          }
                        />
                      </span>
                    </label>
                  )
                case 'profile.handle':
                  return (
                    <label className="settings-list-row">
                      <span className="settings-list-row__text">
                        <span className="settings-list-row__title">{settingLabel(node, t)}</span>
                        <span className="settings-list-row__description">
                          {settingDescription(node, t)}
                        </span>
                      </span>

                      <span className="settings-list-row__control">
                        <input
                          className="settings-list-control"
                          value={handle}
                          placeholder={t('profile.handlePlaceholder')}
                          onChange={(event) =>
                            onUiPreferencesChange({ profileHandle: event.target.value })
                          }
                        />
                      </span>
                    </label>
                  )
                default:
                  return null
              }
            })}
          </div>
        </section>
      ))}

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
