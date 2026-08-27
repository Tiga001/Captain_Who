import { getHostApi, unwrapHostInvocation } from '@mycopilot/host-api'
import {
  BROWSER_DATA_SCHEMA_VERSION,
  type BrowserDataCategory,
  type BrowserDataClearOutput,
  type BrowserDataSummaryOutput,
  type BrowserDataTimeRange,
  type BrowserHistoryDeleteOutput,
  type BrowserHistoryListOutput,
  type BrowserLinkOpenTarget,
  type BrowserPreferencesView
} from '@mycopilot/protocol'

export async function getBrowserPreferences(): Promise<BrowserPreferencesView> {
  return unwrapHostInvocation(await getHostApi().browser.getPreferences())
}

export async function updateBrowserPreferences(
  linkOpenTarget: BrowserLinkOpenTarget
): Promise<BrowserPreferencesView> {
  return unwrapHostInvocation(
    await getHostApi().browser.updatePreferences({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      linkOpenTarget
    })
  )
}

export async function listBrowserHistory(query: string): Promise<BrowserHistoryListOutput> {
  return unwrapHostInvocation(
    await getHostApi().browser.listHistory({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      query,
      limit: 500
    })
  )
}

export async function deleteBrowserHistory(
  historyIds: string[]
): Promise<BrowserHistoryDeleteOutput> {
  return unwrapHostInvocation(
    await getHostApi().browser.deleteHistory({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      historyIds
    })
  )
}

export async function openBrowserHistoryEntry(url: string): Promise<void> {
  unwrapHostInvocation(
    await getHostApi().browser.openHistoryEntry({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      url
    })
  )
}

export function onBrowserHistoryChanged(handler: () => void): () => void {
  return getHostApi().browser.onHistoryChanged(handler)
}

export async function getBrowserDataSummary(
  timeRange: BrowserDataTimeRange
): Promise<BrowserDataSummaryOutput> {
  return unwrapHostInvocation(
    await getHostApi().browser.getDataSummary({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      timeRange
    })
  )
}

export async function clearBrowserData(
  timeRange: BrowserDataTimeRange,
  categories: BrowserDataCategory[]
): Promise<BrowserDataClearOutput> {
  return unwrapHostInvocation(
    await getHostApi().browser.clearData({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      timeRange,
      categories
    })
  )
}
