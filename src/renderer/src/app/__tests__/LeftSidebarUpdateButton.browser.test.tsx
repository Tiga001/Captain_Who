import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { flushSync } from 'react-dom'
import { render } from 'vitest-browser-react'
import type { UpdateState } from '@mycopilot/host-api'
import { getTranslation } from '../../config/frontendTranslations'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getFrontendTheme } from '../../config/frontendTheme'
import { AccountAuthContext } from '../../features/auth/AccountAuthContext'
import { defaultUiPreferences } from '../../features/storage/storageClient'
import { LeftSidebarAccountFooter } from '../shell/sidebar/LeftSidebarAccountFooter'
import '../../styles/global.css'
import '../shell/sidebar/LeftSidebar.css'

const mocks = vi.hoisted(() => ({
  getState: vi.fn(),
  download: vi.fn(),
  onStateChanged: vi.fn(),
  unsubscribe: vi.fn(),
  openSettings: vi.fn(),
  requestLogin: vi.fn(),
  logout: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    updates: {
      getState: mocks.getState,
      download: mocks.download,
      onStateChanged: mocks.onStateChanged
    }
  }
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ resolvedColorScheme: 'light' })
}))

const available: UpdateState = {
  revision: 1,
  status: 'available',
  version: '1.1.0',
  percent: 0,
  error: null
}
const longName = 'Captain With A Very Long Display Name'
let listener: (state: UpdateState) => void
let previousStyle: string | null

function renderFooter({
  signedIn = true,
  language = 'zh-CN'
}: { signedIn?: boolean; language?: 'zh-CN' | 'en-US' } = {}) {
  return render(
    <AccountAuthContext.Provider
      value={{
        state: {
          revision: 1,
          status: signedIn ? 'signedIn' : 'signedOut',
          profile: signedIn
            ? {
                userId: 'update-fixture',
                displayName: longName,
                email: 'captain@example.com',
                avatarDataUrl: null,
                occupation: '',
                organization: ''
              }
            : null,
          remembered: false,
          error: null
        },
        loginRequested: false,
        requestLogin: mocks.requestLogin,
        dismissLogin: vi.fn(),
        canStartTurn: () => false,
        logout: mocks.logout
      }}
    >
      <aside
        className="left-sidebar"
        style={{ width: 240, height: 280, background: 'var(--mc-color-surface-main-panel)' }}
      >
        <LeftSidebarAccountFooter
          onOpenSettings={mocks.openSettings}
          t={(key) => getTranslation(language, key)}
          uiPreferences={defaultUiPreferences()}
        />
      </aside>
    </AccountAuthContext.Provider>
  )
}

function emit(state: UpdateState) {
  flushSync(() => {
    listener(state)
  })
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.getState.mockResolvedValue(available)
  mocks.download.mockResolvedValue({ ...available, revision: 2, status: 'downloading' })
  mocks.onStateChanged.mockImplementation((next) => {
    listener = next
    return mocks.unsubscribe
  })
  previousStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens)))
    document.documentElement.style.setProperty(key, value)
})

afterEach(() => {
  if (previousStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousStyle)
})

it.each(['classic-light', 'classic-dark'] as const)(
  'shows an icon-only accent ring and keeps disabled progress accented in %s',
  async (themeId) => {
    const theme = getFrontendTheme(themeId)
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens)))
      document.documentElement.style.setProperty(key, value)
    const screen = await renderFooter()
    const update = screen.getByRole('button', { name: '下载更新' })
    await expect.element(update).toBeEnabled()
    await expect.element(update).toHaveAttribute('title', '下载更新')
    expect(update.element().textContent).toBe('')
    const button = update.element()
    const icon = button.querySelector('svg')!
    const expected = document.createElement('span')
    expected.style.cssText =
      'color: var(--mc-color-icon-default); background: var(--mc-color-control-selected-background); border-color: var(--mc-color-control-selected-text); border-radius: var(--mc-radius-control)'
    screen.container.append(expected)
    const colors = getComputedStyle(expected)
    expect(getComputedStyle(button).borderColor).toBe(colors.backgroundColor)
    expect(getComputedStyle(button).borderStyle).toBe('solid')
    expect(getComputedStyle(button).borderRadius).toBe('50%')
    expect(button.getBoundingClientRect().width).toBe(28)
    expect(button.getBoundingClientRect().height).toBe(28)
    expect(getComputedStyle(icon).color).toBe(colors.color)
    const screenshotDir = import.meta.env.VITE_CAPTAIN_WHO_UPDATE_SCREENSHOT_DIR
    const bar = screen.container.querySelector('.left-sidebar__footer')!
    await document.fonts.ready
    if (screenshotDir)
      await page.screenshot({
        element: bar,
        path: `${screenshotDir}/update-available-${themeId}.png`
      })
    await emit({ ...available, revision: 2, status: 'downloading', percent: 27 })
    const progress = screen.getByRole('button', { name: '下载中 27%' })
    await expect.element(progress).toBeDisabled()
    await expect.element(progress).toHaveTextContent('下载中 27%')
    expect(progress.element().querySelector('svg')).toBeNull()
    await expect
      .poll(() => getComputedStyle(progress.element()).backgroundColor)
      .toBe(colors.backgroundColor)
    expect(getComputedStyle(progress.element()).color).toBe(colors.borderColor)
    expect(getComputedStyle(progress.element()).borderRadius).toBe(colors.borderRadius)
    expect(getComputedStyle(progress.element()).opacity).toBe('1')
    if (screenshotDir)
      await page.screenshot({
        element: bar,
        path: `${screenshotDir}/update-downloading-${themeId}.png`
      })
    expected.remove()
  }
)

