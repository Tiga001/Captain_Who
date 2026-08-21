import { afterEach, describe, expect, it, vi } from 'vitest'
import { randomUUID } from 'node:crypto'
import { access, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, isAbsolute, join } from 'node:path'
import type { BrowserContext } from 'playwright'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import { BrowserFileBroker } from '../browser/BrowserFileBroker'
import type {
  BrowserRiskAuthorizationContext,
  BrowserRiskFailure
} from '../browser/BrowserRiskCoordinator'

import {
  appendSafeFrameEditorCandidates,
  ManagedPlaywrightMcpHost,
  ManagedPlaywrightMcpHostError,
  type ManagedPlaywrightConnectionFactory,
  type ManagedPlaywrightMcpHostOptions,
  type ManagedPlaywrightSurfaceGroupAdapter,
  type ManagedMcpClient
} from './ManagedPlaywrightMcpHost'
import {
  MANAGED_PLAYWRIGHT_MANIFEST,
  MANAGED_PLAYWRIGHT_PACKAGE_VERSION
} from './managedPlaywrightManifest'
import {
  MANAGED_PLAYWRIGHT_CAPABILITIES,
  MANAGED_PLAYWRIGHT_CATALOG_LOCK
} from './managedPlaywrightCatalog'
import {
  canonicalSha256,
  sensitiveBindingScopeForInvocation,
  sensitivePolicyForTool
} from './managedPlaywrightSensitivePolicy'
import {
  ManagedPlaywrightSensitiveTargetBindingError,
  type ManagedPlaywrightSensitiveTargetBindingStore
} from './ManagedPlaywrightSensitiveTargetBindingBroker'

const trackedHosts = new Set<ManagedPlaywrightMcpHost>()
const RISK_CONTEXT: BrowserRiskAuthorizationContext = {
  runId: 'run-1',
  capabilityId: 'browser_automation',
  activationId: '123e4567-e89b-42d3-a456-426614174000',
  manifestDigest: `sha256:${'a'.repeat(64)}`,
  policyRevision: 1,
  grantExpiresAtMs: 2_000_000,
  invocationId: '123e4567-e89b-42d3-a456-426614174001',
  callId: 'call-1',
  triggerToolName: 'browser_navigate',
  callReason: 'Open the local fixture.'
}
const PARENT_REQUEST_ID = '123e4567-e89b-42d3-a456-426614174002'

afterEach(async () => {
  const hosts = [...trackedHosts]
  trackedHosts.clear()
  await Promise.allSettled(hosts.map(async (host) => host.close()))
})

