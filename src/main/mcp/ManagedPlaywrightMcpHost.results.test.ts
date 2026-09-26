import { afterEach, describe, expect, it, vi } from 'vitest'
import { type ManagedMcpClient } from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const { closeTrackedHosts, fakeHost } = createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
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
})
