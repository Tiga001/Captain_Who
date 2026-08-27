import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { join } from 'node:path'
import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  type BrowserDownloadRecord,
  type BrowserDownloadSettingsRecord
} from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'

const electronHarness = vi.hoisted(() => ({ send: vi.fn(), writeText: vi.fn() }))

vi.mock('electron', () => ({
  BrowserWindow: {
    fromWebContents: () => null,
    getAllWindows: () => [
      {
        isDestroyed: () => false,
        webContents: {
          isDestroyed: () => false,
          send: electronHarness.send
        }
      }
    ]
  },
  clipboard: { writeText: electronHarness.writeText },
  dialog: { showOpenDialog: vi.fn() },
  shell: { openPath: vi.fn(async () => ''), showItemInFolder: vi.fn() }
}))

import type { BrowserDownloadBroker } from '../browser/BrowserDownloadBroker'
import type { CoreServer } from '../core/coreServer'
import { registerBrowserDownloadIpc } from '../ipc/browserDownloadIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const temporaryRoots = new Set<string>()

afterEach(async () => {
  electronHarness.send.mockReset()
  await Promise.all([...temporaryRoots].map((root) => rm(root, { force: true, recursive: true })))
  temporaryRoots.clear()
})

async function createHarness() {
  const root = await mkdtemp(join(tmpdir(), 'mycopilot-download-ipc-test-'))
  temporaryRoots.add(root)
  const filePath = join(root, 'archive.zip')
  await writeFile(filePath, 'data')
  const systemDirectory = join(homedir(), 'Downloads')
  const customDirectory = join(root, 'Custom')
  await mkdir(customDirectory)
  let settings: BrowserDownloadSettingsRecord = {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    locationMode: 'system',
    customDirectory: null,
    askWhereToSave: false,
    revision: 0,
    updatedAt: 0
  }
  let directory = systemDirectory
  let historyChanged: (() => void) | undefined
  let downloadCenterChanged: ((snapshot: unknown) => void) | undefined
  const record: BrowserDownloadRecord = {
    schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
    downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
    displayName: 'archive.zip',
    mimeType: 'application/zip',
    sizeBytes: 4,
    sha256: 'a'.repeat(64),
    createdAt: 1_000,
    source: 'agent',
    absolutePath: filePath,
    sourceOrigin: 'https://example.test',
    conversationId: 'conversation-1',
    projectId: null,
    runId: 'run-1',
    callId: 'call-1'
  }
  const coreServer = {
    clearBrowserDownloadHistory: vi.fn(async () => 1),
    listBrowserDownloads: vi.fn(async () => [record]),
    loadBrowserDownload: vi.fn(async () => record),
    saveBrowserDownloadSettings: vi.fn(async (input) => ({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      locationMode: input.locationMode,
      customDirectory: input.customDirectory,
      askWhereToSave: input.askWhereToSave,
      revision: input.expectedRevision + 1,
      updatedAt: input.updatedAt
    }))
  }
  const broker = {
    downloadCenterResource: vi.fn(() => ({
      absolutePath: filePath,
      sourceUrl: 'https://example.test/archive.zip'
    })),
    downloadCenterSnapshot: vi.fn(() => ({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      revision: 1,
      downloads: []
    })),
    downloadDirectory: vi.fn(() => directory),
    onDownloadCenterChangedEvent: vi.fn((listener: (snapshot: unknown) => void) => {
      downloadCenterChanged = listener
      return vi.fn()
    }),
    onHistoryChangedEvent: vi.fn((listener: () => void) => {
      historyChanged = listener
      return vi.fn()
    }),
    performDownloadCenterAction: vi.fn(async () => 'performed' as const),
    settings: vi.fn(() => settings),
    updateSettings: vi.fn((next: BrowserDownloadSettingsRecord) => {
      settings = next
      directory = next.customDirectory ?? systemDirectory
    })
  }
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const ipcMain = {
    handle: vi.fn((channel: string, handler: (...args: unknown[]) => unknown) => {
      handlers.set(channel, handler)
    }),
    on: vi.fn()
  } as unknown as TrustedIpcMain
  const nativeHost = {
    openDirectory: vi.fn(async () => true),
    revealInFolder: vi.fn(),
    selectDirectory: vi.fn(async () => customDirectory),
    writeText: vi.fn()
  }
  registerBrowserDownloadIpc(
    ipcMain,
    coreServer as unknown as CoreServer,
    broker as unknown as BrowserDownloadBroker,
    nativeHost
  )
  const invoke = (channel: string, value?: unknown) =>
    handlers.get(channel)?.({ sender: {} } as IpcMainInvokeEvent, value)
  return {
    broker,
    coreServer,
    customDirectory,
    downloadCenterChanged,
    filePath,
    historyChanged,
    invoke,
    nativeHost,
    root
  }
}

