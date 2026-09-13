// Account footer menu and profile display for the left sidebar.
import { Settings, LogIn, LogOut } from 'lucide-react'
import { useRef, useState } from 'react'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { useAccountAuth } from '../../../features/auth/AccountAuthContext'
import { AccountAvatar } from '../../../features/auth/AccountAvatar'
import type { UiPreferencesSnapshot } from '../../../features/storage/storageClient'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'

interface LeftSidebarAccountFooterProps {
  onOpenSettings: () => void
  t: (key: TranslationKey) => string
  uiPreferences: UiPreferencesSnapshot
}

export function LeftSidebarAccountFooter({ onOpenSettings, t }: LeftSidebarAccountFooterProps) {
  const [isAccountMenuOpen, setAccountMenuOpen] = useState(false)
  const accountMenuRef = useRef<HTMLDivElement>(null)
  const auth = useAccountAuth()
  const [logoutError, setLogoutError] = useState(false)
  const profile = auth?.state.profile
  const profileDisplayName = profile?.displayName || t('auth.signedOut')
  const profileEmail = profile?.email || t('auth.login')

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
              <AccountAvatar src={profile?.avatarDataUrl} />
            </span>
            <span className="left-sidebar__account-menu-profile-text">
              <span>{profileDisplayName}</span>
              <span>{profileEmail}</span>
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
          <button
            className="left-sidebar__account-menu-item"
            type="button"
            role="menuitem"
            onClick={() => {
              setAccountMenuOpen(false)
              setLogoutError(false)
              if (auth?.state.status === 'signedIn') {
                void auth.logout().then((result) => setLogoutError(!result.ok))
              } else auth?.requestLogin()
            }}
          >
            {auth?.state.status === 'signedIn' ? (
              <LogOut aria-hidden="true" />
            ) : (
              <LogIn aria-hidden="true" />
            )}
            <span>{t(auth?.state.status === 'signedIn' ? 'auth.logout' : 'auth.login')}</span>
          </button>
        </div>
      )}
      {logoutError ? <p role="alert">{t('auth.error.storage')}</p> : null}

      <button
        className="left-sidebar__account-button"
        type="button"
        aria-label={t('sidebar.accountMenu')}
        aria-expanded={isAccountMenuOpen}
        onClick={() => setAccountMenuOpen((isOpen) => !isOpen)}
      >
        <span className="left-sidebar__account-avatar">
          <AccountAvatar src={profile?.avatarDataUrl} />
        </span>
        <span className="left-sidebar__account-text">
          <span>{profileDisplayName}</span>
          <span>{profileEmail}</span>
        </span>
      </button>
    </div>
  )
}
