import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { LocalTokenUsageSummaryOutput } from '@mycopilot/protocol'

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  profileRefresh: vi.fn(),
  licenseGet: vi.fn(),
  licenseRefresh: vi.fn(),
  localized: false,
  dark: false
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: { getLocalTokenUsage: mocks.read },
    auth: { refreshProfile: mocks.profileRefresh, openWebsite: vi.fn() },
    license: { getState: mocks.licenseGet, refresh: mocks.licenseRefresh }
  }
}))
vi.mock('../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../config/frontendTranslations')
  return {
    useFrontendConfig: () => ({
      language: mocks.localized ? 'zh-CN' : 'en-US',
      resolvedColorScheme: mocks.dark ? 'dark' : 'light',
      t: (key: Parameters<typeof getTranslation>[1]) =>
        getTranslation(mocks.localized ? 'zh-CN' : 'en-US', key)
    })
  }
})

import { LocalTokenActivity } from '../../features/settings/pages/LocalTokenActivity'
import { buildTokenYear, shanghaiDate } from '../../features/settings/pages/localTokenActivityData'
import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { LicenseContext } from '../../features/license/LicenseContext'
import { ProfileSettingsPage } from '../../features/settings/pages/ProfileSettingsPage'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getFrontendTheme } from '../../config/frontendTheme'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'

const year = 2028
const fixture: LocalTokenUsageSummaryOutput = {
  timezone: 'Asia/Shanghai',
  startedAt: Date.UTC(2026, 0, 1),
  days: [
    { date: `${year}-01-01`, tokenCount: '9007199254740993' },
    { date: `${year}-01-02`, tokenCount: '7' }
  ],
  todayTokens: '17',
  totalTokens: '9007199254741000',
  peakDailyTokens: '9007199254740993',
  unreportedRequestCount: 2
}

function pendingUsage() {
  let resolve!: (value: LocalTokenUsageSummaryOutput) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<LocalTokenUsageSummaryOutput>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

function AccountProfileFixture() {
  return (
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 1,
          status: 'signedIn',
          profile: {
            userId: 'fixture',
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
            expiresAt: null,
            cacheValidUntil: null,
            verifiedAt: null,
            error: null
          },
          canStartTurn: () => true,
          requestAccess: vi.fn(),
          refresh: mocks.licenseRefresh
        }}
      >
        <ProfileSettingsPage />
      </LicenseContext.Provider>
    </AccountAuthContext.Provider>
  )
}
let previousStyle: string | null
let previousViewport: { width: number; height: number }
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] })
  vi.setSystemTime(new Date('2028-06-15T04:00:00Z'))
  mocks.read.mockReset().mockResolvedValue(fixture)
  mocks.profileRefresh.mockReset().mockResolvedValue({ ok: true })
  mocks.licenseGet.mockReset()
  mocks.licenseRefresh.mockReset()
  mocks.localized = false
  mocks.dark = false
  previousStyle = document.documentElement.getAttribute('style')
  previousViewport = { width: innerWidth, height: innerHeight }
})
afterEach(async () => {
  vi.useRealTimers()
  if (previousStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousStyle)
  await page.viewport(previousViewport.width, previousViewport.height)
})

