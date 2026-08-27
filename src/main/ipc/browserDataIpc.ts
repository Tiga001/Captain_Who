import { BrowserWindow, type ClearDataOptions, type Session } from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import {
  BROWSER_DATA_SCHEMA_VERSION,
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  parseBrowserDataClearInput,
  parseBrowserDataClearOutput,
  parseBrowserDataSummaryInput,
  parseBrowserDataSummaryOutput,
  parseBrowserHistoryDeleteInput,
  parseBrowserHistoryDeleteOutput,
  parseBrowserHistoryListInput,
  parseBrowserHistoryListOutput,
  parseBrowserOpenUrlInput,
  parseBrowserPreferencesUpdateInput,
  type BrowserDataTimeRange
} from '@mycopilot/protocol'

import type { BrowserHistoryService } from '../browser/BrowserHistoryService'
import type { BrowserLinkRouter } from '../browser/BrowserLinkRouter'
import type { CoreServer } from '../core/coreServer'
import type { FaviconResourceCache } from '../resources/FaviconResourceCache'
import type { TrustedIpcMain } from './trustedIpc'

export interface BrowserDataIpcDependencies {
  coreServer: CoreServer
  faviconResourceCache: FaviconResourceCache
  historyService: BrowserHistoryService
  linkRouter: BrowserLinkRouter
  session: Session
}

export function registerBrowserDataIpc(
  ipcMain: TrustedIpcMain,
  dependencies: BrowserDataIpcDependencies
): () => void {
  const broadcastHistoryChanged = (): void => {
    broadcast(HOST_CHANNELS.browser.historyChanged, {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION
    })
  }
  const unsubscribeHistory = dependencies.historyService.onChanged(broadcastHistoryChanged)

  ipcMain.handle(HOST_CHANNELS.browser.preferencesGet, () =>
    captureHostInvocation(async () => dependencies.linkRouter.preferences())
  )
  ipcMain.handle(HOST_CHANNELS.browser.preferencesUpdate, (_event, value) =>
    captureHostInvocation(async () =>
      dependencies.linkRouter.updatePreferences(parseBrowserPreferencesUpdateInput(value))
    )
  )
  ipcMain.handle(HOST_CHANNELS.browser.historyList, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserHistoryListInput(value)
      const records = await dependencies.coreServer.listBrowserHistory(input)
      return parseBrowserHistoryListOutput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        entries: records.slice(0, input.limit),
        truncated: records.length > input.limit
      })
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.historyDelete, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserHistoryDeleteInput(value)
      const deletedCount = await dependencies.coreServer.deleteBrowserHistory(input)
      if (deletedCount > 0) broadcastHistoryChanged()
      return parseBrowserHistoryDeleteOutput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        deletedCount
      })
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.historyOpen, (_event, value) =>
    captureHostInvocation(async () => {
      await dependencies.linkRouter.openInBuiltinBrowser(parseBrowserOpenUrlInput(value))
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.dataSummary, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserDataSummaryInput(value)
      const [owned, cookies, cacheBytes] = await Promise.all([
        dependencies.coreServer.summarizeBrowserOwnedData({
          schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
          since: sinceForRange(input.timeRange)
        }),
        dependencies.session.cookies.get({}),
        dependencies.session.getCacheSize()
      ])
      return parseBrowserDataSummaryOutput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        historyCount: owned.historyCount,
        historySiteCount: owned.historySiteCount,
        downloadCount: owned.downloadCount,
        cookieSiteCount: new Set(
          cookies
            .map((cookie) => cookie.domain?.replace(/^\./u, ''))
            .filter((domain): domain is string => Boolean(domain))
        ).size,
        cacheBytes
      })
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.dataClear, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserDataClearInput(value)
      const clearHistory = input.categories.includes('history')
      const clearDownloads = input.categories.includes('downloadHistory')
      const clearCookies = input.categories.includes('cookiesAndSiteData')
      const clearCache = input.categories.includes('cache')
      const dataTypes: NonNullable<ClearDataOptions['dataTypes']> = []
      if (clearCookies) {
        dataTypes.push(
          'backgroundFetch',
          'cookies',
          'fileSystems',
          'indexedDB',
          'localStorage',
          'serviceWorkers',
          'webSQL'
        )
      }
      if (clearCache) dataTypes.push('cache')
      if (dataTypes.length > 0) await dependencies.session.clearData({ dataTypes })

      const owned = await dependencies.coreServer.clearBrowserOwnedData({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        since: sinceForRange(input.timeRange),
        clearHistory,
        clearDownloads
      })
      if (clearHistory || clearCache) await dependencies.faviconResourceCache.clear()
      if (owned.deletedHistoryCount > 0) broadcastHistoryChanged()
      if (owned.deletedDownloadCount > 0) {
        broadcast(HOST_CHANNELS.browser.downloadHistoryChanged, {
          schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION
        })
      }
      return parseBrowserDataClearOutput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        deletedHistoryCount: owned.deletedHistoryCount,
        deletedDownloadCount: owned.deletedDownloadCount,
        clearedCookiesAndSiteData: clearCookies,
        clearedCache: clearCache
      })
    })
  )

  return unsubscribeHistory
}

function sinceForRange(timeRange: BrowserDataTimeRange, now = Date.now()): number | null {
  const durations: Partial<Record<BrowserDataTimeRange, number>> = {
    lastHour: 60 * 60 * 1000,
    last24Hours: 24 * 60 * 60 * 1000,
    last7Days: 7 * 24 * 60 * 60 * 1000,
    last4Weeks: 28 * 24 * 60 * 60 * 1000
  }
  const duration = durations[timeRange]
  return duration === undefined ? null : Math.max(0, now - duration)
}

function broadcast(channel: string, value: unknown): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
      window.webContents.send(channel, value)
    }
  }
}
