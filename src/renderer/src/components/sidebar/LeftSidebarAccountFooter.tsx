// Account footer menu and profile display for the left sidebar.
import { Settings } from 'lucide-react'
import { useRef, useState } from 'react'
import type { AppLanguage, TranslationKey } from '../../config/frontendTranslations'
import {
  getProfileDisplayName,
  getProfileHandle,
  getProfileInitials
} from '../../features/profile/profileUtils'
import type { UiPreferencesSnapshot } from '../../features/storage/storageClient'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'

interface LeftSidebarAccountFooterProps {
  language: AppLanguage
  onOpenSettings: () => void
  t: (key: TranslationKey) => string
  uiPreferences: UiPreferencesSnapshot
}

export function LeftSidebarAccountFooter({
  language,
  onOpenSettings,
  t,
  uiPreferences
}: LeftSidebarAccountFooterProps) {
  const [isAccountMenuOpen, setAccountMenuOpen] = useState(false)
  const accountMenuRef = useRef<HTMLDivElement>(null)
  const profileDisplayName = getProfileDisplayName(uiPreferences, language)
  const profileHandle = getProfileHandle(uiPreferences)
  const profileInitials = getProfileInitials(profileDisplayName)

  useDismissOnOutsidePointer(accountMenuRef, isAccountMenuOpen, () => setAccountMenuOpen(false))

  return (
    <div
      className="left-sidebar__footer"
      data-menu-open={isAccountMenuOpen || undefined}
      ref={accountMenuRef}
    >
      {isAccountMenuOpen && (
        <div
          className="left-sidebar__account-menu"
          role="menu"
          aria-label={t('sidebar.accountMenu')}
        >
          <div className="left-sidebar__account-menu-profile" aria-hidden="true">
            <span className="left-sidebar__account-avatar left-sidebar__account-avatar--small">
              {uiPreferences.profileAvatarDataUrl ? (
                <img src={uiPreferences.profileAvatarDataUrl} alt="" />
              ) : (
                <span>{profileInitials}</span>
              )}
            </span>
            <span className="left-sidebar__account-menu-profile-text">
              <span>{profileDisplayName}</span>
              <span>@{profileHandle}</span>
            </span>
          </div>

          <div className="left-sidebar__account-menu-divider" />

          <button
            className="left-sidebar__account-menu-item"
            type="button"
            role="menuitem"
            onClick={() => {
              setAccountMenuOpen(false)
              onOpenSettings()
            }}
          >
            <Settings aria-hidden="true" />
            <span>{t('profile.openSettings')}</span>
          </button>
        </div>
      )}

      <button
        className="left-sidebar__account-button"
        type="button"
        aria-label={t('sidebar.accountMenu')}
        aria-expanded={isAccountMenuOpen}
        onClick={() => setAccountMenuOpen((isOpen) => !isOpen)}
      >
        <span className="left-sidebar__account-avatar">
          {uiPreferences.profileAvatarDataUrl ? (
            <img src={uiPreferences.profileAvatarDataUrl} alt="" />
          ) : (
            <span>{profileInitials}</span>
          )}
        </span>
        <span className="left-sidebar__account-text">
          <span>{profileDisplayName}</span>
          <span>@{profileHandle}</span>
        </span>
      </button>
    </div>
  )
}