describe('Browser Download IPC', () => {
  it('returns path-free history and validates Renderer requests before Core access', async () => {
    const harness = await createHarness()
    const settings = await harness.invoke(HOST_CHANNELS.browser.downloadSettingsGet)
    expect(settings).toEqual({
      ok: true,
      value: {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system',
        displayPath: '~/Downloads',
        askWhereToSave: false,
        revision: 0,
        updatedAt: 0
      }
    })

    const history = await harness.invoke(HOST_CHANNELS.browser.downloadHistoryList, {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      query: '',
      limit: 20
    })
    expect(history).toEqual({
      ok: true,
      value: {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        downloads: [
          expect.objectContaining({
            downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
            availability: 'available'
          })
        ],
        truncated: false
      }
    })
    expect(JSON.stringify(history)).not.toContain(harness.root)
    expect(JSON.stringify(history)).not.toContain(harness.filePath)

    const invalid = await harness.invoke(HOST_CHANNELS.browser.downloadHistoryList, {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      query: '',
      limit: 20,
      absolutePath: harness.filePath
    })
    expect(invalid).toMatchObject({ ok: false })
    expect(harness.coreServer.listBrowserDownloads).toHaveBeenCalledTimes(1)
  })

  it('uses only native directory/reveal decisions and broadcasts bounded change events', async () => {
    const harness = await createHarness()
    const changed = await harness.invoke(HOST_CHANNELS.browser.downloadSettingsChooseDirectory)
    expect(harness.nativeHost.selectDirectory).toHaveBeenCalled()
    expect(harness.coreServer.saveBrowserDownloadSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        locationMode: 'custom',
        customDirectory: harness.customDirectory,
        expectedRevision: 0
      })
    )
    expect(harness.broker.updateSettings).toHaveBeenCalled()
    expect(changed).toMatchObject({
      ok: true,
      value: { locationMode: 'custom', revision: 1 }
    })

    await expect(
      harness.invoke(HOST_CHANNELS.browser.downloadHistoryReveal, {
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000'
      })
    ).resolves.toEqual({
      ok: true,
      value: { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION, status: 'shown' }
    })
    expect(harness.nativeHost.revealInFolder).toHaveBeenCalledWith(harness.filePath)

    harness.historyChanged?.()
    expect(electronHarness.send).toHaveBeenCalledWith(
      HOST_CHANNELS.browser.downloadHistoryChanged,
      { schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION }
    )
  })

  it('persists the native save-dialog preference without changing the download directory', async () => {
    const harness = await createHarness()

    const changed = await harness.invoke(HOST_CHANNELS.browser.downloadSettingsSetAskWhereToSave, {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      askWhereToSave: true
    })

    expect(harness.coreServer.saveBrowserDownloadSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        locationMode: 'system',
        customDirectory: null,
        askWhereToSave: true,
        expectedRevision: 0
      })
    )
    expect(changed).toMatchObject({
      ok: true,
      value: { askWhereToSave: true, revision: 1 }
    })
  })

  it('controls live downloads and keeps copy and reveal targets in Main', async () => {
    const harness = await createHarness()
    const input = {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000'
    }

    await expect(harness.invoke(HOST_CHANNELS.browser.downloadCenterGet)).resolves.toMatchObject({
      ok: true,
      value: { revision: 1, downloads: [] }
    })
    await expect(
      harness.invoke(HOST_CHANNELS.browser.downloadCenterAction, {
        ...input,
        action: 'pause'
      })
    ).resolves.toMatchObject({ ok: true, value: { status: 'performed' } })
    expect(harness.broker.performDownloadCenterAction).toHaveBeenCalledWith(
      input.downloadId,
      'pause'
    )

    await harness.invoke(HOST_CHANNELS.browser.downloadCenterAction, {
      ...input,
      action: 'copy_url'
    })
    expect(harness.nativeHost.writeText).toHaveBeenCalledWith('https://example.test/archive.zip')
    await harness.invoke(HOST_CHANNELS.browser.downloadCenterAction, {
      ...input,
      action: 'copy_path'
    })
    expect(harness.nativeHost.writeText).toHaveBeenCalledWith(harness.filePath)
    await harness.invoke(HOST_CHANNELS.browser.downloadCenterAction, {
      ...input,
      action: 'reveal'
    })
    expect(harness.nativeHost.revealInFolder).toHaveBeenCalledWith(harness.filePath)

    await expect(
      harness.invoke(HOST_CHANNELS.browser.downloadDirectoryOpen)
    ).resolves.toMatchObject({ ok: true, value: { status: 'opened' } })
    expect(harness.nativeHost.openDirectory).toHaveBeenCalled()

    harness.downloadCenterChanged?.({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      revision: 2,
      downloads: []
    })
    expect(electronHarness.send).toHaveBeenCalledWith(
      HOST_CHANNELS.browser.downloadCenterChanged,
      expect.objectContaining({ revision: 2 })
    )
  })
})
