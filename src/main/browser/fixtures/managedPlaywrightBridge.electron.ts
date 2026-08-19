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
import { BrowserArtifactBroker } from '../BrowserArtifactBroker'
import { BrowserDownloadBroker } from '../BrowserDownloadBroker'
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
import { ManagedPlaywrightSensitiveTargetBindingBroker } from '../../mcp/ManagedPlaywrightSensitiveTargetBindingBroker'

const READY_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_READY='
const COMPLETION_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_COMPLETION='
const RESULT_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_RESULT='
const RISK_AUTHORIZE_MARKER = 'MYCOPILOT_BROWSER_RISK_AUTHORIZE='
const RISK_CANCEL_MARKER = 'MYCOPILOT_BROWSER_RISK_CANCEL='
const RISK_DECISION_METHOD = 'fixture.browserRisk.decision'
const MAX_INPUT_LINE_BYTES = 8 * 1024 * 1024
const PROFILE_DIRECTORY = mkdtempSync(join(tmpdir(), 'mycopilot-managed-playwright-profile-'))
const SURFACE_ID = 'right-sidebar-managed-playwright-fixture'
let surfaceSequence = 0
let profileDirectoryRemoved = false

app.setPath('userData', PROFILE_DIRECTORY)
app.once('quit', removeFixtureProfileDirectory)
process.once('exit', removeFixtureProfileDirectory)

