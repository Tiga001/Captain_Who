import { createServer } from 'node:http'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createInterface } from 'node:readline'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  BROWSER_WEBVIEW_PARTITION,
  createBrowserSurfaceBootstrapUrl,
  MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD,
  MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
  parseBrowserRiskAuthorizeOutput,
  parseManagedPlaywrightCancelNotification,
  parseManagedPlaywrightCommandNotification,
  type BrowserRiskAuthorizeInput,
  type BrowserRiskAuthorizeOutput,
  type BrowserRiskCancelInput,
  type BrowserSurfaceCommand,
  type ManagedPlaywrightCancelNotification,
  type ManagedPlaywrightCommandNotification,
  type ManagedPlaywrightCompletionInput
} from '@mycopilot/protocol'
import { app, BrowserWindow, session, type WebContents } from 'electron'

import { BrowserSurfaceManager } from '../BrowserSurfaceManager'
import { BrowserTargetBroker } from '../BrowserTargetBroker'
import { BrowserNetworkGuard } from '../BrowserNetworkGuard'
import { BrowserNetworkPolicy, ElectronSessionDnsResolver } from '../BrowserNetworkPolicy'
import { BrowserRiskCoordinator } from '../BrowserRiskCoordinator'
import {
  CoreBrowserRiskAuthorizer,
  type BrowserRiskAuthorizationCore
} from '../CoreBrowserRiskAuthorizer'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from '../../webviews/managedWebviewSecurity'
import {
  createManagedPlaywrightHostFactory,
  ManagedPlaywrightBridgeHost,
  type ManagedPlaywrightBridgeCore
} from '../../mcp/ManagedPlaywrightBridgeHost'

const READY_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_READY='
const COMPLETION_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_COMPLETION='
const RESULT_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_RESULT='
const RISK_AUTHORIZE_MARKER = 'MYCOPILOT_BROWSER_RISK_AUTHORIZE='
const RISK_CANCEL_MARKER = 'MYCOPILOT_BROWSER_RISK_CANCEL='
const RISK_DECISION_METHOD = 'fixture.browserRisk.decision'
const MAX_INPUT_LINE_BYTES = 8 * 1024 * 1024
const PROFILE_DIRECTORY = mkdtempSync(join(tmpdir(), 'mycopilot-managed-playwright-profile-'))
const SURFACE_ID = 'right-sidebar-managed-playwright-fixture'

app.setPath('userData', PROFILE_DIRECTORY)

/**
 * Test-process adapter for the production reverse bridge.
 *
 * Rust writes the same strict notifications that Core sends to Electron Main. This adapter only
 * supplies process framing: the production BridgeHost, official Playwright MCP Host, Surface
 * Manager, broker, transport and Electron guest remain unchanged.
 */
class JsonLineBridgeCore implements ManagedPlaywrightBridgeCore, BrowserRiskAuthorizationCore {
  private readonly cancelHandlers = new Set<(input: ManagedPlaywrightCancelNotification) => void>()
  private readonly commandHandlers = new Set<
    (input: ManagedPlaywrightCommandNotification) => void
  >()
  private readonly riskRequests = new Map<
    string,
    { resolve: (output: BrowserRiskAuthorizeOutput) => void }
  >()

  async completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<void> {
    await writeProtocolLine(`${COMPLETION_MARKER}${JSON.stringify(input)}`)
  }

  async authorizeBrowserRisk(
    input: BrowserRiskAuthorizeInput
  ): Promise<BrowserRiskAuthorizeOutput> {
    if (this.riskRequests.has(input.requestId)) {
      throw new Error('duplicate fixture Browser risk request')
    }
    const pending = new Promise<BrowserRiskAuthorizeOutput>((resolve) => {
      this.riskRequests.set(input.requestId, { resolve })
    })
    await writeProtocolLine(`${RISK_AUTHORIZE_MARKER}${JSON.stringify(input)}`)
    return await pending
  }

  async cancelBrowserRisk(input: BrowserRiskCancelInput): Promise<boolean> {
    const pending = this.riskRequests.get(input.requestId)
    if (pending) {
      this.riskRequests.delete(input.requestId)
      pending.resolve({
        schemaVersion: 1,
        decision: 'cancelled',
        grantId: null,
        reason: null
      })
    }
    await writeProtocolLine(`${RISK_CANCEL_MARKER}${JSON.stringify(input)}`)
    return pending !== undefined
  }

  onManagedPlaywrightCancel(
    handler: (input: ManagedPlaywrightCancelNotification) => void
  ): () => void {
    this.cancelHandlers.add(handler)
    return () => this.cancelHandlers.delete(handler)
  }

  onManagedPlaywrightCommand(
    handler: (input: ManagedPlaywrightCommandNotification) => void
  ): () => void {
    this.commandHandlers.add(handler)
    return () => this.commandHandlers.delete(handler)
  }

