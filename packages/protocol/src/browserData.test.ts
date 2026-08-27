import { describe, expect, it } from 'vitest'

import {
  BROWSER_DATA_SCHEMA_VERSION,
  parseBrowserDataClearInput,
  parseBrowserHistoryEntry,
  parseBrowserHistoryListInput
} from './browserData'

describe('browser data protocol', () => {
  it('allows time ranges only for app-owned records', () => {
    expect(
      parseBrowserDataClearInput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        timeRange: 'last7Days',
        categories: ['history', 'downloadHistory']
      })
    ).toEqual({
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      timeRange: 'last7Days',
      categories: ['history', 'downloadHistory']
    })
    expect(() =>
      parseBrowserDataClearInput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        timeRange: 'last7Days',
        categories: ['cache']
      })
    ).toThrow('all time')
    expect(() =>
      parseBrowserDataClearInput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        timeRange: 'allTime',
        categories: ['history', 'history']
      })
    ).toThrow('unique')
  })

  it('accepts bounded public history metadata and rejects private URL forms', () => {
    const entry = {
      schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
      historyId: 'browser-history:123e4567-e89b-42d3-a456-426614174000',
      url: 'https://example.test/docs',
      title: 'Documentation',
      hostname: 'example.test',
      faviconUrl: 'https://example.test/favicon.ico',
      visitedAt: 1
    }
    expect(parseBrowserHistoryEntry(entry)).toEqual(entry)
    expect(() => parseBrowserHistoryEntry({ ...entry, url: 'file:///tmp/private' })).toThrow(
      'HTTP(S)'
    )
    expect(() =>
      parseBrowserHistoryEntry({ ...entry, url: 'https://user:secret@example.test/private' })
    ).toThrow('credential-free')
    expect(() =>
      parseBrowserHistoryListInput({
        schemaVersion: BROWSER_DATA_SCHEMA_VERSION,
        query: 'x'.repeat(257),
        limit: 20
      })
    ).toThrow('invalid query')
  })
})
