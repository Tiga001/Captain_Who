import { homedir } from 'node:os'
import { isAbsolute, relative, resolve, sep } from 'node:path'
import { lstat } from 'node:fs/promises'
import {
  BrowserWindow,
  dialog,
  shell,
  type IpcMainInvokeEvent,
  type OpenDialogOptions
} from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  parseBrowserDownloadAskWhereToSaveInput,
  parseBrowserDownloadHistoryClearInput,
  parseBrowserDownloadHistoryClearOutput,
  parseBrowserDownloadHistoryListInput,
  parseBrowserDownloadHistoryListOutput,
  parseBrowserDownloadIdInput,
  parseBrowserDownloadRevealOutput,
  parseBrowserDownloadSettingsView,
  type BrowserDownloadAvailability,
  type BrowserDownloadRecord,
  type BrowserDownloadSettingsRecord,
  type BrowserDownloadSettingsView
} from '@mycopilot/protocol'

import type { BrowserDownloadBroker } from '../browser/BrowserDownloadBroker'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

interface BrowserDownloadIpcNativeHost {
  selectDirectory(event: IpcMainInvokeEvent, currentDirectory: string): Promise<string | null>
  revealInFolder(path: string): void
}

export function registerBrowserDownloadIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer,
  broker: BrowserDownloadBroker,
  nativeHost: BrowserDownloadIpcNativeHost = createNativeHost()
): () => void {
  const broadcastChanged = (): void => {
    const notification = { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION }
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.browser.downloadHistoryChanged, notification)
      }
    }
  }
  const unsubscribe = broker.onHistoryChangedEvent(broadcastChanged)

  ipcMain.handle(HOST_CHANNELS.browser.downloadSettingsGet, () =>
    captureHostInvocation(async () => settingsView(broker.settings(), broker.downloadDirectory()))
  )
  ipcMain.handle(HOST_CHANNELS.browser.downloadSettingsChooseDirectory, (event) =>
    captureHostInvocation(async () => {
      const selected = await nativeHost.selectDirectory(event, broker.downloadDirectory())
      if (selected === null) return null
      const current = broker.settings()
      const saved = await coreServer.saveBrowserDownloadSettings({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'custom',
        customDirectory: selected,
        askWhereToSave: current.askWhereToSave,
        expectedRevision: current.revision,
        updatedAt: Date.now()
      })
      broker.updateSettings(saved)
      return settingsView(saved, broker.downloadDirectory())
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.downloadSettingsResetDirectory, () =>
    captureHostInvocation(async () => {
      const current = broker.settings()
      const saved = await coreServer.saveBrowserDownloadSettings({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system',
        customDirectory: null,
        askWhereToSave: current.askWhereToSave,
        expectedRevision: current.revision,
        updatedAt: Date.now()
      })
      broker.updateSettings(saved)
      return settingsView(saved, broker.downloadDirectory())
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.downloadSettingsSetAskWhereToSave, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserDownloadAskWhereToSaveInput(value)
      const current = broker.settings()
      if (input.askWhereToSave === current.askWhereToSave) {
        return settingsView(current, broker.downloadDirectory())
      }
      const saved = await coreServer.saveBrowserDownloadSettings({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: current.locationMode,
        customDirectory: current.customDirectory,
        askWhereToSave: input.askWhereToSave,
        expectedRevision: current.revision,
        updatedAt: Date.now()
      })
      broker.updateSettings(saved)
      return settingsView(saved, broker.downloadDirectory())
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.downloadHistoryList, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserDownloadHistoryListInput(value)
      const records = await coreServer.listBrowserDownloads(input)
      const truncated = records.length > input.limit
      const downloads = await Promise.all(
        records.slice(0, input.limit).map(async (record) => ({
          schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
          downloadId: record.downloadId,
          displayName: record.displayName,
          mimeType: record.mimeType,
          sizeBytes: record.sizeBytes,
          sha256: record.sha256,
          createdAt: record.createdAt,
          source: record.source,
          availability: await downloadAvailability(record),
          sourceOrigin: record.sourceOrigin
        }))
      )
      return parseBrowserDownloadHistoryListOutput({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        downloads,
        truncated
      })
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.downloadHistoryReveal, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserDownloadIdInput(value)
      const record = await coreServer.loadBrowserDownload(input.downloadId)
      if (!record || (await downloadAvailability(record)) === 'missing') {
        return parseBrowserDownloadRevealOutput({
          schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
          status: 'missing'
        })
      }
      nativeHost.revealInFolder(record.absolutePath)
      return parseBrowserDownloadRevealOutput({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        status: 'shown'
      })
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.downloadHistoryClear, (_event, value) =>
    captureHostInvocation(async () => {
      parseBrowserDownloadHistoryClearInput(value)
      const deletedCount = await coreServer.clearBrowserDownloadHistory()
      broadcastChanged()
      return parseBrowserDownloadHistoryClearOutput({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        deletedCount
      })
    })
  )

  return unsubscribe
}

function createNativeHost(): BrowserDownloadIpcNativeHost {
  return {
    selectDirectory: async (event, currentDirectory) => {
      const parent = BrowserWindow.fromWebContents(event.sender) ?? undefined
      const options: OpenDialogOptions = {
        title: 'Select browser download directory',
        defaultPath: currentDirectory,
        properties: ['openDirectory', 'createDirectory']
      }
      const result = parent
        ? await dialog.showOpenDialog(parent, options)
        : await dialog.showOpenDialog(options)
      const selected = result.filePaths[0]
      return result.canceled || !selected ? null : resolve(selected)
    },
    revealInFolder: (path) => shell.showItemInFolder(path)
  }
}

function settingsView(
  record: BrowserDownloadSettingsRecord,
  directory: string
): BrowserDownloadSettingsView {
  return parseBrowserDownloadSettingsView({
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: record.locationMode,
    displayPath: redactHome(directory),
    askWhereToSave: record.askWhereToSave,
    revision: record.revision,
    updatedAt: record.updatedAt
  })
}

function redactHome(path: string): string {
  const home = resolve(homedir())
  const absolute = resolve(path)
  const child = relative(home, absolute)
  if (child === '') return '~'
  if (child !== '..' && !child.startsWith(`..${sep}`) && !isAbsolute(child)) {
    return `~/${child.split(sep).join('/')}`
  }
  return absolute
}

async function downloadAvailability(
  record: BrowserDownloadRecord
): Promise<BrowserDownloadAvailability> {
  try {
    const metadata = await lstat(record.absolutePath)
    if (metadata.isSymbolicLink() || !metadata.isFile()) return 'missing'
    return metadata.size === record.sizeBytes ? 'available' : 'modified'
  } catch {
    return 'missing'
  }
}
