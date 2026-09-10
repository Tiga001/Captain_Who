import { BROWSER_DATA_SCHEMA_VERSION } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'

import { BrowserHistoryService } from '../browser/BrowserHistoryService'
import type { CoreServer } from './coreServer'

function navigation(
  overrides: Partial<Parameters<BrowserHistoryService['recordNavigation']>[0]> = {}
) {
  return {
    surfaceId: 'right-sidebar-browser-test',
    generation: 1,
    url: 'https://example.test/docs',
    title: 'Example docs',
    faviconUrl: 'https://example.test/favicon.ico',
    visitedAt: 1_000,
    ...overrides
  }
}

describe('BrowserHistoryService', () => {
  it('persists only committed HTTP metadata and updates the current entry in order', async () => {
    const registerBrowserHistory = vi.fn(async (entry) => entry)
    const updateBrowserHistoryMetadata = vi.fn(async () => true)
    const changed = vi.fn()
    const service = new BrowserHistoryService({
      registerBrowserHistory,
      updateBrowserHistoryMetadata
    } as unknown as CoreServer)
    service.onChanged(changed)

    service.recordNavigation(navigation())
    await vi.waitFor(() => expect(registerBrowserHistory).toHaveBeenCalledTimes(1))
    const stored = registerBrowserHistory.mock.calls[0]?.[0]
    expect(stored).toMatchObject({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      url: 'https://example.test/docs',
      title: 'Example docs',
      hostname: 'example.test',
      faviconUrl: 'https://example.test/favicon.ico',
      visitedAt: 1_000
    })
    expect(stored?.historyId).toMatch(/^browser-history:/u)

    service.updateMetadata(
      navigation({ title: 'Updated title', faviconUrl: 'https://example.test/new.ico' })
    )
    await vi.waitFor(() => expect(updateBrowserHistoryMetadata).toHaveBeenCalledTimes(1))
    expect(updateBrowserHistoryMetadata).toHaveBeenCalledWith({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      historyId: stored?.historyId,
      title: 'Updated title',
      faviconUrl: 'https://example.test/new.ico'
    })
    expect(changed).toHaveBeenCalledTimes(2)
  })

  it('persists local file navigations with the file name as hostname', async () => {
    const registerBrowserHistory = vi.fn(async (entry) => entry)
    const service = new BrowserHistoryService({
      registerBrowserHistory,
      updateBrowserHistoryMetadata: vi.fn(async () => true)
    } as unknown as CoreServer)

    service.recordNavigation(
      navigation({
        url: 'file:///Users/docs/Predici%20.pdf',
        title: null,
        faviconUrl: null
      })
    )
    await vi.waitFor(() => expect(registerBrowserHistory).toHaveBeenCalledTimes(1))
    expect(registerBrowserHistory.mock.calls[0]?.[0]).toMatchObject({
      url: 'file:///Users/docs/Predici%20.pdf',
      title: 'Predici .pdf',
      hostname: 'predici .pdf',
      faviconUrl: null
    })
  })

  it('ignores private schemes, credential-bearing URLs, and stale metadata', async () => {
    const registerBrowserHistory = vi.fn(async (entry) => entry)
    const updateBrowserHistoryMetadata = vi.fn(async () => true)
    const service = new BrowserHistoryService({
      registerBrowserHistory,
      updateBrowserHistoryMetadata
    } as unknown as CoreServer)

    service.recordNavigation(navigation({ url: 'javascript:alert(1)' }))
    service.recordNavigation(navigation({ url: 'https://user:secret@example.test/private' }))
    service.recordNavigation(navigation())
    await vi.waitFor(() => expect(registerBrowserHistory).toHaveBeenCalledTimes(1))
    service.updateMetadata(navigation({ url: 'https://example.test/other' }))
    service.forgetSurface(navigation())
    service.updateMetadata(navigation({ title: 'Too late' }))
    await Promise.resolve()
    expect(updateBrowserHistoryMetadata).not.toHaveBeenCalled()
  })
})
