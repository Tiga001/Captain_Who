import { afterEach, describe, expect, it, vi } from 'vitest'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import {
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  surfaceView,
  singleSurfaceGroup,
  testSurfaceLeaseApi,
  sensitiveTargetFromSurfaces,
  sensitiveContext,
  fakeBrowserContext,
  fakeHost
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('publishes file tools only as path-free Artifact references and hides upstream paths', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-artifact-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const context = fakeBrowserContext()
    const callTool = vi.fn(async ({ arguments: args }: { arguments: Record<string, unknown> }) => {
      const meta = args._meta as { cwd: string }
      await writeFile(join(meta.cwd, String(args.filename)), Uint8Array.from([1, 2, 3]))
      return {
        content: [{ type: 'text', text: `saved to ${meta.cwd}/${String(args.filename)}` }],
        isError: false
      }
    })
    const host = fakeHost({
      artifactBroker: broker,
      callTool: callTool as ManagedMcpClient['callTool'],
      getBrowserContext: async () => context
    })
    try {
      await expect(
        host.callTool(
          'browser_take_screenshot',
          { scale: 'css', filename: '../../escape.png', call_reason: 'Take a screenshot.' },
          { authorizationContext: RISK_CONTEXT }
        )
      ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
      expect(callTool).not.toHaveBeenCalled()

      const result = await host.callTool(
        'browser_take_screenshot',
        { scale: 'css', filename: 'page.png', call_reason: 'Take a screenshot.' },
        { authorizationContext: { ...RISK_CONTEXT, callId: 'call-screenshot-success' } }
      )
      expect(result).toMatchObject({
        structuredContent: {
          status: 'completed',
          artifacts: [
            {
              schemaVersion: 1,
              kind: 'image',
              displayName: 'page.png',
              mimeType: 'image/png',
              sizeBytes: 3,
              owner: 'browser_automation'
            }
          ]
        },
        isError: false
      })
      const { hostArtifactPublishPath, ...mcpResult } = result
      expect(hostArtifactPublishPath).toEqual(
        expect.stringContaining('browser-automation-artifacts')
      )
      expect(JSON.stringify(mcpResult)).not.toContain(parent)
      expect(JSON.stringify(mcpResult)).not.toContain(hostArtifactPublishPath)
      expect(callTool).toHaveBeenLastCalledWith(
        expect.objectContaining({
          arguments: expect.objectContaining({
            filename: expect.not.stringContaining('page.png/'),
            _meta: { cwd: expect.stringContaining('browser-automation-artifacts') }
          })
        }),
        undefined,
        expect.any(Object)
      )
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('omits host screenshot publish paths when the image exceeds the read_image limit', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-screenshot-too-large-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts'),
      maxArtifactBytes: 9 * 1024 * 1024
    })
    const context = fakeBrowserContext()
    const oversized = Buffer.alloc(8 * 1024 * 1024 + 1, 1)
    const callTool = vi.fn(async ({ arguments: args }: { arguments: Record<string, unknown> }) => {
      const meta = args._meta as { cwd: string }
      await writeFile(join(meta.cwd, String(args.filename)), oversized)
      return {
        content: [{ type: 'text', text: `saved to ${meta.cwd}/${String(args.filename)}` }],
        isError: false
      }
    })
    const host = fakeHost({
      artifactBroker: broker,
      callTool: callTool as ManagedMcpClient['callTool'],
      getBrowserContext: async () => context
    })
    try {
      const result = await host.callTool(
        'browser_take_screenshot',
        { scale: 'css', filename: 'page.png', call_reason: 'Take a screenshot.' },
        { authorizationContext: { ...RISK_CONTEXT, callId: 'call-screenshot-too-large' } }
      )
      expect(result.hostArtifactPublishPath).toBeUndefined()
      expect(result.content[0]).toMatchObject({
        type: 'text',
        text: expect.stringContaining('8 MiB read_image limit')
      })
      expect(result.structuredContent).toMatchObject({
        status: 'completed',
        artifacts: [expect.objectContaining({ kind: 'image', sizeBytes: oversized.length })]
      })
      expect(JSON.stringify(result)).not.toContain(parent)
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('binds a page Artifact to the exact leased surface even when UI selection changes before publication', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-exact-artifact-owner-test-'))
    const surfaces = [surfaceView(0, true)]
    let releasePublish!: () => void
    let signalPublishStarted!: () => void
    let wrongSurfaceRelease: Promise<void> | undefined
    const publishStarted = new Promise<void>((resolve) => {
      signalPublishStarted = resolve
    })
    const publishBarrier = new Promise<void>((resolve) => {
      releasePublish = resolve
    })
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts'),
      beforePublish: async () => {
        surfaces[0].isActive = false
        surfaces.push(surfaceView(1, true))
        wrongSurfaceRelease = broker.releaseSurface({
          surfaceId: surfaces[1].surfaceId,
          generation: surfaces[1].generation
        })
        signalPublishStarted()
        await publishBarrier
      }
    })
    const exactSurface = surfaces[0]
    const exactLease = {
      surfaceId: exactSurface.surfaceId,
      generation: exactSurface.generation,
      selectionRevision: 1,
      index: 0,
      resolveIndex: vi.fn(async () => 0),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => exactSurface),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => exactSurface),
      selectSurface: vi.fn(async () => exactSurface),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      if (name === 'browser_tabs') {
        return { content: [{ type: 'text', text: 'selected' }], isError: false }
      }
      const meta = args._meta as { cwd: string }
      await writeFile(join(meta.cwd, String(args.filename)), Uint8Array.from([7, 8, 9]))
      return { content: [{ type: 'text', text: 'screenshot captured' }], isError: false }
    })
    const host = fakeHost({
      artifactBroker: broker,
      callTool: upstream,
      getBrowserContext: async () => fakeBrowserContext(),
      surfaceGroup
    })
    try {
      const pending = host.callTool(
        'browser_take_screenshot',
        {
          scale: 'css',
          filename: 'exact-surface.png',
          call_reason: 'Capture the exact leased fixture tab.'
        },
        {
          authorizationContext: {
            ...RISK_CONTEXT,
            callId: 'call-exact-artifact-owner',
            triggerToolName: 'browser_take_screenshot'
          }
        }
      )
      await publishStarted
      expect(surfaces.find((surface) => surface.isActive)?.surfaceId).toBe('surface-1')
      releasePublish()
      await expect(pending).resolves.toMatchObject({
        structuredContent: {
          artifacts: [expect.objectContaining({ displayName: 'exact-surface.png' })]
        },
        isError: false
      })
      await wrongSurfaceRelease
      expect(exactLease.finish).toHaveBeenCalledOnce()
    } finally {
      releasePublish()
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('routes PDF through the fixed official tool and publishes only its reserved Artifact', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-pdf-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const exactLease = {
      surfaceId: 'surface-0',
      generation: 1,
      selectionRevision: 1,
      index: 0,
      resolveIndex: vi.fn(async () => 0),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => [surfaceView(0, true)]),
      getSensitiveTargetIdentity: () => ({
        surfaceId: 'surface-0',
        generation: 1,
        navigationEpoch: 1,
        origin: 'http://127.0.0.1'
      }),
      ensureActiveSurface: vi.fn(async () => surfaceView(0, true)),
      listSurfaces: () => [surfaceView(0, true)],
      createSurface: vi.fn(async () => surfaceView(0, true)),
      selectSurface: vi.fn(async () => surfaceView(0, true)),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const upstreamCall = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      if (name === 'browser_tabs') {
        return { content: [{ type: 'text', text: 'selected' }], isError: false }
      }
      expect(name).toBe('browser_pdf_save')
      const meta = args._meta as { cwd: string }
      await writeFile(
        join(meta.cwd, String(args.filename)),
        Uint8Array.from(Buffer.from('%PDF-1.7\nfixture\n%%EOF\n'))
      )
      return {
        content: [{ type: 'text', text: `Saved page as ${meta.cwd}/${String(args.filename)}` }],
        isError: false
      }
    })
    const host = fakeHost({
      artifactBroker: broker,
      callTool: upstreamCall,
      getBrowserContext: async () => fakeBrowserContext(),
      surfaceGroup
    })
    try {
      const result = await host.callTool(
        'browser_pdf_save',
        { filename: 'fixture.pdf', call_reason: 'Save the fixture as a PDF.' },
        { authorizationContext: { ...RISK_CONTEXT, callId: 'call-pdf-success' } }
      )
      expect(upstreamCall.mock.calls.map(([request]) => request.name)).toEqual([
        'browser_tabs',
        'browser_tabs',
        'browser_pdf_save'
      ])
      expect(upstreamCall).toHaveBeenLastCalledWith(
        {
          name: 'browser_pdf_save',
          arguments: expect.objectContaining({
            filename: expect.any(String),
            _meta: { cwd: expect.stringContaining('browser-automation-artifacts') }
          })
        },
        undefined,
        expect.any(Object)
      )
      expect(exactLease.finish).toHaveBeenCalledOnce()
      expect(result).toMatchObject({
        structuredContent: {
          status: 'completed',
          artifacts: [
            {
              schemaVersion: 1,
              kind: 'pdf',
              displayName: 'fixture.pdf',
              mimeType: 'application/pdf',
              owner: 'browser_automation'
            }
          ]
        },
        isError: false
      })
      const { hostArtifactPublishPath, ...mcpResult } = result
      expect(hostArtifactPublishPath).toEqual(
        expect.stringContaining('browser-automation-artifacts')
      )
      expect(await readFile(hostArtifactPublishPath!)).toEqual(
        Buffer.from('%PDF-1.7\nfixture\n%%EOF\n')
      )
      expect(JSON.stringify(mcpResult)).not.toContain(parent)
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('projects the managed Electron PDF sentinel as typed unavailable without raw details', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-pdf-unavailable-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const exactLease = {
      surfaceId: 'surface-0',
      generation: 1,
      selectionRevision: 1,
      index: 0,
      resolveIndex: vi.fn(async () => 0),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => [surfaceView(0, true)]),
      getSensitiveTargetIdentity: () => ({
        surfaceId: exactLease.surfaceId,
        generation: exactLease.generation,
        navigationEpoch: 1,
        origin: 'http://127.0.0.1'
      }),
      ensureActiveSurface: vi.fn(async () => surfaceView(0, true)),
      listSurfaces: () => [surfaceView(0, true)],
      createSurface: vi.fn(async () => surfaceView(0, true)),
      selectSurface: vi.fn(async () => surfaceView(0, true)),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const upstreamCall = vi.fn<ManagedMcpClient['callTool']>(async ({ name }) =>
      name === 'browser_tabs'
        ? { content: [{ type: 'text', text: 'selected' }], isError: false }
        : {
            content: [
              {
                type: 'text',
                text: 'Error: browser.pdf_unavailable INTERNAL_PDF_ERROR_CANARY'
              }
            ],
            isError: true
          }
    )
    const host = fakeHost({
      artifactBroker: broker,
      callTool: upstreamCall,
      getBrowserContext: async () => fakeBrowserContext(),
      surfaceGroup
    })
    try {
      const result = await host.callTool(
        'browser_pdf_save',
        { filename: 'fixture.pdf', call_reason: 'Attempt fixture PDF export.' },
        { authorizationContext: { ...RISK_CONTEXT, callId: 'call-pdf-unavailable' } }
      )
      expect(result).toEqual({
        content: [
          {
            type: 'text',
            text: 'PDF export is unavailable in the current managed Electron browser.'
          }
        ],
        structuredContent: {
          status: 'unavailable',
          code: 'browser.pdf_unavailable',
          platformScope: 'managed_electron'
        },
        isError: true
      })
      expect(JSON.stringify(result)).not.toContain('INTERNAL_PDF_ERROR_CANARY')
      expect(exactLease.finish).toHaveBeenCalledOnce()
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('exports complete official console and network text as protected Host Artifacts', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-diagnostic-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const liveConsoleCanary = 'LIVE_CONSOLE_ARTIFACT_CANARY_A7fQ9vLm2KxP'
    const liveNetworkCanary = 'LIVE_NETWORK_ARTIFACT_CANARY_B9xR4mNp7TzQ'
    const officialText = (name: string): string =>
      name === 'browser_network_requests'
        ? [
            '### Result',
            `1. [GET] https://example.test/api?query=complete => [204] ${liveNetworkCanary}`,
            '2. [POST] http://example.test/submit => [FAILED] connection reset'
          ].join('\n')
        : [
            '### Result',
            'Total messages: 4 (Errors: 1, Warnings: 2)',
            'Returning 4 messages for level "debug"',
            '',
            `[debug] ${liveConsoleCanary}`
          ].join('\n')
    const callTool = vi.fn(
      async ({ name, arguments: args }: { name: string; arguments: Record<string, unknown> }) => ({
        content: [{ type: 'text', text: officialText(name) }],
        structuredContent: { liveDiagnostic: true },
        isError: false,
        observedArguments: args
      })
    )
    const host = fakeHost({
      artifactBroker: broker,
      callTool: callTool as ManagedMcpClient['callTool'],
      getBrowserContext: async () => fakeBrowserContext()
    })
    try {
      const exportDirectory = join(parent, 'exports')
      await mkdir(exportDirectory)
      for (const [name, filename, kind, callId] of [
        ['browser_network_requests', 'requests.log', 'network', 'call-network-export'],
        ['browser_console_messages', 'console.log', 'console', 'call-console-export']
      ] as const) {
        const result = await host.callTool(
          name,
          name === 'browser_network_requests'
            ? { static: false, filename, call_reason: 'Export complete diagnostics.' }
            : {
                level: 'debug',
                all: true,
                filename,
                call_reason: 'Export complete diagnostics.'
              },
          { authorizationContext: { ...RISK_CONTEXT, callId } }
        )
        const artifact = (result.structuredContent as { artifacts: unknown[] }).artifacts[0]
        expect(artifact).toEqual(
          expect.objectContaining({ kind, displayName: filename, preview: 'none' })
        )
        expect(JSON.stringify(result)).not.toContain(liveConsoleCanary)
        expect(JSON.stringify(result)).not.toContain(liveNetworkCanary)
        await expect(broker.readPreview(artifact)).rejects.toMatchObject({
          code: 'browser.artifact.preview_unavailable'
        })
        const destination = join(exportDirectory, filename)
        await expect(broker.exportArtifact(artifact, destination)).resolves.toEqual({
          displayName: filename
        })
        expect(await readFile(destination, 'utf8')).toBe(officialText(name))
      }
      for (const invocation of callTool.mock.calls) {
        const parameters = invocation[0] as { arguments: Record<string, unknown> }
        expect(parameters.arguments).not.toHaveProperty('filename')
        expect(parameters.arguments).not.toHaveProperty('_meta')
      }
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })

  it('publishes storage state as a protected metadata-only Artifact with no preview IPC', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-storage-state-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const callTool = vi.fn(
      async ({ arguments: upstreamArguments }: { arguments: Record<string, unknown> }) => {
        const meta = upstreamArguments._meta as { cwd: string }
        await writeFile(
          join(meta.cwd, String(upstreamArguments.filename)),
          JSON.stringify({
            cookies: [{ name: 'session', value: 'PRIVATE_STORAGE_COOKIE_CANARY' }],
            origins: [
              {
                origin: 'http://127.0.0.1',
                localStorage: [{ name: 'token', value: 'PRIVATE_LOCAL_STORAGE_CANARY' }]
              }
            ]
          })
        )
        return {
          content: [{ type: 'text', text: 'PRIVATE_STORAGE_COOKIE_CANARY' }],
          isError: false
        }
      }
    )
    const host = fakeHost({
      artifactBroker: broker,
      callTool: callTool as ManagedMcpClient['callTool'],
      getBrowserContext: async () => fakeBrowserContext(),
      surfaceGroup: singleSurfaceGroup([])
    })
    const args = {
      call_reason: 'Export the fixture storage state.'
    }
    try {
      const result = await host.callTool('browser_storage_state', args, {
        authorizationContext: sensitiveContext('browser_storage_state', args, {
          callId: 'call-storage-export'
        })
      })
      const artifact = (result.structuredContent as { artifacts: unknown[] }).artifacts[0] as {
        preview: string
      }
      expect(artifact).toEqual(expect.objectContaining({ kind: 'json', preview: 'none' }))
      expect(JSON.stringify(result)).not.toMatch(
        /PRIVATE_STORAGE_COOKIE_CANARY|PRIVATE_LOCAL_STORAGE_CANARY/
      )
      await expect(broker.readPreview(artifact)).rejects.toMatchObject({
        code: 'browser.artifact.preview_unavailable'
      })
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('records a context trace with a profile Artifact owner while no visible tab exists', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-zero-tab-trace-test-'))
    const broker = new BrowserArtifactBroker({
      rootDirectory: join(parent, 'browser-automation-artifacts')
    })
    const start = vi.fn(async () => undefined)
    const stop = vi.fn(async ({ path }: { path: string }) => {
      await writeFile(path, Uint8Array.from([80, 75, 3, 4]))
    })
    const context = fakeBrowserContext()
    Object.assign(context.tracing, { start, stop })
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const surfaceGroup = singleSurfaceGroup(surfaces)
    const upstream = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({
      artifactBroker: broker,
      callTool: upstream,
      getBrowserContext: async () => context,
      surfaceGroup
    })
    const startAuthorization = {
      ...RISK_CONTEXT,
      callId: 'call-zero-tab-trace-start',
      triggerToolName: 'browser_start_tracing'
    }
    try {
      await expect(
        host.callTool(
          'browser_start_tracing',
          { call_reason: 'Start a profile trace before opening a visible tab.' },
          { authorizationContext: startAuthorization }
        )
      ).resolves.toMatchObject({ isError: false })
      const result = await host.callTool(
        'browser_stop_tracing',
        { call_reason: 'Stop and publish the profile trace.' },
        {
          authorizationContext: {
            ...startAuthorization,
            callId: 'call-zero-tab-trace-stop',
            triggerToolName: 'browser_stop_tracing'
          }
        }
      )
      expect(result).toMatchObject({
        structuredContent: {
          artifacts: [expect.objectContaining({ kind: 'trace', displayName: 'browser-trace.zip' })]
        },
        isError: false
      })
      expect(start).toHaveBeenCalledOnce()
      expect(stop).toHaveBeenCalledOnce()
      expect(surfaces).toHaveLength(0)
      expect(surfaceGroup.ensureActiveSurface).not.toHaveBeenCalled()
      expect(surfaceGroup.createSurface).not.toHaveBeenCalled()
      expect(upstream).not.toHaveBeenCalled()
    } finally {
      await host.close()
      await broker.shutdown()
      await rm(parent, { force: true, recursive: true })
    }
  })
})
