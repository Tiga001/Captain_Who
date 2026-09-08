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
  MANAGED_PLAYWRIGHT_POLICY_MANIFEST,
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
  it('describes visual fallback and authorized file inputs without changing upstream contracts', () => {
    const screenshot = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find(
      (tool) => tool.rawName === 'browser_take_screenshot'
    )!
    expect(screenshot.description).toContain('Prefer browser_snapshot and DOM targets')
    expect(screenshot.description).toContain('inspect the screenshot with read_image')
    expect(screenshot.description).toContain('viewport coordinates')
    expect(screenshot.description).toContain('refresh after navigation, scrolling, resizing')
    expect(screenshot.description).not.toContain("can't perform actions based on the screenshot")

    for (const rawName of ['browser_file_upload', 'browser_drop']) {
      const exposed = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find((tool) => tool.rawName === rawName)!
      const upstream = MANAGED_PLAYWRIGHT_CATALOG_LOCK.tools.find((tool) => tool.name === rawName)!
      const properties = exposed.inputSchema.properties as Record<string, Record<string, unknown>>
      const upstreamProperties = upstream.inputSchema.properties as typeof properties
      expect(exposed.description).toContain('authorized workspace-relative or absolute file paths')
      expect(properties.paths.description).toContain('browser-download:<uuid>')
      expect(properties.paths).toEqual({
        ...upstreamProperties.paths,
        description: properties.paths.description
      })
      expect(upstreamProperties.paths.description).not.toContain('browser-download:')
      expect(exposed.upstreamSchemaDigest).toBe(digestJson(upstream.inputSchema))
    }
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find((tool) => tool.rawName === 'browser_file_upload')!
        .description
    ).toContain('Omitting paths cancels the file chooser')
  })

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
    expect(MANAGED_PLAYWRIGHT_EXPOSED_TOOLS).toHaveLength(61)
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find((tool) => tool.rawName === 'browser_take_screenshot')
        ?.description
    ).toContain('read_image.path')
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter((tool) => tool.handlingMode === 'pass_through')
    ).toHaveLength(25)
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter((tool) => tool.handlingMode === 'host_adapted')
    ).toHaveLength(8)
    expect(
      MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter((tool) => tool.handlingMode === 'artifact_managed')
    ).toHaveLength(7)
    const approvalTools = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.filter(
      (tool) => tool.handlingMode === 'approval_required'
    )
    expect(approvalTools.map((tool) => tool.rawName).sort()).toEqual(
      [
        'browser_cookie_clear',
        'browser_cookie_delete',
        'browser_cookie_get',
        'browser_cookie_list',
        'browser_cookie_set',
        'browser_drop',
        'browser_evaluate',
        'browser_file_upload',
        'browser_localstorage_clear',
        'browser_localstorage_delete',
        'browser_localstorage_get',
        'browser_localstorage_list',
        'browser_localstorage_set',
        'browser_network_request',
        'browser_sessionstorage_clear',
        'browser_sessionstorage_delete',
        'browser_sessionstorage_get',
        'browser_sessionstorage_list',
        'browser_sessionstorage_set',
        'browser_set_storage_state',
        'browser_storage_state'
      ].sort()
    )
    expect(approvalTools).toHaveLength(21)
    for (const tool of approvalTools) {
      expect(tool.inputSchema.required).toEqual(expect.arrayContaining(['call_reason']))
      expect(tool.inputSchema).not.toHaveProperty('properties.approval_origin')
    }
    expect(MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.map((tool) => tool.rawName)).not.toContain(
      'browser_run_code_unsafe'
    )
    expect(
      MANAGED_PLAYWRIGHT_POLICY_MANIFEST.tools.find(
        (tool) => tool.rawName === 'browser_run_code_unsafe'
      )
    ).toMatchObject({ exposed: false, handlingMode: 'sandboxed' })

    const snapshot = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find(
      (tool) => tool.rawName === 'browser_snapshot'
    )
    const snapshotProperties = snapshot?.inputSchema.properties as Record<string, unknown>
    expect(snapshotProperties).toHaveProperty('call_reason')
    expect(snapshotProperties).toHaveProperty('filename')
    expect(snapshot?.inputSchema.required).toContain('call_reason')

    const tabs = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find((tool) => tool.rawName === 'browser_tabs')
    expect(tabs?.inputSchema).toMatchObject({
      properties: { action: { enum: ['list', 'new', 'close', 'select'] } },
      required: ['action', 'call_reason']
    })

    const consoleMessages = MANAGED_PLAYWRIGHT_EXPOSED_TOOLS.find(
      (tool) => tool.rawName === 'browser_console_messages'
    )
    expect(consoleMessages?.inputSchema).toMatchObject({
      properties: {
        level: { enum: ['error', 'warning', 'info', 'debug'], default: 'info' },
        all: { type: 'boolean' }
      },
      required: ['level', 'call_reason']
    })
    expect(consoleMessages?.inputSchema).toHaveProperty('properties.filename')
  })

  it('records machine-readable evidence for tools that cannot be safely equivalent in a guest', () => {
    const unsupported = new Map(
      MANAGED_PLAYWRIGHT_POLICY_MANIFEST.tools
        .filter((tool) => tool.handlingMode === 'unsupported')
        .map((tool) => [tool.rawName, tool] as const)
    )
    expect([...unsupported.keys()].sort()).toEqual(
      [
        'browser_annotate',
        'browser_resume',
        'browser_start_video',
        'browser_stop_video',
        'browser_video_chapter',
        'browser_video_hide_actions',
        'browser_video_show_actions'
      ].sort()
    )
    expect(unsupported.get('browser_annotate')).toMatchObject({
      exposed: false,
      reasonCode: 'upstream_external_dashboard_forbidden',
      constraints: expect.arrayContaining(['external_dashboard_process_forbidden'])
    })
    expect(unsupported.get('browser_resume')).toMatchObject({
      exposed: false,
      reasonCode: 'managed_guest_has_no_test_debugger_pause',
      constraints: expect.arrayContaining(['no_managed_pause_source'])
    })
    for (const name of ['browser_start_video', 'browser_stop_video']) {
      expect(unsupported.get(name)).toMatchObject({
        exposed: false,
        reasonCode: 'production_ffmpeg_not_bundled',
        constraints: expect.arrayContaining(['external_video_encoder_unavailable'])
      })
    }
    for (const name of [
      'browser_video_chapter',
      'browser_video_hide_actions',
      'browser_video_show_actions'
    ]) {
      expect(unsupported.get(name)).toMatchObject({
        exposed: false,
        reasonCode: 'managed_video_recording_unavailable',
        constraints: expect.arrayContaining(['managed_video_recording_unavailable'])
      })
    }
  })

  it('matches all upstream capability assignments from the fixed coreBundle', () => {
    const projectRequire = createRequire(import.meta.url)
    const fixedPlaywrightRequire = createRequire(projectRequire.resolve('@playwright/mcp'))
    expect(
      (fixedPlaywrightRequire('playwright-core/package.json') as { version: string }).version
    ).toBe(MANAGED_PLAYWRIGHT_CATALOG_LOCK.playwrightVersion)
    const coreBundle = fixedPlaywrightRequire('playwright-core/lib/coreBundle') as {
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
      exposedToolCount: 61,
      upstreamCatalogDigest:
        'sha256:6c24d29f58242f59fa4e53e46ff5216170a21358d613a8b5f7f5d323c0080fbf',
      policyDigest: 'sha256:cf4b0d4ba01ce5c695096ad4dca499ceb895fd3c08c6f638b5602c1a21dc6e0c'
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
      if (request.url === '/conformance-frame') {
        response.end(`<!doctype html><html><body>
          <label for="frame-subject">Frame Subject</label>
          <input id="frame-subject" />
          <div id="frame-body" role="textbox" aria-label="Frame Body" contenteditable></div>
          <div id="frame-chinese" role="textbox" aria-label="Frame Chinese" contenteditable></div>
          <output id="frame-subject-value">frame-subject:</output>
          <output id="frame-body-value">frame-body:</output>
          <output id="frame-chinese-value">frame-chinese:</output>
          <output id="frame-chinese-events">frame-chinese-events:</output>
          <output id="frame-events">frame-events:</output>
          <script>
            const events = []
            const body = document.querySelector('#frame-body')
            for (const name of ['keydown', 'beforeinput', 'input', 'keyup']) {
              body.addEventListener(name, event => {
                events.push(event.type)
                document.querySelector('#frame-events').textContent =
                  'frame-events:' + events.join(',')
              })
            }
            document.querySelector('#frame-subject').addEventListener('input', event => {
              document.querySelector('#frame-subject-value').textContent =
                'frame-subject:' + event.target.value
            })
            body.addEventListener('input', () => {
              document.querySelector('#frame-body-value').textContent =
                'frame-body:' + body.textContent
            })
            const chinese = document.querySelector('#frame-chinese')
            const chineseEvents = []
            for (const name of ['keydown', 'beforeinput', 'input', 'keyup']) {
              chinese.addEventListener(name, event => {
                chineseEvents.push(event.type)
                document.querySelector('#frame-chinese-events').textContent =
                  'frame-chinese-events:' + chineseEvents.join(',')
              })
            }
            chinese.addEventListener('input', () => {
              document.querySelector('#frame-chinese-value').textContent =
                'frame-chinese:' + chinese.textContent
            })
          </script>
        </body></html>`)
        return
      }
      response.end(`<!doctype html><html><body>
        <main>
          <h1>Official Conformance Fixture</h1>
          <label for="value">Value</label><input id="value" />
          <button id="apply">Apply</button><output id="result">idle</output>
          <iframe src="/conformance-frame"></iframe>
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
      expect(raw.finalSnapshot).toContain('frame-subject:iframe-subject')
      expect(external.finalSnapshot).toContain('frame-subject:iframe-subject')
      expect(raw.finalSnapshot).toContain('frame-body:ab')
      expect(external.finalSnapshot).toContain('frame-body:ab')
      expect(raw.finalSnapshot).toContain('frame-chinese:你好')
      expect(external.finalSnapshot).toContain('frame-chinese:你好')
      expect(raw.finalSnapshot).toContain('frame-chinese-events:beforeinput,input')
      expect(external.finalSnapshot).toContain('frame-chinese-events:beforeinput,input')
      expect(raw.finalSnapshot).toContain(
        'frame-events:keydown,beforeinput,input,keyup,keydown,beforeinput,input,keyup'
      )
      expect(external.finalSnapshot).toContain(
        'frame-events:keydown,beforeinput,input,keyup,keydown,beforeinput,input,keyup'
      )
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
  const frameSubjectRef = officialSnapshotRef(initial, 'Frame Subject')
  const frameBodyRef = officialSnapshotRef(initial, 'Frame Body')
  const frameChineseRef = officialSnapshotRef(initial, 'Frame Chinese')
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
  await client.callTool({
    name: 'browser_fill_form',
    arguments: {
      fields: [
        {
          target: frameSubjectRef,
          name: 'Frame Subject',
          type: 'textbox',
          value: 'iframe-subject'
        }
      ]
    }
  })
  await client.callTool({
    name: 'browser_type',
    arguments: { target: frameBodyRef, text: 'ab', slowly: true }
  })
  await client.callTool({
    name: 'browser_type',
    arguments: { target: frameChineseRef, text: '你好' }
  })
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
