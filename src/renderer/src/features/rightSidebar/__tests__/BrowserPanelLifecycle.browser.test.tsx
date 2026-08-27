import { StrictMode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WebviewTag } from 'electron'

const setWebview = vi.fn()
const SURFACE_INSTANCE_ID = 'instance-browser-00001'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../../browser/useBrowserWebview', () => ({
  useBrowserWebview: () => ({
    currentUrl: null,
    goBack: vi.fn(async () => undefined),
    goForward: vi.fn(async () => undefined),
    hostFallbackError: null,
    isLoaded: false,
    navigationState: {
      canGoBack: false,
      canGoForward: false,
      isLoading: false,
      metadata: { iconUrl: null, title: null, url: null }
    },
    navigateToUrl: vi.fn(async () => undefined),
    reload: vi.fn(async () => undefined),
    setWebview,
    setZoom: vi.fn(async () => undefined)
  })
}))

vi.mock('../../browser/browserSurface', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../browser/browserSurface')>()
  return {
    ...original,
    resolveBrowserSurfaceHostApi: () => ({}),
    synchronizeBrowserSurfaceInstance: (
      _browser: unknown,
      target: { onInstance: (surfaceInstanceId: string) => void }
    ) => {
      target.onInstance(SURFACE_INSTANCE_ID)
      return () => undefined
    },
    synchronizeBrowserSurfaceSelection: () => () => undefined
  }
})

const { BrowserPanel } = await import('../../browser/BrowserPanel')

afterEach(() => {
  setWebview.mockClear()
})

describe('BrowserPanel automation readiness lifecycle', () => {
  it('renders the browser overflow menu in the top-level portal', async () => {
    const onOpenSettings = vi.fn()
    const screen = await render(
      <BrowserPanel
        isActive
        onOpenSettings={onOpenSettings}
        pageId="portal-menu"
        surfaceId="right-sidebar-browser-portal-menu"
      />
    )

    await screen.getByRole('button', { name: 'browser.menu' }).click()
    await expect.poll(() => document.body.querySelector('.browser-panel__menu')).not.toBeNull()
    const menu = document.body.querySelector('.browser-panel__menu') as HTMLElement
    expect(menu.parentElement).toBe(document.body)
    expect(menu.style.width).not.toBe('')
    const menuButtons = Array.from(menu.querySelectorAll<HTMLButtonElement>('button'))
    const downloadButton = menuButtons.find(
      (button) => button.textContent?.trim() === 'browser.downloadCenter.title'
    )
    const historyButton = menuButtons.find(
      (button) => button.textContent?.trim() === 'browser.history'
    )
    const settingsButton = menuButtons.find(
      (button) => button.textContent?.trim() === 'browser.settings'
    )
    expect(downloadButton).toBeTruthy()
    expect(historyButton).toBeTruthy()
    expect(settingsButton).toBeTruthy()
    historyButton?.click()
    expect(onOpenSettings).toHaveBeenCalledWith('history')
  })

  it('does not acknowledge did-attach and only reports the exact surviving document-ready webview', async () => {
    const ready = vi.fn()
    const instanceChanged = vi.fn()
    const screen = await render(
      <StrictMode>
        <BrowserPanel
          automationRequestId="11111111-1111-4111-8111-111111111111"
          isActive
          onAutomationSurfaceReady={ready}
          onSurfaceInstanceChange={instanceChanged}
          pageId="strict-ready"
          surfaceId="right-sidebar-browser-strict-ready"
        />
      </StrictMode>
    )
    const webview = screen.container.querySelector('webview') as WebviewTag | null
    if (!webview) throw new Error('surviving webview missing')

    webview.dispatchEvent(new Event('did-attach'))
    await Promise.resolve()
    expect(ready).not.toHaveBeenCalled()

    webview.dispatchEvent(new Event('dom-ready'))
    await expect.poll(() => ready).toHaveBeenCalledTimes(1)
    expect(ready).toHaveBeenCalledWith(
      'right-sidebar-browser-strict-ready',
      '11111111-1111-4111-8111-111111111111',
      SURFACE_INSTANCE_ID,
      undefined
    )
    expect(instanceChanged).toHaveBeenCalledWith(
      'right-sidebar-browser-strict-ready',
      SURFACE_INSTANCE_ID,
      true
    )

    // did-finish-load is a fallback for an Electron ordering edge, not a second acknowledgement.
    webview.dispatchEvent(new Event('did-finish-load'))
    expect(ready).toHaveBeenCalledTimes(1)
    screen.unmount()
    expect(instanceChanged).toHaveBeenCalledWith(
      'right-sidebar-browser-strict-ready',
      SURFACE_INSTANCE_ID,
      false
    )
    webview.dispatchEvent(new Event('dom-ready'))
    expect(ready).toHaveBeenCalledTimes(1)
  })

  it('joins an early document-ready observation to the later exact did-attach identity', async () => {
    const ready = vi.fn()
    const screen = await render(
      <BrowserPanel
        automationRequestId="22222222-2222-4222-8222-222222222222"
        isActive
        onAutomationSurfaceReady={ready}
        pageId="early-document"
        surfaceId="right-sidebar-browser-early-document"
      />
    )
    const webview = screen.container.querySelector('webview') as WebviewTag | null
    if (!webview) throw new Error('webview missing')

    webview.dispatchEvent(new Event('dom-ready'))
    expect(ready).not.toHaveBeenCalled()
    webview.dispatchEvent(new Event('did-attach'))
    await expect.poll(() => ready).toHaveBeenCalledTimes(1)
    expect(ready).toHaveBeenCalledWith(
      'right-sidebar-browser-early-document',
      '22222222-2222-4222-8222-222222222222',
      SURFACE_INSTANCE_ID,
      undefined
    )
    screen.unmount()
  })
})
