import { EventEmitter } from 'node:events'
import type { Session, WebContents } from 'electron'
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
    managedSession,
    onBeforeRequest,
    setPermissionCheckHandler,
    setPermissionRequestHandler,
    fromPartition: vi.fn(() => managedSession)
  }
})

vi.mock('electron', () => ({
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
  electronFixture.fromPartition.mockClear()
  electronFixture.onBeforeRequest.mockClear()
  electronFixture.setPermissionCheckHandler.mockClear()
  electronFixture.setPermissionRequestHandler.mockClear()
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