it('shows exact local totals, heatmap legend and date tooltips without trend controls or explanatory copy', async () => {
  const screen = await render(<LocalTokenActivity />)
  await expect.element(screen.getByText('9,007,199,254,741,000', { exact: true })).toBeVisible()
  expect(mocks.read).toHaveBeenCalledWith({ from: `${year}-01-01`, to: `${year}-12-31` })
  const firstDay = screen.container.querySelector(
    `[title="${year}-01-01: 9,007,199,254,740,993 Token"]`
  )
  expect(firstDay).not.toBeNull()
  expect(firstDay?.getAttribute('data-level')).toBe('4')
  expect(screen.container.querySelectorAll('.local-token-activity__day[title]')).toHaveLength(
    buildTokenYear(year, []).length
  )
  await expect.element(screen.getByText('Today', { exact: true })).toBeVisible()
  await expect.element(screen.getByText('All time', { exact: true })).toBeVisible()
  await expect.element(screen.getByText('Peak day', { exact: true })).toBeVisible()
  const legend = screen.container.querySelector('.local-token-activity__legend')!
  expect(legend.textContent).toContain('Less')
  expect(legend.textContent).toContain('More')
  expect(legend.querySelectorAll('.local-token-activity__day')).toHaveLength(5)
  expect(screen.container.querySelector('.local-token-activity__views')).toBeNull()
  expect(screen.container.querySelector('.local-token-activity__trend')).toBeNull()
  expect(screen.container.querySelector('select')).toBeNull()
  expect(screen.container.textContent).not.toContain('Dates use Shanghai time')
  expect(screen.container.textContent).not.toContain('No token usage recorded in this year')
  for (const label of ['Daily', 'Weekly', 'Cumulative']) {
    await expect
      .element(screen.getByRole('button', { name: label, exact: true }))
      .not.toBeInTheDocument()
  }
  expect(mocks.read).toHaveBeenCalledTimes(1)
})

it('navigates with previous/next year between 2026 and the current year, including a zero-data year', async () => {
  mocks.read.mockImplementation(async ({ from }: { from: string }) => ({
    ...fixture,
    days: from.startsWith('2027') ? [] : [{ date: from, tokenCount: '10' }]
  }))
  const screen = await render(<LocalTokenActivity />)
  const previous = screen.getByRole('button', { name: 'Previous year', exact: true })
  const next = screen.getByRole('button', { name: 'Next year', exact: true })
  const displayedYear = () =>
    screen.container.querySelector('.local-token-activity__year')?.textContent
  await expect.element(previous).toBeEnabled()
  await expect.element(next).toBeDisabled()
  expect(displayedYear()).toBe('2028')
  await previous.click()
  await expect.poll(displayedYear).toBe('2027')
  expect(mocks.read).toHaveBeenLastCalledWith({ from: '2027-01-01', to: '2027-12-31' })
  const emptyCells = [...screen.container.querySelectorAll('.local-token-activity__day[title]')]
  expect(emptyCells).toHaveLength(365)
  expect(emptyCells.every((cell) => cell.getAttribute('data-level') === '0')).toBe(true)
  expect(emptyCells[0]?.getAttribute('title')).toBe('2027-01-01: 0 Token')
  expect(screen.container.textContent).not.toContain('No token usage recorded in this year')
  await previous.click()
  await expect.poll(displayedYear).toBe('2026')
  await expect.element(previous).toBeDisabled()
  await expect.element(next).toBeEnabled()
  await next.click()
  await expect.poll(displayedYear).toBe('2027')
  await next.click()
  await expect.poll(displayedYear).toBe('2028')
  await expect.element(next).toBeDisabled()
  expect(mocks.read.mock.calls.map(([input]) => input.from)).toEqual([
    '2028-01-01',
    '2027-01-01',
    '2026-01-01',
    '2027-01-01',
    '2028-01-01'
  ])
})

it('shows a recoverable local read error', async () => {
  mocks.read.mockRejectedValueOnce(new Error('read failed'))
  const screen = await render(<LocalTokenActivity />)
  await expect.element(screen.getByRole('alert')).toHaveTextContent('Unable to read local usage')
  expect(
    screen.container.querySelector('.local-token-activity__day[title]')?.getAttribute('title')
  ).toContain('Unable to read local usage')
  await screen.getByRole('button', { name: 'Refresh usage' }).click()
  await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
  await expect.element(screen.getByText('9,007,199,254,741,000', { exact: true })).toBeVisible()
})