describe('ManagedPlaywrightMcpHost', () => {
  it('discovers the exact official 0.0.79 server without resolving a browser context', async () => {
    const getBrowserContext = vi.fn(async () => {
      throw new Error('context getter must stay lazy during discovery')
    })
    const detachAutomation = vi.fn(async () => undefined)
    const host = new ManagedPlaywrightMcpHost({
      getBrowserContext,
      closeSurface: vi.fn(async () => undefined),
      detachAutomation
    })

    const tools = await host.listTools()

    expect(MANAGED_PLAYWRIGHT_PACKAGE_VERSION).toBe('0.0.79')
    expect(getBrowserContext).not.toHaveBeenCalled()
    expect(tools.map((tool) => tool.name)).toEqual(
      MANAGED_PLAYWRIGHT_MANIFEST.tools.map((tool) => tool.rawName)
    )
    expect(tools).toHaveLength(61)
    expect(tools.map((tool) => tool.name)).not.toContain('browser_run_code_unsafe')
    expect(tools.map((tool) => tool.name)).toEqual(
      expect.arrayContaining([
        'browser_hover',
        'browser_select_option',
        'browser_console_messages',
        'browser_network_requests',
        'browser_mouse_click_xy',
        'browser_generate_locator'
      ])
    )
    expect(tools.find((tool) => tool.name === 'browser_snapshot')?.inputSchema).toHaveProperty(
      'properties.filename'
    )
    expect(tools.find((tool) => tool.name === 'browser_tabs')?.inputSchema).toMatchObject({
      properties: { action: { enum: ['list', 'new', 'close', 'select'] } },
      required: ['action', 'call_reason']
    })
    expect(tools.find((tool) => tool.name === 'browser_snapshot')?.annotations).toEqual({
      title: 'Page snapshot',
      readOnlyHint: true,
      destructiveHint: false,
      openWorldHint: true
    })
    for (const tool of tools) {
      expect(tool.inputSchema).toMatchObject({
        properties: { call_reason: { type: 'string', minLength: 1, maxLength: 512 } }
      })
      expect((tool.inputSchema.required as string[]) ?? []).toContain('call_reason')
    }

    await host.close()
    expect(detachAutomation).toHaveBeenCalledOnce()
  })

  it('pins a no-codegen connection whose file reads remain behind the Host FileBroker', async () => {
    let capturedConfig: Parameters<ManagedPlaywrightConnectionFactory>[0] | undefined
    const host = fakeHost({
      createOfficialConnection: async (config) => {
        capturedConfig = config
        return {
          connect: vi.fn(async () => undefined),
          close: vi.fn(async () => undefined)
        }
      }
    })
    await host.connect()
    expect(capturedConfig).toMatchObject({
      browser: { isolated: false },
      capabilities: [...MANAGED_PLAYWRIGHT_CAPABILITIES],
      codegen: 'none',
      imageResponses: 'omit',
      saveSession: false,
      sharedBrowserContext: true
    })
    expect(capturedConfig).not.toHaveProperty('allowUnrestrictedFileAccess')
    const outputDirectory = capturedConfig?.outputDir
    expect(typeof outputDirectory).toBe('string')
    expect(isAbsolute(outputDirectory!)).toBe(true)
    expect((await stat(outputDirectory!)).mode & 0o777).toBe(0o700)
    await host.close()
    await expect(access(outputDirectory!)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('strips call_reason before dispatch and rejects missing or unreviewed arguments', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({ callTool })

    await expect(
      host.callTool('browser_navigate', {
        url: 'http://127.0.0.1/fixture',
        call_reason: 'Open the local fixture.'
      })
    ).resolves.toEqual({ content: [{ type: 'text', text: 'ok' }], isError: false })
    expect(callTool).toHaveBeenCalledWith(
      {
        name: 'browser_navigate',
        arguments: { url: 'http://127.0.0.1/fixture' }
      },
      undefined,
      expect.objectContaining({ resetTimeoutOnProgress: false })
    )

    await expect(
      host.callTool('browser_navigate', { url: 'http://127.0.0.1' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
    await expect(
      host.callTool('browser_snapshot', {
        filename: 42,
        call_reason: 'Write a file.'
      })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
    await expect(
      host.callTool('browser_tabs', { action: 'unknown', call_reason: 'Open another tab.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
    await expect(
      host.callTool('browser_run_code_unsafe', { call_reason: 'Run arbitrary code.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.tool_not_reviewed' })

    await expect(
      host.callTool('browser_console_messages', {
        level: 'debug',
        all: true,
        call_reason: 'Inspect all console output.'
      })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenLastCalledWith(
      {
        name: 'browser_console_messages',
        arguments: { level: 'debug', all: true }
      },
      undefined,
      expect.objectContaining({ resetTimeoutOnProgress: false })
    )
  })

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
    const host = fakeHost({ callTool, surfaceGroup: singleSurfaceGroup() })
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
    const host = fakeHost({ callTool, surfaceGroup: singleSurfaceGroup() })
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

  it('retires a timed-out evaluate generation before releasing the next dispatch slot', async () => {
    let completeLateEvaluation!: (value: unknown) => void
    const lateEvaluation = new Promise<unknown>((resolve) => {
      completeLateEvaluation = resolve
    })
    const callTool = vi.fn(async (input: { name: string }) => {
      if (input.name === 'browser_evaluate') return await lateEvaluation
      return {
        content: [{ type: 'text', text: 'fresh-generation-snapshot' }],
        isError: false
      }
    })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      surfaceGroup: singleSurfaceGroup(),
      toolTimeoutMs: 25
    })
    const args = {
      function: '() => new Promise(() => undefined)',
      call_reason: 'Exercise a never-settling repository fixture script.'
    }
    let terminalCount = 0
    const timedOut = host
      .callTool('browser_evaluate', args, {
        authorizationContext: sensitiveContext('browser_evaluate', args)
      })
      .finally(() => {
        terminalCount += 1
      })

    await expect(timedOut).rejects.toMatchObject({
      code: 'browser.risk_outcome_unknown',
      dispatchCertainty: 'possibly_dispatched'
    })
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Inspect the recovered fixture page.' })
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'fresh-generation-snapshot' }],
      isError: false
    })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)

    completeLateEvaluation({
      content: [{ type: 'text', text: 'stale-generation-result' }],
      isError: false
    })
    await new Promise<void>((resolve) => setImmediate(resolve))
    expect(terminalCount).toBe(1)
    await expect(
      host.callTool('browser_snapshot', {
        call_reason: 'Verify the recovered generation remains authoritative.'
      })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('dispatches exact approved element targets within the active managed surface', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({ callTool, surfaceGroup: singleSurfaceGroup() })
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
      createOfficialConnection,
      surfaceGroup: singleSurfaceGroup()
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
      surfaceGroup: singleSurfaceGroup(),
      toolTimeoutMs: 5
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
        })
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

  it('keeps pure MIME drop automatic while refusing a sensitive grant on that path', async () => {
    const callTool = vi.fn(async () => ({ content: [{ type: 'text', text: 'dropped' }] }))
    const host = fakeHost({ callTool })
    const args = {
      target: '#fixture-drop-zone',
      data: { 'text/plain': 'hello' },
      call_reason: 'Drop plain fixture text.'
    }
    await expect(host.callTool('browser_drop', args)).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledWith(
      {
        name: 'browser_drop',
        arguments: { target: '#fixture-drop-zone', data: { 'text/plain': 'hello' } }
      },
      undefined,
      expect.any(Object)
    )

    await expect(
      host.callTool('browser_drop', args, {
        authorizationContext: sensitiveContext('browser_drop', {
          ...args,
          paths: ['browser-file:00000000-0000-4000-8000-000000000000']
        })
      })
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.sensitive_grant_drifted',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('preserves official file-upload cancellation when paths are omitted', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'File chooser cancelled.' }],
      isError: false
    }))
    const host = fakeHost({ callTool })
    const argumentsValue = {
      call_reason: 'Cancel the current managed page file chooser.'
    }

    await expect(host.callTool('browser_file_upload', argumentsValue)).resolves.toMatchObject({
      isError: false
    })
    expect(callTool).toHaveBeenCalledWith(
      { name: 'browser_file_upload', arguments: {} },
      undefined,
      expect.any(Object)
    )
  })

  it('uses brokered opaque file handles for upload and iframe drop without accepting raw paths', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-file-test-'))
    const source = join(parent, 'selected.txt')
    await writeFile(source, 'selected-file-canary')
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => [source]) }
    })
    await fileBroker.initialize()
    const inputElement = {
      setInputFiles: vi.fn<(paths: readonly string[]) => Promise<void>>(async () => undefined),
      evaluate: vi.fn<(callback: unknown) => Promise<void>>(async () => undefined),
      dispose: vi.fn(async () => undefined)
    }
    const targetElement = {
      evaluateHandle: vi.fn<(callback: unknown) => Promise<{ asElement(): typeof inputElement }>>(
        async () => ({ asElement: () => inputElement })
      ),
      evaluate: vi.fn<
        (callback: unknown, argument: unknown) => Promise<{ accepted: boolean; fileCount: number }>
      >(async () => ({ accepted: true, fileCount: 1 })),
      dispose: vi.fn(async () => undefined)
    }
    const locator = { elementHandle: vi.fn(async () => targetElement) }
    const page = { locator: vi.fn(() => locator) }
    const context = {
      ...fakeBrowserContext(),
      pages: vi.fn(() => [page])
    } as unknown as BrowserContext
    const callTool = vi.fn(async ({ name }: { name: string }) => ({
      content: [
        {
          type: 'text' as const,
          text: name === 'browser_snapshot' ? '- region "Drop completed" [ref=e42]' : 'uploaded'
        }
      ],
      isError: false
    }))
    const host = fakeHost({
      callTool: callTool as ManagedMcpClient['callTool'],
      fileBroker,
      getBrowserContext: vi.fn(async () => context),
      surfaceGroup: singleSurfaceGroup()
    })
    try {
      const [uploadReference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: 'call-file-upload'
        },
        paths: [source]
      })
      const handle = uploadReference.handle
      expect(fileBroker.snapshot().handles).toBe(1)

      const uploadArguments = {
        paths: [handle!],
        call_reason: 'Upload the selected fixture file.'
      }
      await expect(
        host.callTool('browser_file_upload', uploadArguments, {
          authorizationContext: sensitiveContext('browser_file_upload', uploadArguments, {
            callId: 'call-file-upload'
          })
        })
      ).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledOnce()
      const uploadedPaths =
        (callTool.mock.calls[0]?.[0] as { arguments?: { paths?: string[] } }).arguments?.paths ?? []
      const uploadMeta = (
        callTool.mock.calls[0]?.[0] as { arguments?: { _meta?: { cwd?: string } } }
      ).arguments?._meta
      expect(uploadedPaths).toHaveLength(1)
      expect(uploadedPaths[0]).not.toBe(source)
      expect(uploadedPaths[0]).not.toBe(handle)
      expect(isAbsolute(uploadedPaths[0])).toBe(true)
      expect(uploadMeta).toEqual({ cwd: dirname(dirname(uploadedPaths[0])) })
      expect(basename(uploadedPaths[0])).toBe('selected.txt')
      expect(uploadedPaths[0]).toContain('.file-input-')
      expect(uploadedPaths[0]).not.toContain('browser-automation-files')
      await expect(access(uploadedPaths[0])).resolves.toBeUndefined()
      expect(fileBroker.snapshot().handles).toBe(0)
      expect(fileBroker.snapshot().retained).toMatchObject({ leases: 1, files: 1 })

      const [dropReference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: 'call-file-drop'
        },
        paths: [source]
      })
      const dropHandle = dropReference.handle
      const dropArguments = {
        target: 'f2e9',
        paths: [dropHandle!],
        call_reason: 'Drop the selected fixture file into the exact iframe target.'
      }
      await expect(
        host.callTool('browser_drop', dropArguments, {
          authorizationContext: sensitiveContext('browser_drop', dropArguments, {
            callId: 'call-file-drop'
          })
        })
      ).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledTimes(2)
      expect(callTool.mock.calls[1]?.[0]).toMatchObject({
        name: 'browser_snapshot',
        arguments: {}
      })
      expect(page.locator).toHaveBeenCalledWith('aria-ref=f2e9')
      const droppedPaths = inputElement.setInputFiles.mock.calls[0]?.[0] as string[]
      expect(droppedPaths).toHaveLength(1)
      expect(basename(droppedPaths[0])).toBe('selected.txt')
      const dropCallback = String(targetElement.evaluate.mock.calls[0]?.[0])
      expect(
        [...dropCallback.matchAll(/new DragEvent\(["'](dragenter|dragover|drop)["']/gu)].map(
          (match) => match[1]
        )
      ).toEqual(['dragenter', 'dragover', 'drop'])
      expect(dropCallback).toContain('dragover.defaultPrevented')
      expect(fileBroker.snapshot().handles).toBe(0)
      expect(fileBroker.snapshot().retained).toMatchObject({ leases: 2, files: 2 })

      const guessedArguments = {
        paths: [source],
        call_reason: 'Upload a guessed path.'
      }
      await expect(
        host.callTool('browser_file_upload', guessedArguments, {
          authorizationContext: sensitiveContext('browser_file_upload', guessedArguments, {
            callId: 'call-guessed-upload'
          })
        })
      ).rejects.toMatchObject({
        code: 'mcp.builtin_playwright.invalid_arguments',
        dispatchCertainty: 'definitely_not_dispatched'
      })
      expect(callTool).toHaveBeenCalledTimes(2)
      await host.close()
      await expect(access(uploadedPaths[0])).resolves.toBeUndefined()
      await host.releaseRun(RISK_CONTEXT.runId)
      await expect(access(uploadedPaths[0])).rejects.toMatchObject({ code: 'ENOENT' })
      expect(fileBroker.snapshot()).toEqual({
        handles: 0,
        bytes: 0,
        retained: { leases: 0, files: 0, bytes: 0 }
      })
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('uses proposal-frozen handles while keeping the original workspace path out of upstream dispatch', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-one-call-upload-test-'))
    const source = join(parent, '浙江大学2026年招生资料汇编.pptx')
    await writeFile(source, 'fixture-presentation')
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => []) }
    })
    const callId = 'call-one-call-upload'
    const [reference] = await fileBroker.freezeResolvedForRead({
      owner: {
        runId: RISK_CONTEXT.runId,
        activationId: RISK_CONTEXT.activationId,
        capabilityId: 'browser_automation',
        toolCallId: callId
      },
      paths: [source]
    })
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValue({ content: [{ type: 'text', text: 'uploaded' }], isError: false })
    const host = fakeHost({
      callTool,
      fileBroker,
      preparedFileHandles: [reference.handle],
      surfaceGroup: singleSurfaceGroup()
    })
    const argumentsValue = {
      paths: [source],
      call_reason: 'Upload the workspace presentation in this original Tool call.'
    }
    try {
      await expect(
        host.callTool('browser_file_upload', argumentsValue, {
          authorizationContext: sensitiveContext('browser_file_upload', argumentsValue, {
            callId
          })
        })
      ).resolves.toMatchObject({ isError: false })
      const upstream = callTool.mock.calls[0]?.[0] as {
        arguments?: { paths?: string[]; _meta?: { cwd?: string } }
      }
      const stagedPath = upstream.arguments?.paths?.[0]
      expect(stagedPath).toBeTypeOf('string')
      expect(stagedPath).not.toBe(source)
      expect(stagedPath).not.toBe(reference.handle)
      expect(basename(stagedPath!)).toBe('浙江大学2026年招生资料汇编.pptx')
      expect(JSON.stringify(upstream)).not.toContain(source)
      expect(JSON.stringify(upstream)).not.toContain(reference.handle)
      expect(fileBroker.snapshot()).toMatchObject({
        handles: 0,
        retained: { leases: 1, files: 1 }
      })
      await host.releaseRun(RISK_CONTEXT.runId)
      await expect(access(stagedPath!)).rejects.toMatchObject({ code: 'ENOENT' })
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('drops an approved large file through a same-frame file input without sending base64 upstream', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-large-drop-test-'))
    const source = join(parent, 'large-drop.bin')
    await writeFile(source, Buffer.alloc(1_100_000, 'L'))
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => [source]) }
    })
    await fileBroker.initialize()
    const inputElement = {
      setInputFiles: vi.fn<(paths: readonly string[]) => Promise<void>>(async () => undefined),
      evaluate: vi.fn<(callback: unknown) => Promise<void>>(async () => undefined),
      dispose: vi.fn(async () => undefined)
    }
    const targetElement = {
      evaluateHandle: vi.fn<(callback: unknown) => Promise<{ asElement(): typeof inputElement }>>(
        async () => ({ asElement: () => inputElement })
      ),
      evaluate: vi.fn<
        (callback: unknown, argument: unknown) => Promise<{ accepted: boolean; fileCount: number }>
      >(async () => ({ accepted: true, fileCount: 1 })),
      dispose: vi.fn(async () => undefined)
    }
    const locator = { elementHandle: vi.fn(async () => targetElement) }
    const page = { locator: vi.fn(() => locator) }
    const context = {
      ...fakeBrowserContext(),
      pages: vi.fn(() => [page])
    } as unknown as BrowserContext
    const callTool = vi.fn(async () => ({
      content: [
        {
          type: 'text' as const,
          text: '- region "Drop completed" [ref=e42]\n  - text: fixture-large-drop.bin'
        }
      ],
      isError: false
    }))
    const host = fakeHost({
      callTool: callTool as ManagedMcpClient['callTool'],
      fileBroker,
      getBrowserContext: vi.fn(async () => context),
      surfaceGroup: singleSurfaceGroup()
    })
    try {
      const [reference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: 'call-large-drop'
        },
        paths: [source]
      })
      const handle = reference.handle

      const dropArguments = {
        target: "frameLocator('iframe').locator('[data-drop-zone]')",
        paths: [handle!],
        data: { 'text/plain': 'approved-fixture' },
        call_reason: 'Drop the approved large fixture into the exact iframe target.'
      }
      await expect(
        host.callTool('browser_drop', dropArguments, {
          authorizationContext: sensitiveContext('browser_drop', dropArguments, {
            callId: 'call-large-drop'
          })
        })
      ).resolves.toMatchObject({
        content: expect.arrayContaining([
          expect.objectContaining({ type: 'text', text: expect.stringContaining('[ref=e42]') })
        ]),
        structuredContent: { status: 'completed', fileCount: 1, snapshotIncluded: true },
        isError: false
      })

      expect(callTool).toHaveBeenCalledOnce()
      expect(callTool).toHaveBeenCalledWith(
        { name: 'browser_snapshot', arguments: {} },
        undefined,
        expect.any(Object)
      )
      expect(page.locator).toHaveBeenCalledWith(
        'iframe >> internal:control=enter-frame >> [data-drop-zone]'
      )
      const stagedPaths = inputElement.setInputFiles.mock.calls[0]?.[0] as string[]
      expect(stagedPaths).toHaveLength(1)
      expect(stagedPaths[0]).not.toBe(source)
      expect(basename(stagedPaths[0])).toBe('large-drop.bin')
      expect(targetElement.evaluate.mock.calls[0]?.[1]).toMatchObject({
        data: { 'text/plain': 'approved-fixture' },
        fileInput: inputElement
      })
      expect(inputElement.evaluate).toHaveBeenCalledOnce()
      expect(inputElement.dispose).toHaveBeenCalledOnce()
      expect(targetElement.dispose).toHaveBeenCalledOnce()
      expect(fileBroker.snapshot().retained).toMatchObject({ leases: 1, files: 1 })

      await host.releaseRun(RISK_CONTEXT.runId)
      await expect(access(stagedPaths[0])).rejects.toMatchObject({ code: 'ENOENT' })
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
  })

  it('enforces every exposed Host schema before any upstream dispatch', async () => {
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'unexpected dispatch' }],
      isError: false
    }))
    const host = fakeHost({ callTool })

    for (const tool of MANAGED_PLAYWRIGHT_MANIFEST.tools) {
      await expect(
        host.callTool(tool.rawName, {
          call_reason: `Validate ${tool.rawName}.`,
          __unexpected: true
        })
      ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })

      const required = Array.isArray(tool.inputSchema.required)
        ? tool.inputSchema.required.filter(
            (name): name is string => typeof name === 'string' && name !== 'call_reason'
          )
        : []
      if (required.length > 0) {
        await expect(
          host.callTool(tool.rawName, { call_reason: `Validate ${tool.rawName}.` })
        ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
      }
    }

    expect(callTool).not.toHaveBeenCalled()
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
    expect(upstream).toHaveBeenCalledTimes(1)

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

  it('returns the bounded official network-list result unchanged for the live call', async () => {
    const liveCanary = 'LIVE_NETWORK_RESULT_CANARY_A7fQ9vLm2KxP'
    const structuredCanary = 'LIVE_NETWORK_STRUCTURED_CANARY_E6vM3zJp9HkR'
    const officialText = [
      '### Result',
      `1. [GET] https://example.test/api/items?query=complete => [200] ${liveCanary}`,
      '2. [POST] http://example.test/submit => [FAILED] connection reset',
      '### Page',
      '- Page URL: https://example.test/current'
    ].join('\n')
    const callTool = vi
      .fn()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: officialText }],
        structuredContent: { value: structuredCanary },
        isError: false
      })
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: `### Error\n${liveCanary}` }],
        isError: true
      })
    const host = fakeHost({ callTool })

    const result = await host.callTool('browser_network_requests', {
      static: false,
      call_reason: 'Inspect request failures.'
    })

    expect(result).toEqual({
      content: [{ type: 'text', text: officialText }],
      structuredContent: { value: structuredCanary },
      isError: false
    })
    expect(JSON.stringify(result)).toContain(liveCanary)
    expect(JSON.stringify(result)).toContain(structuredCanary)

    const failed = await host.callTool('browser_network_requests', {
      static: false,
      call_reason: 'Inspect request failures.'
    })
    expect(failed).toEqual({
      content: [{ type: 'text', text: `### Error\n${liveCanary}` }],
      isError: true
    })
  })

  it('returns the bounded official console result unchanged for the live call', async () => {
    const liveCanary = 'LIVE_CONSOLE_RESULT_CANARY_A7fQ9vLm2KxP'
    const structuredCanary = 'LIVE_CONSOLE_STRUCTURED_CANARY_B9xR4mNp7TzQ'
    const officialText = [
      '### Result',
      'Total messages: 3 (Errors: 1, Warnings: 1)',
      'Returning 3 messages for level "debug"',
      '',
      `[debug] ${liveCanary}`
    ].join('\n')
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: officialText }],
      structuredContent: { messages: [structuredCanary] },
      isError: false
    }))
    const host = fakeHost({ callTool })

    const result = await host.callTool('browser_console_messages', {
      level: 'debug',
      all: true,
      call_reason: 'Inspect all page console output.'
    })
    expect(result).toEqual({
      content: [{ type: 'text', text: officialText }],
      structuredContent: { messages: [structuredCanary] },
      isError: false
    })
    expect(JSON.stringify(result)).toContain(liveCanary)
    expect(JSON.stringify(result)).toContain(structuredCanary)
    expect(callTool).toHaveBeenCalledWith(
      {
        name: 'browser_console_messages',
        arguments: { level: 'debug', all: true }
      },
      undefined,
      expect.objectContaining({ resetTimeoutOnProgress: false })
    )
  })

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

  it('delegates tab semantics to the fixed official group connection while Host owns resize', async () => {
    let surfaces = [surfaceView(0, true)]
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
      ensureActiveSurface: vi.fn(async () => surfaces[0]),
      listSurfaces: () => surfaces,
      createSurface: vi.fn(async () => {
        surfaces = surfaces.map((surface) => ({ ...surface, isActive: false }))
        const created = surfaceView(surfaces.length, true)
        surfaces.push(created)
        return created
      }),
      selectSurface: vi.fn(async ({ index }) => {
        surfaces = surfaces.map((surface) => ({ ...surface, isActive: surface.index === index }))
        return surfaces[index]
      }),
      closeSurfaceByIndex: vi.fn(async (index) => {
        const selected = index ?? surfaces.find((surface) => surface.isActive)?.index ?? 0
        surfaces = surfaces
          .filter((surface) => surface.index !== selected)
          .map((surface, nextIndex) => ({ ...surface, index: nextIndex }))
        if (surfaces.length > 0 && !surfaces.some((surface) => surface.isActive)) {
          surfaces[0] = { ...surfaces[0], isActive: true }
        }
      }),
      resizeActiveSurface: vi.fn(async ({ width, height }) => ({ width, height }))
    }
    const callTool = vi.fn<ManagedMcpClient['callTool']>().mockResolvedValue({
      content: [{ type: 'text', text: '- 0: Fixture (current)' }],
      isError: false
    })
    const host = fakeHost({ callTool, surfaceGroup })

    const listed = await host.callTool('browser_tabs', {
      action: 'list',
      call_reason: 'List tabs.'
    })
    expect(surfaceGroup.ensureActiveSurface).not.toHaveBeenCalled()
    expect(JSON.stringify(listed)).not.toMatch(/surface-0|generation|webContents|target/i)

    await expect(
      host.callTool('browser_tabs', { action: 'new', call_reason: 'Open a new tab.' })
    ).resolves.toMatchObject({ isError: false })
    await expect(
      host.callTool('browser_tabs', { action: 'select', index: 0, call_reason: 'Select a tab.' })
    ).resolves.toMatchObject({ isError: false })
    await expect(
      host.callTool('browser_tabs', { action: 'close', index: 1, call_reason: 'Close a tab.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool.mock.calls.map(([request]) => request)).toEqual([
      { name: 'browser_tabs', arguments: { action: 'list' } },
      { name: 'browser_tabs', arguments: { action: 'new' } },
      { name: 'browser_tabs', arguments: { action: 'select', index: 0 } },
      { name: 'browser_tabs', arguments: { action: 'close', index: 1 } }
    ])
    expect(surfaceGroup.createSurface).not.toHaveBeenCalled()
    expect(surfaceGroup.selectSurface).not.toHaveBeenCalled()
    expect(surfaceGroup.closeSurfaceByIndex).not.toHaveBeenCalled()
    await expect(
      host.callTool('browser_resize', { width: 960, height: 640, call_reason: 'Resize.' })
    ).resolves.toMatchObject({ structuredContent: { width: 960, height: 640 } })
    expect(surfaceGroup.resizeActiveSurface).toHaveBeenCalledWith({ width: 960, height: 640 })
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
      artifacts: () => [],
      finish: vi.fn(() => {
        order.push('risk-finished')
      })
    } as unknown as BrowserNetworkOperationLease
    const finishIntent = vi.fn(() => authority.finish())
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
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
      artifacts: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const beginToolSurfaceLease = vi.fn(async () => {
      throw new Error('browser_navigate must not precreate a blank Surface')
    })
    const beginExistingToolSurfaceLease = vi.fn(async () => null)
    const finishIntent = vi.fn(() => authority.finish())
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
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
      artifacts: () => [],
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
    expect(beginToolSurfaceLeaseByIndex).toHaveBeenCalledWith(0)
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
      artifacts: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
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
      artifacts: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
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

  it('acknowledges the exact dispatch boundary and authoritative response in order', async () => {
    const order: string[] = []
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => {
      order.push('official-call')
      return { content: [{ type: 'text', text: 'snapshot' }], isError: false }
    })
    const onDispatchPhase = vi.fn(async (phase: string) => {
      order.push(phase)
      return true
    })
    const host = fakeHost({ callTool: upstream })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Verify the local dispatch acknowledgement.' },
        { onDispatchPhase }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(order).toEqual(['possibly_dispatched', 'official-call', 'response_received'])
  })

  it('fails closed before the official handler when dispatch acknowledgement is rejected', async () => {
    const upstream = vi.fn<ManagedMcpClient['callTool']>(async () => ({
      content: [{ type: 'text', text: 'must not run' }],
      isError: false
    }))
    const host = fakeHost({ callTool: upstream })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Verify rejected dispatch acknowledgement.' },
        { onDispatchPhase: vi.fn(async () => false) }
      )
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.protocol_error',
      dispatchCertainty: 'definitely_not_dispatched'
    })
    expect(upstream).not.toHaveBeenCalled()
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
      artifacts: () => [],
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
      artifacts: () => [],
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

  it('isolates persistent network state to one run and cleans it at run completion', async () => {
    const setOffline = vi.fn(async () => undefined)
    const route = vi.fn(async () => undefined)
    const unroute = vi.fn(async () => undefined)
    const context = fakeBrowserContext({ route, setOffline, unroute })
    const callTool = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({ callTool, getBrowserContext: async () => context })
    const runA = { ...RISK_CONTEXT, runId: 'run-a', callId: 'call-a' }
    const runB = { ...RISK_CONTEXT, runId: 'run-b', callId: 'call-b' }

    await host.callTool(
      'browser_network_state_set',
      { state: 'offline', call_reason: 'Test offline state.' },
      { authorizationContext: runA }
    )
    expect(setOffline).toHaveBeenLastCalledWith(true)
    await expect(
      host.callTool(
        'browser_network_state_set',
        { state: 'online', call_reason: 'Restore another run.' },
        { authorizationContext: runB }
      )
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.busy' })
    expect(setOffline).not.toHaveBeenCalledWith(false)

    await host.releaseRun('run-a')
    expect(setOffline).toHaveBeenLastCalledWith(false)
    await expect(
      host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1/fixture', call_reason: 'Navigate after cleanup.' },
        { authorizationContext: runB }
      )
    ).resolves.toMatchObject({ isError: false })
  })

  it('preserves the fixed 0.0.79 route fulfill, header rewrite, list, and unroute semantics', async () => {
    type FixtureRouteHandler = (route: {
      fulfill(input: unknown): Promise<void>
      continue(input: unknown): Promise<void>
      request(): { headers(): Record<string, string> }
    }) => Promise<void>
    const contextRoute = vi.fn<(pattern: string, handler: FixtureRouteHandler) => Promise<void>>(
      async () => undefined
    )
    const contextUnroute = vi.fn<(pattern: string, handler: FixtureRouteHandler) => Promise<void>>(
      async () => undefined
    )
    const context = fakeBrowserContext({
      route: contextRoute as unknown as (...args: unknown[]) => Promise<void>,
      unroute: contextUnroute as unknown as (...args: unknown[]) => Promise<void>
    })
    const host = fakeHost({ getBrowserContext: async () => context })
    const authorizationContext = { ...RISK_CONTEXT, runId: 'route-run' }

    await expect(
      host.callTool(
        'browser_route',
        {
          pattern: '**/fixture-response',
          status: 201,
          body: 'super-secret-response-body',
          contentType: 'application/json',
          headers: ['X-Added: added-value'],
          removeHeaders: 'Authorization, X-Remove',
          call_reason: 'Mock the local fixture response.'
        },
        { authorizationContext }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(contextRoute).toHaveBeenCalledTimes(1)
    expect(contextRoute.mock.calls[0][0]).toBe('**/fixture-response')
    const fulfillHandler = contextRoute.mock.calls[0][1]
    const fulfill = vi.fn(async () => undefined)
    const continueRequest = vi.fn(async () => undefined)
    await fulfillHandler({
      fulfill,
      continue: continueRequest,
      request: () => ({ headers: () => ({ authorization: 'original' }) })
    })
    expect(fulfill).toHaveBeenCalledWith({
      status: 201,
      contentType: 'application/json',
      body: 'super-secret-response-body'
    })
    expect(continueRequest).not.toHaveBeenCalled()

    const listed = await host.callTool(
      'browser_route_list',
      { call_reason: 'List the local fixture routes.' },
      { authorizationContext: { ...authorizationContext, callId: 'route-list-call' } }
    )
    expect(listed).toEqual({
      content: [
        {
          type: 'text',
          text: '1. **/fixture-response (status=201, body=super-secret-response-body, contentType=application/json, addHeaders={"X-Added":"added-value"}, removeHeaders=Authorization,X-Remove)'
        }
      ],
      isError: false
    })

    await expect(
      host.callTool(
        'browser_unroute',
        { pattern: '**/fixture-response', call_reason: 'Remove the response route.' },
        { authorizationContext: { ...authorizationContext, callId: 'route-unroute-call' } }
      )
    ).resolves.toMatchObject({ isError: false })
    expect(contextUnroute).toHaveBeenCalledWith('**/fixture-response', fulfillHandler)

    await host.callTool(
      'browser_route',
      {
        pattern: '**/fixture-request',
        headers: ['X-Added: replacement'],
        removeHeaders: 'Authorization, X-Remove',
        call_reason: 'Rewrite local fixture request headers.'
      },
      { authorizationContext: { ...authorizationContext, callId: 'route-header-call' } }
    )
    const rewriteHandler = contextRoute.mock.calls.at(-1)![1]
    const rewriteFulfill = vi.fn(async () => undefined)
    const rewriteContinue = vi.fn(async () => undefined)
    await rewriteHandler({
      fulfill: rewriteFulfill,
      continue: rewriteContinue,
      request: () => ({
        headers: () => ({ authorization: 'credential', 'x-remove': 'remove-me', keep: 'yes' })
      })
    })
    expect(rewriteFulfill).not.toHaveBeenCalled()
    expect(rewriteContinue).toHaveBeenCalledWith({
      headers: { keep: 'yes', 'X-Added': 'replacement' }
    })

    await expect(
      host.callTool(
        'browser_unroute',
        { call_reason: 'Remove every local fixture route.' },
        { authorizationContext: { ...authorizationContext, callId: 'route-unroute-all-call' } }
      )
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'Removed all 1 route(s)' }],
      isError: false
    })
    expect(contextUnroute).toHaveBeenLastCalledWith('**/fixture-request', rewriteHandler)
    await expect(
      host.callTool(
        'browser_route_list',
        { call_reason: 'Confirm all local fixture routes were removed.' },
        { authorizationContext: { ...authorizationContext, callId: 'route-empty-list-call' } }
      )
    ).resolves.toEqual({
      content: [{ type: 'text', text: 'No active routes' }],
      isError: false
    })
  })

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
      getActiveSurfaceIdentity: () => ({ surfaceId: 'surface-1', generation: 1 }),
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
      expect(JSON.stringify(result)).not.toContain(parent)
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
      getActiveSurfaceIdentity: () => {
        const active = surfaces.find((surface) => surface.isActive)!
        return { surfaceId: active.surfaceId, generation: active.generation }
      },
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
      getActiveSurfaceIdentity: () => ({ surfaceId: 'surface-1', generation: 1 }),
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
      expect(JSON.stringify(result)).not.toContain(parent)
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
      getActiveSurfaceIdentity: () => ({
        surfaceId: exactLease.surfaceId,
        generation: exactLease.generation
      }),
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
      getActiveSurfaceIdentity: () => ({ surfaceId: 'surface-1', generation: 1 }),
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

  it('preflights browser_navigate before upstream dispatch', async () => {
    const order: string[] = []
    const check = vi.fn(async () => {
      order.push('preflight')
    })
    const callTool = vi.fn(async () => {
      order.push('upstream')
      return { content: [{ type: 'text', text: 'ok' }], isError: false }
    })
    const risk = riskLease({ check })
    risk.markDispatched.mockImplementation(() => order.push('dispatched'))
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1:3000/', call_reason: 'Open the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toMatchObject({ isError: false })

    expect(order).toEqual(['preflight', 'dispatched', 'upstream'])
    expect(check).toHaveBeenCalledWith('http://127.0.0.1:3000/')
    expect(risk.markDispatched).toHaveBeenCalledOnce()
    expect(risk.finish).toHaveBeenCalledOnce()
  })

  it('returns a rejected risk decision as a normal tool-level result without dispatch', async () => {
    const failure: BrowserRiskFailure = {
      code: 'browser.risk_rejected',
      dispatchCertainty: 'definitely_not_dispatched',
      reason: 'Use a public page instead.'
    }
    const risk = riskLease({
      check: vi.fn(async () => {
        throw new Error('blocked before dispatch')
      }),
      failure
    })
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_navigate',
        { url: 'http://127.0.0.1:3000/', call_reason: 'Open the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).resolves.toEqual({
      content: [
        {
          type: 'text',
          text: 'The user declined this browser destination or operation. Reason: Use a public page instead.'
        }
      ],
      structuredContent: {
        status: 'browser.risk_rejected',
        dispatchCertainty: 'definitely_not_dispatched',
        rejectionReason: 'Use a public page instead.'
      },
      isError: true
    })
    expect(callTool).not.toHaveBeenCalled()
    expect(risk.markDispatched).not.toHaveBeenCalled()
  })

  it('maps a post-dispatch webRequest refusal to outcome unknown', async () => {
    const failure: BrowserRiskFailure = {
      code: 'browser.risk_rejected',
      dispatchCertainty: 'possibly_dispatched'
    }
    const risk = riskLease({ failure })
    const host = fakeHost({
      callTool: vi.fn(async () => {
        throw new Error('net::ERR_BLOCKED_BY_CLIENT including an untrusted URL')
      }),
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'button', call_reason: 'Open the link.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
    expect(risk.markDispatched).toHaveBeenCalledOnce()
  })

  it('does not settle a tool before an asynchronous boundary decision finishes', async () => {
    let release!: () => void
    let failure: BrowserRiskFailure | undefined
    const risk = riskLease({
      failure: () => failure,
      settle: async () =>
        await new Promise<void>((resolve) => {
          release = () => {
            failure = {
              code: 'browser.risk_rejected',
              dispatchCertainty: 'possibly_dispatched'
            }
            resolve()
          }
        })
    })
    const upstream = vi.fn(async () => ({
      content: [{ type: 'text', text: 'ok' }],
      isError: false
    }))
    const host = fakeHost({
      callTool: upstream,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    const pending = host.callTool(
      'browser_click',
      { target: 'risky link', call_reason: 'Open the link.' },
      { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
    )
    let settled = false
    void pending.then(
      () => {
        settled = true
      },
      () => {
        settled = true
      }
    )
    await vi.waitFor(() => expect(risk.settle).toHaveBeenCalledOnce())
    expect(upstream).toHaveBeenCalledOnce()
    expect(settled).toBe(false)

    release()
    await expect(pending).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
  })

  it('never auto-replays a possibly-dispatched browser operation', async () => {
    const risk = riskLease({
      failure: {
        code: 'browser.risk_outcome_unknown',
        dispatchCertainty: 'possibly_dispatched'
      }
    })
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({
      callTool,
      beginNetworkOperation: vi.fn(async () => risk.lease)
    })

    await expect(
      host.callTool(
        'browser_click',
        { target: 'download', call_reason: 'Download the fixture.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({ code: 'browser.risk_outcome_unknown' })
    expect(callTool).toHaveBeenCalledOnce()
  })

  it('reports phase-accurate certainty before and after upstream dispatch', async () => {
    const surfaceUnavailable = fakeHost({
      beginNetworkOperation: vi.fn(async () => {
        throw Object.assign(new Error('safe fixture error'), {
          code: 'browser.surface_unavailable'
        })
      })
    })
    await expect(
      surfaceUnavailable.callTool(
        'browser_snapshot',
        { call_reason: 'Inspect the page.' },
        { authorizationContext: RISK_CONTEXT, parentRequestId: PARENT_REQUEST_ID }
      )
    ).rejects.toMatchObject({
      code: 'browser.surface_unavailable',
      dispatchCertainty: 'definitely_not_dispatched'
    })

    const controller = new AbortController()
    const upstream = vi.fn(
      async (_input, _schema, options) =>
        await new Promise((_resolve, reject) => {
          options?.signal?.addEventListener('abort', () => reject(new Error('aborted')), {
            once: true
          })
        })
    )
    const dispatched = fakeHost({
      beginNetworkOperation: vi.fn(async () => riskLease({}).lease),
      callTool: upstream
    })
    const pending = dispatched.callTool(
      'browser_snapshot',
      { call_reason: 'Inspect the page.' },
      {
        authorizationContext: RISK_CONTEXT,
        parentRequestId: PARENT_REQUEST_ID,
        signal: controller.signal
      }
    )
    await vi.waitFor(() => expect(upstream).toHaveBeenCalledOnce())
    controller.abort('task_cancelled')
    await expect(pending).rejects.toMatchObject({
      dispatchCertainty: 'possibly_dispatched'
    })
  })

  it('propagates cancellation and bounds untrusted results', async () => {
    const callTool = vi.fn(
      async (
        _params: unknown,
        _schema: undefined,
        options?: { signal?: AbortSignal }
      ): Promise<unknown> =>
        new Promise((_resolve, reject) => {
          options?.signal?.addEventListener('abort', () => reject(new Error('aborted')), {
            once: true
          })
        })
    )
    const host = fakeHost({ callTool })
    const controller = new AbortController()
    const pending = host.callTool(
      'browser_snapshot',
      { call_reason: 'Read the local page.' },
      { signal: controller.signal }
    )
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'mcp.builtin_playwright.cancelled' })

    const oversized = fakeHost({
      callTool: vi.fn(async () => ({
        content: [{ type: 'text', text: 'x'.repeat(256 * 1024 + 1) }],
        isError: false
      }))
    })
    await expect(
      oversized.callTool('browser_snapshot', { call_reason: 'Read the local page.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })

    const oversizedResource = fakeHost({
      callTool: vi.fn(async () => ({
        content: [
          {
            type: 'resource',
            resource: {
              uri: 'fixture://oversized',
              blob: 'x'.repeat(1024 * 1024 + 1),
              mimeType: 'application/octet-stream'
            }
          }
        ],
        isError: false
      }))
    })
    await expect(
      oversizedResource.callTool('browser_snapshot', { call_reason: 'Read the local page.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })
  })

  it('keeps an official connection after rejecting an authoritative oversized response', async () => {
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'x'.repeat(256 * 1024 + 1) }],
        isError: false
      })
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh bounded response' }],
        isError: false
      })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const dispatchPhases: string[] = []
    const onDispatchPhase = vi.fn(async (phase: 'possibly_dispatched' | 'response_received') => {
      dispatchPhases.push(phase)
      return true
    })
    const host = fakeHost({ callTool, createOfficialConnection, detachAutomation })

    await expect(
      host.callTool(
        'browser_snapshot',
        { call_reason: 'Exercise an oversized authoritative response.' },
        { onDispatchPhase }
      )
    ).rejects.toMatchObject({
      code: 'mcp.builtin_playwright.output_too_large',
      dispatchCertainty: 'response_received'
    })
    expect(dispatchPhases).toEqual(['possibly_dispatched', 'response_received'])
    expect(detachAutomation).not.toHaveBeenCalled()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read a bounded response next.' })
    ).resolves.toMatchObject({ isError: false })
    expect(createOfficialConnection).toHaveBeenCalledOnce()
    expect(detachAutomation).not.toHaveBeenCalled()
  })

  it('bounds structuredContent before cloning or serializing it', async () => {
    let deep: Record<string, unknown> = { leaf: true }
    for (let index = 0; index < 34; index += 1) deep = { nested: deep }
    const deepHost = fakeHost({
      callTool: vi.fn(async () => ({ content: [], structuredContent: deep, isError: false }))
    })
    await expect(
      deepHost.callTool('browser_snapshot', { call_reason: 'Inspect the local page.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })

    const wideHost = fakeHost({
      callTool: vi.fn(async () => ({
        content: [],
        structuredContent: { neutral: 'x'.repeat(64 * 1024 + 1) },
        isError: false
      }))
    })
    await expect(
      wideHost.callTool('browser_snapshot', { call_reason: 'Inspect the local page.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })

    const manyNodesHost = fakeHost({
      callTool: vi.fn(async () => ({
        content: [],
        structuredContent: {
          groups: Array.from({ length: 256 }, () => Array.from({ length: 16 }, () => true))
        },
        isError: false
      }))
    })
    await expect(
      manyNodesHost.callTool('browser_snapshot', { call_reason: 'Inspect the local page.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })
  })

  it('bounds embedded resource and resource-link strings before constructing the result', async () => {
    const oversizedBlocks: Array<[string, Record<string, unknown>]> = [
      [
        'embedded resource text',
        {
          type: 'resource',
          resource: { uri: 'fixture://text', text: 'x'.repeat(256 * 1024 + 1) }
        }
      ],
      [
        'embedded resource URI',
        {
          type: 'resource',
          resource: { uri: `fixture://${'u'.repeat(8 * 1024)}`, text: 'safe' }
        }
      ],
      [
        'resource-link name',
        {
          type: 'resource_link',
          uri: 'fixture://link',
          name: 'n'.repeat(1024 + 1)
        }
      ],
      [
        'resource-link title',
        {
          type: 'resource_link',
          uri: 'fixture://link',
          name: 'link',
          title: 't'.repeat(4 * 1024 + 1)
        }
      ],
      [
        'resource-link description',
        {
          type: 'resource_link',
          uri: 'fixture://link',
          name: 'link',
          description: 'd'.repeat(16 * 1024 + 1)
        }
      ],
      [
        'resource MIME type',
        {
          type: 'resource',
          resource: { uri: 'fixture://text', text: 'safe', mimeType: 'm'.repeat(256 + 1) }
        }
      ]
    ]

    for (const [label, block] of oversizedBlocks) {
      const host = fakeHost({
        callTool: vi.fn(async () => ({ content: [block], isError: false }))
      })
      await expect(
        host.callTool('browser_snapshot', { call_reason: `Check ${label}.` })
      ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })
    }
  })

  it('enforces a cumulative escaped-string budget across content blocks', async () => {
    const host = fakeHost({
      callTool: vi.fn(async () => ({
        content: Array.from({ length: 3 }, () => ({
          type: 'resource',
          resource: {
            uri: 'fixture://escaped',
            text: '\0'.repeat(256 * 1024)
          }
        })),
        isError: false
      }))
    })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Inspect bounded resource text.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.output_too_large' })
  })

  it('normalizes every supported SDK content block to the Rust domain wire', async () => {
    const host = fakeHost({
      callTool: vi.fn(async () => ({
        content: [
          { type: 'text', text: 'visible' },
          { type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' },
          { type: 'audio', data: 'YXVkaW8=', mimeType: 'audio/wav' },
          {
            type: 'resource',
            resource: { uri: 'fixture://text', text: 'resource text', mimeType: 'text/plain' }
          },
          {
            type: 'resource',
            resource: {
              uri: 'fixture://blob',
              blob: 'YmxvYg==',
              mimeType: 'application/octet-stream'
            }
          },
          {
            type: 'resource_link',
            uri: 'fixture://link',
            name: 'fixture-link',
            mimeType: 'text/plain',
            size: 7
          }
        ],
        isError: false
      }))
    })

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Inspect the local fixture.' })
    ).resolves.toEqual({
      content: [
        { type: 'text', text: 'visible' },
        { type: 'image', data: 'aW1hZ2U=', mime_type: 'image/png' },
        { type: 'audio', data: 'YXVkaW8=', mime_type: 'audio/wav' },
        {
          type: 'embedded_resource',
          resource: {
            contentType: 'text',
            uri: 'fixture://text',
            text: 'resource text',
            mime_type: 'text/plain'
          }
        },
        {
          type: 'embedded_resource',
          resource: {
            contentType: 'blob',
            uri: 'fixture://blob',
            data: 'YmxvYg==',
            mime_type: 'application/octet-stream'
          }
        },
        {
          type: 'resource_link',
          resource: {
            uri: 'fixture://link',
            name: 'fixture-link',
            mimeType: 'text/plain',
            size: 7
          }
        }
      ],
      isError: false
    })
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
      artifacts: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      listSurfaces: () => [],
      createSurface: vi.fn(async () => {
        throw new Error('official target creation owns this fixture')
      }),
      selectSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginTargetCreationIntent: vi.fn(() => () => authority.finish())
    }
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockRejectedValueOnce(new Error('fixture target attach rejected'))
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
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
    expect(callTool).toHaveBeenCalledOnce()
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read a fresh local fixture context.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledTimes(2)
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
      artifacts: () => [],
      finish: vi.fn()
    } as unknown as BrowserNetworkOperationLease
    const surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter = {
      getSensitiveTargetIdentity: () => null,
      ensureActiveSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      listSurfaces: () => [],
      createSurface: vi.fn(async () => {
        throw new Error('official target creation owns this fixture')
      }),
      selectSurface: vi.fn(async () => {
        throw new Error('no surface')
      }),
      closeSurfaceByIndex: vi.fn(async () => undefined),
      beginTargetCreationIntent: vi.fn(() => () => authority.finish())
    }
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'Target page, context or browser has been closed' }],
        structuredContent: { errorCode: 'target_closed' },
        isError: true
      })
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
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
    expect(callTool).toHaveBeenCalledOnce()
    expect(detachAutomation).toHaveBeenCalledOnce()

    await expect(
      host.callTool('browser_snapshot', { call_reason: 'Read a fresh local fixture context.' })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenCalledTimes(2)
    expect(createOfficialConnection).toHaveBeenCalledTimes(2)
  })

  it('retires terminal and rejected post-drop snapshots without replaying the file drop', async () => {
    const parent = await mkdtemp(join(tmpdir(), 'mycopilot-host-drop-retirement-test-'))
    const source = join(parent, 'approved-drop.txt')
    await writeFile(source, 'approved-drop-fixture')
    const fileBroker = new BrowserFileBroker({
      rootDirectory: join(parent, 'browser-automation-files'),
      selectionProvider: { selectFiles: vi.fn(async () => []) }
    })
    await fileBroker.initialize()
    const inputElement = {
      setInputFiles: vi.fn<(paths: readonly string[]) => Promise<void>>(async () => undefined),
      evaluate: vi.fn<(callback: unknown) => Promise<void>>(async () => undefined),
      dispose: vi.fn(async () => undefined)
    }
    const targetElement = {
      evaluateHandle: vi.fn<(callback: unknown) => Promise<{ asElement(): typeof inputElement }>>(
        async () => ({ asElement: () => inputElement })
      ),
      evaluate: vi.fn<
        (callback: unknown, argument: unknown) => Promise<{ accepted: boolean; fileCount: number }>
      >(async () => ({ accepted: true, fileCount: 1 })),
      dispose: vi.fn(async () => undefined)
    }
    const locator = { elementHandle: vi.fn(async () => targetElement) }
    const page = { locator: vi.fn(() => locator) }
    const context = {
      ...fakeBrowserContext(),
      pages: vi.fn(() => [page])
    } as unknown as BrowserContext
    const callTool = vi
      .fn<ManagedMcpClient['callTool']>()
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'Target page, context or browser has been closed' }],
        structuredContent: { errorCode: 'target_closed' },
        isError: true
      })
      .mockRejectedValueOnce(new Error('fixture post-drop transport closed'))
      .mockResolvedValueOnce({
        content: [{ type: 'text', text: 'fresh snapshot' }],
        isError: false
      })
    const createOfficialConnection = vi.fn(async () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined)
    }))
    const detachAutomation = vi.fn(async () => undefined)
    const host = fakeHost({
      callTool,
      createOfficialConnection,
      detachAutomation,
      fileBroker,
      getBrowserContext: vi.fn(async () => context),
      surfaceGroup: singleSurfaceGroup()
    })
    const freezeDropHandle = async (callId: string): Promise<string> => {
      const [reference] = await fileBroker.freezeResolvedForRead({
        owner: {
          runId: RISK_CONTEXT.runId,
          activationId: RISK_CONTEXT.activationId,
          capabilityId: 'browser_automation',
          toolCallId: callId
        },
        paths: [source]
      })
      return reference.handle!
    }

    try {
      const terminalCallId = 'call-drop-terminal-snapshot'
      const terminalArguments = {
        target: 'e17',
        paths: [await freezeDropHandle(terminalCallId)],
        call_reason: 'Exercise a terminal post-drop snapshot response.'
      }
      await expect(
        host.callTool('browser_drop', terminalArguments, {
          authorizationContext: sensitiveContext('browser_drop', terminalArguments, {
            callId: terminalCallId
          })
        })
      ).resolves.toMatchObject({
        structuredContent: { status: 'completed', fileCount: 1, snapshotIncluded: false },
        isError: false
      })
      expect(callTool).toHaveBeenCalledOnce()
      expect(createOfficialConnection).toHaveBeenCalledOnce()
      expect(detachAutomation).toHaveBeenCalledOnce()

      const rejectedCallId = 'call-drop-rejected-snapshot'
      const rejectedArguments = {
        target: 'e17',
        paths: [await freezeDropHandle(rejectedCallId)],
        call_reason: 'Exercise a rejected post-drop snapshot request.'
      }
      await expect(
        host.callTool('browser_drop', rejectedArguments, {
          authorizationContext: sensitiveContext('browser_drop', rejectedArguments, {
            callId: rejectedCallId
          })
        })
      ).rejects.toMatchObject({ dispatchCertainty: 'possibly_dispatched' })
      expect(callTool).toHaveBeenCalledTimes(2)
      expect(createOfficialConnection).toHaveBeenCalledTimes(2)
      expect(detachAutomation).toHaveBeenCalledTimes(2)

      await expect(
        host.callTool('browser_snapshot', {
          call_reason: 'Read a fresh fixture after the rejected post-drop snapshot.'
        })
      ).resolves.toMatchObject({ isError: false })
      expect(callTool).toHaveBeenCalledTimes(3)
      expect(createOfficialConnection).toHaveBeenCalledTimes(3)
      expect(inputElement.setInputFiles).toHaveBeenCalledTimes(2)
    } finally {
      await fileBroker.shutdown()
      await rm(parent, { recursive: true, force: true })
    }
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

  it('fails closed when the fixed upstream catalog drifts', async () => {
    const host = fakeHost({
      listTools: vi.fn(async () => ({ tools: [] }))
    })
    await expect(host.listTools()).rejects.toEqual(
      new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
    )
  })
})

function surfaceView(index: number, isActive: boolean) {
  return {
    surfaceId: `surface-${index}`,
    index,
    title: `Tab ${index}`,
    url: `http://127.0.0.1/tab-${index}`,
    isActive,
    generation: index + 1
  }
}

function singleSurfaceGroup(
  surfaces: ReturnType<typeof surfaceView>[] = [surfaceView(0, true)]
): ManagedPlaywrightSurfaceGroupAdapter {
  return {
    getSensitiveTargetIdentity: () => sensitiveTargetFromSurfaces(surfaces),
    ensureActiveSurface: vi.fn(async () => {
      const active = surfaces.find((surface) => surface.isActive)
      if (!active) throw new Error('no active surface')
      return active
    }),
    listSurfaces: () => surfaces,
    createSurface: vi.fn(async () => {
      const created = surfaceView(surfaces.length, true)
      for (const surface of surfaces) surface.isActive = false
      surfaces.push(created)
      return created
    }),
    selectSurface: vi.fn(async ({ index }) => {
      for (const surface of surfaces) surface.isActive = surface.index === index
      return surfaces[index]
    }),
    closeSurfaceByIndex: vi.fn(async () => undefined)
  }
}

function sensitiveContext(
  toolName: string,
  modelArguments: Record<string, unknown>,
  overrides: Partial<BrowserRiskAuthorizationContext> = {}
): BrowserRiskAuthorizationContext {
  const policy = sensitivePolicyForTool(toolName)
  if (!policy) throw new Error(`Missing sensitive policy for ${toolName}`)
  const bindingScope = sensitiveBindingScopeForInvocation(toolName, modelArguments)
  const origin = bindingScope === 'managed_browser_profile' ? null : 'http://127.0.0.1'
  const argumentsDigest = canonicalSha256(modelArguments)
  const expiresAtMs = Date.now() + 60_000
  const targetBindingId = randomUUID()
  const targetBindingDigest = canonicalSha256({ targetBindingId })
  return {
    ...RISK_CONTEXT,
    callId: `call-${toolName}`,
    triggerToolName: toolName,
    grantExpiresAtMs: expiresAtMs + 60_000,
    ...overrides,
    builtinToolGrant: {
      grantId: randomUUID(),
      approvalId: randomUUID(),
      argumentsDigest,
      targetBindingId,
      targetBindingDigest,
      resourceScopeDigest: canonicalSha256({
        schemaVersion: 2,
        toolId: toolName,
        argumentsDigest,
        origin,
        riskKinds: [...policy.riskKinds],
        scope: bindingScope,
        targetBindingDigest
      }),
      origin,
      riskKinds: [...policy.riskKinds],
      expiresAtMs
    }
  }
}

function fakeBrowserContext(
  overrides: {
    route?: (...args: unknown[]) => Promise<void>
    setOffline?: (offline: boolean) => Promise<void>
    unroute?: (...args: unknown[]) => Promise<void>
  } = {}
): BrowserContext {
  return {
    route: overrides.route ?? vi.fn(async () => undefined),
    unroute: overrides.unroute ?? vi.fn(async () => undefined),
    setOffline: overrides.setOffline ?? vi.fn(async () => undefined),
    once: vi.fn(),
    off: vi.fn(),
    tracing: {
      start: vi.fn(async () => undefined),
      stop: vi.fn(async () => undefined)
    }
  } as unknown as BrowserContext
}

function closableBrowserContext(): { context: BrowserContext; close(): void } {
  let connected = true
  let handleClose: (() => void) | undefined
  const context = {
    route: vi.fn(async () => undefined),
    unroute: vi.fn(async () => undefined),
    setOffline: vi.fn(async () => undefined),
    once: vi.fn((event: string, handler: () => void) => {
      if (event === 'close') handleClose = handler
      return context
    }),
    off: vi.fn((event: string, handler: () => void) => {
      if (event === 'close' && handleClose === handler) handleClose = undefined
      return context
    }),
    browser: vi.fn(() => ({ isConnected: () => connected })),
    tracing: {
      start: vi.fn(async () => undefined),
      stop: vi.fn(async () => undefined)
    }
  } as unknown as BrowserContext
  return {
    context,
    close: () => {
      connected = false
      handleClose?.()
    }
  }
}

function fakeHost(overrides: {
  artifactBroker?: BrowserArtifactBroker
  fileBroker?: BrowserFileBroker
  beginNetworkOperation?: ManagedPlaywrightMcpHostOptions['beginNetworkOperation']
  beginTargetCreationOperation?: ManagedPlaywrightMcpHostOptions['beginTargetCreationOperation']
  callTool?: ManagedMcpClient['callTool']
  closeSurface?: () => Promise<void>
  createClient?: () => ManagedMcpClient
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  detachAutomation?: () => Promise<void>
  getActiveSurfaceIdentity?: ManagedPlaywrightMcpHostOptions['getActiveSurfaceIdentity']
  getBrowserContext?: () => Promise<BrowserContext>
  listTools?: ManagedMcpClient['listTools']
  preparedFileHandles?: readonly string[]
  surfaceGroup?: ManagedPlaywrightSurfaceGroupAdapter
  toolTimeoutMs?: number
}): ManagedPlaywrightMcpHost {
  const upstreamTools = MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: structuredClone(tool.inputSchema),
    annotations: structuredClone(tool.annotations ?? {})
  }))
  const host = new ManagedPlaywrightMcpHost({
    artifactBroker: overrides.artifactBroker,
    fileBroker: overrides.fileBroker,
    beginNetworkOperation: overrides.beginNetworkOperation,
    beginTargetCreationOperation: overrides.beginTargetCreationOperation,
    getActiveSurfaceIdentity: overrides.getActiveSurfaceIdentity,
    getBrowserContext:
      overrides.getBrowserContext ??
      (async () => {
        throw new Error('not needed by fake MCP client')
      }),
    surfaceGroup: overrides.surfaceGroup,
    sensitiveTargetBindings: fakeSensitiveTargetBindings(
      overrides.surfaceGroup,
      overrides.preparedFileHandles
    ),
    toolTimeoutMs: overrides.toolTimeoutMs,
    closeSurface: overrides.closeSurface ?? vi.fn(async () => undefined),
    detachAutomation: overrides.detachAutomation ?? vi.fn(async () => undefined),
    createOfficialConnection:
      overrides.createOfficialConnection ??
      (async () => ({
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined)
      })),
    createClient:
      overrides.createClient ??
      (() => ({
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined),
        listTools: overrides.listTools ?? vi.fn(async () => ({ tools: upstreamTools })),
        callTool:
          overrides.callTool ??
          vi.fn(async () => ({ content: [{ type: 'text', text: 'ok' }], isError: false }))
      }))
  })
  trackedHosts.add(host)
  return host
}

