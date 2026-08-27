import { getHostApi, unwrapHostInvocation } from '@mycopilot/host-api'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  type BrowserDownloadHistoryListOutput,
  type BrowserDownloadRevealOutput,
  type BrowserDownloadSettingsView
} from '@mycopilot/protocol'

export async function getBrowserDownloadSettings(): Promise<BrowserDownloadSettingsView> {
  return unwrapHostInvocation(await getHostApi().browser.getDownloadSettings())
}

export async function chooseBrowserDownloadDirectory(): Promise<BrowserDownloadSettingsView | null> {
  return unwrapHostInvocation(await getHostApi().browser.chooseDownloadDirectory())
}

export async function resetBrowserDownloadDirectory(): Promise<BrowserDownloadSettingsView> {
  return unwrapHostInvocation(await getHostApi().browser.resetDownloadDirectory())
}

export async function setBrowserDownloadAskWhereToSave(
  askWhereToSave: boolean
): Promise<BrowserDownloadSettingsView> {
  return unwrapHostInvocation(
    await getHostApi().browser.setDownloadAskWhereToSave({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      askWhereToSave
    })
  )
}

export async function listBrowserDownloadHistory(
  query: string,
  limit = 200
): Promise<BrowserDownloadHistoryListOutput> {
  return unwrapHostInvocation(
    await getHostApi().browser.listDownloadHistory({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      query,
      limit
    })
  )
}

export async function revealBrowserDownload(
  downloadId: string
): Promise<BrowserDownloadRevealOutput> {
  return unwrapHostInvocation(
    await getHostApi().browser.revealDownload({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      downloadId
    })
  )
}

export async function clearBrowserDownloadHistory(): Promise<number> {
  return unwrapHostInvocation(
    await getHostApi().browser.clearDownloadHistory({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION
    })
  ).deletedCount
}

export function onBrowserDownloadHistoryChanged(handler: () => void): () => void {
  return getHostApi().browser.onDownloadHistoryChanged(handler)
}
