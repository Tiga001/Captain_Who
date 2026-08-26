import { describe, expect, it } from 'vitest'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  createBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceActionInput,
  parseBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceCommand,
  parseBrowserSurfaceReadyInput,
  parseBrowserSurfaceReadyOutput,
  parseBrowserSurfaceSelectedInput,
  parseBrowserSurfaceSelectedOutput,
  parseBrowserSurfaceState
} from './browser'

const REQUEST_ID = '5ee8f693-c8e6-48ab-a2c1-9ed774bc18a9'
const SURFACE_ID = 'right-sidebar-browser-browser-1234'
const SURFACE_INSTANCE_ID = 'instance-00000001'

describe('browser surface protocol', () => {
  it('strictly binds navigation actions and state without admitting internal data URLs', () => {
    const action = {
      schemaVersion: 1,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID,
      action: 'navigate',
      url: 'https://example.test/path?q=value'
    }
    expect(parseBrowserSurfaceActionInput(action)).toEqual(action)
    expect(() =>
      parseBrowserSurfaceActionInput({ ...action, url: 'data:text/html,unsafe' })
    ).toThrow('navigation URL')
    expect(() => parseBrowserSurfaceActionInput({ ...action, action: 'reload' })).toThrow(
      'Only browser navigation actions'
    )

    expect(
      parseBrowserSurfaceState({
        schemaVersion: 1,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        stateRevision: 4,
        url: action.url,
        title: 'Example',
        faviconUrl: null,
        canGoBack: false,
        canGoForward: false,
        isLoading: false,
        presentation: 'host-fallback',
        crashError: null,
        loadError: {
          kind: 'dns',
          errorCode: -105,
          errorDescription: 'ERR_NAME_NOT_RESOLVED',
          failedUrl: action.url,
          title: 'Cannot open page',
          heading: 'Cannot open page',
          summary: 'could not be found.',
          suggestions: ['Check DNS']
        }
      })
    ).toMatchObject({
      stateRevision: 4,
      url: action.url,
      presentation: 'host-fallback',
      loadError: { kind: 'dns' }
    })
  })

  it('strictly separates renderer failures from network load failures', () => {
    const crashState = {
      schemaVersion: 1,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID,
      stateRevision: 5,
      url: 'https://example.test/page',
      title: 'Page renderer stopped',
      faviconUrl: null,
      canGoBack: true,
      canGoForward: false,
      isLoading: false,
      presentation: 'crash-page',
      loadError: null,
      crashError: {
        kind: 'renderer_crashed',
        title: 'Page renderer stopped',
        heading: 'Page renderer stopped',
        summary: 'The page renderer exited unexpectedly.',
        actionLabel: 'Recreate page'
      }
    }

    expect(parseBrowserSurfaceState(crashState)).toMatchObject({
      presentation: 'crash-page',
      crashError: { kind: 'renderer_crashed' }
    })
    expect(() =>
      parseBrowserSurfaceState({
        ...crashState,
        loadError: {
          kind: 'generic',
          errorCode: -2,
          errorDescription: 'ERR_FAILED',
          failedUrl: crashState.url,
          title: 'Failed',
          heading: 'Failed',
          summary: 'Failed',
          suggestions: []
        }
      })
    ).toThrow('multiple failures')
    expect(() => parseBrowserSurfaceState({ ...crashState, presentation: 'content' })).toThrow(
      'content presentation'
    )
    expect(() => parseBrowserSurfaceState({ ...crashState, crashError: null })).toThrow(
      'requires a renderer failure'
    )
    expect(() => parseBrowserSurfaceState({ ...crashState, url: 'data:text/html,forged' })).toThrow(
      'navigation URL'
    )
  })

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
        schemaVersion: 1,
        kind: 'createSurface',
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        activate: false
      })
    ).toEqual({
      schemaVersion: 1,
      kind: 'createSurface',
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID,
      activate: false
    })
    expect(
      parseBrowserSurfaceCommand({
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        kind: 'ensureAttached',
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID
      })
    ).toEqual({
      schemaVersion: 1,
      kind: 'ensureAttached',
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID
    })
    expect(
      parseBrowserSurfaceCommand({
        schemaVersion: 1,
        kind: 'resizeSurface',
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        width: 1280,
        height: 720
      })
    ).toEqual({
      schemaVersion: 1,
      kind: 'resizeSurface',
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID,
      width: 1280,
      height: 720
    })
    expect(
      parseBrowserSurfaceCommand({
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        kind: 'closeSurface',
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toEqual({
      schemaVersion: 1,
      kind: 'closeSurface',
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID
    })
  })

  it('rejects unknown fields and malformed identities', () => {
    expect(() =>
      parseBrowserSurfaceCommand({
        schemaVersion: 1,
        kind: 'ensureAttached',
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        webContentsId: 42
      })
    ).toThrow('unknown fields')
    expect(() =>
      parseBrowserSurfaceReadyInput({
        schemaVersion: 1,
        requestId: 'not-random',
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toThrow('request identity')
    expect(
      parseBrowserSurfaceCommand({
        schemaVersion: 1,
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
    expect(() =>
      parseBrowserSurfaceReadyInput({
        schemaVersion: 1,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: 'short'
      })
    ).toThrow('instance identity')
  })

  it('strictly parses the renderer readiness acknowledgement', () => {
    expect(
      parseBrowserSurfaceReadyInput({
        schemaVersion: 1,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        viewport: { height: 0, width: 0 }
      })
    ).toEqual({
      schemaVersion: 1,
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID,
      viewport: { height: 0, width: 0 }
    })
    expect(
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: true,
        status: 'applied',
        reason: 'surface_ready',
        retryable: false,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toEqual({
      schemaVersion: 1,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID
    })
    expect(
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: false,
        status: 'stale',
        reason: 'request_expired',
        retryable: false,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toEqual({
      schemaVersion: 1,
      accepted: false,
      status: 'stale',
      reason: 'request_expired',
      retryable: false,
      requestId: REQUEST_ID,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID
    })
    expect(
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: false,
        status: 'noop',
        reason: 'not_registered',
        retryable: true,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toMatchObject({ status: 'noop', reason: 'not_registered', retryable: true })
    expect(
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: false,
        status: 'stale',
        reason: 'instance_mismatch',
        retryable: true,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toMatchObject({ status: 'stale', reason: 'instance_mismatch', retryable: true })
    expect(() =>
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: false,
        status: 'noop',
        reason: 'request_expired',
        retryable: false,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toThrow('noop output')
    expect(() =>
      parseBrowserSurfaceReadyOutput({
        schemaVersion: 1,
        accepted: false,
        status: 'stale',
        reason: 'instance_mismatch',
        retryable: false,
        requestId: REQUEST_ID,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID
      })
    ).toThrow('stale output')
  })

  it('strictly parses bound, probe, and clear manual surface selections', () => {
    expect(
      parseBrowserSurfaceSelectedInput({
        schemaVersion: 1,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        selectionRevision: 3
      })
    ).toEqual({
      schemaVersion: 1,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID,
      selectionRevision: 3
    })
    expect(
      parseBrowserSurfaceSelectedInput({
        schemaVersion: 1,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: null,
        selectionRevision: 4
      })
    ).toEqual({
      schemaVersion: 1,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: null,
      selectionRevision: 4
    })
    expect(
      parseBrowserSurfaceSelectedInput({
        schemaVersion: 1,
        surfaceId: null,
        surfaceInstanceId: null,
        selectionRevision: 5
      })
    ).toEqual({
      schemaVersion: 1,
      surfaceId: null,
      surfaceInstanceId: null,
      selectionRevision: 5
    })
  })

  it('strictly parses applied, no-op, and stale selection results', () => {
    expect(
      parseBrowserSurfaceSelectedOutput({
        schemaVersion: 1,
        status: 'applied',
        reason: 'selection_applied',
        retryable: false,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        selectionRevision: 3,
        authoritativeRevision: 3
      })
    ).toEqual({
      schemaVersion: 1,
      status: 'applied',
      reason: 'selection_applied',
      retryable: false,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: SURFACE_INSTANCE_ID,
      selectionRevision: 3,
      authoritativeRevision: 3
    })
    expect(
      parseBrowserSurfaceSelectedOutput({
        schemaVersion: 1,
        status: 'noop',
        reason: 'instance_required',
        retryable: true,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        selectionRevision: 4,
        authoritativeRevision: 3
      })
    ).toMatchObject({ status: 'noop', reason: 'instance_required', retryable: true })
    expect(
      parseBrowserSurfaceSelectedOutput({
        schemaVersion: 1,
        status: 'noop',
        reason: 'not_registered',
        retryable: true,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: null,
        selectionRevision: 5,
        authoritativeRevision: 3
      })
    ).toMatchObject({ status: 'noop', reason: 'not_registered', surfaceInstanceId: null })
    expect(
      parseBrowserSurfaceSelectedOutput({
        schemaVersion: 1,
        status: 'stale',
        reason: 'stale_revision',
        retryable: false,
        surfaceId: null,
        surfaceInstanceId: null,
        selectionRevision: 2,
        authoritativeRevision: 3
      })
    ).toMatchObject({ status: 'stale', reason: 'stale_revision' })
  })

  it('rejects malformed selection bindings, revisions, result variants, and unknown fields', () => {
    expect(() =>
      parseBrowserSurfaceSelectedInput({
        schemaVersion: 1,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        selectionRevision: 1,
        webContentsId: 42
      })
    ).toThrow('unknown fields')
    expect(() =>
      parseBrowserSurfaceSelectedInput({
        schemaVersion: 1,
        surfaceId: null,
        surfaceInstanceId: SURFACE_INSTANCE_ID,
        selectionRevision: 1
      })
    ).toThrow('cannot carry an instance')
    expect(() =>
      parseBrowserSurfaceSelectedInput({
        schemaVersion: 1,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: null,
        selectionRevision: 0
      })
    ).toThrow('selection revision')
    expect(() =>
      parseBrowserSurfaceSelectedOutput({
        schemaVersion: 1,
        status: 'applied',
        reason: 'not_registered',
        retryable: true,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: null,
        selectionRevision: 1,
        authoritativeRevision: 0
      })
    ).toThrow('applied')
    expect(() =>
      parseBrowserSurfaceSelectedOutput({
        schemaVersion: 1,
        status: 'stale',
        reason: 'instance_required',
        retryable: true,
        surfaceId: SURFACE_ID,
        surfaceInstanceId: null,
        selectionRevision: 1,
        authoritativeRevision: 0
      })
    ).toThrow('selection reason')
  })
})
