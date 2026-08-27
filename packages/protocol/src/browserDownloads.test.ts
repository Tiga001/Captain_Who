import { describe, expect, it } from 'vitest'

import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  parseBrowserDownloadAskWhereToSaveInput,
  parseBrowserDownloadHistoryListOutput,
  parseBrowserDownloadReference,
  parseBrowserDownloadRegistrationInput,
  parseBrowserDownloadSettingsRecord,
  parseBrowserDownloadSettingsView
} from './browserDownloads'

const reference = {
  schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
  downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
  displayName: 'archive.zip',
  mimeType: 'application/zip',
  sizeBytes: 351,
  sha256: 'a'.repeat(64),
  createdAt: 1_000,
  source: 'agent'
} as const

describe('Browser Download protocol', () => {
  it('accepts only a strict path-free durable reference', () => {
    expect(parseBrowserDownloadReference(reference)).toEqual(reference)
    expect(parseBrowserDownloadReference({ ...reference, sizeBytes: 0 })).toMatchObject({
      sizeBytes: 0
    })
    expect(() =>
      parseBrowserDownloadReference({ ...reference, absolutePath: '/Users/private/archive.zip' })
    ).toThrow(/unexpected field absolutePath/)
    expect(() =>
      parseBrowserDownloadReference({ ...reference, displayName: '../../archive.zip' })
    ).toThrow(/path-free/)
    expect(() =>
      parseBrowserDownloadReference({ ...reference, displayName: '<img onerror=secret>.zip' })
    ).toThrow(/path-free/)
    expect(() =>
      parseBrowserDownloadReference({ ...reference, displayName: ' archive.zip' })
    ).toThrow(/canonical/)
    expect(() =>
      parseBrowserDownloadReference({ ...reference, downloadId: 'browser-download:not-an-id' })
    ).toThrow(/invalid id/)
  })

  it('keeps Renderer settings and history path-free', () => {
    expect(
      parseBrowserDownloadSettingsView({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system',
        displayPath: '~/Downloads',
        askWhereToSave: false,
        revision: 0,
        updatedAt: 0
      })
    ).toEqual({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      locationMode: 'system',
      displayPath: '~/Downloads',
      askWhereToSave: false,
      revision: 0,
      updatedAt: 0
    })
    expect(() =>
      parseBrowserDownloadHistoryListOutput({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        downloads: [
          {
            ...reference,
            availability: 'available',
            sourceOrigin: 'https://example.test',
            absolutePath: '/Users/private/Downloads/archive.zip'
          }
        ],
        truncated: false
      })
    ).toThrow(/absolutePath/)
  })

  it('separates private Core records from public references', () => {
    expect(
      parseBrowserDownloadRegistrationInput({
        ...reference,
        absolutePath: '/Users/private/Downloads/archive.zip',
        sourceOrigin: 'https://example.test',
        conversationId: 'conversation-1',
        runId: 'run-1',
        callId: 'call-1'
      })
    ).toMatchObject({ absolutePath: '/Users/private/Downloads/archive.zip' })
    expect(() =>
      parseBrowserDownloadRegistrationInput({
        ...reference,
        displayName: '../archive.zip',
        absolutePath: '/Users/private/Downloads/archive.zip',
        sourceOrigin: 'https://example.test',
        conversationId: 'conversation-1',
        runId: 'run-1',
        callId: 'call-1'
      })
    ).toThrow(/path-free/)
    expect(() =>
      parseBrowserDownloadRegistrationInput({
        ...reference,
        absolutePath: '/Users/private/Downloads/archive.zip',
        sourceOrigin: 'https://example.test',
        conversationId: null,
        runId: null,
        callId: null
      })
    ).toThrow(/require conversation/)
  })

  it('enforces the custom-directory settings invariant', () => {
    expect(
      parseBrowserDownloadSettingsRecord({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'custom',
        customDirectory: '/Users/private/Downloads',
        askWhereToSave: true,
        revision: 1,
        updatedAt: 2
      })
    ).toMatchObject({ locationMode: 'custom' })
    expect(() =>
      parseBrowserDownloadSettingsRecord({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        locationMode: 'system',
        customDirectory: '/Users/private/Downloads',
        askWhereToSave: false,
        revision: 1,
        updatedAt: 2
      })
    ).toThrow(/custom location/)
  })

  it('accepts only an explicit boolean for the save-dialog preference', () => {
    expect(
      parseBrowserDownloadAskWhereToSaveInput({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        askWhereToSave: true
      })
    ).toEqual({
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      askWhereToSave: true
    })
    expect(() =>
      parseBrowserDownloadAskWhereToSaveInput({
        schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
        askWhereToSave: 1
      })
    ).toThrow(/expected a boolean/)
  })
})