function removeFixtureProfileDirectory(): void {
  if (profileDirectoryRemoved) return
  profileDirectoryRemoved = true
  rmSync(PROFILE_DIRECTORY, { force: true, recursive: true })
}

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

  async completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<boolean> {
    await writeProtocolLine(`${COMPLETION_MARKER}${JSON.stringify(input)}`)
    return true
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
  const artifactBroker = new BrowserArtifactBroker({
    rootDirectory: join(PROFILE_DIRECTORY, 'browser-automation-artifacts')
  })
  const downloadBroker = new BrowserDownloadBroker({
    artifacts: artifactBroker,
    expectedSession: managedSession
  })
  const networkPolicy = new BrowserNetworkPolicy({
    dnsResolver: new ElectronSessionDnsResolver(managedSession)
  })
  const networkGuard = new BrowserNetworkGuard({
    accessPolicy: 'host_boundaries_only',
    coordinator: new BrowserRiskCoordinator({
      authorizer: new CoreBrowserRiskAuthorizer(core),
      policy: networkPolicy
    }),
    expectedSession: managedSession,
    downloadBroker,
    policy: networkPolicy
  })
  initializeManagedWebviewSessions({ networkGuard })
  const server = createServer((request, response) => {
    if (request.url?.startsWith('/api/ping')) {
      response.setHeader('content-type', 'application/json; charset=utf-8')
      response.end(JSON.stringify({ ok: true }))
      return
    }
    if (request.url === '/download') {
      response.setHeader('content-type', 'text/plain; charset=utf-8')
      response.setHeader('content-disposition', 'attachment; filename="fixture-download.txt"')
      response.end('repository-owned download fixture')
      return
    }
    response.setHeader('content-type', 'text/html; charset=utf-8')
    if (request.url === '/secondary') {
      response.end(`<!doctype html>
        <html><body><main><h1>Secondary Fixture Page</h1></main></body></html>`)
      return
    }
    if (request.url === '/mail-frame') {
      response.end(`<!doctype html>
        <html><body>
          <label for="frame-subject">Frame Subject</label>
          <input id="frame-subject" />
          <div id="slow-editor" contenteditable></div>
          <div id="mail-body" contenteditable></div>
          <output id="frame-status">frame-idle</output>
          <output id="subject-value">subject:</output>
          <output id="body-value">body:</output>
          <output id="body-events">body-events:</output>
          <output id="slow-events">slow-events:</output>
          <script>
            const slowEvents = []
            const slowEditor = document.querySelector('#slow-editor')
            for (const eventName of ['keydown', 'beforeinput', 'input', 'keyup']) {
              slowEditor.addEventListener(eventName, event => {
                slowEvents.push(event.type)
                document.querySelector('#slow-events').textContent =
                  'slow-events:' + slowEvents.join(',')
              })
            }
            document.addEventListener('focusin', event => {
              document.querySelector('#frame-status').textContent =
                'focused:' + (event.target.id || 'unknown')
            })
            document.querySelector('#frame-subject').addEventListener('input', event => {
              document.querySelector('#subject-value').textContent =
                'subject:' + event.target.value
            })
            document.querySelector('#mail-body').addEventListener('input', event => {
              document.querySelector('#body-value').textContent =
                'body:' + event.target.textContent
              document.querySelector('#frame-status').textContent =
                'focused:' + (document.activeElement.id || 'unknown')
            })
            const bodyEvents = []
            for (const eventName of ['keydown', 'beforeinput', 'input', 'keyup']) {
              document.querySelector('#mail-body').addEventListener(eventName, event => {
                bodyEvents.push(event.type)
                document.querySelector('#body-events').textContent =
                  'body-events:' + bodyEvents.join(',')
              })
            }
          </script>
        </body></html>`)
      return
    }
    response.end(`<!doctype html>
      <html><body>
        <main>
          <h1>Managed Playwright Bridge Fixture</h1>
          <label for="message">Message</label>
          <input id="message" />
          <label for="plan">Plan</label>
          <select id="plan">
            <option value="basic">Basic</option>
            <option value="pro">Pro</option>
          </select>
          <button id="apply">Apply</button>
          <button id="dialog">Show dialog</button>
          <a href="/download" download="fixture-download.txt">Download fixture</a>
          <div id="drag-source" role="button" tabindex="0" draggable="true">Drag source</div>
          <div id="drop-target" role="region" aria-label="Drop target">Drop target</div>
          <ul aria-label="Visible items"><li>Alpha</li><li>Beta</li></ul>
          <output id="output">idle</output>
          <iframe id="mail-frame" src="/mail-frame"></iframe>
        </main>
        <script>
          console.warn('managed fixture warning')
          fetch('/api/ping?token=fixture-query-canary').catch(() => undefined)
          document.querySelector('#apply').addEventListener('click', () => {
            document.querySelector('#output').textContent =
              'applied:' + document.querySelector('#message').value
          })
          document.querySelector('#dialog').addEventListener('click', () => {
            alert('managed fixture dialog')
          })
          document.querySelector('#drag-source').addEventListener('dragstart', event => {
            event.dataTransfer.setData('text/plain', 'fixture-drag')
          })
          document.querySelector('#drop-target').addEventListener('dragover', event => {
            event.preventDefault()
          })
          document.querySelector('#drop-target').addEventListener('drop', event => {
            event.preventDefault()
            document.querySelector('#output').textContent =
              event.dataTransfer.getData('text/plain') === 'fixture-drag' ? 'dragged' : 'bad-drag'
          })
        </script>
      </body></html>`)
  })
  await listenOnLoopback(server)
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('local fixture did not bind')
  const fixtureUrl = `http://127.0.0.1:${address.port}/interactive`
  const secondaryUrl = `http://127.0.0.1:${address.port}/secondary`

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
  let pendingEnsure:
    Extract<BrowserSurfaceCommand, { kind: 'ensureAttached' | 'createSurface' }> | undefined
  let guest: WebContents | undefined
  const guests = new Map<string, WebContents>()
  let ensureCommands = 0
  let closeCommands = 0
  const manager = new BrowserSurfaceManager({
    attachTimeoutMs: 10_000,
    broker,
    networkGuard,
    closeTimeoutMs: 5_000,
    createSurfaceId: () =>
      surfaceSequence++ === 0 ? SURFACE_ID : `right-sidebar-managed-playwright-${surfaceSequence}`,
    resolveHost: () => window.webContents,
    sendCommand: (_host, command) => {
      if (command.kind === 'ensureAttached' || command.kind === 'createSurface') {
        ensureCommands += 1
        pendingEnsure = command
        const existingGuest = guests.get(command.surfaceId)
        void ensureFixtureSurface(
          window,
          command.surfaceId,
          Boolean(existingGuest && !existingGuest.isDestroyed())
        ).then(
          (alreadyAttached) => {
            if (alreadyAttached && pendingEnsure?.requestId === command.requestId) {
              manager.attach(window.webContents, {
                schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
                requestId: command.requestId,
                surfaceId: command.surfaceId
              })
              pendingEnsure = undefined
            }
          },
          () => undefined
        )
        return
      }
      if (command.kind === 'selectSurface' || command.kind === 'resizeSurface') {
        void applyFixtureSurfaceCommand(window, command).then((viewport) => {
          manager.attach(window.webContents, {
            schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
            requestId: command.requestId,
            surfaceId: command.surfaceId,
            ...(viewport ? { viewport } : {})
          })
        })
        return
      }
      closeCommands += 1
      void removeFixtureSurface(window, command.surfaceId)
    }
  })
  configureManagedWebviewHost(window.webContents, {
    networkGuard,
    targetRegistry: manager
  })
  window.webContents.on('did-attach-webview', (_event, attachedGuest) => {
    guest = attachedGuest
    const command = pendingEnsure
    if (!command) return
    guests.set(command.surfaceId, attachedGuest)
    attachedGuest.once('destroyed', () => guests.delete(command.surfaceId))
    manager.attach(window.webContents, {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      requestId: command.requestId,
      surfaceId: command.surfaceId
    })
    pendingEnsure = undefined
  })

  const sensitiveTargetBindings = new ManagedPlaywrightSensitiveTargetBindingBroker({
    beginDispatchFence: (target) => manager.beginSensitiveDispatchFence(target),
    getActiveTarget: () => manager.getSensitiveTargetIdentity()
  })
  const bridgeHost = new ManagedPlaywrightBridgeHost({
    core,
    sensitiveTargetBindings,
    createHost: createManagedPlaywrightHostFactory({
      getBrowserContext: () => manager.getBrowserContext(),
      closeSurface: () => manager.closeSurface(),
      detachAutomation: () => manager.detachAutomation(),
      beginNetworkOperation: (input) => manager.beginNetworkOperation(input),
      artifactBroker,
      finalizeBrowserRun: (runId) => networkGuard.finalizeRun(runId),
      getActiveSurfaceIdentity: () => manager.getActiveSurfaceIdentity(),
      sensitiveTargetBindings,
      releaseBrowserCapability: (activationId) => networkGuard.releaseCapability(activationId),
      releaseBrowserToolCall: (input) => networkGuard.releaseToolCall(input),
      surfaceGroup: manager
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
      `${READY_MARKER}${JSON.stringify({ fixtureUrl, schemaVersion: 1, secondaryUrl })}`
    )
    await inputClosed
    if (inputFailure) throw inputFailure
  } finally {
    await bridgeHost.close().catch(() => undefined)
    await manager.shutdown().catch(() => undefined)
    await artifactBroker.shutdown().catch(() => undefined)
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
    await closeServer(server)
    app.quit()
  }
}

async function ensureFixtureSurface(
  window: BrowserWindow,
  surfaceId: string,
  isAlreadyAttached: boolean
): Promise<boolean> {
  const bootstrapUrl = createBrowserSurfaceBootstrapUrl(surfaceId)
  return await window.webContents.executeJavaScript(`(() => {
    const existing = document.querySelector('webview[data-surface-id=${JSON.stringify(surfaceId)}]')
    if (existing) {
      existing.hidden = false
      return true
    }
    const webview = document.createElement('webview')
    webview.dataset.surfaceId = ${JSON.stringify(surfaceId)}
    webview.setAttribute('partition', ${JSON.stringify(BROWSER_WEBVIEW_PARTITION)})
    webview.setAttribute('src', ${JSON.stringify(bootstrapUrl)})
    webview.style.width = '320px'
    webview.style.height = '220px'
    document.querySelector('#host').appendChild(webview)
    return ${JSON.stringify(isAlreadyAttached)}
  })()`)
}

async function applyFixtureSurfaceCommand(
  window: BrowserWindow,
  command: Extract<BrowserSurfaceCommand, { kind: 'selectSurface' | 'resizeSurface' }>
): Promise<{ height: number; width: number } | undefined> {
  return await window.webContents.executeJavaScript(`(() => {
    const webview = document.querySelector('webview[data-surface-id=${JSON.stringify(command.surfaceId)}]')
    if (!webview) throw new Error('managed surface missing')
    webview.hidden = false
    ${
      command.kind === 'resizeSurface'
        ? `webview.style.width = ${JSON.stringify(`${command.width}px`)};
           webview.style.height = ${JSON.stringify(`${command.height}px`)};`
        : ''
    }
    if (${JSON.stringify(command.kind)} !== 'resizeSurface') return undefined
    const bounds = webview.getBoundingClientRect()
    return {
      height: Math.round(Math.max(0, Math.min(bounds.bottom, window.innerHeight) - Math.max(bounds.top, 0))),
      width: Math.round(Math.max(0, Math.min(bounds.right, window.innerWidth) - Math.max(bounds.left, 0)))
    }
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

void main().catch(() => app.exit(1))
