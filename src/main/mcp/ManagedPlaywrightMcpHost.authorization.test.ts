import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import { canonicalSha256 } from './managedPlaywrightSensitivePolicy'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const {
  closeTrackedHosts,
  RISK_CONTEXT,
  surfaceView,
  singleSurfaceGroup,
  testSurfaceLeaseApi,
  sensitiveContext,
  sensitiveTargetFromSurfaces,
  fakeHost
} = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('rejects missing, expired, or origin-drifted sensitive grants before starting MCP or creating a tab', async () => {
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const surfaces = [surfaceView(0, true)]
    const surfaceGroup = singleSurfaceGroup(surfaces)
    const host = fakeHost({ createOfficialConnection, surfaceGroup })
    const args = {
      function: '() => document.title',
      call_reason: 'Read the fixture title.'
    }

    await expect(
      host.callTool('browser_evaluate', args, {
        authorizationContext: { ...RISK_CONTEXT, grantExpiresAtMs: Date.now() + 120_000 }
      })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_missing',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    const expired = sensitiveContext('browser_evaluate', args)
    expired.builtinToolGrant = {
      ...expired.builtinToolGrant!,
      expiresAtMs: Date.now() - 1
    }
    await expect(
      host.callTool('browser_evaluate', args, { authorizationContext: expired })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_expired',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    surfaces.splice(0)
    await expect(
      host.callTool('browser_evaluate', args, {
        authorizationContext: sensitiveContext('browser_evaluate', args)
      })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_origin_drifted',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    expect(createOfficialConnection).not.toHaveBeenCalled()
    expect(surfaceGroup.ensureActiveSurface).not.toHaveBeenCalled()
    expect(surfaceGroup.createSurface).not.toHaveBeenCalled()
    expect(surfaces).toHaveLength(0)
  })

  it('uses only the Main-frozen origin authority for a sensitive page call', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'host-origin-authoritative' }],
      isError: false
    }))
    const host = fakeHost({ callTool })
    const argumentsValue = {
      function: '() => document.title',
      call_reason: 'Exercise the managed fixture origin binding.'
    }
    const authorizationContext = sensitiveContext('browser_evaluate', argumentsValue)
    const grant = authorizationContext.builtinToolGrant!
    authorizationContext.builtinToolGrant = {
      ...grant,
      origin: 'http://127.0.0.1',
      resourceScopeDigest: canonicalSha256({
        schemaVersion: 2,
        toolId: 'browser_evaluate',
        argumentsDigest: grant.argumentsDigest,
        origin: 'http://127.0.0.1',
        riskKinds: [...grant.riskKinds],
        scope: 'managed_surface',
        targetBindingDigest: grant.targetBindingDigest
      })
    }

    await expect(
      host.callTool('browser_evaluate', argumentsValue, { authorizationContext })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('dispatches a profile-scoped cookie read with zero managed tabs and no page lease', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'No cookies found' }],
      isError: false
    }))
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const surfaceGroup = singleSurfaceGroup(surfaces)
    const host = fakeHost({ callTool, surfaceGroup })
    const args = {
      call_reason: 'List cookies in the isolated managed browser profile.'
    }

    await expect(
      host.callTool('browser_cookie_list', args, {
        authorizationContext: sensitiveContext('browser_cookie_list', args)
      })
    ).resolves.toMatchObject({ isError: false })
    expect(surfaceGroup.ensureActiveSurface).not.toHaveBeenCalled()
    expect(surfaceGroup.createSurface).not.toHaveBeenCalled()
    expect(callTool).toHaveBeenCalledWith(
      { name: 'browser_cookie_list', arguments: {} },
      undefined,
      expect.objectContaining({ resetTimeoutOnProgress: false })
    )
  })

  it('freezes the current page when cookie_set omits domain and fails closed after tab drift', async () => {
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const surfaces = [surfaceView(0, true)]
    const surfaceGroup = singleSurfaceGroup(surfaces)
    const args = {
      name: 'fixture-cookie',
      value: 'fixture-value',
      call_reason: 'Set a cookie using the current managed fixture host.'
    }
    const authorizationContext = sensitiveContext('browser_cookie_set', args)
    surfaces[0].isActive = false
    surfaces.push({ ...surfaceView(1, true), url: 'http://localhost/other-tab' })
    const host = fakeHost({ callTool, createOfficialConnection, surfaceGroup })

    await expect(
      host.callTool('browser_cookie_set', args, { authorizationContext })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_origin_drifted',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(createOfficialConnection).not.toHaveBeenCalled()
    expect(callTool).not.toHaveBeenCalled()
  })

  it('keeps cookie_set with explicit domain profile-scoped while creating exactly one required tab', async () => {
    const surfaces: ReturnType<typeof surfaceView>[] = []
    const beginToolSurfaceLease = vi.fn(async () => {
      const created = surfaceView(0, true)
      surfaces.push(created)
      return {
        surfaceId: created.surfaceId,
        generation: created.generation,
        selectionRevision: 1,
        index: 0,
        resolveIndex: vi.fn(async () => 0),
        closeSurface: vi.fn(async () => undefined),
        finish: vi.fn()
      }
    })
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      ...testSurfaceLeaseApi(() => surfaces),
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => surfaces[0]),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => surfaces[0]),
      selectSurface: vi.fn(async () => surfaces[0]),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginToolSurfaceLease
    }
    const callTool = vi.fn<ManagedMcpClient['callTool']>(async ({ name }) => ({
      content: [{ type: 'text', text: name === 'browser_tabs' ? 'selected' : 'cookie set' }],
      isError: false
    }))
    const host = fakeHost({ callTool, surfaceGroup })
    const args = {
      name: 'fixture-cookie',
      value: 'fixture-value',
      domain: '127.0.0.1',
      call_reason: 'Set a profile cookie for the exact fixture domain.'
    }

    await expect(
      host.callTool('browser_cookie_set', args, {
        authorizationContext: sensitiveContext('browser_cookie_set', args)
      })
    ).resolves.toMatchObject({ isError: false })
    expect(beginToolSurfaceLease).toHaveBeenCalledOnce()
    expect(surfaces).toHaveLength(1)
    expect(callTool.mock.calls.map(([request]) => request.name)).toEqual([
      'browser_tabs',
      'browser_tabs',
      'browser_cookie_set'
    ])
    expect(callTool).toHaveBeenLastCalledWith(
      {
        name: 'browser_cookie_set',
        arguments: { name: 'fixture-cookie', value: 'fixture-value', domain: '127.0.0.1' }
      },
      undefined,
      expect.any(Object)
    )
  })

  it('dispatches an approved page evaluation exactly once and strips Host approval fields', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'fixture-result' }],
      isError: false
    }))
    const host = fakeHost({ callTool })
    const args = {
      function: '() => document.title',
      call_reason: 'Read the fixture title.'
    }
    const authorizationContext = sensitiveContext('browser_evaluate', args)

    await expect(
      host.callTool('browser_evaluate', args, { authorizationContext })
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'fixture-result' }],
      isError: false
    })
    expect(callTool).toHaveBeenCalledOnce()
    expect(callTool).toHaveBeenCalledWith(
      {
        name: 'browser_evaluate',
        arguments: { function: '() => document.title' }
      },
      undefined,
      expect.objectContaining({ resetTimeoutOnProgress: false })
    )
    await expect(
      host.callTool('browser_evaluate', args, { authorizationContext })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_reused',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('dispatches exact approved element targets within the active managed surface', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({ callTool })
    for (const [index, target] of [
      'e17',
      'f1e2',
      'frameLocator("iframe").locator("[contenteditable]")',
      '#unknown-selector'
    ].entries()) {
      const argumentsValue = {
        function: '(element) => element.textContent',
        element: 'Reviewed element in the managed fixture surface',
        target,
        call_reason: 'Read the exact reviewed fixture element.'
      }
      await expect(
        host.callTool('browser_evaluate', argumentsValue, {
          authorizationContext: sensitiveContext('browser_evaluate', argumentsValue, {
            callId: `call-frame-scope-${index}`
          })
        })
      ).resolves.toMatchObject({ isError: false })
    }
    expect(callTool).toHaveBeenCalledTimes(4)
  })

  it('delegates an approved request index directly to the fixed official Tab ledger', async () => {
    const requests = ['fixture-original-request']
    const callTool = vi.fn(
      async ({ name, arguments: args }: { name: string; arguments: Record<string, unknown> }) =>
        name === 'browser_network_requests'
          ? {
              content: [
                {
                  type: 'text' as const,
                  text: requests
                    .map((request, index) => `${index + 1}. [GET] http://127.0.0.1/${request}`)
                    .join('\n')
                }
              ],
              isError: false
            }
          : {
              content: [
                {
                  type: 'text' as const,
                  text: `detail:${requests[Number(args.index) - 1]}`
                }
              ],
              isError: false
            }
    )
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const host = fakeHost({
      callTool,
      createOfficialConnection
    })
    const argumentsValue = {
      index: 1,
      part: 'request-headers',
      call_reason: 'Read the reviewed fixture request headers.'
    }
    await expect(
      host.callTool('browser_network_request', argumentsValue, {
        authorizationContext: sensitiveContext('browser_network_request', argumentsValue)
      })
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'detail:fixture-original-request' }],
      isError: false
    })
    expect(createOfficialConnection).toHaveBeenCalledOnce()
    expect(callTool).toHaveBeenLastCalledWith(
      {
        name: 'browser_network_request',
        arguments: { index: 1, part: 'request-headers' }
      },
      undefined,
      expect.any(Object)
    )
  })

  it('lets the rebuilt fixed official Tab authoritatively resolve a request index', async () => {
    const callTool = vi.fn(
      async ({ name }: { name: string; arguments: Record<string, unknown> }) => {
        if (name === 'browser_network_requests') {
          return {
            content: [
              { type: 'text' as const, text: '1. [GET] http://127.0.0.1/fixture-original' }
            ],
            isError: false
          }
        }
        if (name === 'browser_evaluate') return new Promise<never>(() => undefined)
        return {
          content: [{ type: 'text' as const, text: 'rebuilt-official-ledger-result' }],
          isError: false
        }
      }
    )
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const host = fakeHost({
      callTool: callTool as ManagedMcpClient['callTool'],
      createOfficialConnection,
      toolTimeoutMs: 500
    })
    await host.callTool(
      'browser_network_requests',
      { static: false, call_reason: 'List the reviewed fixture requests.' },
      {
        authorizationContext: {
          ...RISK_CONTEXT,
          callId: 'call-network-list-before-rebuild',
          triggerToolName: 'browser_network_requests'
        }
      }
    )
    const evaluateArguments = {
      function: '() => new Promise(() => {})',
      call_reason: 'Retire this fixture connection on timeout.'
    }
    await expect(
      host.callTool('browser_evaluate', evaluateArguments, {
        authorizationContext: sensitiveContext('browser_evaluate', evaluateArguments, {
          callId: 'call-evaluate-retire-network-ledger'
        }),
        timeoutMs: 5
      })
    ).rejects.toMatchObject({
      code: 'browser.risk_outcome_unknown',
      dispatchCertainty: 'possibly_dispatched'
    })

    const requestArguments = {
      index: 1,
      part: 'response-body',
      call_reason: 'Read this index from the current fixed official Tab ledger.'
    }
    await expect(
      host.callTool('browser_network_request', requestArguments, {
        authorizationContext: sensitiveContext('browser_network_request', requestArguments, {
          callId: 'call-network-stale-after-rebuild'
        })
      })
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'rebuilt-official-ledger-result' }],
      isError: false
    })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
    expect(callTool.mock.calls.map(([request]) => request.name)).toEqual([
      'browser_network_requests',
      'browser_evaluate',
      'browser_network_request'
    ])
  })

  it.each([
    {
      name: 'page origin navigation',
      mutate: (surfaces: ReturnType<typeof surfaceView>[]) => {
        surfaces[0].url = 'http://localhost/after-approval'
      }
    },
    {
      name: 'active tab selection',
      mutate: (surfaces: ReturnType<typeof surfaceView>[]) => {
        surfaces[0].isActive = false
        surfaces.push({
          ...surfaceView(1, true),
          url: 'http://127.0.0.1/other-tab'
        })
      }
    },
    {
      name: 'surface generation replacement',
      mutate: (surfaces: ReturnType<typeof surfaceView>[]) => {
        surfaces[0].generation += 1
      }
    }
  ])(
    'fails closed when $name races grant validation before official dispatch',
    async ({ mutate }) => {
      let releaseConnection!: () => void
      let signalConnectionStarted!: () => void
      const connectionStarted = new Promise<void>((resolve) => {
        signalConnectionStarted = resolve
      })
      const connectionBarrier = new Promise<void>((resolve) => {
        releaseConnection = resolve
      })
      const createOfficialConnection = vi.fn(async () => {
        signalConnectionStarted()
        await connectionBarrier
        return {
          connect: vi.fn(async () => undefined),
          close: vi.fn(async () => undefined)
        }
      })
      const callTool = vi.fn(async () => ({
        content: [{ type: 'text', text: 'must-not-dispatch' }],
        isError: false
      }))
      const surfaces = [surfaceView(0, true)]
      const args = {
        function: '() => document.title',
        call_reason: 'Read the fixture title.'
      }
      const host = fakeHost({
        callTool,
        createOfficialConnection,
        surfaceGroup: singleSurfaceGroup(surfaces)
      })

      const pending = host.callTool('browser_evaluate', args, {
        authorizationContext: sensitiveContext('browser_evaluate', args)
      })
      await connectionStarted
      mutate(surfaces)
      releaseConnection()

      await expect(pending).rejects.toMatchObject({
        code: 'mcp.builtin_playwright.sensitive_grant_origin_drifted',
        dispatchCertainty: 'definitely_not_dispatched'
      })
      expect(callTool).not.toHaveBeenCalled()
    }
  )
})
