import { afterEach, describe, expect, it, vi } from 'vitest'
import type { BrowserContext } from 'playwright'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import {
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  PARENT_REQUEST_ID,
  surfaceView,
  singleSurfaceGroup,
  testSurfaceLeaseApi,
  sensitiveTargetFromSurfaces,
  fakeBrowserContext,
  fakeHost
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('routes browser_resize through the exact current surface lease', async () => {
    const surface = surfaceView(0, true)
    const resizeSurface = vi.fn(async (input: { height: number; width: number }) => input)
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroup([surface]),
      beginToolSurfaceLease: vi.fn(async () => ({
        closeSurface: vi.fn(async () => undefined),
        finish: vi.fn(),
        generation: surface.generation,
        index: surface.index,
        resolveIndex: vi.fn(async () => surface.index),
        resizeSurface,
        selectionRevision: 1,
        surfaceId: surface.surfaceId
      }))
    }
    const host = fakeHost({ surfaceGroup })

    await expect(
      host.callTool('browser_resize', {
        width: 960,
        height: 640,
        call_reason: 'Resize the exact managed surface.'
      })
    ).resolves.toMatchObject({ structuredContent: { width: 960, height: 640 } })
    expect(resizeSurface).toHaveBeenCalledWith({ width: 960, height: 640 })
  })

  it('lets fixed official browser_tabs create exactly one first tab under targetless run authority', async () => {
    const order: string[] = []
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const authority = {
      action: 'new' as const,
      claim: vi.fn<(binding: unknown) => Promise<void>>(async () => {
        order.push('authority-claimed')
      }),
      finish: vi.fn(() => {
        order.push('authority-finished')
      })
    }
    const preflight = vi.fn(async () => {
      order.push('preflight')
    })
    const beginTargetCreationAuthority = vi.fn(async () => authority)
    const targetlessLease = {
      ready: vi.fn(async () => {
        order.push('download-ready')
      }),
      preflight,
      beginTargetCreationAuthority,
      markDispatched: vi.fn(() => {
        order.push('risk-dispatched')
      }),
      settle: vi.fn(async () => {
        order.push('risk-settled')
      }),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn(() => {
        order.push('risk-finished')
      })
    } as unknown as BrowserNetworkOperationLease
    const finishIntent = vi.fn(() => authority.finish())
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => surfaces[0]),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        throw new Error('generic create must not run')
      }),
      selectSurface: vi.fn(async () => surfaces[0]),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginTargetCreationIntent: vi.fn((_intent, receivedAuthority) => {
        expect(receivedAuthority).toBe(authority)
        order.push('intent-installed')
        return finishIntent
      }),
      createInitialTargetSurface: vi.fn(async () => {
        throw new Error('Host must not precreate or manually navigate the first tab')
      })
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      order.push(`upstream:${name}:${String(args.action ?? '')}`)
      if (name === 'browser_tabs' && args.action === 'new') {
        const created = surfaceView(0, true)
        surfaces.push(created)
        await authority.claim({} as never)
      }
      return {
        content: [{ type: 'text', text: '- 0: Fixture (current)' }],
        isError: false
      }
    })
    const beginTargetCreationOperation = vi.fn(async () => targetlessLease)
    const host = fakeHost({
      beginTargetCreationOperation,
      callTool: upstream,
      surfaceGroup
    })
    const authorizationContext = {
      ...RISK_CONTEXT,
      callId: 'call-tabs-new-first',
      triggerToolName: 'browser_tabs'
    }

    await expect(
      host.callTool(
        'browser_tabs',
        {
          action: 'new',
          url: 'http://127.0.0.1/fixture-first-tab',
          call_reason: 'Open the first local fixture tab.'
        },
        { authorizationContext, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(surfaces).toHaveLength(1)
    expect(upstream.mock.calls.map(([request]) => request.name)).toEqual(['browser_tabs'])
    expect(upstream.mock.calls[0]?.[0]).toEqual({
      name: 'browser_tabs',
      arguments: { action: 'new', url: 'http://127.0.0.1/fixture-first-tab' }
    })
    expect(surfaceGroup.createInitialTargetSurface).not.toHaveBeenCalled()
    expect(beginTargetCreationAuthority).toHaveBeenCalledWith({
      action: 'new',
      runId: authorizationContext.runId,
      activationId: authorizationContext.activationId,
      capabilityId: 'browser_automation',
      toolCallId: authorizationContext.callId,
      toolId: 'browser_tabs',
      url: 'http://127.0.0.1/fixture-first-tab'
    })
    expect(preflight).not.toHaveBeenCalled()
    expect(order.indexOf('risk-dispatched')).toBeLessThan(
      order.indexOf('upstream:browser_tabs:new')
    )
    expect(order.indexOf('intent-installed')).toBeLessThan(
      order.indexOf('upstream:browser_tabs:new')
    )
    expect(targetlessLease.settle).toHaveBeenCalledOnce()
    expect(finishIntent).toHaveBeenCalledOnce()
  })

  it('lets first browser_navigate create its page under targetless run authority', async () => {
    const url = 'http://127.0.0.1/fixture-first-navigation'
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const authority = {
      action: 'new' as const,
      claim: vi.fn<(binding: unknown) => Promise<void>>(async () => undefined),
      finish: vi.fn()
    }
    const targetlessLease = {
      ready: vi.fn(async () => undefined),
      beginTargetCreationAuthority: vi.fn(async () => authority),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const beginToolSurfaceLease = vi.fn(async () => {
      throw new Error('browser_navigate must not precreate a blank Surface')
    })
    const beginExistingToolSurfaceLease = vi.fn(async () => null)
    const finishIntent = vi.fn(() => authority.finish())
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => {
        throw new Error('the context must remain empty until fixed Playwright creates its Page')
      }),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        throw new Error('generic Surface creation must not run')
      }),
      selectSurface: vi.fn(async () => {
        throw new Error('there is no Surface to select before navigation')
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease,
      beginExistingToolSurfaceLease,
      beginTargetCreationIntent: vi.fn((_intent, receivedAuthority) => {
        expect(receivedAuthority).toBe(authority)
        return finishIntent
      })
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      expect(name).toBe('browser_navigate')
      expect(args).toEqual({ url })
      surfaces.push({ ...surfaceView(0, true), url })
      await authority.claim({} as never)
      return {
        content: [{ type: 'text', text: `Navigated to ${url}` }],
        isError: false
      }
    })
    const beginTargetCreationOperation = vi.fn(async () => targetlessLease)
    const host = fakeHost({
      beginTargetCreationOperation,
      callTool: upstream,
      surfaceGroup
    })
    const authorizationContext = {
      ...RISK_CONTEXT,
      callId: 'call-navigate-first',
      triggerToolName: 'browser_navigate'
    }

    await expect(
      host.callTool(
        'browser_navigate',
        { url, call_reason: 'Open the first local fixture page.' },
        { authorizationContext, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(beginExistingToolSurfaceLease).toHaveBeenCalledOnce()
    expect(beginToolSurfaceLease).not.toHaveBeenCalled()
    expect(surfaces).toHaveLength(1)
    expect(upstream).toHaveBeenCalledOnce()
    expect(targetlessLease.beginTargetCreationAuthority).toHaveBeenCalledWith({
      action: 'new',
      runId: authorizationContext.runId,
      activationId: authorizationContext.activationId,
      capabilityId: 'browser_automation',
      toolCallId: authorizationContext.callId,
      toolId: 'browser_navigate',
      url
    })
    expect(authority.claim).toHaveBeenCalledOnce()
    expect(targetlessLease.markDispatched).toHaveBeenCalledOnce()
    expect(targetlessLease.settle).toHaveBeenCalledOnce()
    expect(finishIntent).toHaveBeenCalledOnce()
  })

  it('rebinds a retained page before browser_navigate when trusted selection is temporarily empty', async () => {
    const url = 'http://127.0.0.1/fixture-retained-navigation'
    const retained = { ...surfaceView(0, false), url: 'http://127.0.0.1/retained' }
    const exactLease = {
      surfaceId: retained.surfaceId,
      generation: retained.generation,
      selectionRevision: 2,
      index: 0,
      resolveIndex: vi.fn(async () => 0),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const beginExistingToolSurfaceLease = vi.fn(async () => null)
    const beginToolSurfaceLease = vi.fn(async () => {
      throw new Error('retained navigation must not call ensureSurface')
    })
    const beginToolSurfaceLeaseByIndex = vi.fn(async () => exactLease)
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => retained),
      listSurfaces: () => [retained],
      createSurface: vi.fn(async () => retained),
      selectSurface: vi.fn(async () => retained),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease,
      beginToolSurfaceLeaseByIndex,
      beginExistingToolSurfaceLease,
      beginTargetCreationIntent: vi.fn(() => {
        throw new Error('retained navigation must not use zero-page target creation')
      })
    }
    const risk = {
      preflight: vi.fn(async () => undefined),
      markDispatched: vi.fn(),
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const beginNetworkOperation = vi.fn(async () => risk)
    const beginTargetCreationOperation = vi.fn(async () => {
      throw new Error('retained navigation must use the exact guest risk lease')
    })
    const context = fakeBrowserContext()
    let contextGetter: (() => Promise<BrowserContext>) | undefined
    const createOfficialConnection = vi.fn(async (_config, getter) => {
      contextGetter = getter
      return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
    })
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
      if (name === 'browser_tabs' && args.action === 'list') await contextGetter!()
      return {
        content: [{ type: 'text', text: `Navigated to ${url}` }],
        isError: false
      }
    })
    const host = fakeHost({
      beginNetworkOperation,
      beginTargetCreationOperation,
      callTool: upstream,
      createOfficialConnection,
      getBrowserContext: async () => context,
      surfaceGroup
    })
    const authorizationContext = {
      ...RISK_CONTEXT,
      callId: 'call-navigate-retained',
      triggerToolName: 'browser_navigate'
    }

    await expect(
      host.callTool(
        'browser_navigate',
        { url, call_reason: 'Navigate the retained visible fixture page.' },
        { authorizationContext, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(beginExistingToolSurfaceLease).toHaveBeenCalledOnce()
    expect(beginToolSurfaceLease).not.toHaveBeenCalled()
    expect(beginToolSurfaceLeaseByIndex).toHaveBeenCalledWith(
      0,
      expect.objectContaining({
        owner: expect.objectContaining({ runId: RISK_CONTEXT.runId }),
        contextOnly: false
      })
    )
    expect(beginTargetCreationOperation).not.toHaveBeenCalled()
    expect(beginNetworkOperation).toHaveBeenCalledOnce()
    expect(risk.preflight).toHaveBeenCalledWith(url)
    expect(upstream.mock.calls.map(([request]) => request)).toEqual([
      { name: 'browser_tabs', arguments: { action: 'list' } },
      { name: 'browser_tabs', arguments: { action: 'select', index: 0 } },
      { name: 'browser_navigate', arguments: { url } }
    ])
    expect(exactLease.finish).toHaveBeenCalledOnce()
  })

  it.each(['select', 'close'] as const)(
    'hydrates a fresh official retained-page context before browser_tabs %s',
    async (action) => {
      const retained = surfaceView(0, true)
      const exactLease = {
        surfaceId: retained.surfaceId,
        generation: retained.generation,
        selectionRevision: 1,
        index: 0,
        resolveIndex: vi.fn(async () => 0),
        closeSurface: vi.fn(async () => undefined),
        finish: vi.fn()
      }
      const createSurface = vi.fn(async () => {
        throw new Error('retained tab action must not create a Surface')
      })
      const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
        ...singleSurfaceGroup([retained]),
        createSurface,
        beginToolSurfaceLeaseByIndex: vi.fn(async () => exactLease)
      }
      const context = fakeBrowserContext()
      let contextGetter: (() => Promise<BrowserContext>) | undefined
      const createOfficialConnection = vi.fn(async (_config, getter) => {
        contextGetter = getter
        return { connect: vi.fn(async () => undefined), close: vi.fn(async () => undefined) }
      })
      const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ name, arguments: args }) => {
        if (name === 'browser_tabs' && args.action === 'list') await contextGetter!()
        return { content: [{ type: 'text', text: String(args.action) }], isError: false }
      })
      const host = fakeHost({
        callTool: upstream,
        createOfficialConnection,
        getBrowserContext: async () => context,
        surfaceGroup
      })

      await expect(
        host.callTool('browser_tabs', {
          action,
          index: 0,
          call_reason: `${action} the retained local fixture tab.`
        })
      ).resolves.toMatchObject({ isError: false })

      expect(upstream.mock.calls.map(([request]) => request)).toEqual([
        { name: 'browser_tabs', arguments: { action: 'list' } },
        { name: 'browser_tabs', arguments: { action, index: 0 } }
      ])
      expect(createSurface).not.toHaveBeenCalled()
      expect(exactLease.finish).toHaveBeenCalledOnce()
    }
  )

  it('maps a resolved retained-context hydration error to response_received', async () => {
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
      beginToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => ({
      content: [{ type: 'text', text: 'Tab 0 not found' }],
      isError: true
    }))
    const phases: string[] = []
    const host = fakeHost({ callTool: upstream, surfaceGroup })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Hydrate the retained fixture page.' },
        {
          onDispatchPhase: vi.fn(async (phase) => {
            phases.push(phase)
            return true
          })
        }
      )
    ).rejects.toMatchObject({
      code: 'browser.target_closed',
      dispatchCertainty: 'response_received'
    })
    expect(phases).toEqual(['possibly_dispatched', 'response_received'])
    expect(upstream).toHaveBeenCalledOnce()
  })

  it('keeps a rejected retained-context hydration call possibly dispatched', async () => {
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
      beginToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const detachAutomation = vi.fn(async () => undefined)
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => {
      throw new Error('fixture hydration transport rejected')
    })
    const phases: string[] = []
    const host = fakeHost({ callTool: upstream, detachAutomation, surfaceGroup })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Exercise rejected retained hydration.' },
        {
          onDispatchPhase: vi.fn(async (phase) => {
            phases.push(phase)
            return true
          })
        }
      )
    ).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
    expect(phases).toEqual(['possibly_dispatched'])
    expect(detachAutomation).toHaveBeenCalledOnce()
  })

  it('fails closed without creating or guessing when retained pages have no trusted selection', async () => {
    const retained = [surfaceView(0, false), surfaceView(1, false)]
    const beginToolSurfaceLeaseByIndex = vi.fn(async () => {
      throw new Error('ambiguous retained pages must not be leased by guess')
    })
    const beginTargetCreationIntent = vi.fn(() => {
      throw new Error('ambiguous retained pages are not a zero-page context')
    })
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...singleSurfaceGroup(retained),
      beginExistingToolSurfaceLease: vi.fn(async () => null),
      beginToolSurfaceLeaseByIndex,
      beginTargetCreationIntent
    }
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const host = fakeHost({ createOfficialConnection, surfaceGroup })

    await expect(
      host.callTool('browser_navigate', {
        url: 'http://127.0.0.1/ambiguous-retained',
        call_reason: 'Do not guess among retained local pages.'
      })
    ).rejects.toMatchObject({
      code: 'browser.surface_unavailable',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(beginToolSurfaceLeaseByIndex).not.toHaveBeenCalled()
    expect(beginTargetCreationIntent).not.toHaveBeenCalled()
    expect(createOfficialConnection).not.toHaveBeenCalled()
  })

  it('creates one authorized blank target when the last tab closes during list lease admission', async () => {
    const surfaces: ReturnType<typeof surfaceView>[] = [surfaceView(0, true)]
    const claimAuthority = vi.fn<(binding: unknown) => Promise<void>>(async () => undefined)
    const authority = {
      action: 'new' as const,
      claim: claimAuthority,
      finish: vi.fn()
    }
    const markTargetCreationDispatched = vi.fn()
    const targetlessLease = {
      ready: vi.fn(async () => undefined),
      preflight: vi.fn(async () => undefined),
      beginTargetCreationAuthority: vi.fn(async () => authority),
      markDispatched: markTargetCreationDispatched,
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => surfaces[0]),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        throw new Error('unowned generic creation must not run')
      }),
      selectSurface: vi.fn(async () => surfaces[0]),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginExistingToolSurfaceLease: vi.fn(async () => {
        surfaces.splice(0)
        return null
      }),
      beginTargetCreationIntent: vi.fn((_intent, receivedAuthority) => {
        expect(receivedAuthority).toBe(authority)
        return () => authority.finish()
      }),
      createInitialTargetSurface: vi.fn(async () => {
        throw new Error('Host must let fixed official list create the initial target')
      })
    }
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => {
      const created = surfaceView(0, true)
      surfaces.push(created)
      await authority.claim({} as never)
      return {
        content: [{ type: 'text', text: '- 0: about:blank (current)' }],
        isError: false
      }
    })
    const host = fakeHost({
      beginTargetCreationOperation: vi.fn(async () => targetlessLease),
      callTool: upstream,
      surfaceGroup
    })
    const authorizationContext = {
      ...RISK_CONTEXT,
      callId: 'call-tabs-list-empty',
      triggerToolName: 'browser_tabs'
    }

    await expect(
      host.callTool(
        'browser_tabs',
        {
          action: 'list',
          url: 'http://127.0.0.1/must-not-navigate',
          call_reason: 'List the managed tabs.'
        },
        { authorizationContext, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(surfaces).toHaveLength(1)
    expect(surfaceGroup.beginExistingToolSurfaceLease).toHaveBeenCalledOnce()
    expect(surfaceGroup.createInitialTargetSurface).not.toHaveBeenCalled()
    expect(upstream).toHaveBeenCalledOnce()
    expect(upstream).toHaveBeenCalledWith(
      {
        name: 'browser_tabs',
        arguments: { action: 'list', url: 'http://127.0.0.1/must-not-navigate' }
      },
      undefined,
      expect.any(Object)
    )
    expect(targetlessLease.beginTargetCreationAuthority).toHaveBeenCalledWith({
      action: 'new',
      runId: authorizationContext.runId,
      activationId: authorizationContext.activationId,
      capabilityId: 'browser_automation',
      toolCallId: authorizationContext.callId,
      toolId: 'browser_tabs',
      url: 'about:blank'
    })
    expect(markTargetCreationDispatched.mock.invocationCallOrder[0]).toBeLessThan(
      claimAuthority.mock.invocationCallOrder[0]
    )
  })

  it('keeps zero-tab target creation definitely undispatched when the context handshake fails', async () => {
    const authority = {
      action: 'new' as const,
      claim: vi.fn<(binding: unknown) => Promise<void>>(async () => undefined),
      finish: vi.fn()
    }
    const markTargetCreationDispatched = vi.fn()
    const targetlessLease = {
      ready: vi.fn(async () => undefined),
      beginTargetCreationAuthority: vi.fn(async () => authority),
      markDispatched: markTargetCreationDispatched,
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => []),
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => {
        throw new Error('must remain empty')
      }),
      listSurfaces: () => [],
      createSurface: vi.fn(async () => {
        throw new Error('must remain empty')
      }),
      selectSurface: vi.fn(async () => {
        throw new Error('must remain empty')
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginTargetCreationIntent: vi.fn((_intent, receivedAuthority) => {
        expect(receivedAuthority).toBe(authority)
        return () => authority.finish()
      })
    }
    const host = fakeHost({
      beginTargetCreationOperation: vi.fn(async () => targetlessLease),
      createOfficialConnection: vi.fn(async () => {
        throw new Error('fixture context handshake failed')
      }),
      surfaceGroup
    })
    const onDispatchPhase = vi.fn(async () => true)

    await expect(
      host.callTool(
        'browser_tabs',
        {
          action: 'new',
          url: 'http://127.0.0.1/fixture-never-dispatched',
          call_reason: 'Exercise a failed empty-context handshake.'
        },
        {
          authorizationContext: {
            ...RISK_CONTEXT,
            callId: 'call-tabs-connect-failure',
            triggerToolName: 'browser_tabs'
          },
          parentRequestId: PARENT_REQUEST_ID,
          onDispatchPhase
        }
      )
    ).rejects.toMatchObject({ dispatchCertainty: 'definitely_not_dispatched' })

    expect(markTargetCreationDispatched).not.toHaveBeenCalled()
    expect(onDispatchPhase).not.toHaveBeenCalled()
    expect(authority.claim).not.toHaveBeenCalled()
    expect(authority.finish).toHaveBeenCalledOnce()
  })

  it.each([
    ['surface generation drift', 'browser.target_closed'],
    ['surface group drift', 'browser.surface_unavailable']
  ] as const)(
    'keeps %s definitely undispatched when the exact lease index rejects',
    async (_case, code) => {
      const surface = surfaceView(0, true)
      const exactLease = {
        surfaceId: surface.surfaceId,
        generation: surface.generation,
        selectionRevision: 1,
        index: 0,
        resolveIndex: vi.fn(async () => {
          throw Object.assign(new Error(code), { code })
        }),
        closeSurface: vi.fn(async () => undefined),
        finish: vi.fn()
      }
      const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
        ...singleSurfaceGroup([surface]),
        beginToolSurfaceLease: vi.fn(async () => exactLease)
      }
      const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => ({
        content: [{ type: 'text', text: 'must not run' }],
        isError: false
      }))
      const onDispatchPhase = vi.fn(async () => true)
      const host = fakeHost({ callTool: upstream, surfaceGroup })

      await expect(
        host.callTool(
          'browser_snapshot',
          { call_reason: 'Exercise an exact local surface lease drift.' },
          { onDispatchPhase }
        )
      ).rejects.toMatchObject({ code, dispatchCertainty: 'definitely_not_dispatched' })

      expect(exactLease.resolveIndex).toHaveBeenCalledOnce()
      expect(onDispatchPhase).not.toHaveBeenCalled()
      expect(upstream).not.toHaveBeenCalled()
      expect(exactLease.finish).toHaveBeenCalledOnce()
    }
  )

  it('freezes browser_tabs close by exact generation and treats that target close as expected', async () => {
    const order: string[] = []
    const surface = surfaceView(1, false)
    const exactLease = {
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      selectionRevision: 4,
      index: 1,
      resolveIndex: vi.fn(async () => 1),
      closeSurface: vi.fn(async () => undefined),
      finish: vi.fn()
    }
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => [surfaceView(0, true), surface]),
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => surface),
      listSurfaces: () => [surfaceView(0, true), surface],
      createSurface: vi.fn(async () => surface),
      selectSurface: vi.fn(async () => surface),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease: vi.fn(async () => exactLease),
      beginToolSurfaceLeaseByIndex: vi.fn(async () => exactLease)
    }
    const expectedClose = vi.fn(() => {
      order.push('expect-close')
    })
    const risk = {
      ready: vi.fn(async () => undefined),
      preflight: vi.fn(async () => undefined),
      expectTargetClose: expectedClose,
      markDispatched: vi.fn(() => {
        order.push('risk-dispatched')
      }),
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ arguments: args }) => {
      order.push(`upstream-${String(args.action)}`)
      return { content: [{ type: 'text', text: '- 0: Fixture (current)' }], isError: false }
    })
    const host = fakeHost({
      beginNetworkOperation: vi.fn(async () => risk),
      callTool: upstream,
      surfaceGroup
    })

    await expect(
      host.callTool(
        'browser_tabs',
        { action: 'close', index: 1, call_reason: 'Close the exact background fixture tab.' },
        {
          authorizationContext: {
            ...RISK_CONTEXT,
            callId: 'call-tabs-close-exact',
            triggerToolName: 'browser_tabs'
          },
          parentRequestId: PARENT_REQUEST_ID
        }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(expectedClose).toHaveBeenCalledWith({
      surfaceId: surface.surfaceId,
      generation: surface.generation
    })
    expect(order.indexOf('expect-close')).toBeLessThan(order.indexOf('risk-dispatched'))
    expect(order.indexOf('risk-dispatched')).toBeLessThan(order.indexOf('upstream-list'))
    expect(order.indexOf('upstream-list')).toBeLessThan(order.indexOf('upstream-close'))
    expect(upstream).toHaveBeenCalledWith(
      { name: 'browser_tabs', arguments: { action: 'close', index: 1 } },
      undefined,
      expect.any(Object)
    )
    expect(exactLease.finish).toHaveBeenCalledOnce()
  })

  it('synchronizes an existing browser_tabs list only after its exact focus risk lease is active', async () => {
    const order: string[] = []
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
      ...testSurfaceLeaseApi(() => [surface]),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces([surface]),
      ensureActiveSurface: vi.fn(async () => surface),
      listSurfaces: () => [surface],
      createSurface: vi.fn(async () => surface),
      selectSurface: vi.fn(async () => surface),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease: vi.fn(async () => exactLease),
      beginExistingToolSurfaceLease: vi.fn(async () => exactLease)
    }
    const risk = {
      ready: vi.fn(async () => undefined),
      preflight: vi.fn(async () => undefined),
      markDispatched: vi.fn(() => order.push('risk-dispatched')),
      settle: vi.fn(async () => undefined),
      failure: () => null,
      downloads: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async ({ arguments: args }) => {
      order.push(`upstream:${String(args.action)}`)
      return { content: [{ type: 'text', text: '- 0: Fixture (current)' }], isError: false }
    })
    const host = fakeHost({
      beginNetworkOperation: vi.fn(async () => risk),
      callTool: upstream,
      surfaceGroup
    })

    await expect(
      host.callTool(
        'browser_tabs',
        { action: 'list', call_reason: 'List the current managed fixture tabs.' },
        {
          authorizationContext: {
            ...RISK_CONTEXT,
            callId: 'call-tabs-list-existing',
            triggerToolName: 'browser_tabs'
          },
          parentRequestId: PARENT_REQUEST_ID
        }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(order).toEqual(['risk-dispatched', 'upstream:list', 'upstream:select', 'upstream:list'])
    expect(risk.markDispatched).toHaveBeenCalledOnce()
    expect(exactLease.finish).toHaveBeenCalledOnce()
  })
})
