import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js'
import { createConnection } from '@playwright/mcp'
import { chromium as officeChromium } from '@mycopilot/office-playwright-runtime'
import { chmod, mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { createRequire } from 'node:module'
import { afterEach, describe, expect, it } from 'vitest'

import {
  MANAGED_PLAYWRIGHT_CAPABILITIES,
  MANAGED_PLAYWRIGHT_CATALOG_LOCK,
  MANAGED_PLAYWRIGHT_EXPOSED_TOOLS,
  MANAGED_PLAYWRIGHT_TOOL_CAPABILITIES,
  digestJson,
  managedPlaywrightCatalogConformanceReport,
  modelSchemaForOfficialPlaywrightTool,
  validateAndIndexOfficialPlaywrightCatalog
} from './managedPlaywrightCatalog'

const cleanupDirectories = new Set<string>()

afterEach(async () => {
  const directories = [...cleanupDirectories]
  cleanupDirectories.clear()
  await Promise.allSettled(
    directories.map((directory) => rm(directory, { force: true, recursive: true }))
  )
})

describe('managed Playwright fixed Catalog', () => {
  it('locks all 69 upstream schemas while exposing only the reviewed Host-overlay subset', () => {
    expect(MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools).toHaveLength(69)
    expect(
      [...new Set(MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => tool.capability))].sort()
    ).toEqual([...MANAGED_PLAYWRIGHT_TOOL_CAPABILITIES].sort())
    expect(
      Object.fromEntries(
        MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools
          .filter((tool) =>
            [
              'browser_navigate',
              'browser_navigate_back',
              'browser_press_key',
              'browser_tabs',
              'browser_type'
            ].includes(tool.name)
          )
          .map((tool) => [tool.name, tool.capability])
      )
    ).toEqual({
      browser_navigate: 'core-navigation',
      browser_navigate_back: 'core-navigation',
      browser_press_key: 'core-input',
      browser_tabs: 'core-tabs',
      browser_type: 'core-input'
    })
    expect(MANAGED_PLAYWRIGHT_EXPOSED_TOOLS).toHaveLength(31)
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter((tool) => tool.handlingMode === 'pass_through')
    ).toHaveLength(26)
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter((tool) => tool.handlingMode === 'host_adapted')
    ).toHaveLength(2)
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter((tool) => tool.handlingMode === 'artifact_managed')
    ).toHaveLength(3)
    expect(MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.map((tool) => tool.rawName)).not.toContain(
      'browser_run_code_unsafe'
    )

    const snapshot = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find(
      (tool) => tool.rawName === 'browser_snapshot'
    )
    const snapshotProperties = snapshot?.inputSchema.properties as Record<string, unknown>
    expect(snapshotProperties).toHaveProperty('call_reason')
    expect(snapshotProperties).not.toHaveProperty('filename')
    expect(snapshot?.inputSchema.required).toContain('call_reason')

    const tabs = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find((tool) => tool.rawName === 'browser_tabs')
    expect(tabs?.inputSchema).toMatchObject({
      properties: { action: { enum: ['list'] } },
      required: ['action', 'call_reason']
    })

    const consoleMessages = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find(
      (tool) => tool.rawName === 'browser_console_messages'
    )
    expect(consoleMessages?.inputSchema).toMatchObject({
      properties: { level: { enum: ['error', 'warning'], default: 'warning' } },
      required: ['level', 'call_reason']
    })
    expect(consoleMessages?.inputSchema).not.toHaveProperty('properties.filename')
    expect(consoleMessages?.inputSchema).not.toHaveProperty('properties.all')
  })

  it('matches all upstream capability assignments from the fixed coreBundle', () => {
    const require = createRequire(import.meta.url)
    const coreBundle = require('playwright-core/lib/coreBundle') as {
      tools: {
        browserTools: Array<{
          capability: string
          skillOnly?: boolean
          schema: { name: string }
        }>
      }
    }
    const lockedNames = new Set(MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => tool.name))
    const selected = coreBundle.tools.browserTools.filter((tool) =>
      lockedNames.has(tool.schema.name)
    )
    const actual = Object.fromEntries(selected.map((tool) => [tool.schema.name, tool.capability]))
    const locked = Object.fromEntries(
      MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => [tool.name, tool.capability])
    )

    expect(lockedNames.size).toBe(69)
    expect(selected).toHaveLength(69)
    expect(new Set(selected.map((tool) => tool.schema.name)).size).toBe(69)
    expect(actual).toEqual(locked)

    // The pinned core source defines nine Skill-only helpers in addition to the real MCP
    // listTools Catalog. `filteredTools` deliberately removes these before publication.
    const sourceOnly = coreBundle.tools.browserTools.filter(
      (tool) => !lockedNames.has(tool.schema.name)
    )
    expect(sourceOnly).toHaveLength(9)
    expect(sourceOnly.every((tool) => tool.skillOnly === true)).toBe(true)
    expect(sourceOnly.map((tool) => tool.schema.name).sort()).toEqual(
      [
        'browser_check',
        'browser_console_clear',
        'browser_keydown',
        'browser_keyup',
        'browser_navigate_forward',
        'browser_network_clear',
        'browser_press_sequentially',
        'browser_reload',
        'browser_uncheck'
      ].sort()
    )
  })

  it('rejects overlays that remove required fields or rewrite structural schema', () => {
    const upstream = {
      type: 'object',
      properties: {
        action: { type: 'string', enum: ['list', 'new'], default: 'new' },
        target: { type: 'string' }
      },
      required: ['target'],
      additionalProperties: false
    }

    expect(() =>
      modelSchemaForOfficialPlaywrightTool(upstream, {
        addCallReason: true,
        removeProperties: ['target']
      })
    ).toThrow('catalog_drift')
    expect(() =>
      modelSchemaForOfficialPlaywrightTool(upstream, {
        addCallReason: true,
        propertyOverrides: { action: { type: 'number' } }
      })
    ).toThrow('catalog_drift')
    expect(() =>
      modelSchemaForOfficialPlaywrightTool(upstream, {
        addCallReason: true,
        propertyOverrides: { action: { enum: ['list', 'unknown'] } }
      })
    ).toThrow('catalog_drift')
  })

  it('fails closed on title or annotation drift as well as schema drift', () => {
    const live = MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.map((tool) => ({
      name: tool.name,
      description: tool.description,
      inputSchema: structuredClone(tool.inputSchema),
      annotations: structuredClone(tool.annotations ?? {})
    }))
    expect(() => validateAndIndexOfficialPlaywrightCatalog(live)).not.toThrow()
    expect(managedPlaywrightCatalogConformanceReport(live)).toEqual({
      schemaVersion: 1,
      packageName: '@playwright/mcp',
      packageVersion: '0.0.79',
      status: 'exact',
      upstreamToolCount: 69,
      exposedToolCount: 31,
      upstreamCatalogDigest:
        'sha256:6c24d29f58242f59fa4e53e46ff5216170a21358d613a8b5f7f5d323c0080fbf',
      policyDigest: 'sha256:5fede120ec5887791faae7c543b0df1c07883eb8350e71af522a6f6d05facc03'
    })

    const titleDrift = structuredClone(live)
    Object.assign(titleDrift[0], { title: 'Unexpected upstream title' })
    expect(() => validateAndIndexOfficialPlaywrightCatalog(titleDrift)).toThrow('catalog_drift')

    const outputSchemaDrift = structuredClone(live)
    Object.assign(outputSchemaDrift[0], { outputSchema: { type: 'object' }, _meta: {} })
    expect(() => validateAndIndexOfficialPlaywrightCatalog(outputSchemaDrift)).toThrow(
      'catalog_drift'
    )

    const annotationDrift = structuredClone(live)
    annotationDrift[0].annotations = {
      ...annotationDrift[0].annotations,
      destructiveHint: !annotationDrift[0].annotations.destructiveHint
    }
    expect(() => validateAndIndexOfficialPlaywrightCatalog(annotationDrift)).toThrow(
      'catalog_drift'
    )
  })

  it('matches RawOfficial createConnection discovery exactly without resolving a browser', async () => {
    const outputDirectory = await secureTemporaryDirectory('mycopilot-raw-playwright-catalog-')
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair()
    const server = await createConnection(officialConfig(outputDirectory), async () => {
      throw new Error('Catalog discovery must not resolve a BrowserContext')
    })
    const client = new Client(
      { name: 'mycopilot-raw-playwright-catalog-test', version: '1.0.0' },
      { capabilities: {} }
    )
    try {
      await Promise.all([server.connect(serverTransport), client.connect(clientTransport)])
      const catalog = await client.listTools()
      expect(catalog.tools).toHaveLength(69)
      expect(() => validateAndIndexOfficialPlaywrightCatalog(catalog.tools)).not.toThrow()
    } finally {
      await Promise.allSettled([
        client.close(),
        server.close(),
        clientTransport.close(),
        serverTransport.close()
      ])
    }
  })

  it('matches the fixed official stdio CLI ExternalReference Catalog without launching a user browser', async () => {
    const outputDirectory = await secureTemporaryDirectory('mycopilot-external-playwright-catalog-')
    const require = createRequire(import.meta.url)
    const packageRoot = dirname(require.resolve('@playwright/mcp/package.json'))
    const transport = new StdioClientTransport({
      command: process.execPath,
      args: [
        join(packageRoot, 'cli.js'),
        '--isolated',
        '--headless',
        '--browser=chromium',
        '--codegen=none',
        '--image-responses=omit',
        `--caps=${MANAGED_PLAYWRIGHT_CAPABILITIES.join(',')}`,
        `--executable-path=${officeChromium.executablePath()}`,
        `--output-dir=${outputDirectory}`
      ],
      stderr: 'pipe'
    })
    const client = new Client(
      { name: 'mycopilot-external-playwright-catalog-test', version: '1.0.0' },
      { capabilities: {} }
    )
    try {
      await client.connect(transport)
      const catalog = await client.listTools()
      expect(catalog.tools).toHaveLength(69)
      const indexed = validateAndIndexOfficialPlaywrightCatalog(catalog.tools)
      expect(indexed.size).toBe(69)
      expect(
        digestJson(
          [...indexed.values()]
            .map((tool) => ({ name: tool.name, schema: tool.inputSchema }))
            .sort((left, right) => left.name.localeCompare(right.name))
        )
      ).toBe(
        digestJson(
          MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools
            .map((tool) => ({
              name: tool.name,
              schema: tool.inputSchema
            }))
            .sort((left, right) => left.name.localeCompare(right.name))
        )
      )
    } finally {
      await Promise.allSettled([client.close(), transport.close()])
    }
  }, 30_000)

  it('keeps RawOfficial and ExternalReference behavior aligned on one isolated local fixture', async () => {
    const requestLedger: string[] = []
    const fixture = createServer((request, response) => {
      requestLedger.push(request.url ?? '')
      response.setHeader('content-type', 'text/html; charset=utf-8')
      response.end(`<!doctype html><html><body>
        <main>
          <h1>Official Conformance Fixture</h1>
          <label for="value">Value</label><input id="value" />
          <button id="apply">Apply</button><output id="result">idle</output>
        </main>
        <script>
          document.querySelector('#apply').addEventListener('click', () => {
            document.querySelector('#result').textContent =
              'applied:' + document.querySelector('#value').value
          })
        </script>
      </body></html>`)
    })
    await listenOnLoopback(fixture)
    const address = fixture.address()
    if (!address || typeof address === 'string') throw new Error('local fixture did not bind')
    const fixtureUrl = `http://127.0.0.1:${address.port}/conformance`
    const rawOutput = await secureTemporaryDirectory('mycopilot-raw-playwright-behavior-')
    const externalOutput = await secureTemporaryDirectory('mycopilot-external-playwright-behavior-')
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair()
    const rawServer = await createConnection(
      {
        ...officialConfig(rawOutput),
        browser: {
          isolated: true,
          launchOptions: { executablePath: officeChromium.executablePath(), headless: true }
        }
      },
      undefined
    )
    const rawClient = new Client(
      { name: 'mycopilot-raw-playwright-behavior-test', version: '1.0.0' },
      { capabilities: {} }
    )
    const require = createRequire(import.meta.url)
    const packageRoot = dirname(require.resolve('@playwright/mcp/package.json'))
    const externalTransport = new StdioClientTransport({
      command: process.execPath,
      args: [
        join(packageRoot, 'cli.js'),
        '--isolated',
        '--headless',
        '--browser=chromium',
        '--codegen=none',
        '--image-responses=omit',
        `--caps=${MANAGED_PLAYWRIGHT_CAPABILITIES.join(',')}`,
        `--executable-path=${officeChromium.executablePath()}`,
        `--output-dir=${externalOutput}`
      ],
      stderr: 'pipe'
    })
    const externalClient = new Client(
      { name: 'mycopilot-external-playwright-behavior-test', version: '1.0.0' },
      { capabilities: {} }
    )

    try {
      await Promise.all([
        rawServer.connect(serverTransport),
        rawClient.connect(clientTransport),
        externalClient.connect(externalTransport)
      ])
      const rawStart = requestLedger.length
      const raw = await runOfficialConformanceWorkflow(rawClient, fixtureUrl)
      const rawRequests = requestLedger.slice(rawStart)
      const externalStart = requestLedger.length
      const external = await runOfficialConformanceWorkflow(externalClient, fixtureUrl)
      const externalRequests = requestLedger.slice(externalStart)

      expect(raw.finalSnapshot).toContain('applied:conformance')
      expect(external.finalSnapshot).toContain('applied:conformance')
      expect(normalizeOfficialSnapshot(raw.finalSnapshot)).toBe(
        normalizeOfficialSnapshot(external.finalSnapshot)
      )
      expect(raw.invalidArgumentsRejected).toBe(true)
      expect(external.invalidArgumentsRejected).toBe(true)
      expect(rawRequests).toEqual(externalRequests)
    } finally {
      await Promise.allSettled([
        rawClient.callTool({ name: 'browser_close', arguments: {} }),
        externalClient.callTool({ name: 'browser_close', arguments: {} })
      ])
      await Promise.allSettled([
        rawClient.close(),
        rawServer.close(),
        clientTransport.close(),
        serverTransport.close(),
        externalClient.close(),
        externalTransport.close()
      ])
      fixture.closeAllConnections()
      await new Promise<void>((resolve) => fixture.close(() => resolve()))
    }
  }, 60_000)
})

