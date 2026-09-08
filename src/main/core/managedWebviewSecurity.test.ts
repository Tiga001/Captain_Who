import { EventEmitter } from 'node:events'
import type {
  BrowserWindowConstructorOptions,
  Session,
  WebContents,
  WindowOpenHandlerResponse
} from 'electron'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { BROWSER_WEBVIEW_PARTITION, createBrowserSurfaceBootstrapUrl } from '@mycopilot/protocol'
import type { BrowserNetworkGuard } from '../browser/BrowserNetworkGuard'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions,
  type ManagedWebviewTargetRegistry
} from '../webviews/managedWebviewSecurity'

const electronFixture = vi.hoisted(() => {
  const onBeforeRequest = vi.fn()
  const setPermissionCheckHandler = vi.fn()
  const setPermissionRequestHandler = vi.fn()
  const managedSession = {
    setPermissionCheckHandler,
    setPermissionRequestHandler,
    webRequest: { onBeforeRequest }
  }
  return {
    createBrowserWindow: vi.fn(),
    managedSession,
    onBeforeRequest,
    setPermissionCheckHandler,
    setPermissionRequestHandler,
    fromPartition: vi.fn(() => managedSession)
  }
})

vi.mock('electron', () => ({
  BrowserWindow: class {
    readonly webContents: WebContents
    on = vi.fn()
    once = vi.fn()
    removeListener = vi.fn()

    static fromWebContents() {
      return {
        isDestroyed: () => false,
        getBounds: () => ({ x: 100, y: 100, width: 1_200, height: 900 })
      }
    }

    constructor(options: BrowserWindowConstructorOptions & { webContents: WebContents }) {
      electronFixture.createBrowserWindow(options)
      this.webContents = options.webContents
    }
  },
  session: { fromPartition: electronFixture.fromPartition }
}))

class WebContentsFixture extends EventEmitter {
  destroyed = false
  readonly session = electronFixture.managedSession as unknown as Session
  close = vi.fn(() => {
    this.destroyed = true
    this.emit('destroyed')
  })
  loadURL = vi.fn(async (url: string) => {
    this.url = url
  })
  setWindowOpenHandler = vi.fn()

  constructor(public url: string) {
    super()
  }

  getURL(): string {
    return this.url
  }

  isDestroyed(): boolean {
    return this.destroyed
  }

  asWebContents(): WebContents {
    return this as unknown as WebContents
  }
}

beforeEach(() => {
  electronFixture.createBrowserWindow.mockClear()
  electronFixture.fromPartition.mockClear()
  electronFixture.onBeforeRequest.mockClear()
  electronFixture.setPermissionCheckHandler.mockClear()
  electronFixture.setPermissionRequestHandler.mockClear()
})