it('disables both year arrows when the current year is the 2026 lower bound', async () => {
  vi.setSystemTime(new Date('2026-09-13T04:00:00Z'))
  mocks.read.mockResolvedValueOnce({ ...fixture, days: [] })
  const screen = await render(<LocalTokenActivity />)
  await expect.element(screen.getByRole('button', { name: 'Refresh usage' })).toBeEnabled()
  expect(screen.container.querySelector('.local-token-activity__year')?.textContent).toBe('2026')
  await expect.element(screen.getByRole('button', { name: 'Previous year' })).toBeDisabled()
  await expect.element(screen.getByRole('button', { name: 'Next year' })).toBeDisabled()
  expect(mocks.read).toHaveBeenCalledExactlyOnceWith({ from: '2026-01-01', to: '2026-12-31' })
})

it('keeps the profile, totals, heatmap DOM and scroll positions while a local refresh is pending', async () => {
  const screen = await render(
    <div
      className="settings-page"
      data-testid="profile-scroll"
      style={{
        display: 'block',
        position: 'relative',
        width: 420,
        height: 480,
        overflowY: 'auto',
        padding: 20
      }}
    >
      <AccountProfileFixture />
    </div>
  )
  const refresh = screen.getByRole('button', { name: 'Refresh usage', exact: true })
  await expect.element(refresh).toBeEnabled()
  await expect.poll(() => mocks.profileRefresh.mock.calls.length).toBe(1)
  const hero = screen.container.querySelector('.profile-settings-hero')!
  const calendar = screen.container.querySelector('.local-token-activity__calendar')!
  const firstCell = calendar.querySelector('.local-token-activity__day[title]')!
  const horizontalScroll = screen.container.querySelector<HTMLElement>(
    '.local-token-activity__calendar-scroll'
  )!
  const profileScroll = screen.getByTestId('profile-scroll').element() as HTMLElement
  refresh.element().scrollIntoView({ block: 'center' })
  horizontalScroll.scrollLeft = 80
  const scrollTop = profileScroll.scrollTop
  const scrollLeft = horizontalScroll.scrollLeft
  expect(scrollTop).toBeGreaterThan(0)
  expect(scrollLeft).toBeGreaterThan(0)
  const pending = pendingUsage()
  mocks.read.mockImplementationOnce(() => pending.promise)
  await refresh.click()
  await expect.element(refresh).toBeDisabled()
  await expect.element(screen.getByRole('button', { name: 'Previous year' })).toBeDisabled()
  await expect.element(screen.getByRole('button', { name: 'Next year' })).toBeDisabled()
  expect(refresh.element().querySelector('[data-spinning="true"]')).not.toBeNull()
  expect(screen.container.querySelector('.profile-settings-hero')).toBe(hero)
  expect(screen.container.querySelector('.local-token-activity__calendar')).toBe(calendar)
  expect(calendar.querySelector('.local-token-activity__day[title]')).toBe(firstCell)
  expect(screen.container.querySelector('.local-token-activity__summary')?.textContent).toContain(
    '9,007,199,254,741,000'
  )
  expect(profileScroll.scrollTop).toBe(scrollTop)
  expect(horizontalScroll.scrollLeft).toBe(scrollLeft)
  expect(mocks.profileRefresh).toHaveBeenCalledTimes(1)
  expect(mocks.licenseGet).not.toHaveBeenCalled()
  expect(mocks.licenseRefresh).not.toHaveBeenCalled()
  pending.resolve({ ...fixture, totalTokens: '9007199254741100' })
  await expect.element(refresh).toBeEnabled()
  expect(screen.container.querySelector('.local-token-activity__summary')?.textContent).toContain(
    '9,007,199,254,741,100'
  )
  expect(screen.container.querySelector('.local-token-activity__calendar')).toBe(calendar)
  expect(profileScroll.scrollTop).toBe(scrollTop)
  expect(horizontalScroll.scrollLeft).toBe(scrollLeft)
  expect(mocks.read).toHaveBeenCalledTimes(2)
  expect(mocks.read).toHaveBeenLastCalledWith({ from: '2028-01-01', to: '2028-12-31' })
  expect(mocks.profileRefresh).toHaveBeenCalledTimes(1)
  expect(mocks.licenseGet).not.toHaveBeenCalled()
  expect(mocks.licenseRefresh).not.toHaveBeenCalled()
})

