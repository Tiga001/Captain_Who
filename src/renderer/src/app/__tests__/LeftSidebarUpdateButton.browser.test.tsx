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
  showAbout: vi.fn(),
  openDocumentation: vi.fn(),
  requestLogin: vi.fn(),
  logout: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    app: {
      showAbout: mocks.showAbout,
      openDocumentation: mocks.openDocumentation
    },
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
  mocks.showAbout.mockResolvedValue(undefined)
  mocks.openDocumentation.mockResolvedValue(undefined)
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

it('keeps Chinese update labels to four characters and English labels concise', () => {
  for (const key of [
    'update.download',
    'update.downloading',
    'update.preparing',
    'update.installing',
    'update.error.checkFailed',
    'update.error.downloadFailed',
    'update.error.installFailed'
  ] as const) {
    const chinese = getTranslation('zh-CN', key).replace(' {percent}%', '')
    expect(chinese).toMatch(/^\p{Script=Han}{4}$/u)
    expect(getTranslation('en-US', key).split(' ').length).toBeLessThanOrEqual(2)
  }
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
    const progress = screen.getByRole('button', { name: '正在下载 27%' })
    await expect.element(progress).toBeDisabled()
    await expect.element(progress).toHaveTextContent('正在下载 27%')
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
    await emit({ ...available, revision: 3, status: 'preparing', percent: 100 })
    const preparing = screen.getByRole('button', { name: '校验更新' })
    await expect.element(preparing).toBeDisabled()
    expect(preparing.element()).toBe(button)
    expect(getComputedStyle(preparing.element()).backgroundColor).toBe(colors.backgroundColor)
    expect(getComputedStyle(preparing.element()).color).toBe(colors.borderColor)
    expect(getComputedStyle(preparing.element()).borderRadius).toBe(colors.borderRadius)
    if (screenshotDir)
      await page.screenshot({
        element: bar,
        path: `${screenshotDir}/update-preparing-${themeId}.png`
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
    await expect.element(screen.getByRole('button', { name: '帮助' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '账户菜单' })).toBeVisible()
    expect(mocks.download).not.toHaveBeenCalled()
  }
)

it('keeps a failed startup read hidden and accepts a later availability event', async () => {
  mocks.getState.mockRejectedValue(new Error('offline'))
  const screen = await renderFooter()
  expect(screen.container.querySelector('.left-sidebar__update')).toBeNull()
  await expect.element(screen.getByRole('button', { name: '帮助' })).toBeVisible()
  await emit(available)
  await expect.element(screen.getByRole('button', { name: '下载更新' })).toBeEnabled()
  await expect.element(screen.getByRole('button', { name: '帮助' })).not.toBeInTheDocument()
  expect(mocks.download).not.toHaveBeenCalled()
})

it.each(['zh-CN', 'en-US'] as const)(
  'shows exactly two independent help actions while signed out in %s',
  async (language) => {
    mocks.getState.mockResolvedValue({ ...available, status: 'idle', version: null })
    const screen = await renderFooter({ signedIn: false, language })
    const help = screen.getByRole('button', { name: getTranslation(language, 'help.menu') })
    const account = screen.getByRole('button', {
      name: getTranslation(language, 'sidebar.accountMenu')
    })
    expect(account.element().contains(help.element())).toBe(false)
    await account.click()
    await expect
      .element(screen.getByRole('menu'))
      .toHaveAccessibleName(getTranslation(language, 'sidebar.accountMenu'))
    await help.click()
    await expect
      .element(screen.getByRole('menu'))
      .toHaveAccessibleName(getTranslation(language, 'help.menu'))
    expect(screen.container.querySelectorAll('[role="menuitem"]')).toHaveLength(2)
    await screen.getByRole('menuitem', { name: getTranslation(language, 'help.about') }).click()
    expect(mocks.showAbout).toHaveBeenCalledExactlyOnceWith()
    await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
    await help.click()
    await screen
      .getByRole('menuitem', { name: getTranslation(language, 'help.documentation') })
      .click()
    expect(mocks.openDocumentation).toHaveBeenCalledExactlyOnceWith()
    await expect.element(help).toHaveFocus()
    await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
    expect(mocks.requestLogin).not.toHaveBeenCalled()
    expect(mocks.logout).not.toHaveBeenCalled()
    expect(mocks.openSettings).not.toHaveBeenCalled()
    expect(mocks.download).not.toHaveBeenCalled()
  }
)

it('dismisses help with Escape or outside clicks and supports keyboard navigation', async () => {
  mocks.getState.mockResolvedValue({ ...available, status: 'disabled', version: null })
  const screen = await renderFooter()
  const help = screen.getByRole('button', { name: '帮助' })
  const account = screen.getByRole('button', { name: '账户菜单' })
  ;(account.element() as HTMLButtonElement).focus()
  await userEvent.keyboard('{Tab}{Enter}')
  await expect.element(screen.getByRole('menuitem', { name: '关于 Captain Who' })).toHaveFocus()
  await userEvent.keyboard('{ArrowDown}')
  await expect.element(screen.getByRole('menuitem', { name: '查看文档' })).toHaveFocus()
  await userEvent.keyboard('{Home}')
  await expect.element(screen.getByRole('menuitem', { name: '关于 Captain Who' })).toHaveFocus()
  await userEvent.keyboard('{End}{Escape}')
  await expect.element(help).toHaveFocus()
  await expect.element(screen.getByRole('menu')).not.toBeInTheDocument()
  await userEvent.keyboard('{ArrowUp}')
  await expect.element(screen.getByRole('menuitem', { name: '查看文档' })).toHaveFocus()
  await userEvent.keyboard('{Enter}')
  expect(mocks.openDocumentation).toHaveBeenCalledOnce()
  await help.click()
  await account.click()
  await expect.element(screen.getByRole('menu', { name: '帮助' })).not.toBeInTheDocument()
  await expect.element(screen.getByRole('menu', { name: '账户菜单' })).toBeVisible()
})

it('closes an open help menu when an update arrives and keeps all update states exclusive', async () => {
  mocks.getState.mockResolvedValue({ ...available, status: 'idle', version: null })
  const screen = await renderFooter()
  await screen.getByRole('button', { name: '帮助' }).click()
  await expect.element(screen.getByRole('menu', { name: '帮助' })).toBeVisible()
  let revision = 2
  for (const status of ['available', 'downloading', 'preparing', 'installing', 'error'] as const) {
    emit({ ...available, revision: revision++, status })
    await expect.element(screen.getByRole('button', { name: '帮助' })).not.toBeInTheDocument()
    await expect.element(screen.getByRole('menu', { name: '帮助' })).not.toBeInTheDocument()
    expect(screen.container.querySelector('.left-sidebar__update')).not.toBeNull()
  }
  emit({ ...available, revision: revision++, status: 'idle', version: null })
  await expect
    .element(screen.getByRole('button', { name: '帮助' }))
    .toHaveAttribute('aria-expanded', 'false')
})

it('reports a failed help action safely and supports retry without changing account state', async () => {
  mocks.getState.mockResolvedValue({ ...available, status: 'idle', version: null })
  mocks.showAbout.mockRejectedValueOnce(new Error('private internal error detail'))
  const screen = await renderFooter()
  await screen.getByRole('button', { name: '帮助' }).click()
  await screen.getByRole('menuitem', { name: '关于 Captain Who' }).click()
  await expect.element(screen.getByRole('alert')).toHaveTextContent('打开失败，请重试。')
  expect(screen.container.textContent).not.toContain('private internal error')
  await screen.getByRole('button', { name: '帮助' }).click()
  await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
  await screen.getByRole('menuitem', { name: '关于 Captain Who' }).click()
  expect(mocks.showAbout).toHaveBeenCalledTimes(2)
})

it.each(['classic-light', 'classic-dark'] as const)(
  'fits the compact footer and opens help above its trigger in %s',
  async (themeId) => {
    const theme = getFrontendTheme(themeId)
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens)))
      document.documentElement.style.setProperty(key, value)
    mocks.getState.mockResolvedValue({ ...available, status: 'idle', version: null })
    const screen = await renderFooter({ language: 'en-US' })
    const help = screen.getByRole('button', { name: 'Help' })
    const icon = help.element().querySelector('svg')!
    expect(icon.getBoundingClientRect().width).toBe(18)
    expect(icon.getBoundingClientRect().height).toBe(18)
    expect(icon.getAttribute('aria-hidden')).toBe('true')
    expect(getComputedStyle(help.element()).borderRadius).toBe('8px')
    const screenshotDir = import.meta.env.VITE_CAPTAIN_WHO_HELP_SCREENSHOT_DIR
    if (screenshotDir) {
      await document.fonts.ready
      await page.screenshot({
        element: screen.container.querySelector('aside')!,
        path: `${screenshotDir}/help-idle-${themeId}.png`
      })
    }
    await help.click()
    await document.fonts.ready
    const footer = screen.container.querySelector('.left-sidebar__footer')!
    const account = screen.container.querySelector('.left-sidebar__account-button')!
    const menu = screen.getByRole('menu', { name: 'Help' }).element()
    const bounds = footer.getBoundingClientRect()
    const trigger = help.element().getBoundingClientRect()
    const popup = menu.getBoundingClientRect()
    expect(bounds.height).toBe(48)
    expect(trigger.width).toBe(28)
    expect(trigger.height).toBe(28)
    expect(account.getBoundingClientRect().right).toBeLessThanOrEqual(trigger.left)
    expect(trigger.right).toBeLessThanOrEqual(bounds.right)
    expect(popup.bottom).toBeLessThanOrEqual(trigger.top)
    expect(popup.left).toBeGreaterThanOrEqual(bounds.left)
    expect(popup.right).toBeLessThanOrEqual(bounds.right)
    expect(menu.scrollWidth).toBeLessThanOrEqual(menu.clientWidth)
    expect(
      screen.container.querySelector('.left-sidebar__account-text > span')!.clientWidth
    ).toBeGreaterThan(0)
    if (screenshotDir)
      await page.screenshot({
        element: screen.container.querySelector('aside')!,
        path: `${screenshotDir}/help-${themeId}.png`
      })
  }
)

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
  await expect.element(screen.getByRole('button', { name: '正在下载 0%' })).toBeDisabled()
  await emit({ ...available, revision: 3, status: 'downloading', percent: 42.9 })
  const progress = screen.getByRole('button', { name: '正在下载 42%' })
  await expect.element(progress).toBeDisabled()
  await expect.element(screen.getByRole('status')).toHaveTextContent('正在下载 42%')
  await emit({ ...available, revision: 4, status: 'preparing', percent: 100 })
  await expect.element(screen.getByRole('button', { name: '校验更新' })).toBeDisabled()
  await expect.element(screen.getByRole('status')).toHaveTextContent('校验更新')
  await emit({ ...available, revision: 5, status: 'installing', percent: 100 })
  await expect.element(screen.getByRole('button', { name: '正在重启' })).toBeDisabled()
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
  await expect.element(screen.getByRole('button', { name: '正在下载 61%' })).toBeDisabled()
  await screen.unmount()
  expect(mocks.unsubscribe).toHaveBeenCalledOnce()
})

