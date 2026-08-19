import { describe, expect, it } from 'vitest'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  createBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceCommand,
  parseBrowserSurfaceReadyInput,
  parseBrowserSurfaceReadyOutput
} from './browser'

const REQUEST_ID = '5ee8f693-c8e6-48ab-a2c1-9ed774bc18a9'
const SURFACE_ID = 'right-sidebar-browser-browser-1234'

describe('browser surface protocol', () => {
  it('round-trips a bounded inert bootstrap URL', () => {
    const url = createBrowserSurfaceBootstrapUrl(SURFACE_ID)
    expect(url.startsWith('about:blank#')).toBe(true)
    expect(parseBrowserSurfaceBootstrapUrl(url)).toBe(SURFACE_ID)
    expect(parseBrowserSurfaceBootstrapUrl('https://example.invalid/')).toBeNull()
    expect(parseBrowserSurfaceBootstrapUrl('about:blank#mycopilot-browser-surface=%00')).toBeNull()
  })

  it('strictly parses ensure and close commands', () => {
    expect(
      parseBrowserSurfaceCommand({
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        kind: 'ensureAttached',
        requestId: REQUEST_ID
      })
    ).toEqual({ schemaVersion: 1, kind: 'ensureAttached', requestId: REQUEST_ID })
    expect(
      parseBrowserSurfaceCommand({
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        kind: 'closeSurface',
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID
      })
    ).toEqual({
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID
    })
  })

  it('rejects unknown fields and malformed identities', () => {
    expect(() =>
      parseBrowserSurfaceCommand({
        schemaVersion: 1,
        kind: 'ensureAttached',
        requestId: REQUEST_ID,
        webContentsId: 42
      })
    ).toThrow('unknown fields')
    expect(() =>
      parseBrowserSurfaceReadyInput({
        schemaVersion: 1,
        requestId: 'not-random',
        surfaceId: SURFACE_ID
      })
    ).toThrow('request identity')
  })

  it('strictly parses the renderer readiness acknowledgement', () => {
    expect(
      parseBrowserSurfaceReadyInput({
        schemaVersion: 1,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID
      })
    ).toEqual({ schemaVersion: 1, requestId: REQUEST_ID, surfaceId: SURFACE_ID })
    expect(
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: true,
        surfaceId: SURFACE_ID
      })
    ).toEqual({ schemaVersion: 1, accepted: true, surfaceId: SURFACE_ID })
  })
})