it('retains the previous totals and heatmap when refresh fails and allows a local retry', async () => {
  const screen = await render(<LocalTokenActivity />)
  const refresh = screen.getByRole('button', { name: 'Refresh usage' })
  await expect.element(refresh).toBeEnabled()
  const calendar = screen.container.querySelector('.local-token-activity__calendar')!
  const pending = pendingUsage()
  mocks.read.mockImplementationOnce(() => pending.promise)
  await refresh.click()
  await expect.element(refresh).toBeDisabled()
  pending.reject(new Error('local read temporarily unavailable'))
  await expect.element(screen.getByRole('alert')).toHaveTextContent('Unable to read local usage')
  await expect.element(refresh).toBeEnabled()
  expect(screen.container.querySelector('.local-token-activity__calendar')).toBe(calendar)
  expect(calendar.querySelector('[title="2028-01-01: 9,007,199,254,740,993 Token"]')).not.toBeNull()
  await expect.element(screen.getByText('9,007,199,254,741,000', { exact: true })).toBeVisible()
  await refresh.click()
  await expect.element(refresh).toBeEnabled()
  await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
  expect(mocks.read).toHaveBeenCalledTimes(3)
})

it('keeps the displayed year attached to its old snapshot when switching years fails', async () => {
  const screen = await render(<LocalTokenActivity />)
  const previous = screen.getByRole('button', { name: 'Previous year' })
  const displayedYear = () =>
    screen.container.querySelector('.local-token-activity__year')?.textContent
  await expect.element(previous).toBeEnabled()
  const calendar = screen.container.querySelector('.local-token-activity__calendar')!
  const pending = pendingUsage()
  mocks.read.mockImplementationOnce(() => pending.promise)
  await previous.click()
  await expect.element(previous).toBeDisabled()
  expect(displayedYear()).toBe('2028')
  expect(calendar.querySelector('[title^="2027-"]')).toBeNull()
  pending.reject(new Error('older year read failed'))
  await expect.element(previous).toBeEnabled()
  await expect.element(screen.getByRole('alert')).toHaveTextContent('Unable to read local usage')
  expect(displayedYear()).toBe('2028')
  expect(screen.container.querySelector('.local-token-activity__calendar')).toBe(calendar)
  expect(calendar.querySelector('[title="2028-01-01: 9,007,199,254,740,993 Token"]')).not.toBeNull()
  expect(calendar.querySelector('[title^="2027-"]')).toBeNull()
  mocks.read.mockResolvedValueOnce({ ...fixture, days: [] })
  await previous.click()
  await expect.poll(displayedYear).toBe('2027')
  expect(mocks.read).toHaveBeenLastCalledWith({ from: '2027-01-01', to: '2027-12-31' })
  expect(calendar.querySelector('[title="2027-01-01: 0 Token"]')).not.toBeNull()
  await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
})

