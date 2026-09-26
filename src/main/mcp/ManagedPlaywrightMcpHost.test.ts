import { afterEach, describe, expect, it, vi } from 'vitest'
import { access, stat } from 'node:fs/promises'
import { isAbsolute } from 'node:path'
import {
  ManagedPlaywrightMcpHost,
  ManagedPlaywrightMcpHostError,
  type ManagedPlaywrightConnectionFactory
} from './ManagedPlaywrightMcpHost'
import {
  MANAGED_PLAYWRIGHT_MANIFEST,
  MANAGED_PLAYWRIGHT_PACKAGE_VERSION
} from './managedPlaywrightManifest'
import { MANAGED_PLAYWRIGHT_CAPABILITIES } from './managedPlaywrightCatalog'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

const { closeTrackedHosts, trackedHosts, singleSurfaceGroup, fakeHost } =
  createManagedPlaywrightHostTestFixture()

afterEach(closeTrackedHosts)

describe('ManagedPlaywrightMcpHost', () => {
  it('discovers the exact official 0.0.79 server without resolving a browser context', async () => {
    const getBrowserContext = vi.fn(async () => {
      throw new Error('context getter must stay lazy during discovery')
    })
    const detachAutomation = vi.fn(async () => undefined)
    const host = new ManagedPlaywrightMcpHost({
      getBrowserContext,
      surfaceGroup: singleSurfaceGroup(),
      closeSurface: vi.fn(async () => undefined),
      detachAutomation
    })
    trackedHosts.add(host)

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
      allowUnrestrictedFileAccess: true,
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

  it('fails closed when the fixed upstream catalog drifts', async () => {
    const host = fakeHost({
      listTools: vi.fn(async () => ({ tools: [] }))
    })
    await expect(host.listTools()).rejects.toEqual(
      new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
    )
  })
})
