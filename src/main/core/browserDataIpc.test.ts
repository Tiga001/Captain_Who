import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { BROWSER_DATA_SCHEMA_VERSION } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'

const electronHarness = vi.hoisted(() => ({ send: vi.fn() }))

vi.mock('electron', () => ({
  BrowserWindow: {
    getAllWindows: () => [
      {
        isDestroyed: () => false,
        webContents: {
          isDestroyed: () => false,
          send: electronHarness.send
        }
      }
    ]
  }
}))

import type { BrowserHistoryService } from '../browser/BrowserHistoryService'
import type { BrowserLinkRouter } from '../browser/BrowserLinkRouter'
import type { FaviconResourceCache } from '../resources/FaviconResourceCache'
import { registerBrowserDataIpc } from '../ipc/browserDataIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'
import type { CoreServer } from './coreServer'

afterEach(() => {
  electronHarness.send.mockReset()
  vi.restoreAllMocks()
})

function createHarness() {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const ipcMain = {
    handle: vi.fn((channel: string, handler: (...args: unknown[]) => unknown) => {
      handlers.set(channel, handler)
    }),
    on: vi.fn()
  } as unknown as TrustedIpcMain
  let historyListener: (() => void) | undefined
  const coreServer = {
    clearBrowserOwnedData: vi.fn(async () => ({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      deletedHistoryCount: 2,
      deletedDownloadCount: 3
    })),
    deleteBrowserHistory: vi.fn(async () => 1),
    listBrowserHistory: vi.fn(async () => []),
    summarizeBrowserOwnedData: vi.fn(async () => ({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      historyCount: 4,
      historySiteCount: 2,
      downloadCount: 3
    }))
  }
  const linkRouter = {
    openInBuiltinBrowser: vi.fn(async () => undefined),
    preferences: vi.fn(() => ({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      linkOpenTarget: 'system' as const,
      revision: 0,
      updatedAt: 0
    })),
    updatePreferences: vi.fn(async (input) => ({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      linkOpenTarget: input.linkOpenTarget,
      revision: 1,
      updatedAt: 1
    }))
  }
  const session = {
    clearData: vi.fn(async () => undefined),
    cookies: {
      get: vi.fn(async () => [
        { domain: '.example.test' },
        { domain: 'example.test' },
        { domain: 'other.test' }
      ])
    },
    getCacheSize: vi.fn(async () => 4096)
  }
  const faviconResourceCache = { clear: vi.fn(async () => undefined) }
  const dispose = registerBrowserDataIpc(ipcMain, {
    coreServer: coreServer as unknown as CoreServer,
    faviconResourceCache: faviconResourceCache as unknown as FaviconResourceCache,
    historyService: {
      onChanged: vi.fn((listener: () => void) => {
        historyListener = listener
        return vi.fn()
      })
    } as unknown as BrowserHistoryService,
    linkRouter: linkRouter as unknown as BrowserLinkRouter,
    session: session as never
  })
  const invoke = (channel: string, value?: unknown) =>
    handlers.get(channel)?.({ sender: {} } as IpcMainInvokeEvent, value) as Promise<unknown>
  return {
    coreServer,
    dispose,
    faviconResourceCache,
    historyChanged: () => historyListener?.(),
    invoke,
    linkRouter,
    session
  }
}

describe('Browser data IPC', () => {
  it('summarizes app-owned records by range without pretending cookies or cache are ranged', async () => {
    vi.spyOn(Date, 'now').mockReturnValue(10 * 24 * 60 * 60 * 1000)
    const harness = createHarness()

    await expect(
      harness.invoke(HOST_CHANNELS.browser.dataSummary, {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        timeRange: 'last7Days'
      })
    ).resolves.toEqual({
      ok: true,
      value: {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        historyCount: 4,
        historySiteCount: 2,
        downloadCount: 3,
        cookieSiteCount: 2,
        cacheBytes: 4096
      }
    })
    expect(harness.coreServer.summarizeBrowserOwnedData).toHaveBeenCalledWith({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      since: 3 * 24 * 60 * 60 * 1000
    })
    expect(harness.session.cookies.get).toHaveBeenCalledWith({})
    expect(harness.session.getCacheSize).toHaveBeenCalled()
  })

  it('enforces all-time-only Electron data and ranges only owned history records', async () => {
    vi.spyOn(Date, 'now').mockReturnValue(8 * 60 * 60 * 1000)
    const harness = createHarness()

    const invalid = await harness.invoke(HOST_CHANNELS.browser.dataClear, {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      timeRange: 'lastHour',
      categories: ['cookiesAndSiteData']
    })
    expect(invalid).toMatchObject({ ok: false })
    expect(harness.session.clearData).not.toHaveBeenCalled()
    expect(harness.coreServer.clearBrowserOwnedData).not.toHaveBeenCalled()

    await expect(
      harness.invoke(HOST_CHANNELS.browser.dataClear, {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        timeRange: 'lastHour',
        categories: ['history', 'downloadHistory']
      })
    ).resolves.toMatchObject({
      ok: true,
      value: { deletedHistoryCount: 2, deletedDownloadCount: 3 }
    })
    expect(harness.coreServer.clearBrowserOwnedData).toHaveBeenCalledWith({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      since: 7 * 60 * 60 * 1000,
      clearHistory: true,
      clearDownloads: true
    })
    expect(harness.session.clearData).not.toHaveBeenCalled()
    expect(harness.faviconResourceCache.clear).toHaveBeenCalledTimes(1)

    await harness.invoke(HOST_CHANNELS.browser.dataClear, {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      timeRange: 'allTime',
      categories: ['cookiesAndSiteData', 'cache']
    })
    expect(harness.session.clearData).toHaveBeenCalledWith({
      dataTypes: [
        'backgroundFetch',
        'cookies',
        'fileSystems',
        'indexedDB',
        'localStorage',
        'serviceWorkers',
        'webSQL',
        'cache'
      ]
    })
  })

  it('opens history in the built-in browser and broadcasts bounded change events', async () => {
    const harness = createHarness()
    const url = 'https://example.test/history'

    await expect(
      harness.invoke(HOST_CHANNELS.browser.historyOpen, {
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        url
      })
    ).resolves.toEqual({ ok: true, value: undefined })
    expect(harness.linkRouter.openInBuiltinBrowser).toHaveBeenCalledWith({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      url
    })

    harness.historyChanged()
    expect(electronHarness.send).toHaveBeenCalledWith(HOST_CHANNELS.browser.historyChanged, {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION
    })
  })
})
