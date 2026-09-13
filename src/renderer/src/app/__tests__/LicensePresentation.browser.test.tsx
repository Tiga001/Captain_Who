import type { ReactNode } from 'react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { cdp, page } from 'vitest/browser'
import type { CDPSession } from '@vitest/browser-playwright'
import { render } from 'vitest-browser-react'
import { getTranslation } from '../../config/frontendTranslations'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getFrontendTheme } from '../../config/frontendTheme'
import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { LicenseContext } from '../../features/license/LicenseContext'
import { ProfileSettingsPage } from '../../features/settings/pages/ProfileSettingsPage'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { LeftSidebarAccountFooter } from '../shell/sidebar/LeftSidebarAccountFooter'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'
import '../../features/startup/AppStartupScreen.css'
import '../shell/sidebar/LeftSidebar.css'

const mocks = vi.hoisted(() => ({
  dark: false,
  refreshProfile: vi.fn(),
  localUsage: vi.fn(),
  refreshLicense: vi.fn(),
  requestAccess: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    auth: { refreshProfile: mocks.refreshProfile, openWebsite: vi.fn() },
    agent: { getLocalTokenUsage: mocks.localUsage }
  }
}))
vi.mock('../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../config/frontendTranslations')
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      resolvedColorScheme: mocks.dark ? 'dark' : 'light',
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
    })
  }
})

const expiresAt = '2027-09-13T16:00:00Z'

function FixtureProviders({ children }: { children: ReactNode }) {
  return (
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 1,
          status: 'signedIn',
          profile: {
            userId: 'local-visual-fixture',
            displayName: 'Captain',
            email: 'captain@example.com',
            avatarDataUrl: null,
            occupation: '',
            organization: ''
          },
          remembered: true,
          error: null
        },
        loginRequested: false,
        requestLogin: vi.fn(),
        dismissLogin: vi.fn(),
        canStartTurn: () => true,
        logout: async () => ({ ok: true })
      }}
    >
      <LicenseContext.Provider
        value={{
          state: {
            revision: 1,
            status: 'allowed',
            reason: 'active',
            expiresAt,
            verifiedAt: '2026-09-13T04:00:00Z',
            cacheValidUntil: '2026-09-14T04:00:00Z',
            error: null
          },
          canStartTurn: () => true,
          requestAccess: mocks.requestAccess,
          refresh: mocks.refreshLicense
        }}
      >
        {children}
      </LicenseContext.Provider>
    </AccountAuthContext.Provider>
  )
}

let previousStyle: string | null
let previousScheme: string | null
let previousViewport: { width: number; height: number }
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] })
  vi.setSystemTime(new Date('2026-09-13T04:00:00Z'))
  vi.clearAllMocks()
  mocks.refreshProfile.mockResolvedValue({ ok: true })
  mocks.localUsage.mockResolvedValue({
    timezone: 'Asia/Shanghai',
    startedAt: Date.UTC(2026, 0, 1),
    days: [
      { date: '2026-09-10', tokenCount: '4800' },
      { date: '2026-09-11', tokenCount: '8200' },
      { date: '2026-09-12', tokenCount: '12000' },
      { date: '2026-09-13', tokenCount: '6000' }
    ],
    todayTokens: '6000',
    totalTokens: '31000',
    peakDailyTokens: '12000',
    unreportedRequestCount: 0
  })
  previousStyle = document.documentElement.getAttribute('style')
  previousScheme = document.documentElement.getAttribute('data-color-scheme')
  previousViewport = { width: innerWidth, height: innerHeight }
})
afterEach(async () => {
  vi.useRealTimers()
  if (previousStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousStyle)
  if (previousScheme === null) document.documentElement.removeAttribute('data-color-scheme')
  else document.documentElement.setAttribute('data-color-scheme', previousScheme)
  await page.viewport(previousViewport.width, previousViewport.height)
  if (import.meta.env.VITE_CAPTAIN_WHO_LICENSE_SCREENSHOT_DIR)
    await (cdp() as CDPSession).send('Emulation.clearDeviceMetricsOverride')
})