function sensitiveTargetFromSurfaces(surfaces: ReturnType<typeof surfaceView>[]) {
  const surface = surfaces.find((candidate) => candidate.isActive)
  if (!surface) return null
  try {
    return {
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      navigationEpoch: 1,
      origin: new URL(surface.url).origin
    }
  } catch {
    return null
  }
}

function fakeSensitiveTargetBindings(
  surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter | undefined,
  preparedFileHandles?: readonly string[]
): ManagedPlaywrightSensitiveTargetBindingStore {
  const consumed = new Set<string>()
  return {
    acquire: (authorization) => {
      const grant = authorization.builtinToolGrant
      const target = surfaceGroup?.getSensitiveTargetIdentity()
      const profileScoped = grant?.origin === null
      if (!grant || (!profileScoped && !target)) {
        throw new ManagedPlaywrightSensitiveTargetBindingError('origin_drifted')
      }
      if (consumed.has(grant.targetBindingId)) {
        throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
      }
      let dispatched = false
      return {
        ...(target && !profileScoped ? { target: Object.freeze({ ...target }) } : {}),
        ...(preparedFileHandles ? { preparedFileHandles } : {}),
        markDispatched: () => {
          if (dispatched || consumed.has(grant.targetBindingId)) {
            throw new ManagedPlaywrightSensitiveTargetBindingError('reused')
          }
          dispatched = true
          consumed.add(grant.targetBindingId)
        },
        finish: () => undefined
      }
    },
    releaseRun: () => 0,
    releaseToolCall: () => 0
  }
}

function riskLease(options: {
  check?: (input: unknown) => Promise<void>
  failure?: BrowserRiskFailure | (() => BrowserRiskFailure | undefined)
  settle?: () => Promise<void>
}): {
  finish: ReturnType<typeof vi.fn>
  lease: BrowserNetworkOperationLease
  markDispatched: ReturnType<typeof vi.fn>
  settle: ReturnType<typeof vi.fn>
} {
  const finish = vi.fn()
  const markDispatched = vi.fn()
  const settle = vi.fn(options.settle ?? (async () => undefined))
  const lease = {
    preflight: options.check ?? vi.fn(async () => undefined),
    operation: {
      check: options.check ?? vi.fn(async () => undefined)
    },
    failure: () =>
      (typeof options.failure === 'function' ? options.failure() : options.failure) ?? null,
    markDispatched,
    settle,
    artifacts: () => [],
    finish
  } as unknown as BrowserNetworkOperationLease
  return { finish, lease, markDispatched, settle }
}