describe('managed native popup admission', () => {
  function setup(
    withGuard = true,
    canCreateNativePopup?: ManagedWebviewTargetRegistry['canCreateNativePopup']
  ) {
    const host = new WebContentsFixture('file:///renderer.html')
    const opener = new WebContentsFixture(createBrowserSurfaceBootstrapUrl('popup-opener'))
    const child = new WebContentsFixture('about:blank')
    const handlePopup = vi.fn()
    const createNativePopup = vi.fn<NonNullable<ManagedWebviewTargetRegistry['createNativePopup']>>(
      (input) => {
        const window = input.createWindow(input.options)
        input.configureGuest(window.webContents)
        return window.webContents
      }
    )
    const networkGuard = {
      registerGuest: vi.fn(),
      recordBlockedNavigation: vi.fn(),
      handleWindowOpen: vi.fn()
    } as unknown as BrowserNetworkGuard
    configureManagedWebviewHost(host.asWebContents(), {
      networkGuard: withGuard ? networkGuard : undefined,
      targetRegistry: {
        registerManagedGuest: vi.fn(),
        createNativePopup,
        canCreateNativePopup,
        handlePopup
      }
    })
    host.emit('did-attach-webview', {}, opener.asWebContents())
    const open = opener.setWindowOpenHandler.mock.calls[0][0] as (input: {
      url: string
    }) => WindowOpenHandlerResponse
    return { child, createNativePopup, handlePopup, host, networkGuard, open, opener }
  }

  it.each(['https://login.example.com/authorize', 'http://localhost:3000/login', 'about:blank'])(
    'preserves the native child and opener for %s while applying Main-owned window policy',
    (url) => {
      const { child, createNativePopup, host, open, opener } = setup()
      const response = open({ url })
      expect(response.action).toBe('allow')
      expect(response.overrideBrowserWindowOptions?.webPreferences).toMatchObject({
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        session: electronFixture.managedSession,
        webSecurity: true,
        webviewTag: false
      })
      const nativeOptions = {
        webContents: child.asWebContents(),
        show: true,
        width: 99_999,
        height: 1,
        x: -99_999,
        alwaysOnTop: true,
        frame: false,
        kiosk: true,
        title: 'Untrusted title',
        webPreferences: {
          preload: '/private/unsafe-preload.js',
          nodeIntegration: true,
          sandbox: false,
          webSecurity: false,
          partition: 'persist:untrusted'
        }
      }
      const returned = response.createWindow?.(nativeOptions)
      expect(returned).toBe(child.asWebContents())
      expect(createNativePopup).toHaveBeenCalledWith(
        expect.objectContaining({ guest: opener, url })
      )
      expect(electronFixture.createBrowserWindow).toHaveBeenCalledWith(
        expect.objectContaining({
          webContents: child,
          show: false,
          width: 520,
          height: 680,
          x: 440,
          y: 210,
          title: `CaptainWho-${url}`,
          frame: true,
          alwaysOnTop: false,
          kiosk: false,
          webPreferences: expect.objectContaining({
            nodeIntegration: false,
            nodeIntegrationInWorker: false,
            nodeIntegrationInSubFrames: false,
            contextIsolation: true,
            sandbox: true,
            partition: BROWSER_WEBVIEW_PARTITION,
            session: electronFixture.managedSession,
            webSecurity: true,
            webviewTag: false
          })
        })
      )
      const used = electronFixture.createBrowserWindow.mock.calls[0][0]
      expect(used.webPreferences.preload).toBeUndefined()
      expect(child.setWindowOpenHandler).toHaveBeenCalledOnce()
      expect(child.listenerCount('will-attach-webview')).toBe(0)
      expect(host.listenerCount('will-attach-webview')).toBe(1)
      expect(child.loadURL).not.toHaveBeenCalled()
      expect(opener.loadURL).not.toHaveBeenCalled()
      const event = { preventDefault: vi.fn() }
      child.emit('will-navigate', event, 'file:///private/secret')
      expect(event.preventDefault).toHaveBeenCalledOnce()
    }
  )

  it.each(['file:///private/secret', 'javascript:alert(1)', 'data:text/html,test'])(
    'does not admit an unsupported native popup destination %s',
    (url) => {
      const { createNativePopup, open } = setup()
      expect(open({ url })).toEqual({ action: 'deny' })
      expect(createNativePopup).not.toHaveBeenCalled()
    }
  )

  it('retains the legacy denied-window fallback when no network guard is installed', () => {
    const { createNativePopup, handlePopup, open, opener } = setup(false)
    expect(open({ url: 'https://login.example.com/authorize' })).toEqual({ action: 'deny' })
    expect(handlePopup).toHaveBeenCalledWith({
      guest: opener,
      url: 'https://login.example.com/authorize'
    })
    expect(createNativePopup).not.toHaveBeenCalled()
  })

  it('rejects a capacity-denied popup before Electron can allocate a child or invoke its factory', () => {
    const canCreateNativePopup = vi.fn(() => false)
    const { createNativePopup, handlePopup, open, opener } = setup(true, canCreateNativePopup)
    const response = open({ url: 'https://login.example.com/authorize' })
    expect(response).toEqual({ action: 'deny' })
    expect(canCreateNativePopup).toHaveBeenCalledWith({
      guest: opener,
      url: 'https://login.example.com/authorize'
    })
    expect(response.createWindow).toBeUndefined()
    expect(createNativePopup).not.toHaveBeenCalled()
    expect(electronFixture.createBrowserWindow).not.toHaveBeenCalled()
    expect(handlePopup).not.toHaveBeenCalled()
  })

  it('denies the popup when synchronous admission fails unexpectedly', () => {
    const { createNativePopup, open } = setup(true, () => {
      throw new Error('Fixture admission failed')
    })
    expect(open({ url: 'https://login.example.com/authorize' })).toEqual({ action: 'deny' })
    expect(createNativePopup).not.toHaveBeenCalled()
    expect(electronFixture.createBrowserWindow).not.toHaveBeenCalled()
  })

  it.each(['before-adoption', 'after-adoption'] as const)(
    'closes and returns the exact native child instead of throwing on %s failure',
    (stage) => {
      const { child, createNativePopup, open, opener } = setup()
      createNativePopup.mockImplementationOnce((input) => {
        if (stage === 'after-adoption') input.createWindow(input.options)
        throw new Error('Fixture popup admission failed')
      })
      const response = open({ url: 'https://login.example.com/authorize' })
      const nativeOptions = { webContents: child.asWebContents(), webPreferences: {} }
      expect(response.createWindow?.(nativeOptions)).toBe(child.asWebContents())
      expect(child.close).toHaveBeenCalledExactlyOnceWith({ waitForBeforeUnload: false })
      expect(child.isDestroyed()).toBe(true)
      expect(child.loadURL).not.toHaveBeenCalled()
      expect(opener.isDestroyed()).toBe(false)
    }
  )

  it('does not close an already-destroyed child again when Manager rejects admission', () => {
    const { child, createNativePopup, open } = setup()
    createNativePopup.mockImplementationOnce(() => {
      child.close()
      throw new Error('Fixture popup already closed')
    })
    const nativeOptions = { webContents: child.asWebContents(), webPreferences: {} }
    expect(open({ url: 'about:blank' }).createWindow?.(nativeOptions)).toBe(child.asWebContents())
    expect(child.close).toHaveBeenCalledOnce()
  })

  it('fails closed rather than replacing a missing native child WebContents', () => {
    const { createNativePopup, open } = setup()
    const response = open({ url: 'about:blank' })
    expect(() => response.createWindow?.({ webPreferences: {} })).toThrow(
      'Managed popup requires the exact native child WebContents'
    )
    expect(createNativePopup).not.toHaveBeenCalled()
    expect(electronFixture.createBrowserWindow).not.toHaveBeenCalled()
  })
})

