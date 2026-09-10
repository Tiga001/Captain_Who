import { StrictMode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WebviewTag } from 'electron'
import type { BrowserSurfaceSelectedInput, BrowserSurfaceSelectedOutput } from '@mycopilot/protocol'

const setWebview = vi.fn()
const navigateToUrl = vi.fn(async () => undefined)
const SURFACE_INSTANCE_ID = 'instance-browser-00001'
let mainSelectedSurfaceId: string | null = null
const surfaceSelected = vi.fn(
  async (input: BrowserSurfaceSelectedInput): Promise<BrowserSurfaceSelectedOutput> => {
    if (input.surfaceInstanceId === null) {
      return {
        ...input,
        status: 'noop',
        reason: 'instance_required',
        retryable: true,
        authoritativeRevision: input.selectionRevision,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      }
    }
    mainSelectedSurfaceId = input.surfaceId
    return {
      ...input,
      status: 'applied',
      reason: 'selection_applied',
      retryable: false,
      authoritativeRevision: input.selectionRevision
    }
  }
)

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
    navigateToUrl,
    reload: vi.fn(async () => undefined),
    setWebview,
    setZoom: vi.fn(async () => undefined)
  })
}))

vi.mock('../../browser/browserSurface', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../browser/browserSurface')>()
  return {
    ...original,
    resolveBrowserSurfaceHostApi: () => ({ surfaceSelected }),
    synchronizeBrowserSurfaceInstance: (
      _browser: unknown,
      target: { onInstance: (surfaceInstanceId: string) => void }
    ) => {
      target.onInstance(SURFACE_INSTANCE_ID)
      return () => undefined
    }
  }
})

const { BrowserPanel } = await import('../../browser/BrowserPanel')

afterEach(() => {
  setWebview.mockClear()
  navigateToUrl.mockClear()
  surfaceSelected.mockClear()
  mainSelectedSurfaceId = null
})

describe('BrowserPanel address input', () => {
  it('opens a local development server with its port and path', async () => {
    const screen = await render(<BrowserPanel isActive pageId="local-address" />)

    await screen
      .getByRole('textbox', { name: 'browser.addressPlaceholder' })
      .fill('localhost:5173/login')
    await screen.getByRole('button', { name: 'browser.open' }).click()

    expect(navigateToUrl).toHaveBeenCalledWith('http://localhost:5173/login')
    expect(screen.container.querySelector('[role="alert"]')).toBeNull()
  })

  it('explains an invalid address and clears the error when the user corrects it', async () => {
    const screen = await render(<BrowserPanel isActive pageId="invalid-address" />)
    const address = screen.getByRole('textbox', { name: 'browser.addressPlaceholder' })

    await address.fill('javascript:alert(1)')
    await screen.getByRole('button', { name: 'browser.open' }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('browser.invalidAddress')
    await expect.element(address).toHaveAttribute('aria-invalid', 'true')
    await expect.element(address).toHaveValue('javascript:alert(1)')
    expect(navigateToUrl).not.toHaveBeenCalled()

    await address.fill('example.com:8080/report')
    await expect.element(address).not.toHaveAttribute('aria-invalid')
    expect(screen.container.querySelector('[role="alert"]')).toBeNull()
    await screen.getByRole('button', { name: 'browser.open' }).click()
    expect(navigateToUrl).toHaveBeenCalledWith('https://example.com:8080/report')
  })
})

describe('BrowserPanel automation readiness lifecycle', () => {
  it('reselects the exact existing webview when focus returns from a native popup without a tab change', async () => {
    const surfaceId = 'right-sidebar-browser-popup-opener'
    const screen = await render(
      <BrowserPanel isActive pageId="popup-opener" surfaceId={surfaceId} />
    )
    const webview = screen.container.querySelector('webview') as WebviewTag
    webview.dispatchEvent(new Event('did-attach'))
    await expect.poll(() => mainSelectedSurfaceId).toBe(surfaceId)
    const previousRevision = surfaceSelected.mock.calls.at(-1)![0].selectionRevision

    // Main changes selection when its separate native popup receives focus. The sidebar page
    // stays selected and mounted, so no isActive effect can repair this by itself.
    mainSelectedSurfaceId = 'managed-native-popup'
    surfaceSelected.mockClear()
    webview.dispatchEvent(new Event('focus'))

    await expect.poll(() => mainSelectedSurfaceId).toBe(surfaceId)
    expect(screen.container.querySelector('webview')).toBe(webview)
    expect(surfaceSelected.mock.calls.map(([input]) => input.surfaceInstanceId)).toEqual([
      null,
      SURFACE_INSTANCE_ID
    ])
    expect(surfaceSelected.mock.calls.at(-1)![0].selectionRevision).toBeGreaterThan(
      previousRevision
    )
  })

  it('ignores focus from a background or retired webview instead of stealing native popup selection', async () => {
    const screen = await render(<BrowserPanel isActive pageId="background-opener" />)
    const webview = screen.container.querySelector('webview') as WebviewTag
    webview.dispatchEvent(new Event('did-attach'))
    await expect.poll(() => mainSelectedSurfaceId).toBe('right-sidebar-browser-background-opener')
    await screen.rerender(<BrowserPanel isActive={false} pageId="background-opener" />)
    mainSelectedSurfaceId = 'managed-native-popup'
    surfaceSelected.mockClear()

    webview.dispatchEvent(new Event('focus'))
    await Promise.resolve()
    expect(surfaceSelected).not.toHaveBeenCalled()
    expect(mainSelectedSurfaceId).toBe('managed-native-popup')

    screen.unmount()
    webview.dispatchEvent(new Event('focus'))
    await Promise.resolve()
    expect(surfaceSelected).not.toHaveBeenCalled()
    expect(mainSelectedSurfaceId).toBe('managed-native-popup')
  })

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