it.each(['disabled', 'checking', 'idle', 'error'] as const)(
  'hides the update action when there is no known version and status is %s',
  async (status) => {
    mocks.getState.mockResolvedValue({
      ...available,
      status,
      version: null,
      error: status === 'error' ? 'checkFailed' : null
    })
    const screen = await renderFooter()
    await expect.poll(() => mocks.getState.mock.calls.length).toBe(1)
    expect(screen.container.querySelector('.left-sidebar__update')).toBeNull()
    await expect.element(screen.getByRole('button', { name: '账户菜单' })).toBeVisible()
    expect(mocks.download).not.toHaveBeenCalled()
  }
)

it('keeps a failed startup read hidden and accepts a later availability event', async () => {
  mocks.getState.mockRejectedValue(new Error('offline'))
  const screen = await renderFooter()
  expect(screen.container.querySelector('.left-sidebar__update')).toBeNull()
  await emit(available)
  await expect.element(screen.getByRole('button', { name: '下载更新' })).toBeEnabled()
  expect(mocks.download).not.toHaveBeenCalled()
})

it('has independent account and update buttons, including when signed out and without a license provider', async () => {
  const screen = await renderFooter({ signedIn: false })
  const account = screen.getByRole('button', { name: '账户菜单' })
  const update = screen.getByRole('button', { name: '下载更新' })
  await expect.element(update).toBeEnabled()
  expect(account.element().contains(update.element())).toBe(false)
  expect(update.element().closest('.left-sidebar__account-button')).toBeNull()
  await account.click()
  await expect.element(screen.getByRole('menu')).toBeVisible()
  expect(mocks.download).not.toHaveBeenCalled()
  await account.click()
  await update.click()
  expect(mocks.download).toHaveBeenCalledOnce()
  await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
  expect(mocks.requestLogin).not.toHaveBeenCalled()
  expect(mocks.logout).not.toHaveBeenCalled()
  expect(mocks.openSettings).not.toHaveBeenCalled()
})

it('supports keyboard focus and activation without opening the account menu', async () => {
  const screen = await renderFooter()
  const account = screen.getByRole('button', { name: '账户菜单' })
  const update = screen.getByRole('button', { name: '下载更新' })
  await expect.element(update).toBeEnabled()
  ;(account.element() as HTMLButtonElement).focus()
  await userEvent.keyboard('{Tab}')
  await expect.element(update).toHaveFocus()
  expect(getComputedStyle(update.element()).outlineStyle).not.toBe('none')
  await userEvent.keyboard('{Enter}')
  expect(mocks.download).toHaveBeenCalledOnce()
  await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
})

it('blocks rapid clicks while awaiting the host and displays only reported progress until restart', async () => {
  let resolveDownload!: (state: UpdateState) => void
  mocks.download.mockImplementation(
    () =>
      new Promise<UpdateState>((resolve) => {
        resolveDownload = resolve
      })
  )
  const screen = await renderFooter()
  const update = screen.getByRole('button', { name: '下载更新' })
  await expect.element(update).toBeEnabled()
  flushSync(() => {
    const button = update.element() as HTMLButtonElement
    button.click()
    button.click()
  })
  expect(mocks.download).toHaveBeenCalledOnce()
  await expect.element(update).toBeDisabled()
  resolveDownload({ ...available, revision: 2, status: 'downloading', percent: 0 })
  await expect.element(screen.getByRole('button', { name: '下载中 0%' })).toBeDisabled()
  await emit({ ...available, revision: 3, status: 'downloading', percent: 42.9 })
  const progress = screen.getByRole('button', { name: '下载中 42%' })
  await expect.element(progress).toBeDisabled()
  await expect.element(screen.getByRole('status')).toHaveTextContent('下载中 42%')
  await emit({ ...available, revision: 4, status: 'downloading', percent: 100 })
  await expect.element(screen.getByRole('button', { name: '下载中 100%' })).toBeDisabled()
  await emit({ ...available, revision: 5, status: 'installing', percent: 100 })
  await expect.element(screen.getByRole('button', { name: '正在重启…' })).toBeDisabled()
  expect(mocks.download).toHaveBeenCalledOnce()
})