describe('managed webview session requests', () => {
  it('admits only the Chromium PDF Viewer entry as an internal main frame', () => {
    initializeManagedWebviewSessions()
    const listener = electronFixture.onBeforeRequest.mock.calls[0]?.[0] as
      | ((
          details: { method: string; resourceType: 'mainFrame'; url: string },
          callback: (response: { cancel?: boolean }) => void
        ) => void)
      | undefined
    expect(listener).toBeDefined()

    const callback = vi.fn()
    listener?.(
      {
        method: 'GET',
        resourceType: 'mainFrame',
        url: 'chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/index.html'
      },
      callback
    )
    expect(callback).toHaveBeenLastCalledWith({})

    listener?.(
      {
        method: 'GET',
        resourceType: 'mainFrame',
        url: 'chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/options.html'
      },
      callback
    )
    expect(callback).toHaveBeenLastCalledWith({ cancel: true })

    listener?.(
      {
        method: 'GET',
        resourceType: 'mainFrame',
        url: 'chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index.html'
      },
      callback
    )
    expect(callback).toHaveBeenLastCalledWith({ cancel: true })
  })
})

describe('managed webview bootstrap registration', () => {
  it('allows only the registry-owned opaque internal-page URL', () => {
    const surfaceId = 'right-sidebar-browser-internal-page'
    const host = new WebContentsFixture('file:///renderer.html')
    const guest = new WebContentsFixture(createBrowserSurfaceBootstrapUrl(surfaceId))
    const internalUrl = 'mycopilot-browser-internal://page/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
    const recordBlockedNavigation = vi.fn()
    const targetRegistry: ManagedWebviewTargetRegistry = {
      registerManagedGuest: vi.fn(),
      isInternalNavigationAllowed: vi.fn((_guest, url) => url === internalUrl)
    }
    const networkGuard = {
      registerGuest: vi.fn(),
      recordBlockedNavigation,
      handleWindowOpen: vi.fn()
    } as unknown as BrowserNetworkGuard

    configureManagedWebviewHost(host.asWebContents(), { networkGuard, targetRegistry })
    host.emit('did-attach-webview', {}, guest.asWebContents())
    const allowedEvent = { preventDefault: vi.fn() }
    const blockedEvent = { preventDefault: vi.fn() }

    guest.emit('will-navigate', allowedEvent, internalUrl)
    guest.emit('will-navigate', blockedEvent, `${internalUrl}%20untrusted`)

    expect(allowedEvent.preventDefault).not.toHaveBeenCalled()
    expect(blockedEvent.preventDefault).toHaveBeenCalledTimes(1)
    expect(recordBlockedNavigation).toHaveBeenCalledTimes(1)
  })

  it('registers the trusted surface during did-attach as soon as its bootstrap is identified', () => {
    const surfaceId = 'right-sidebar-browser-startup-race'
    const host = new WebContentsFixture('file:///renderer.html')
    const guest = new WebContentsFixture(createBrowserSurfaceBootstrapUrl(surfaceId))
    const registerGuest = vi.fn()
    const registerManagedGuest = vi.fn()
    const networkGuard = {
      registerGuest,
      recordBlockedNavigation: vi.fn(),
      handleWindowOpen: vi.fn()
    } as unknown as BrowserNetworkGuard
    const targetRegistry = { registerManagedGuest } satisfies ManagedWebviewTargetRegistry

    configureManagedWebviewHost(host.asWebContents(), { networkGuard, targetRegistry })
    host.emit('did-attach-webview', {}, guest.asWebContents())

    expect(registerGuest).toHaveBeenCalledWith({ generation: 0, guest, surfaceId })
    expect(registerManagedGuest).toHaveBeenCalledWith({
      documentReady: false,
      guest,
      host,
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId
    })
    expect(guest.listenerCount('dom-ready')).toBe(0)
    expect(guest.listenerCount('did-finish-load')).toBe(0)
    expect(guest.close).not.toHaveBeenCalled()
  })

  it('registers on bootstrap navigation without waiting for dom-ready', () => {
    const surfaceId = 'right-sidebar-browser-navigation-race'
    const bootstrapUrl = createBrowserSurfaceBootstrapUrl(surfaceId)
    const host = new WebContentsFixture('file:///renderer.html')
    const guest = new WebContentsFixture('about:blank')
    const registerManagedGuest = vi.fn()

    configureManagedWebviewHost(host.asWebContents(), {
      targetRegistry: { registerManagedGuest }
    })
    host.emit('did-attach-webview', {}, guest.asWebContents())
    expect(registerManagedGuest).not.toHaveBeenCalled()

    guest.emit('did-start-navigation', { isMainFrame: true, url: bootstrapUrl })

    expect(registerManagedGuest).toHaveBeenCalledWith({
      documentReady: false,
      guest,
      host,
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId
    })
    expect(guest.listenerCount('dom-ready')).toBe(0)
  })

  it('marks registration document-ready when bootstrap is first observable at dom-ready', () => {
    const surfaceId = 'right-sidebar-browser-ready-registration'
    const host = new WebContentsFixture('file:///renderer.html')
    const guest = new WebContentsFixture('about:blank')
    const registerManagedGuest = vi.fn()

    configureManagedWebviewHost(host.asWebContents(), {
      targetRegistry: { registerManagedGuest }
    })
    host.emit('did-attach-webview', {}, guest.asWebContents())
    guest.url = createBrowserSurfaceBootstrapUrl(surfaceId)
    guest.emit('dom-ready')

    expect(registerManagedGuest).toHaveBeenCalledWith({
      documentReady: true,
      guest,
      host,
      partition: BROWSER_WEBVIEW_PARTITION,
      surfaceId
    })
  })
})