  acceptLine(line: string): void {
    if (Buffer.byteLength(line, 'utf8') > MAX_INPUT_LINE_BYTES) {
      throw new Error('managed Playwright fixture input exceeded its limit')
    }
    const envelope = expectRecord(JSON.parse(line))
    if (envelope.jsonrpc !== '2.0' || typeof envelope.method !== 'string') {
      throw new Error('managed Playwright fixture received an invalid notification')
    }
    if (envelope.method === MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD) {
      const input = parseManagedPlaywrightCommandNotification(envelope.params)
      process.stderr.write(`managed fixture command: ${input.command.type}\n`)
      for (const handler of this.commandHandlers) handler(input)
      return
    }
    if (envelope.method === MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD) {
      const input = parseManagedPlaywrightCancelNotification(envelope.params)
      for (const handler of this.cancelHandlers) handler(input)
      return
    }
    if (envelope.method === RISK_DECISION_METHOD) {
      const params = expectRecord(envelope.params)
      if (typeof params.requestId !== 'string') {
        throw new Error('managed Playwright fixture received an invalid risk decision id')
      }
      const output = parseBrowserRiskAuthorizeOutput(params.output)
      const pending = this.riskRequests.get(params.requestId)
      if (!pending) return
      this.riskRequests.delete(params.requestId)
      pending.resolve(output)
      return
    }
    throw new Error('managed Playwright fixture received an unsupported notification')
  }
}

