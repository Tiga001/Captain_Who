import { afterEach, describe, expect, it, vi } from 'vitest'
import { access, stat } from 'node:fs/promises'
import { isAbsolute } from 'node:path'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import type {
  BrowserRiskAuthorizationContext,
  BrowserRiskFailure
} from '../browser/BrowserRiskCoordinator'

import {
  ManagedPlaywrightMcpHost,
  ManagedPlaywrightMcpHostError,
  type ManagedPlaywrightConnectionFactory,
  type ManagedPlaywrightMcpHostOptions,
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
    expect(tools).toHaveLength(31)
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
    expect(tools.find((tool) => tool.name === 'browser_snapshot')?.inputSchema).not.toHaveProperty(
      'properties.filename'
    )
    expect(tools.find((tool) => tool.name === 'browser_tabs')?.inputSchema).toMatchObject({
      properties: { action: { enum: ['list'] } },
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

  it('pins a no-codegen, image-omitting connection in a private ephemeral output directory', async () => {
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
        filename: '/tmp/forbidden.md',
        call_reason: 'Write a file.'
      })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
    await expect(
      host.callTool('browser_tabs', { action: 'new', call_reason: 'Open another tab.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
    await expect(
      host.callTool('browser_run_code_unsafe', { call_reason: 'Run arbitrary code.' })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.tool_not_reviewed' })

    await expect(
      host.callTool('browser_console_messages', {
        level: 'info',
        call_reason: 'Inspect console output.'
      })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })
    await expect(
      host.callTool('browser_console_messages', {
        level: 'warning',
        all: true,
        call_reason: 'Inspect console output.'
      })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.invalid_arguments' })

    await expect(
      host.callTool('browser_console_messages', {
        level: 'warning',
        call_reason: 'Inspect warnings.'
      })
    ).resolves.toMatchObject({ isError: false })
    expect(callTool).toHaveBeenLastCalledWith(
      {
        name: 'browser_console_messages',
        arguments: { level: 'warning' }
      },
      undefined,
      expect.objectContaining({ resetTimeoutOnProgress: false })
    )
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

  it('redacts credential-bearing network-list output at the Host boundary', async () => {
    const host = fakeHost({
      callTool: vi.fn(async () => ({
        content: [
          {
            type: 'text',
            text: [
              '### Result',
              '1. [GET] https://alice:password@example.test/api/items?token=query-canary#fragment-canary => [200] OK',
              'Authorization: bearer-header-canary',
              'Cookie: cookie-header-canary'
            ].join('\n')
          }
        ],
        isError: false
      }))
    })

    const result = await host.callTool('browser_network_requests', {
      static: false,
      call_reason: 'Inspect request failures.'
    })

    expect(result).toEqual({
      content: [
        {
          type: 'text',
          text: [
            '### Result',
            '1. [GET] https://example.test/api/items => [200] OK',
            'Authorization: [redacted]',
            'Cookie: [redacted]'
          ].join('\n')
        }
      ],
      isError: false
    })
    expect(JSON.stringify(result)).not.toMatch(
      /alice|password|query-canary|fragment-canary|bearer-header-canary|cookie-header-canary/
    )
  })

  it('redacts page-controlled console output and rejects non-text diagnostic results', async () => {
    const callTool = vi
      .fn()
      .mockResolvedValueOnce({
        content: [
          {
            type: 'text',
            text: [
              'Error: fetch https://alice:password@example.test/failure?token=query-canary#fragment-canary',
              'Set-Cookie: cookie-header-canary',
              'token=console-token-canary',
              'Authorization=Bearer console-bearer-canary'
            ].join('\n')
          }
        ],
        isError: false
      })
      .mockResolvedValueOnce({
        content: [{ type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' }],
        isError: false
      })
    const host = fakeHost({ callTool })

    const result = await host.callTool('browser_console_messages', {
      level: 'error',
      call_reason: 'Inspect page errors.'
    })
    expect(result).toEqual({
      content: [
        {
          type: 'text',
          text: [
            'Error: fetch https://example.test/failure',
            'Set-Cookie: [redacted]',
            'token=[redacted]',
            'Authorization=[redacted]'
          ].join('\n')
        }
      ],
      isError: false
    })
    expect(JSON.stringify(result)).not.toMatch(
      /alice|password|query-canary|fragment-canary|cookie-header-canary|console-token-canary|console-bearer-canary/
    )

    await expect(
      host.callTool('browser_console_messages', {
        level: 'error',
        call_reason: 'Inspect page errors.'
      })
    ).rejects.toMatchObject({ code: 'mcp.builtin_playwright.protocol_error' })
  })

  it('intercepts browser_close through the Host-owned surface and never sends Target.closeTarget', async () => {
    const closeSurface = vi.fn(async () => undefined)
    const callTool = vi.fn(async () => ({ content: [], isError: false }))
    const host = fakeHost({ callTool, closeSurface })

    await expect(
      host.callTool('browser_close', { call_reason: 'Close the managed browser page.' })
    ).resolves.toMatchObject({ isError: false })

    expect(closeSurface).toHaveBeenCalledOnce()
    expect(callTool).not.toHaveBeenCalled()
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

  it('fails closed when the fixed upstream catalog drifts', async () => {
    const host = fakeHost({
      listTools: vi.fn(async () => ({ tools: [] }))
    })
    await expect(host.listTools()).rejects.toEqual(
      new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
    )
  })
})

function fakeHost(overrides: {
  beginNetworkOperation?: ManagedPlaywrightMcpHostOptions['beginNetworkOperation']
  callTool?: ManagedMcpClient['callTool']
  closeSurface?: () => Promise<void>
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  detachAutomation?: () => Promise<void>
  listTools?: ManagedMcpClient['listTools']
}): ManagedPlaywrightMcpHost {
  const upstreamTools = MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: structuredClone(tool.inputSchema),
    annotations: structuredClone(tool.annotations ?? {})
  }))
  const host = new ManagedPlaywrightMcpHost({
    beginNetworkOperation: overrides.beginNetworkOperation,
    getBrowserContext: async () => {
      throw new Error('not needed by fake MCP client')
    },
    closeSurface: overrides.closeSurface ?? vi.fn(async () => undefined),
    detachAutomation: overrides.detachAutomation ?? vi.fn(async () => undefined),
    createOfficialConnection:
      overrides.createOfficialConnection ??
      (async () => ({
        connect: vi.fn(async () => undefined),
        close: vi.fn(async () => undefined)
      })),
    createClient: () => ({
      connect: vi.fn(async () => undefined),
      close: vi.fn(async () => undefined),
      listTools: overrides.listTools ?? vi.fn(async () => ({ tools: upstreamTools })),
      callTool:
        overrides.callTool ??
        vi.fn(async () => ({ content: [{ type: 'text', text: 'ok' }], isError: false }))
    })
  })
  trackedHosts.add(host)
  return host
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
    finish
  } as unknown as BrowserNetworkOperationLease
  return { finish, lease, markDispatched, settle }
}
