import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AuthState, LicenseState } from '@mycopilot/host-api'
import { getTranslation } from '../../config/frontendTranslations'
import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { LicenseContext } from '../../features/license/LicenseContext'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { LeftSidebarAccountFooter } from '../shell/sidebar/LeftSidebarAccountFooter'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ resolvedColorScheme: 'light' })
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const requestLogin = vi.fn()
const logout = vi.fn()
const openSettings = vi.fn()
const requestAccess = vi.fn()
const refreshLicense = vi.fn()
const profile = {
  userId: 'captain',
  displayName: 'Captain',
  email: 'captain@example.com',
  avatarDataUrl: null,
  occupation: '',
  organization: ''
}

function renderFooter({
  authStatus = 'signedIn',
  licenseStatus = 'allowed',
  expiresAt = null
}: {
  authStatus?: AuthState['status']
  licenseStatus?: LicenseState['status']
  expiresAt?: string | null
} = {}) {
  const state: LicenseState = {
    revision: 1,
    status: licenseStatus,
    reason: licenseStatus === 'allowed' ? 'active' : null,
    expiresAt,
    verifiedAt: null,
    cacheValidUntil: null,
    error: null
  }
  return render(
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 1,
          status: authStatus,
          profile: authStatus === 'signedIn' ? profile : null,
          error: null,
          remembered: true
        },
        loginRequested: false,
        requestLogin,
        dismissLogin: vi.fn(),
        canStartTurn: () => authStatus === 'signedIn',
        logout
      }}
    >
      <LicenseContext.Provider
        value={{
          state,
          canStartTurn: () => authStatus === 'signedIn' && licenseStatus === 'allowed',
          requestAccess,
          refresh: async () => {
            refreshLicense()
            return state
          }
        }}
      >
        <LeftSidebarAccountFooter
          onOpenSettings={openSettings}
          t={(key) => getTranslation('zh-CN', key)}
          uiPreferences={defaultUiPreferences()}
        />
      </LicenseContext.Provider>
    </AccountAuthContext.Provider>
  )
}

function licenseText(container: HTMLElement): string | null | undefined {
  return container.querySelector('.left-sidebar__account-menu-license-value')?.textContent
}

async function openMenu(screen: Awaited<ReturnType<typeof renderFooter>>) {
  await screen.getByRole('button', { name: getTranslation('zh-CN', 'sidebar.accountMenu') }).click()
  await expect.element(screen.getByRole('menu')).toBeVisible()
}

beforeEach(() => {
  vi.clearAllMocks()
  logout.mockResolvedValue({ ok: true })
})

it('keeps the collapsed footer to avatar and one username, with email and Shanghai expiry only in the menu', async () => {
  const screen = await renderFooter({ expiresAt: '2026-09-13T16:00:00Z' })
  const collapsedText = screen.container.querySelector('.left-sidebar__account-text')!
  expect(collapsedText.querySelectorAll(':scope > span')).toHaveLength(1)
  expect(collapsedText.textContent).toBe(profile.displayName)
  expect(screen.container.querySelector('.left-sidebar__account-avatar')).not.toBeNull()
  expect(screen.container.textContent).not.toContain(profile.email)
  expect(screen.container.textContent).not.toContain('截止')
  expect(licenseText(screen.container)).toBeUndefined()
  await openMenu(screen)
  const menuHeader = screen.container.querySelector('.left-sidebar__account-menu-profile-text')!
  expect(Array.from(menuHeader.children, (element) => element.textContent)).toEqual([
    profile.displayName,
    profile.email
  ])
  expect(licenseText(screen.container)).toBe('截止 2026-09-14')
  expect(screen.container.textContent).not.toContain('长期有效')
})