it('ignores late snapshots and earlier download replies after newer events', async () => {
  let resolveSnapshot!: (state: UpdateState) => void
  let resolveDownload!: (state: UpdateState) => void
  mocks.getState.mockImplementation(
    () =>
      new Promise<UpdateState>((resolve) => {
        resolveSnapshot = resolve
      })
  )
  mocks.download.mockImplementation(
    () =>
      new Promise<UpdateState>((resolve) => {
        resolveDownload = resolve
      })
  )
  const screen = await renderFooter()
  expect(mocks.onStateChanged.mock.invocationCallOrder[0]).toBeLessThan(
    mocks.getState.mock.invocationCallOrder[0]
  )
  await emit({ ...available, revision: 2 })
  await screen.getByRole('button', { name: '下载更新' }).click()
  await emit({ ...available, revision: 4, status: 'downloading', percent: 61 })
  resolveSnapshot({ ...available, status: 'idle', version: null })
  resolveDownload({ ...available, revision: 3, status: 'downloading', percent: 0 })
  await expect.element(screen.getByRole('button', { name: '下载中 61%' })).toBeDisabled()
  await screen.unmount()
  expect(mocks.unsubscribe).toHaveBeenCalledOnce()
})

it.each([
  ['downloadFailed', '下载失败，请重试'],
  ['installFailed', '重启失败，请重试']
] as const)('restores an accessible retry action after %s', async (error, message) => {
  const screen = await renderFooter()
  await screen.getByRole('button', { name: '下载更新' }).click()
  await emit({ ...available, revision: 3, status: 'error', error })
  const retry = screen.getByRole('button', { name: '下载更新' })
  await expect.element(retry).toBeEnabled()
  await expect.element(screen.getByRole('alert')).toHaveTextContent(message)
  expect(retry.element().getAttribute('aria-describedby')).toBe(
    screen.getByRole('alert').element().id
  )
  mocks.download.mockResolvedValue({ ...available, revision: 4, status: 'downloading', percent: 0 })
  await retry.click()
  expect(mocks.download).toHaveBeenCalledTimes(2)
  await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
  await expect.element(screen.getByRole('button', { name: '下载中 0%' })).toBeDisabled()
})

it('recovers from a rejected download request without inventing a new host revision', async () => {
  mocks.download.mockRejectedValueOnce(new Error('IPC unavailable'))
  const screen = await renderFooter()
  await screen.getByRole('button', { name: '下载更新' }).click()
  await expect.element(screen.getByRole('alert')).toHaveTextContent('下载失败，请重试')
  await expect.element(screen.getByRole('button', { name: '下载更新' })).toBeEnabled()
  await emit(available)
  await expect.element(screen.getByRole('alert')).toHaveTextContent('下载失败，请重试')
  await screen.getByRole('button', { name: '下载更新' }).click()
  await expect.element(screen.getByRole('button', { name: '下载中 0%' })).toBeDisabled()
  expect(mocks.download).toHaveBeenCalledTimes(2)
})

it.each(['zh-CN', 'en-US'] as const)(
  'fits the 48px account bar and truncates the username in %s',
  async (language) => {
    const screen = await renderFooter({ language })
    await expect
      .element(screen.getByRole('button', { name: getTranslation(language, 'update.download') }))
      .toBeVisible()
    await document.fonts.ready
    const bar = screen.container.querySelector('.left-sidebar__footer')!
    const account = screen.container.querySelector('.left-sidebar__account-button')!
    const name = screen.container.querySelector('.left-sidebar__account-text > span')!
    const update = screen.container.querySelector('.left-sidebar__update')!
    const avatar = screen.container.querySelector('.left-sidebar__account-avatar')!
    const checkLayout = () => {
      const rect = bar.getBoundingClientRect()
      expect(rect.height).toBe(48)
      expect(avatar.getBoundingClientRect().width).toBe(28)
      expect(name.clientWidth).toBeGreaterThan(0)
      expect(name.scrollWidth).toBeGreaterThan(name.clientWidth)
      expect(getComputedStyle(name).textOverflow).toBe('ellipsis')
      expect(account.getBoundingClientRect().right).toBeLessThanOrEqual(
        update.getBoundingClientRect().left
      )
      expect(update.getBoundingClientRect().right).toBeLessThanOrEqual(rect.right)
      expect(update.getBoundingClientRect().top).toBeGreaterThanOrEqual(rect.top)
      expect(update.getBoundingClientRect().bottom).toBeLessThanOrEqual(rect.bottom)
      expect(bar.scrollWidth).toBeLessThanOrEqual(bar.clientWidth)
      expect(name.parentElement!.children).toHaveLength(1)
      expect(screen.container.textContent).not.toContain('captain@example.com')
    }
    checkLayout()
    const screenshotDir = import.meta.env.VITE_CAPTAIN_WHO_UPDATE_SCREENSHOT_DIR
    if (screenshotDir)
      await page.screenshot({
        element: bar,
        path: `${screenshotDir}/update-available-${language}.png`
      })
    await emit({ ...available, revision: 2, status: 'downloading', percent: 42 })
    checkLayout()
    if (screenshotDir)
      await page.screenshot({
        element: bar,
        path: `${screenshotDir}/update-downloading-${language}.png`
      })
    await emit({ ...available, revision: 3, status: 'error', error: 'downloadFailed' })
    checkLayout()
    if (screenshotDir)
      await page.screenshot({ element: bar, path: `${screenshotDir}/update-error-${language}.png` })
  }
)