it.each(['classic-light', 'classic-dark'] as const)(
  'keeps the compact account button, menu email and read-only license row readable in %s',
  async (themeId) => {
    mocks.dark = themeId === 'classic-dark'
    document.documentElement.setAttribute('data-color-scheme', mocks.dark ? 'dark' : 'light')
    const theme = getFrontendTheme(themeId)
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens)))
      document.documentElement.style.setProperty(key, value)
    const screenshotDir = import.meta.env.VITE_CAPTAIN_WHO_LICENSE_SCREENSHOT_DIR
    // Keep the Vitest frame at native scale when exporting the tall profile fixture.
    if (screenshotDir)
      await (cdp() as CDPSession).send('Emulation.setDeviceMetricsOverride', {
        width: 1280,
        height: 1600,
        deviceScaleFactor: 1,
        mobile: false
      })
    await page.viewport(1060, 1500)
    const footer = await render(
      <FixtureProviders>
        <aside
          className="left-sidebar"
          style={{
            width: 280,
            height: 320,
            background: 'var(--mc-color-surface-main-panel)'
          }}
        >
          <LeftSidebarAccountFooter
            onOpenSettings={vi.fn()}
            t={(key) => getTranslation('zh-CN', key)}
            uiPreferences={defaultUiPreferences()}
          />
        </aside>
      </FixtureProviders>
    )
    await document.fonts.ready
    const accountBar = footer.container.querySelector('.left-sidebar__footer')!
    const accountButton = footer.container.querySelector('.left-sidebar__account-button')!
    const accountText = footer.container.querySelector('.left-sidebar__account-text')!
    const accountAvatar = accountButton.querySelector('.left-sidebar__account-avatar')!
    const barRect = accountBar.getBoundingClientRect()
    const buttonRect = accountButton.getBoundingClientRect()
    const avatarRect = accountAvatar.getBoundingClientRect()
    const textRect = accountText.getBoundingClientRect()
    expect(barRect.height).toBe(48)
    expect(avatarRect.width).toBe(28)
    expect(avatarRect.height).toBe(28)
    expect(
      Math.abs(avatarRect.top + avatarRect.height / 2 - (barRect.top + barRect.height / 2))
    ).toBeLessThanOrEqual(1)
    expect(
      Math.abs(textRect.top + textRect.height / 2 - (avatarRect.top + avatarRect.height / 2))
    ).toBeLessThanOrEqual(1)
    expect(buttonRect.left).toBeGreaterThanOrEqual(barRect.left)
    expect(buttonRect.right).toBeLessThanOrEqual(barRect.right)
    expect(accountButton.scrollWidth).toBeLessThanOrEqual(accountButton.clientWidth)
    expect(accountButton.scrollHeight).toBeLessThanOrEqual(accountButton.clientHeight)
    expect(accountText.children).toHaveLength(1)
    expect(accountText.textContent).toBe('Captain')
    expect(footer.container.textContent).not.toContain('captain@example.com')
    if (screenshotDir)
      await page.screenshot({
        element: accountBar,
        path: `${screenshotDir}/account-bar-${themeId}.png`
      })
    await footer
      .getByRole('button', { name: getTranslation('zh-CN', 'sidebar.accountMenu') })
      .click()
    await expect.element(footer.getByRole('menu')).toBeVisible()
    expect(accountText.children).toHaveLength(1)
    expect(accountText.textContent).toBe('Captain')
    const menuEmail = footer.container.querySelector(
      '.left-sidebar__account-menu-profile-text > span:last-child'
    )!
    expect(menuEmail.textContent).toBe('captain@example.com')
    const license = footer.container.querySelector('.left-sidebar__account-menu-license')!
    const licenseValue = license.querySelector('.left-sidebar__account-menu-license-value')!
    expect(license.querySelector('.left-sidebar__account-menu-license-label')?.textContent).toBe(
      '软件许可'
    )
    expect(licenseValue.textContent).toBe('截止 2027-09-14')
    expect(license.matches('button, [role="menuitem"]')).toBe(false)
    expect(license.querySelector('svg.captain-who-line-icon')?.getAttribute('stroke')).toBe(
      'currentColor'
    )
    expect(license.querySelector('svg.captain-who-line-icon')?.getAttribute('fill')).toBe('none')
    expect(
      footer.container
        .querySelector('.left-sidebar__account-menu-profile .left-sidebar__account-avatar')!
        .getBoundingClientRect().width
    ).toBe(30)
    for (const text of [accountText, menuEmail, license, licenseValue]) {
      expect(text.scrollWidth).toBeLessThanOrEqual(text.clientWidth)
    }
    expect(footer.container.textContent).toContain('captain@example.com')
    await document.fonts.ready
    if (screenshotDir)
      await page.screenshot({
        element: footer.container.querySelector('.left-sidebar')!,
        path: `${screenshotDir}/footer-${themeId}.png`
      })
    await footer.unmount()

    const profile = await render(
      <FixtureProviders>
        <div
          className="settings-page"
          style={{ display: 'block', position: 'relative', padding: 40 }}
        >
          <ProfileSettingsPage />
        </div>
      </FixtureProviders>
    )
    await expect.element(profile.getByText('31,000', { exact: true })).toBeVisible()
    await expect.element(profile.getByText('软件许可证', { exact: true })).toBeVisible()
    expect(profile.container.querySelector('time')?.getAttribute('datetime')).toBe(expiresAt)
    expect(profile.container.textContent).toContain('captain@example.com')
    const content = profile.container.querySelector('.profile-settings-page')!
    expect(content.scrollWidth).toBeLessThanOrEqual(content.clientWidth)
    expect(mocks.refreshProfile).toHaveBeenCalledOnce()
    expect(mocks.localUsage).toHaveBeenCalledOnce()
    expect(mocks.refreshLicense).not.toHaveBeenCalled()
    expect(mocks.requestAccess).not.toHaveBeenCalled()
    await document.fonts.ready
    if (screenshotDir)
      await page.screenshot({
        element: profile.container,
        path: `${screenshotDir}/profile-${themeId}.png`
      })
  }
)