function officialConfig(outputDirectory: string): Parameters<typeof createConnection>[0] {
  return {
    browser: { isolated: false },
    capabilities: [...MANAGED_PLAYWRIGHT_CAPABILITIES],
    codegen: 'none',
    imageResponses: 'omit',
    outputDir: outputDirectory,
    saveSession: false,
    sharedBrowserContext: true
  }
}

async function secureTemporaryDirectory(prefix: string): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), prefix))
  cleanupDirectories.add(directory)
  await chmod(directory, 0o700)
  return directory
}

async function listenOnLoopback(server: ReturnType<typeof createServer>): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
}

async function runOfficialConformanceWorkflow(
  client: Client,
  fixtureUrl: string
): Promise<{ finalSnapshot: string; invalidArgumentsRejected: boolean }> {
  await client.callTool({ name: 'browser_navigate', arguments: { url: fixtureUrl } })
  const initial = toolText(await client.callTool({ name: 'browser_snapshot', arguments: {} }))
  const inputRef = officialSnapshotRef(initial, 'Value')
  const buttonRef = officialSnapshotRef(initial, 'Apply')
  await client.callTool({
    name: 'browser_fill_form',
    arguments: {
      fields: [
        {
          target: inputRef,
          name: 'Value',
          type: 'textbox',
          value: 'conformance'
        }
      ]
    }
  })
  await client.callTool({ name: 'browser_click', arguments: { target: buttonRef } })
  await client.callTool({ name: 'browser_wait_for', arguments: { text: 'applied:conformance' } })
  const finalSnapshot = toolText(await client.callTool({ name: 'browser_snapshot', arguments: {} }))
  let invalidArgumentsRejected = false
  try {
    const invalid = await client.callTool({ name: 'browser_click', arguments: {} })
    invalidArgumentsRejected = invalid.isError === true
  } catch {
    invalidArgumentsRejected = true
  }
  return { finalSnapshot, invalidArgumentsRejected }
}

function toolText(result: Awaited<ReturnType<Client['callTool']>>): string {
  const content = (result as { content?: unknown }).content
  if (!Array.isArray(content)) throw new Error('official Tool result omitted content')
  return content
    .filter(
      (block): block is { type: 'text'; text: string } =>
        typeof block === 'object' &&
        block !== null &&
        (block as { type?: unknown }).type === 'text' &&
        typeof (block as { text?: unknown }).text === 'string'
    )
    .map((block) => block.text)
    .join('\n')
}

function officialSnapshotRef(snapshot: string, accessibleName: string): string {
  const line = snapshot
    .split('\n')
    .find((candidate) => candidate.includes(accessibleName) && candidate.includes('[ref='))
  if (!line) throw new Error(`official snapshot omitted ${accessibleName}`)
  const match = line.match(/\[ref=([^\]]+)\]/)
  if (!match) throw new Error(`official snapshot omitted the ${accessibleName} ref`)
  return match[1]
}

function normalizeOfficialSnapshot(snapshot: string): string {
  return snapshot.replace(/\s*\[ref=[^\]]+\]/g, '').trim()
}