it.each([
  ['downloadFailed', '下载失败'],
  ['installFailed', '重启失败']
] as const)('restores an accessible retry action after %s', async (error, message) => {
  const screen = await renderFooter()
  await screen.getByRole('button', { name: '下载更新' }).click()
  await emit({ ...available, revision: 3, status: 'preparing', percent: 100 })
  await emit({ ...available, revision: 4, status: 'error', error })
  const retry = screen.getByRole('button', { name: '下载更新' })
  await expect.element(retry).toBeEnabled()
  await expect.element(screen.getByRole('alert')).toHaveTextContent(message)
  expect(retry.element().getAttribute('aria-describedby')).toBe(
    screen.getByRole('alert').element().id
  )
  mocks.download.mockResolvedValue({ ...available, revision: 5, status: 'downloading', percent: 0 })
  await retry.click()
  expect(mocks.download).toHaveBeenCalledTimes(2)
  await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
  await expect.element(screen.getByRole('button', { name: '正在下载 0%' })).toBeDisabled()
})

it('recovers from a rejected download request without inventing a new host revision', async () => {
  mocks.download.mockRejectedValueOnce(new Error('IPC unavailable'))
  const screen = await renderFooter()
  await screen.getByRole('button', { name: '下载更新' }).click()
  await expect.element(screen.getByRole('alert')).toHaveTextContent('下载失败')
  await expect.element(screen.getByRole('button', { name: '下载更新' })).toBeEnabled()
  await emit(available)
  await expect.element(screen.getByRole('alert')).toHaveTextContent('下载失败')
  await screen.getByRole('button', { name: '下载更新' }).click()
  await expect.element(screen.getByRole('button', { name: '正在下载 0%' })).toBeDisabled()
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
    await emit({ ...available, revision: 3, status: 'preparing', percent: 100 })
    await expect
      .element(screen.getByRole('button', { name: getTranslation(language, 'update.preparing') }))
      .toBeDisabled()
    checkLayout()
    await emit({ ...available, revision: 4, status: 'error', error: 'downloadFailed' })
    checkLayout()
    if (screenshotDir)
      await page.screenshot({ element: bar, path: `${screenshotDir}/update-error-${language}.png` })
  }
)
