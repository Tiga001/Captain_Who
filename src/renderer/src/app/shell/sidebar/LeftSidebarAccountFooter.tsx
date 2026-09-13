// Account footer menu and profile display for the left sidebar.
import { Settings, LogIn, LogOut } from 'lucide-react'
import { useRef, useState } from 'react'
import type { LicenseState } from '@mycopilot/host-api'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { useAccountAuth } from '../../../features/auth/AccountAuthContext'
import { AccountAvatar } from '../../../features/auth/AccountAvatar'
import { CaptainWhoLineIcon } from '../../../components/icons/CaptainWhoLineIcon'
import { useLicense } from '../../../features/license/LicenseContext'
import type { UiPreferencesSnapshot } from '../../../features/storage/storageClient'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import { LeftSidebarUpdateButton } from './LeftSidebarUpdateButton'

interface LeftSidebarAccountFooterProps {
  onOpenSettings: () => void
  t: (key: TranslationKey) => string
  uiPreferences: UiPreferencesSnapshot
}

const licenseDateFormatter = new Intl.DateTimeFormat('en-CA', {
  timeZone: 'Asia/Shanghai',
  year: 'numeric',
  month: '2-digit',
  day: '2-digit'
})

function licenseSubtitle(
  state: LicenseState | undefined,
  t: LeftSidebarAccountFooterProps['t']
): string {
  if (!state) return t('license.state.checking')
  if (state.status === 'signedOut') return t('license.state.signedOut')
  if (state.expiresAt === null) {
    return t(state.status === 'allowed' ? 'license.noExpiry' : `license.state.${state.status}`)
  }

  const expires = new Date(state.expiresAt)
  if (!Number.isFinite(expires.getTime())) return t('license.state.unavailable')
  const parts = licenseDateFormatter.formatToParts(expires)
  const date = ['year', 'month', 'day']
    .map((type) => parts.find((part) => part.type === type)?.value)
    .join('-')
  return `${t('license.until')} ${date}`
}

export function LeftSidebarAccountFooter({ onOpenSettings, t }: LeftSidebarAccountFooterProps) {
  const [isAccountMenuOpen, setAccountMenuOpen] = useState(false)
  const accountMenuRef = useRef<HTMLDivElement>(null)
  const auth = useAccountAuth()
  const license = useLicense()
  const [logoutError, setLogoutError] = useState(false)
  const profile = auth?.state.status === 'signedIn' ? auth.state.profile : null
  const profileDisplayName = profile?.displayName || t('auth.signedOut')
  const licenseValue =
    auth?.state.status === 'signedIn'
      ? licenseSubtitle(license?.state, t)
      : t('license.state.signedOut')

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
              {profile?.email ? <span>{profile.email}</span> : null}
            </span>
          </div>

          <div className="left-sidebar__account-menu-divider" />

          <div className="left-sidebar__account-menu-license">
            <CaptainWhoLineIcon />
            <span className="left-sidebar__account-menu-license-label">
              {t('license.menuLabel')}
            </span>
            <span className="left-sidebar__account-menu-license-value" title={licenseValue}>
              {licenseValue}
            </span>
          </div>

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
        </span>
      </button>
      <LeftSidebarUpdateButton t={t} />
    </div>
  )
}
