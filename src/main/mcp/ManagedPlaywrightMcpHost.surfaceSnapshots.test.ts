import { afterEach, describe, expect, it, vi } from 'vitest'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { BrowserContext } from 'playwright'
import { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import {
  appendSafeFrameEditorCandidates,
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  surfaceView,
  singleSurfaceGroup,
  fakeBrowserContext,
  fakeHost
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('reports structured surface failures while leaving explicit recovery navigation available', async () => {
    const surface = surfaceView(0, true)
    surface.loadError = {
      errorCode: -106,
      errorDescription: 'ERR_INTERNET_DISCONNECTED',
      failedUrl: 'http://127.0.0.1/offline',
      heading: 'Unable to reach this site',
      kind: 'offline',
      suggestions: ['Check the network connection'],
      summary: 'The computer is offline.',
      title: 'Unable to reach this site'
    }
    surface.presentation = 'error-page'
    surface.url = surface.loadError.failedUrl
    const lease = {
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn(),
      generation: surface.generation,
      index: surface.index,
      resolveIndex: vi.fn(async () => surface.index),
      selectionRevision: 1,
      surfaceId: surface.surfaceId
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroup([surface]),
      beginExistingToolSurfaceLease: vi.fn(async () => lease),
      beginToolSurfaceLease: vi.fn(async () => lease)
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => ({
      content: [{ type: 'text', text: 'navigated' }],
      isError: false
    }))
    const host = fakeHost({ callTool: upstream, surfaceGroup })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Inspect the failed page.' })
    ).resolves.toMatchObject({
      isError: true,
      structuredContent: {
        generation: surface.generation,
        loadError: { failedUrl: surface.url, kind: 'offline' },
        status: 'load_error',
        surfaceId: surface.surfaceId
      }
    })
    expect(upstream).not.toHaveBeenCalled()

    await expect(
      host.callTool('browser_navigate', {
        call_reason: 'Retry the exact failed destination.',
        url: surface.url
      })
    ).resolves.toMatchObject({ isError: false })
    expect(upstream).toHaveBeenCalledWith(
      { arguments: { url: surface.url }, name: 'browser_navigate' },
      undefined,
      expect.any(Object)
    )
    const callsAfterNavigate = upstream.mock.calls.length

    surface.loadError = null
    surface.crashError = {
      actionLabel: 'Recreate page',
      heading: 'Page renderer stopped',
      kind: 'renderer_crashed',
      summary: 'The page renderer exited unexpectedly.',
      title: 'Page renderer stopped'
    }
    surface.presentation = 'crash-page'
    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Inspect the crashed page.' })
    ).resolves.toMatchObject({
      isError: true,
      structuredContent: {
        crashError: { kind: 'renderer_crashed' },
        status: 'renderer_failure'
      }
    })
    expect(upstream).toHaveBeenCalledTimes(callsAfterNavigate)
  })

  it('allows a screenshot of an internal error page while blocking page-content automation', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-error-screenshot-test-'))
    const artifactBroker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const surface = surfaceView(0, true)
    surface.loadError = {
      errorCode: -106,
      errorDescription: 'ERR_INTERNET_DISCONNECTED',
      failedUrl: surface.url,
      heading: 'Unable to reach this site',
      kind: 'offline',
      suggestions: ['Check the network connection'],
      summary: 'The computer is offline.',
      title: 'Unable to reach this site'
    }
    surface.presentation = 'error-page'
    const lease = {
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn(),
      generation: surface.generation,
      index: surface.index,
      resolveIndex: vi.fn(async () => surface.index),
      selectionRevision: 1,
      surfaceId: surface.surfaceId
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroup([surface]),
      beginExistingToolSurfaceLease: vi.fn(async () => lease),
      beginToolSurfaceLease: vi.fn(async () => lease)
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ arguments: args, name }) => {
      if (name === 'browser_tabs') {
        return { content: [{ type: 'text', text: 'selected' }], isError: false }
      }
      const meta = args._meta as { cwd: string }
      await writeFile(join(meta.cwd, String(args.filename)), Uint8Array.from([1, 2, 3, 4]))
      return { content: [{ type: 'text', text: 'error page captured' }], isError: false }
    })
    const host = fakeHost({
      artifactBroker,
      callTool: upstream,
      getBrowserContext: async () => fakeBrowserContext(),
      surfaceGroup
    })
    const screenshotArguments = {
      filename: 'load-error.png',
      scale: 'css',
      call_reason: 'Capture the visible managed-browser load error.'
    }
    try {
      await expect(
        host.callTool('browser_take_screenshot', screenshotArguments, {
          authorizationContext: { ...RISK_CONTEXT, callId: 'call-error-page-screenshot' }
        })
      ).resolves.toMatchObject({
        isError: false,
        structuredContent: {
          artifacts: [expect.objectContaining({ displayName: 'load-error.png', kind: 'image' })],
          status: 'completed'
        }
      })
      expect(upstream.mock.calls.at(-1)?.[0].name).toBe('browser_take_screenshot')
      const callsAfterScreenshot = upstream.mock.calls.length

      await expect(
        host.callTool('browser_snapshot', { call_reason: 'Inspect target-site content.' })
      ).resolves.toMatchObject({
        isError: true,
        structuredContent: { loadError: { kind: 'offline' }, status: 'load_error' }
      })
      expect(upstream).toHaveBeenCalledTimes(callsAfterScreenshot)
      expect(lease.finish).toHaveBeenCalledTimes(2)
    } finally {
      await host.close()
      await artifactBroker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('projects frame failures to value-free typed results', async () => {
    const diagnostics = [
      [
        'Ref f1e2 not found in the current page snapshot. Try capturing new snapshot.',
        'stale_frame_ref'
      ],
      ['"frameLocator(SECRET_SELECTOR)" does not match any elements.', 'frame_not_found'],
      ['Element is not editable', 'frame_not_editable'],
      ['frame_input_delivery_failed', 'frame_input_delivery_failed'],
      ['Frame was detached', 'frame_detached'],
      ['Target page, context or browser has been closed', 'target_closed']
    ] as const
    const callTool = vi.fn()
    for (const [diagnostic] of diagnostics) {
      callTool.mockResolvedValueOnce({
        content: [{ type: 'text', text: diagnostic }],
        isError: true
      })
    }
    const host = fakeHost({ callTool })

    for (const [, errorCode] of diagnostics) {
      const result = await host.callTool('browser_type', {
        target: 'frameLocator("iframe").locator("[contenteditable]")',
        text: 'fixture-value-canary',
        call_reason: 'Exercise a local iframe failure.'
      })
      expect(result).toEqual({
        content: [{ type: 'text', text: `Managed browser frame operation failed: ${errorCode}.` }],
        structuredContent: { status: 'failed', errorCode, contentOmitted: true },
        isError: true
      })
      expect(JSON.stringify(result)).not.toContain('SECRET_SELECTOR')
      expect(JSON.stringify(result)).not.toContain('fixture-value-canary')
    }
  })

  it('adds a value-free editor candidate when an otherwise successful snapshot is blank', async () => {
    const candidateElement = {
      dispose: vi.fn(async () => undefined)
    }
    const childFrame = {
      childFrames: () => [],
      frameElement: vi.fn(async () => ({
        evaluate: vi.fn(async () => ({ index: 0, type: 'iframe' })),
        dispose: vi.fn(async () => undefined)
      })),
      locator: vi.fn(() => ({
        evaluateAll: vi.fn(async () => [{ index: 0, kind: 'contenteditable' }]),
        nth: vi.fn(() => ({ elementHandle: vi.fn(async () => candidateElement) }))
      }))
    }
    const mainFrame = { childFrames: () => [childFrame] }
    const context = {
      pages: () => [{ mainFrame: () => mainFrame }]
    } as unknown as BrowserContext

    const result = await appendSafeFrameEditorCandidates(
      { content: [{ type: 'text', text: 'Page snapshot is empty.' }], isError: false },
      context,
      () => 'managed-frame-editor:123e4567-e89b-42d3-a456-426614174099'
    )
    expect(result.content).toEqual([
      { type: 'text', text: 'Page snapshot is empty.' },
      {
        type: 'text',
        text: expect.stringContaining('managed-frame-editor:123e4567-e89b-42d3-a456-426614174099')
      }
    ])
    expect(JSON.stringify(result)).not.toContain('nth=')
    expect(JSON.stringify(result)).not.toMatch(/https?:\/\/|PRIVATE_FRAME_SECRET_CANARY/)
  })

  it('binds a blank-iframe editor target to one exact element and rejects stale generations', async () => {
    const element = {
      click: vi.fn(async () => undefined),
      dispose: vi.fn(async () => undefined),
      evaluate: vi.fn(async () => 'contenteditable'),
      fill: vi.fn(async () => undefined),
      press: vi.fn(async () => undefined),
      type: vi.fn(async () => undefined)
    }
    const frame = {
      childFrames: () => [],
      frameElement: vi.fn(async () => ({
        evaluate: vi.fn(async () => ({ index: 0, type: 'iframe' })),
        dispose: vi.fn(async () => undefined)
      })),
      isDetached: () => false,
      locator: vi.fn(() => ({
        evaluateAll: vi.fn(async () => [{ index: 0, kind: 'contenteditable' }]),
        nth: vi.fn(() => ({ elementHandle: vi.fn(async () => element) }))
      }))
    }
    const context = {
      ...fakeBrowserContext(),
      pages: () => [{ mainFrame: () => ({ childFrames: () => [frame] }) }]
    } as unknown as BrowserContext
    const surfaces = [surfaceView(0, true)]
    const upstream = vi.fn(async ({ name }: { name: string }) => ({
      content: [
        {
          type: 'text' as const,
          text: name === 'browser_snapshot' ? 'Page snapshot is empty.' : 'must-not-dispatch'
        }
      ],
      isError: false
    }))
    const host = fakeHost({
      callTool: upstream as ManagedMcpClient['callTool'],
      getBrowserContext: async () => context,
      surfaceGroup: singleSurfaceGroup(surfaces)
    })

    const snapshot = await host.callTool(
      'browser_snapshot',
      { call_reason: 'Locate the blank fixture editor.' },
      { authorizationContext: RISK_CONTEXT }
    )
    const snapshotText = snapshot.content
      .flatMap((block) => (block.type === 'text' ? [block.text] : []))
      .join('\n')
    const opaqueTarget = snapshotText.match(/managed-frame-editor:[0-9a-f-]{36}/u)?.[0]
    expect(opaqueTarget).toBeTruthy()

    await expect(
      host.callTool(
        'browser_type',
        {
          target: opaqueTarget,
          text: '你好',
          call_reason: 'Fill the exact blank fixture editor.'
        },
        { authorizationContext: RISK_CONTEXT }
      )
    ).resolves.toMatchObject({ isError: false })
    expect(element.fill).toHaveBeenCalledWith('你好')
    expect(
      upstream.mock.calls.filter(([request]) => request.name === 'browser_snapshot')
    ).toHaveLength(1)

    const secondSnapshot = await host.callTool(
      'browser_snapshot',
      { call_reason: 'Refresh the blank fixture editor.' },
      { authorizationContext: RISK_CONTEXT }
    )
    const secondTarget = secondSnapshot.content
      .flatMap((block) => (block.type === 'text' ? [block.text] : []))
      .join('\n')
      .match(/managed-frame-editor:[0-9a-f-]{36}/u)?.[0]
    expect(secondTarget).toBeTruthy()
    surfaces[0].generation += 1

    await expect(
      host.callTool(
        'browser_type',
        {
          target: secondTarget,
          text: 'must-not-land',
          call_reason: 'Exercise a stale blank editor token.'
        },
        { authorizationContext: RISK_CONTEXT }
      )
    ).resolves.toMatchObject({
      isError: true,
      structuredContent: { status: 'failed', errorCode: 'stale_frame_ref' }
    })
    expect(element.fill).toHaveBeenCalledTimes(1)
    expect(JSON.stringify(secondSnapshot)).not.toContain('nth=')
  })
})
