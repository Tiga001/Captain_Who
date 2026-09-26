import { afterEach, describe, expect, it, vi } from 'vitest'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import {
  type ManagedPlaywrightConnectionFactory,
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import { MANAGED_PLAYWRIGHT_CATALOG_LOCK } from './managedPlaywrightCatalog'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  PARENT_REQUEST_ID,
  surfaceView,
  singleSurfaceGroup,
  testSurfaceLeaseApi,
  fakeBrowserContext,
  closableBrowserContext,
  fakeHost
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('browser_close retires only automation and lazily reconnects without closing a UI tab', async () => {
    const closeSurface = vi.fn(async () => undefined)
    const detachAutomation = vi.fn(async () => undefined)
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'fresh automation generation' }],
      isError: false
    }))
    const host = fakeHost({
      callTool,
      closeSurface,
      createOfficialConnection,
      detachAutomation
    })

    await expect(
      host.callTool('browser_close', { call_reason: 'Close the managed browser page.' })
    ).resolves.toMatchObject({ isError: false })

    expect(closeSurface).not.toHaveBeenCalled()
    expect(callTool).not.toHaveBeenCalled()
    expect(createOfficialConnection).not.toHaveBeenCalled()
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Reconnect to the retained page.' })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledOnce()
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('invalidates a cancelled pending connection and closes any late generation', async () => {
    let resolveConnection:
      ((connection: Awaited<ReturnType<ManagedPlaywrightConnectionFactory>>) => void) | undefined
    const close = vi.fn(async () => undefined)
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      detachAutomation,
      createOfficialConnection: () =>
        new Promise((resolve) => {
          resolveConnection = resolve
        })
    })
    const controller = new AbortController()
    const pending = host.connect(controller.signal)
    await vi.waitFor(() => expect(resolveConnection).toBeTypeOf('function'))
    controller.abort()
    resolveConnection?.({ connect: vi.fn(async () => undefined), close })
    await expect(pending).rejects.toMatchObject({ code: 'mcp.builtin_playwright.cancelled' })
    await vi.waitFor(() => expect(close).toHaveBeenCalledOnce())
    expect(detachAutomation).toHaveBeenCalled()
  })

  it('retires a closed managed BrowserContext before the next independent tool call', async () => {
    const first = closableBrowserContext()
    const second = closableBrowserContext()
    let activeContext = first.context
    const getBrowserContext = vi.fn(async () => activeContext)
    const detachAutomation = vi.fn(async () => undefined)
    const createOfficialConnection = vi.fn(async (_config, contextGetter) => {
      await contextGetter()
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const host = fakeHost({ createOfficialConnection, detachAutomation, getBrowserContext })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the local fixture.' })
    ).resolves.toMatchObject({ isError: false })
    first.close()
    await vi.waitFor(() => expect(detachAutomation).toHaveBeenCalledOnce())
    activeContext = second.context

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the local fixture again.' })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
    expect(getBrowserContext).toHaveBeenCalledTimes(4)
  })

  it('does not replay a closed-target response and reconnects the following tool call', async () => {
    const first = closableBrowserContext()
    const second = closableBrowserContext()
    const contexts = [first.context, second.context]
    const callTool = vi
      .fn()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'Target page, context or browser has been closed' }],
        isError: true
      })
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
      })
    const createOfficialConnection = vi.fn(async (_config, contextGetter) => {
      await contextGetter()
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      getBrowserContext: async () => {
        const context = contexts.shift()
        if (!context) throw new Error('unexpected third context')
        return context
      }
    })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the closed local fixture.' })
    ).resolves.toMatchObject({ isError: true })
    expect(callTool).toHaveBeenCalledOnce()
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the replacement local fixture.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledTimes(2)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('retires a transport that closes after dispatch before the next independent call', async () => {
    const first = closableBrowserContext()
    const second = closableBrowserContext()
    let activeContext = first.context
    const callTool = vi
      .fn()
      .mockRejectedValueOnce(new Error('Transport is closed'))
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
      })
    const createOfficialConnection = vi.fn(async (_config, contextGetter) => {
      await contextGetter()
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      getBrowserContext: async () => activeContext
    })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the local closed transport.' })
    ).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
    expect(callTool).toHaveBeenCalledOnce()
    expect(detachAutomation).toHaveBeenCalledOnce()

    activeContext = second.context
    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the reconnected local fixture.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledTimes(2)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('rebuilds the official connection after the first surface attach rejects during exact selection', async () => {
    const surface = surfaceView(0, true)
    const exactLease = {
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      selectionRevision: 1,
      index: 0,
      resolveIndex: vi.fn(async () => 0),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroup([surface]),
      beginToolSurfaceLease: vi.fn(async () => exactLease),
      beginExistingToolSurfaceLease: vi.fn(async () => exactLease)
    }
    let officialCall = 0
    const callTool = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      officialCall += 1
      if (officialCall === 1) throw new Error('fixture surface attach rejected')
      if (name === 'browser_tabs' && args.action === 'select') {
        return { content: [{ type: 'text', text: 'selected' }], isError: false }
      }
      return { content: [{ type: 'text', text: 'fresh snapshot' }], isError: false }
    })
    const context = fakeBrowserContext()
    const createOfficialConnection = vi.fn(async (_config, contextGetter) => {
      await contextGetter()
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      getBrowserContext: async () => context,
      surfaceGroup
    })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Trigger the local attach fixture.' })
    ).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
    expect(callTool).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_get_config', { call_reason: 'Read local managed configuration.' })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the replacement local fixture.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledTimes(3)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
    expect(detachAutomation).toHaveBeenCalledOnce()
  })

  it('retires a rejected zero-tab target creation call before the next independent tool', async () => {
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const authority = {
      action: 'new' as const,
      claim: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const targetlessLease = {
      ready: vi.fn(async () => undefined),
      preflight: vi.fn(async () => undefined),
      beginTargetCreationAuthority: vi.fn(async () => authority),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        throw new Error('official target creation owns this fixture')
      }),
      selectSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginTargetCreationIntent: vi.fn(() => () => authority.finish())
    }
    let rejectTargetCreation = true
    const callTool = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      if (name === 'browser_tabs' && args.action === 'new' && rejectTargetCreation) {
        rejectTargetCreation = false
        throw new Error('fixture target attach rejected')
      }
      return { content: [{ type: 'text', text: 'fresh snapshot' }], isError: false }
    })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      beginTargetCreationOperation: vi.fn(async () => targetlessLease),
      callTool,
      createOfficialConnection,
      detachAutomation,
      surfaceGroup
    })

    await expect(
      host.callTool(
        'browser_tabs',
        { action: 'new', call_reason: 'Create the first local fixture target.' },
        {
          authorizationContext: {
            ...RISK_CONTEXT,
            callId: 'call-zero-tab-attach-reject',
            triggerToolName: 'browser_tabs'
          },
          parentRequestId: PARENT_REQUEST_ID
        }
      )
    ).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
    expect(
      callTool.mock.calls.filter(
        ([request]) => request.name === 'browser_tabs' && request.arguments.action === 'new'
      )
    ).toHaveLength(1)
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read a fresh local fixture context.' })
    ).resolves.toMatchObject({ isError: false })
    expect(
      callTool.mock.calls.filter(([request]) => request.name === 'browser_snapshot')
    ).toHaveLength(1)
    expect(
      callTool.mock.calls.filter(
        ([request]) => request.name === 'browser_tabs' && request.arguments.action === 'new'
      )
    ).toHaveLength(1)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('invalidates a lazily unbound connection when its MCP transport closes', async () => {
    const clients: ManagedMcpClient[] = []
    const createClient = vi.fn(() => {
      const client: ManagedMcpClient = {
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined),
        listTools: vi.fn(async () => ({
          tools: MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
            name: tool.name,
            description: tool.description,
            inputSchema: structuredClone(tool.inputSchema),
            annotations: structuredClone(tool.annotations ?? {})
          }))
        })),
        callTool: vi.fn(async () => ({
          content: [{ type: 'text', text: 'fresh snapshot' }],
          isError: false
        }))
      }
      clients.push(client)
      return client
    })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({ createClient, createOfficialConnection, detachAutomation })

    await expect(host.listTools()).resolves.toHaveLength(61)
    expect(clients).toHaveLength(1)
    clients[0]?.onclose?.()
    await vi.waitFor(() => expect(detachAutomation).toHaveBeenCalledOnce())

    await expect(host.listTools()).resolves.toHaveLength(61)
    expect(clients).toHaveLength(2)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('waits for a delayed stale-generation detach before creating the replacement connection', async () => {
    const order: string[] = []
    let releaseFirstDetach!: () => void
    let signalFirstDetachStarted!: () => void
    const firstDetachStarted = new Promise<void>((resolve) => {
      signalFirstDetachStarted = resolve
    })
    const firstDetachBarrier = new Promise<void>((resolve) => {
      releaseFirstDetach = resolve
    })
    const detachAutomation = vi.fn(async () => {
      const invocation = detachAutomation.mock.calls.length
      order.push(`detach:${invocation}:start`)
      if (invocation === 1) {
        signalFirstDetachStarted()
        await firstDetachBarrier
      }
      order.push(`detach:${invocation}:end`)
    })
    const clients: ManagedMcpClient[] = []
    const createClient = vi.fn(() => {
      const client: ManagedMcpClient = {
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined),
        listTools: vi.fn(async () => ({
          tools: MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
            name: tool.name,
            description: tool.description,
            inputSchema: structuredClone(tool.inputSchema),
            annotations: structuredClone(tool.annotations ?? {})
          }))
        })),
        callTool: vi.fn(async () => ({ content: [{ type: 'text', text: 'ok' }], isError: false }))
      }
      clients.push(client)
      return client
    })
    const createOfficialConnection = vi.fn(async () => {
      order.push(`create:${createOfficialConnection.mock.calls.length}`)
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const host = fakeHost({ createClient, createOfficialConnection, detachAutomation })

    await expect(host.listTools()).resolves.toHaveLength(61)
    clients[0]?.onclose?.()
    await firstDetachStarted

    const replacement = host.listTools()
    await new Promise<void>((resolve) => setImmediate(resolve))
    expect(createOfficialConnection).toHaveBeenCalledOnce()

    releaseFirstDetach()
    await expect(replacement).resolves.toHaveLength(61)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
    expect(order.indexOf('detach:1:end')).toBeLessThan(order.indexOf('create:2'))
  })

  it('coalesces a transport-close and caller-abort race into one generation retirement', async () => {
    let signalOfficialCallStarted!: () => void
    let resolveStaleCall!: (result: unknown) => void
    const officialCallStarted = new Promise<void>((resolve) => {
      signalOfficialCallStarted = resolve
    })
    const staleCall = new Promise<unknown>((resolve) => {
      resolveStaleCall = resolve
    })
    const clients: ManagedMcpClient[] = []
    const createClient = vi.fn(() => {
      const generation = clients.length
      const client: ManagedMcpClient = {
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined),
        listTools: vi.fn(async () => ({
          tools: MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
            name: tool.name,
            description: tool.description,
            inputSchema: structuredClone(tool.inputSchema),
            annotations: structuredClone(tool.annotations ?? {})
          }))
        })),
        callTool: vi.fn(async () => {
          if (generation === 0) {
            signalOfficialCallStarted()
            return await staleCall
          }
          return { content: [{ type: 'text', text: 'fresh snapshot' }], isError: false }
        })
      }
      clients.push(client)
      return client
    })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({ createClient, createOfficialConnection, detachAutomation })
    const controller = new AbortController()

    const pending = host.callTool(
      'browser_snapshot',
      { call_reason: 'Exercise a local close and abort race.' },
      { signal: controller.signal }
    )
    await officialCallStarted
    controller.abort()
    clients[0]?.onclose?.()

    await expect(pending).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
    expect(detachAutomation).toHaveBeenCalledOnce()
    expect(clients[0]?.close).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the replacement local fixture.' })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
    expect(detachAutomation).toHaveBeenCalledOnce()

    resolveStaleCall({ content: [{ type: 'text', text: 'stale result' }], isError: false })
    await new Promise<void>((resolve) => setImmediate(resolve))
    expect(detachAutomation).toHaveBeenCalledOnce()
  })

  it('retires a resolved terminal surface-selection error without replaying the current call', async () => {
    const surface = surfaceView(0, true)
    const exactLease = {
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      selectionRevision: 1,
      index: 0,
      resolveIndex: vi.fn(async () => 0),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroup([surface]),
      beginToolSurfaceLease: vi.fn(async () => exactLease),
      beginExistingToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'Target page, context or browser has been closed' }],
        isError: true
      })
      .mockResolvedValueOnce({ content: [{ type: 'text', text: 'selected' }], isError: false })
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
      })
    const context = fakeBrowserContext()
    const createOfficialConnection = vi.fn(async (_config, contextGetter) => {
      await contextGetter()
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      getBrowserContext: async () => context,
      surfaceGroup
    })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Exercise a terminal select response.' })
    ).rejects.toMatchObject({
      code: 'browser.target_closed',
      dispatchCertainty: 'response_received'
    })
    expect(callTool).toHaveBeenCalledOnce()
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read the replacement local fixture.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledTimes(3)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('preserves a resolved terminal zero-tab result and reconnects only the next call', async () => {
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const authority = {
      action: 'new' as const,
      claim: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const targetlessLease = {
      ready: vi.fn(async () => undefined),
      preflight: vi.fn(async () => undefined),
      beginTargetCreationAuthority: vi.fn(async () => authority),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        throw new Error('official target creation owns this fixture')
      }),
      selectSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginTargetCreationIntent: vi.fn(() => () => authority.finish())
    }
    let terminalTargetCreation = true
    const callTool = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      if (name === 'browser_tabs' && args.action === 'new' && terminalTargetCreation) {
        terminalTargetCreation = false
        return {
          content: [{ type: 'text', text: 'Target page, context or browser has been closed' }],
          structuredContent: { errorCode: 'target_closed' },
          isError: true
        }
      }
      return { content: [{ type: 'text', text: 'fresh snapshot' }], isError: false }
    })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      beginTargetCreationOperation: vi.fn(async () => targetlessLease),
      callTool,
      createOfficialConnection,
      detachAutomation,
      surfaceGroup
    })

    await expect(
      host.callTool(
        'browser_tabs',
        { action: 'new', call_reason: 'Exercise a terminal first-target response.' },
        {
          authorizationContext: {
            ...RISK_CONTEXT,
            callId: 'call-zero-tab-terminal-result',
            triggerToolName: 'browser_tabs'
          },
          parentRequestId: PARENT_REQUEST_ID
        }
      )
    ).resolves.toMatchObject({
      structuredContent: { errorCode: 'target_closed' },
      isError: true
    })
    expect(
      callTool.mock.calls.filter(
        ([request]) => request.name === 'browser_tabs' && request.arguments.action === 'new'
      )
    ).toHaveLength(1)
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read a fresh local fixture context.' })
    ).resolves.toMatchObject({ isError: false })
    expect(
      callTool.mock.calls.filter(([request]) => request.name === 'browser_snapshot')
    ).toHaveLength(1)
    expect(
      callTool.mock.calls.filter(
        ([request]) => request.name === 'browser_tabs' && request.arguments.action === 'new'
      )
    ).toHaveLength(1)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('keeps an authoritative isError result on the reusable official connection', async () => {
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fixture tool-level failure' }],
        structuredContent: { status: 'failed', errorCode: 'fixture_failure' },
        isError: true
      })
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh authoritative response' }],
        isError: false
      })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({ callTool, createOfficialConnection, detachAutomation })

    await expect(
      host.callTool('browser_network_requests', {
        static: false,
        call_reason: 'Exercise a local authoritative tool error.'
      })
    ).resolves.toMatchObject({ isError: true })
    await expect(
      host.callTool('browser_network_requests', {
        static: false,
        call_reason: 'Exercise a following local response.'
      })
    ).resolves.toMatchObject({ isError: false })

    expect(createOfficialConnection).toHaveBeenCalledOnce()
    expect(detachAutomation).not.toHaveBeenCalled()
  })
})