it('does not relabel a returned snapshot when the current year changes during its read', async () => {
  vi.setSystemTime(new Date('2028-12-31T15:59:59Z'))
  const initial = pendingUsage()
  mocks.read.mockImplementationOnce(() => initial.promise)
  const screen = await render(<LocalTokenActivity />)
  const displayedYear = () =>
    screen.container.querySelector('.local-token-activity__year')?.textContent
  await expect.poll(() => mocks.read.mock.calls.length).toBe(1)
  expect(displayedYear()).toBe('2028')
  const initialCalendar = screen.container.querySelector('.local-token-activity__calendar')!
  expect(initialCalendar.querySelector('[title="2028-01-01: Loading usage…"]')).not.toBeNull()
  expect(screen.container.querySelectorAll('.local-token-activity__summary strong')).toHaveLength(3)
  expect(
    [...screen.container.querySelectorAll('.local-token-activity__summary strong')].every(
      (value) => value.textContent === '—'
    )
  ).toBe(true)
  vi.setSystemTime(new Date('2028-12-31T16:00:01Z'))
  initial.resolve(fixture)
  const next = screen.getByRole('button', { name: 'Next year' })
  await expect.element(next).toBeEnabled()
  expect(displayedYear()).toBe('2028')
  expect(screen.container.querySelector('.local-token-activity__calendar')).toBe(initialCalendar)
  expect(
    initialCalendar.querySelector('[title="2028-01-01: 9,007,199,254,740,993 Token"]')
  ).not.toBeNull()
  const nextYear = pendingUsage()
  mocks.read.mockImplementationOnce(() => nextYear.promise)
  await next.click()
  await expect.element(next).toBeDisabled()
  expect(displayedYear()).toBe('2028')
  expect(initialCalendar.querySelector('[title^="2029-"]')).toBeNull()
  nextYear.resolve({ ...fixture, days: [{ date: '2029-01-01', tokenCount: '23' }] })
  await expect.poll(displayedYear).toBe('2029')
  expect(initialCalendar.querySelector('[title="2029-01-01: 23 Token"]')).not.toBeNull()
  expect(initialCalendar.querySelector('[title^="2028-"]')).toBeNull()
  expect(mocks.read).toHaveBeenLastCalledWith({ from: '2029-01-01', to: '2029-12-31' })
  await expect.element(next).toBeDisabled()
})

it('handles Shanghai day boundaries, leap years and exact large daily token counts', () => {
  expect(shanghaiDate(Date.parse('2026-09-12T16:00:00Z'))).toBe('2026-09-13')
  expect(buildTokenYear(2028, [])).toHaveLength(366)
  const points = buildTokenYear(year, fixture.days)
  expect(points[0].tokens).toBe(BigInt('9007199254740993'))
  expect(points.reduce((sum, point) => sum + point.tokens, BigInt(0))).toBe(
    BigInt('9007199254741000')
  )
})

it.each(['classic-light', 'classic-dark'] as const)(
  'keeps the complete profile readable in %s',
  async (themeId) => {
    vi.setSystemTime(new Date('2026-09-13T04:00:00Z'))
    const visualYear = Number(shanghaiDate().slice(0, 4))
    mocks.localized = true
    mocks.dark = themeId === 'classic-dark'
    mocks.read.mockResolvedValue({
      ...fixture,
      todayTokens: '24893',
      totalTokens: '1368140',
      peakDailyTokens: '68312',
      days: buildTokenYear(visualYear, [])
        .filter((point) => point.date <= shanghaiDate())
        .map((point, index) => ({
          date: point.date,
          tokenCount:
            point.date === shanghaiDate()
              ? '24893'
              : index === 1
                ? '68312'
                : index % 5 === 0
                  ? '0'
                  : String((index * 1301) % 8000)
        }))
    })
    await page.viewport(1060, 1600)
    const theme = getFrontendTheme(themeId)
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens)))
      document.documentElement.style.setProperty(key, value)
    const screen = await render(
      <div
        className="settings-page"
        style={{ display: 'block', position: 'relative', padding: 40 }}
      >
        <AccountAuthContext.Provider
          value={{
            state: {
              revision: 1,
              status: 'signedIn',
              profile: {
                userId: 'fixture',
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
                expiresAt: null,
                cacheValidUntil: null,
                verifiedAt: null,
                error: null
              },
              canStartTurn: () => true,
              requestAccess: vi.fn(),
              refresh: vi.fn()
            }}
          >
            <ProfileSettingsPage />
          </LicenseContext.Provider>
        </AccountAuthContext.Provider>
      </div>
    )
    await expect.element(screen.getByText('1,368,140', { exact: true })).toBeVisible()
    await document.fonts.ready
    const profile = screen.container.querySelector('.profile-settings-page')!
    expect(profile.scrollWidth).toBeLessThanOrEqual(profile.clientWidth)
    const screenshotDir = import.meta.env.VITE_CAPTAIN_WHO_UI_SCREENSHOT_DIR
    if (screenshotDir)
      await page.screenshot({
        element: screen.container,
        path: `${screenshotDir}/profile-${themeId}.png`
      })
  }
)