async function main(): Promise<void> {
  if (process.argv.some((argument) => argument.startsWith('--remote-debugging-port'))) {
    throw new Error('remote debugging port is forbidden')
  }

  await app.whenReady()
  const core = new JsonLineBridgeCore()
  const managedSession = session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  const networkPolicy = new BrowserNetworkPolicy({
    dnsResolver: new ElectronSessionDnsResolver(managedSession)
  })
  const networkGuard = new BrowserNetworkGuard({
    coordinator: new BrowserRiskCoordinator({
      authorizer: new CoreBrowserRiskAuthorizer(core),
      policy: networkPolicy
    }),
    expectedSession: managedSession,
    policy: networkPolicy
  })
  initializeManagedWebviewSessions({ networkGuard })
  const rejectedServer = createServer((_request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    response.end('<!doctype html><html><body><h1>Rejected destination</h1></body></html>')
  })
  await listenOnLoopback(rejectedServer)
  const rejectedAddress = rejectedServer.address()
  if (!rejectedAddress || typeof rejectedAddress === 'string') {
    throw new Error('local rejected fixture did not bind')
  }
  const rejectedUrl = `http://127.0.0.1:${rejectedAddress.port}/rejected`

  const redirectTargetServer = createServer((_request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    response.end('<!doctype html><html><body><h1>Redirect target</h1></body></html>')
  })
  await listenOnLoopback(redirectTargetServer)
  const redirectTargetAddress = redirectTargetServer.address()
  if (!redirectTargetAddress || typeof redirectTargetAddress === 'string') {
    throw new Error('local redirect target fixture did not bind')
  }
  const redirectTargetUrl = `http://127.0.0.1:${redirectTargetAddress.port}/target`

  const server = createServer((request, response) => {
    if (request.url === '/redirect') {
      response.statusCode = 302
      response.setHeader('location', redirectTargetUrl)
      response.end()
      return
    }
    response.setHeader('content-type', 'text/html; charset=utf-8')
    response.end(`<!doctype html>
      <html><body>
        <main>
          <h1>Managed Playwright Bridge Fixture</h1>
          <label for="message">Message</label>
          <input id="message" />
          <button id="apply">Apply</button>
          <output id="output">idle</output>
        </main>
        <script>
          document.querySelector('#apply').addEventListener('click', () => {
            document.querySelector('#output').textContent =
              'applied:' + document.querySelector('#message').value
          })
        </script>
      </body></html>`)
  })
  await listenOnLoopback(server)
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('local fixture did not bind')
  const fixtureUrl = `http://127.0.0.1:${address.port}/interactive`
  const redirectUrl = `http://127.0.0.1:${address.port}/redirect`

  const window = new BrowserWindow({
    height: 320,
    opacity: 0,
    show: true,
    skipTaskbar: true,
    width: 480,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      webviewTag: true
    }
  })
  await window.loadURL(
    'data:text/html;charset=utf-8,<html><body><div id="host"></div></body></html>'
  )

  const broker = new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, managedSession)
  let pendingEnsure: Extract<BrowserSurfaceCommand, { kind: 'ensureAttached' }> | undefined
  let guest: WebContents | undefined
  let ensureCommands = 0
  let closeCommands = 0
  const manager = new BrowserSurfaceManager({
    attachTimeoutMs: 10_000,
    broker,
    networkGuard,
    closeTimeoutMs: 5_000,
    resolveHost: () => window.webContents,
    sendCommand: (_host, command) => {
      if (command.kind === 'ensureAttached') {
        process.stderr.write('managed fixture surface: ensureAttached\n')
        ensureCommands += 1
        pendingEnsure = command
        void ensureFixtureSurface(window, Boolean(guest && !guest.isDestroyed())).then(
          (alreadyAttached) => {
            if (alreadyAttached && pendingEnsure?.requestId === command.requestId) {
              manager.attach(window.webContents, {
                schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
                requestId: command.requestId,
                surfaceId: SURFACE_ID
              })
              pendingEnsure = undefined
            }
          },
          () => undefined
        )
        return
      }
      closeCommands += 1
      void removeFixtureSurface(window, command.surfaceId)
    }
  })
  configureManagedWebviewHost(window.webContents, { targetRegistry: manager })
  window.webContents.on('did-attach-webview', (_event, attachedGuest) => {
    process.stderr.write('managed fixture surface: did-attach-webview\n')
    guest = attachedGuest
    const command = pendingEnsure
    if (!command) return
    manager.attach(window.webContents, {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    pendingEnsure = undefined
  })

  const bridgeHost = new ManagedPlaywrightBridgeHost({
    core,
    createHost: createManagedPlaywrightHostFactory({
      getBrowserContext: async () => {
        process.stderr.write('managed fixture surface: getBrowserContext start\n')
        const context = await manager.getBrowserContext()
        process.stderr.write('managed fixture surface: getBrowserContext ready\n')
        return context
      },
      closeSurface: () => manager.closeSurface(),
      detachAutomation: () => manager.detachAutomation(),
      beginNetworkOperation: (input) => manager.beginNetworkOperation(input)
    })
  })
  const input = createInterface({ input: process.stdin })
  let inputFailure: Error | undefined
  const inputClosed = new Promise<void>((resolve) => {
    input.on('line', (line) => {
      try {
        core.acceptLine(line)
      } catch (error) {
        inputFailure = new Error(
          `managed Playwright fixture rejected its input: ${error instanceof Error ? error.message : 'unknown'}`
        )
        input.close()
      }
    })
    input.once('close', resolve)
  })

  try {
    await writeProtocolLine(
      `${READY_MARKER}${JSON.stringify({ fixtureUrl, redirectUrl, rejectedUrl, schemaVersion: 1 })}`
    )
    await inputClosed
    if (inputFailure) throw inputFailure
  } finally {
    await bridgeHost.close().catch(() => undefined)
    await manager.shutdown().catch(() => undefined)
    await writeProtocolLine(
      `${RESULT_MARKER}${JSON.stringify({
        broker: broker.snapshot(),
        closeCommands,
        ensureCommands,
        mainWindowAlive: !window.isDestroyed(),
        riskGuard: networkGuard.snapshot(),
        targetClosed: !guest || guest.isDestroyed()
      })}`
    ).catch(() => undefined)
    if (!window.isDestroyed()) window.destroy()
    await Promise.all([
      closeServer(server),
      closeServer(rejectedServer),
      closeServer(redirectTargetServer)
    ])
    app.quit()
  }
}

async function ensureFixtureSurface(
  window: BrowserWindow,
  isAlreadyAttached: boolean
): Promise<boolean> {
  const bootstrapUrl = createBrowserSurfaceBootstrapUrl(SURFACE_ID)
  return await window.webContents.executeJavaScript(`(() => {
    const existing = document.querySelector('webview[data-surface-id=${JSON.stringify(SURFACE_ID)}]')
    if (existing) {
      existing.hidden = false
      return true
    }
    const webview = document.createElement('webview')
    webview.dataset.surfaceId = ${JSON.stringify(SURFACE_ID)}
    webview.setAttribute('partition', ${JSON.stringify(BROWSER_WEBVIEW_PARTITION)})
    webview.setAttribute('src', ${JSON.stringify(bootstrapUrl)})
    webview.style.width = '320px'
    webview.style.height = '220px'
    document.querySelector('#host').appendChild(webview)
    return ${JSON.stringify(isAlreadyAttached)}
  })()`)
}

async function removeFixtureSurface(window: BrowserWindow, surfaceId: string): Promise<void> {
  await window.webContents.executeJavaScript(`(() => {
    document.querySelector('webview[data-surface-id=${JSON.stringify(surfaceId)}]')?.remove()
  })()`)
}

async function writeProtocolLine(line: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    process.stdout.write(`${line}\n`, (error) => (error ? reject(error) : resolve()))
  })
}

async function listenOnLoopback(server: ReturnType<typeof createServer>): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
}

async function closeServer(server: ReturnType<typeof createServer>): Promise<void> {
  await new Promise<void>((resolve) => server.close(() => resolve()))
}

function expectRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('managed Playwright fixture expected an object')
  }
  return value as Record<string, unknown>
}

void main()
  .catch((error: unknown) => {
    process.stderr.write(
      `Managed Playwright bridge fixture failed: ${error instanceof Error ? error.message : 'unknown'}\n`
    )
    app.exit(1)
  })
  .finally(() => {
    rmSync(PROFILE_DIRECTORY, { force: true, recursive: true })
  })
