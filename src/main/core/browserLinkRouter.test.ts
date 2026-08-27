import { BROWSER_DATA_SCHEMA_VERSION } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'

vi.mock('electron', () => ({
  shell: { openExternal: vi.fn(async () => undefined) }
}))

import type { BrowserSurfaceManager } from '../browser/BrowserSurfaceManager'
import { BrowserLinkRouter } from '../browser/BrowserLinkRouter'
import type { CoreServer } from './coreServer'

function createHarness(linkOpenTarget: 'system' | 'builtin' = 'system') {
  const openSystemUrl = vi.fn(async () => undefined)
  const createSurface = vi.fn(async () => undefined)
  const saveBrowserPreferences = vi.fn(async (input) => ({
    schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
    linkOpenTarget: input.linkOpenTarget,
    revision: input.expectedRevision + 1,
    updatedAt: input.updatedAt
  }))
  const router = new BrowserLinkRouter({
    coreServer: { saveBrowserPreferences } as unknown as CoreServer,
    initialPreferences: {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      linkOpenTarget,
      revision: 0,
      updatedAt: 0
    },
    openSystemUrl,
    surfaceManager: { createSurface } as unknown as BrowserSurfaceManager
  })
  return { createSurface, openSystemUrl, router, saveBrowserPreferences }
}

describe('BrowserLinkRouter', () => {
  it('routes app links through the configured target and keeps mail links in the system', async () => {
    const harness = createHarness()

    await harness.router.openAppUrl('https://example.test/system')
    expect(harness.openSystemUrl).toHaveBeenCalledWith('https://example.test/system')
    expect(harness.createSurface).not.toHaveBeenCalled()

    await harness.router.updatePreferences({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      linkOpenTarget: 'builtin'
    })
    await harness.router.openAppUrl('https://example.test/builtin')
    expect(harness.createSurface).toHaveBeenCalledWith({
      activate: true,
      url: 'https://example.test/builtin'
    })

    await harness.router.openAppUrl('mailto:person@example.test')
    expect(harness.openSystemUrl).toHaveBeenCalledWith('mailto:person@example.test')
  })

  it('always opens history entries in the built-in browser and rejects unsafe URLs', async () => {
    const harness = createHarness('system')

    await harness.router.openInBuiltinBrowser({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      url: 'https://example.test/history'
    })
    expect(harness.createSurface).toHaveBeenCalledWith({
      activate: true,
      url: 'https://example.test/history'
    })
    await expect(
      harness.router.openAppUrl('https://user:secret@example.test/private')
    ).rejects.toThrow('Credential-bearing')
    await expect(harness.router.openAppUrl('file:///tmp/private')).rejects.toThrow(
      'Unsupported app URL protocol'
    )
  })
})
