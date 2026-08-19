import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { describe, expect, it, vi } from 'vitest'

import type { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import { registerBrowserArtifactIpc } from '../ipc/browserArtifactIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const artifact = {
  schemaVersion: 1,
  artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
  kind: 'pdf',
  displayName: 'page.pdf',
  mimeType: 'application/pdf',
  sizeBytes: 42,
  createdAt: 1_000,
  expiresAt: 2_000,
  lifecycle: 'run',
  owner: 'browser_automation',
  preview: 'none'
} as const

const previewArtifact = {
  ...artifact,
  kind: 'image',
  displayName: 'page.png',
  mimeType: 'image/png',
  sizeBytes: 2,
  preview: 'image'
} as const

function createHarness(destination: string | null | Error) {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const ipcMain = {
    handle: vi.fn((channel: string, handler: (...args: unknown[]) => unknown) => {
      handlers.set(channel, handler)
    }),
    on: vi.fn()
  } as unknown as TrustedIpcMain
  const broker = {
    exportArtifact: vi.fn(async () => ({ displayName: 'user-copy.pdf' })),
    readPreview: vi.fn()
  }
  const showSaveDialog = vi.fn(async () => {
    if (destination instanceof Error) throw destination
    return destination
  })
  registerBrowserArtifactIpc(ipcMain, broker as never, showSaveDialog)
  const invokeExport = (value: unknown) =>
    handlers.get(HOST_CHANNELS.browser.artifactExport)?.(
      {} as IpcMainInvokeEvent,
      value
    ) as Promise<unknown>
  return { broker, invokeExport, showSaveDialog }
}

describe('Browser Artifact IPC', () => {
  it('parses the complete reference and returns only bounded preview bytes', async () => {
    const handlers = new Map<string, (...args: unknown[]) => unknown>()
    const ipcMain = {
      handle: vi.fn((channel: string, handler: (...args: unknown[]) => unknown) => {
        handlers.set(channel, handler)
      }),
      on: vi.fn()
    } as unknown as TrustedIpcMain
    const readPreview = vi.fn(async () => ({
      artifact: previewArtifact,
      bytes: Uint8Array.from([1, 2])
    }))
    registerBrowserArtifactIpc(
      ipcMain,
      { readPreview } as unknown as BrowserArtifactBroker,
      vi.fn(async () => null)
    )
    const handler = handlers.get(HOST_CHANNELS.browser.artifactReadPreview)
    await expect(
      handler?.({} as IpcMainInvokeEvent, {
        schemaVersion: 1,
        artifact: previewArtifact
      })
    ).resolves.toEqual({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact: previewArtifact,
        bytes: Uint8Array.from([1, 2])
      }
    })
    expect(readPreview).toHaveBeenCalledWith(previewArtifact)

    const invalid = await handler?.({} as IpcMainInvokeEvent, {
      schemaVersion: 1,
      artifactId: previewArtifact.artifactId
    })
    expect(invalid).toMatchObject({
      ok: false,
      error: { message: expect.stringMatching(/artifactId/) }
    })
    expect(readPreview).toHaveBeenCalledTimes(1)
  })

  it('uses only the native-dialog destination and returns a path-free export receipt', async () => {
    const harness = createHarness('/Users/fixture/user-copy.pdf')
    const result = await harness.invokeExport({ schemaVersion: 1, artifact })

    expect(harness.showSaveDialog).toHaveBeenCalledWith(expect.anything(), 'page.pdf')
    expect(harness.broker.exportArtifact).toHaveBeenCalledWith(
      artifact,
      '/Users/fixture/user-copy.pdf'
    )
    expect(result).toEqual({
      ok: true,
      value: { schemaVersion: 1, status: 'exported', displayName: 'user-copy.pdf' }
    })
    expect(JSON.stringify(result)).not.toContain('/Users/fixture')
  })

  it('returns normal cancellation without calling the Broker', async () => {
    const harness = createHarness(null)
    await expect(harness.invokeExport({ schemaVersion: 1, artifact })).resolves.toEqual({
      ok: true,
      value: { schemaVersion: 1, status: 'cancelled' }
    })
    expect(harness.broker.exportArtifact).not.toHaveBeenCalled()
  })

  it('rejects Renderer-provided paths before opening the save dialog', async () => {
    const harness = createHarness('/Users/fixture/user-copy.pdf')
    const result = await harness.invokeExport({
      schemaVersion: 1,
      artifact,
      path: '/Users/fixture/renderer-selected.pdf'
    })
    expect(result).toMatchObject({ ok: false })
    expect(harness.showSaveDialog).not.toHaveBeenCalled()
    expect(harness.broker.exportArtifact).not.toHaveBeenCalled()
  })

  it('redacts native dialog failure details', async () => {
    const harness = createHarness(new Error('failed near /Users/fixture/private/export.pdf'))
    const result = await harness.invokeExport({ schemaVersion: 1, artifact })
    expect(result).toEqual({
      ok: false,
      error: { message: 'browser.artifact.unavailable' }
    })
    expect(JSON.stringify(result)).not.toContain('/Users/fixture')
    expect(harness.broker.exportArtifact).not.toHaveBeenCalled()
  })
})
