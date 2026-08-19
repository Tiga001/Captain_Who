import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'

import type { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import { registerBrowserArtifactIpc } from '../ipc/browserArtifactIpc'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const artifact = {
  schemaVersion: 1,
  artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
  kind: 'image',
  displayName: 'page.png',
  mimeType: 'image/png',
  sizeBytes: 2,
  createdAt: 1_000,
  expiresAt: 2_000,
  lifecycle: 'run',
  owner: 'browser_automation',
  preview: 'image'
} as const

describe('Browser Artifact IPC', () => {
  it('parses the complete reference and returns only bounded preview bytes', async () => {
    let handler: ((event: unknown, value: unknown) => unknown) | undefined
    const ipcMain = {
      handle: vi.fn((_channel, nextHandler) => {
        handler = nextHandler
      }),
      on: vi.fn()
    } as unknown as TrustedIpcMain
    const readPreview = vi.fn(async () => ({ artifact, bytes: Uint8Array.from([1, 2]) }))
    registerBrowserArtifactIpc(ipcMain, { readPreview } as unknown as BrowserArtifactBroker)
    expect(ipcMain.handle).toHaveBeenCalledWith(
      HOST_CHANNELS.browser.artifactReadPreview,
      expect.any(Function)
    )
    await expect(handler?.({}, { schemaVersion: 1, artifact })).resolves.toEqual({
      ok: true,
      value: { schemaVersion: 1, artifact, bytes: Uint8Array.from([1, 2]) }
    })
    expect(readPreview).toHaveBeenCalledWith(artifact)

    const invalid = await handler?.({}, { schemaVersion: 1, artifactId: artifact.artifactId })
    expect(invalid).toMatchObject({
      ok: false,
      error: { message: expect.stringMatching(/artifactId/) }
    })
    expect(readPreview).toHaveBeenCalledTimes(1)
  })
})