it('places one read-only software license row after the divider and before Settings', async () => {
  const screen = await renderFooter()
  await openMenu(screen)
  const row = screen.container.querySelector('.left-sidebar__account-menu-license')!
  const divider = screen.container.querySelector('.left-sidebar__account-menu-divider')!
  const settings = screen
    .getByRole('menuitem', { name: getTranslation('zh-CN', 'profile.openSettings') })
    .element()
  expect(row.querySelector('.left-sidebar__account-menu-license-label')?.textContent).toBe(
    '软件许可'
  )
  const icon = row.querySelector('svg.captain-who-line-icon')!
  expect(icon).not.toBeNull()
  expect(icon.getAttribute('fill')).toBe('none')
  expect(icon.getAttribute('stroke')).toBe('currentColor')
  expect(icon.querySelector('path')).not.toBeNull()
  expect(row.matches('button, a, [role="button"], [role="menuitem"]')).toBe(false)
  expect(row.tagName).toBe('DIV')
  expect(row.hasAttribute('tabindex')).toBe(false)
  expect(row.querySelector('button, a, [tabindex]')).toBeNull()
  expect(divider.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  expect(row.compareDocumentPosition(settings) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  expect(screen.container.querySelectorAll('.left-sidebar__account-menu-license')).toHaveLength(1)
  ;(row as HTMLElement).click()
  expect(openSettings).not.toHaveBeenCalled()
  expect(requestLogin).not.toHaveBeenCalled()
  expect(logout).not.toHaveBeenCalled()
  expect(requestAccess).not.toHaveBeenCalled()
  expect(refreshLicense).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('menu')).toBeVisible()
})

it('shows long-term validity only in the license row for a signed-in allowed license with null expiry', async () => {
  const screen = await renderFooter()
  await openMenu(screen)
  expect(licenseText(screen.container)).toBe('长期有效')
  expect(screen.container.textContent).toContain(profile.email)
  expect(screen.container.querySelector('.left-sidebar__account-text')?.textContent).toBe(
    profile.displayName
  )
})

it('retains the known expiry date for an expired license without claiming it is active', async () => {
  const screen = await renderFooter({ licenseStatus: 'denied', expiresAt: '2026-09-12T16:00:00Z' })
  await openMenu(screen)
  expect(licenseText(screen.container)).toBe('截止 2026-09-13')
  expect(screen.container.textContent).not.toContain('长期有效')
})

it.each(['checking', 'unavailable', 'denied', 'signedOut'] as const)(
  'shows the real license state instead of long-term validity when expiry is null and status is %s',
  async (licenseStatus) => {
    const screen = await renderFooter({ licenseStatus })
    await openMenu(screen)
    const expected = getTranslation('zh-CN', `license.state.${licenseStatus}`)
    expect(licenseText(screen.container)).toBe(expected)
    expect(screen.container.textContent).not.toContain('长期有效')
    expect(screen.container.textContent).toContain(profile.email)
  }
)

it.each(['signedOut', 'checking', 'error'] as const)(
  'does not display a stale allowed license when account status is %s',
  async (authStatus) => {
    const screen = await renderFooter({ authStatus })
    await openMenu(screen)
    expect(licenseText(screen.container)).toBe('未登录')
    expect(screen.container.textContent).not.toContain('长期有效')
    expect(screen.container.textContent).not.toContain(profile.email)
  }
)

it('does not mistake an invalid expiration for a long-term license or render an invalid date', async () => {
  const screen = await renderFooter({ expiresAt: 'invalid-date' })
  await openMenu(screen)
  expect(licenseText(screen.container)).toBe('暂时无法验证')
  expect(screen.container.textContent).not.toContain('长期有效')
  expect(screen.container.textContent).not.toContain('Invalid Date')
})

it('preserves the signed-out login entry and closes the menu when selected', async () => {
  const screen = await renderFooter({ authStatus: 'signedOut', licenseStatus: 'signedOut' })
  await openMenu(screen)
  await screen.getByRole('menuitem', { name: '登录', exact: true }).click()
  expect(requestLogin).toHaveBeenCalledOnce()
  expect(logout).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
})

it('preserves logout and settings actions', async () => {
  const screen = await renderFooter()
  await openMenu(screen)
  await screen.getByRole('menuitem', { name: '退出登录' }).click()
  expect(logout).toHaveBeenCalledOnce()
  expect(requestLogin).not.toHaveBeenCalled()
  await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
  await openMenu(screen)
  await screen
    .getByRole('menuitem', { name: getTranslation('zh-CN', 'profile.openSettings') })
    .click()
  expect(openSettings).toHaveBeenCalledOnce()
  await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
})
