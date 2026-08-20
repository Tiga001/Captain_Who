import { createServer } from 'node:http'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
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
  type ManagedPlaywrightCompletionInput,
  type ManagedPlaywrightDispatchPhaseInput
} from '@mycopilot/protocol'
import { app, BrowserWindow, session, type WebContents } from 'electron'

import { BrowserSurfaceManager } from '../BrowserSurfaceManager'
import { BrowserArtifactBroker } from '../BrowserArtifactBroker'
import { BrowserDownloadBroker } from '../BrowserDownloadBroker'
import { BrowserFileBroker } from '../BrowserFileBroker'
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
const DISPATCH_PHASE_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_DISPATCH_PHASE='
const RESULT_MARKER = 'MYCOPILOT_MANAGED_PLAYWRIGHT_RESULT='
const RISK_AUTHORIZE_MARKER = 'MYCOPILOT_BROWSER_RISK_AUTHORIZE='
const RISK_CANCEL_MARKER = 'MYCOPILOT_BROWSER_RISK_CANCEL='
const RISK_DECISION_METHOD = 'fixture.browserRisk.decision'
const DISPATCH_PHASE_ACK_METHOD = 'fixture.managedPlaywright.dispatchPhaseAck'
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
  private readonly dispatchPhaseRequests = new Map<
    string,
    { resolve: (accepted: boolean) => void }
  >()

  async completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<boolean> {
    await writeProtocolLine(`${COMPLETION_MARKER}${JSON.stringify(input)}`)
    return true
  }

  async acknowledgeManagedPlaywrightDispatchPhase(
    input: ManagedPlaywrightDispatchPhaseInput
  ): Promise<boolean> {
    const key = dispatchPhaseRequestKey(input.requestId, input.phase)
    if (this.dispatchPhaseRequests.has(key)) {
      throw new Error('duplicate fixture managed Playwright dispatch phase')
    }
    const pending = new Promise<boolean>((resolve) => {
      this.dispatchPhaseRequests.set(key, { resolve })
    })
    await writeProtocolLine(`${DISPATCH_PHASE_MARKER}${JSON.stringify(input)}`)
    return await pending
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
    if (envelope.method === DISPATCH_PHASE_ACK_METHOD) {
      const params = expectRecord(envelope.params)
      if (
        typeof params.requestId !== 'string' ||
        (params.phase !== 'possibly_dispatched' && params.phase !== 'response_received') ||
        typeof params.accepted !== 'boolean'
      ) {
        throw new Error('managed Playwright fixture received an invalid dispatch phase ack')
      }
      const key = dispatchPhaseRequestKey(params.requestId, params.phase)
      const pending = this.dispatchPhaseRequests.get(key)
      if (!pending) return
      this.dispatchPhaseRequests.delete(key)
      pending.resolve(params.accepted)
      return
    }
    throw new Error('managed Playwright fixture received an unsupported notification')
  }
}

function dispatchPhaseRequestKey(requestId: string, phase: string): string {
  return `${requestId}:${phase}`
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
  const selectedFixturePath = join(PROFILE_DIRECTORY, 'fixture-upload.txt')
  const largeDropFixturePath = join(PROFILE_DIRECTORY, 'fixture-large-drop.bin')
  const storageStateFixturePath = join(PROFILE_DIRECTORY, 'fixture-storage-state.json')
  let fileSelectionCount = 0
  writeFileSync(selectedFixturePath, 'repository-owned upload fixture', { mode: 0o600 })
  writeFileSync(largeDropFixturePath, Buffer.alloc(1_100_000, 'L'), { mode: 0o600 })
  const selectedFixturePaths = [
    selectedFixturePath,
    selectedFixturePath,
    largeDropFixturePath,
    storageStateFixturePath
  ]
  const fileBroker = new BrowserFileBroker({
    rootDirectory: join(PROFILE_DIRECTORY, 'browser-automation-files'),
    selectionProvider: {
      selectFiles: async () => [
        selectedFixturePaths[Math.min(fileSelectionCount++, selectedFixturePaths.length - 1)]
      ]
    }
  })
  await fileBroker.initialize()
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
    if (request.method === 'POST' && request.url === '/api/upload') {
      const chunks: Buffer[] = []
      let sizeBytes = 0
      request.on('data', (chunk: Buffer) => {
        sizeBytes += chunk.byteLength
        if (sizeBytes > 1024 * 1024) {
          response.statusCode = 413
          response.end()
          request.destroy()
          return
        }
        chunks.push(chunk)
      })
      request.on('end', () => {
        if (response.writableEnded) return
        const multipart = Buffer.concat(chunks).toString('utf8')
        response.setHeader('content-type', 'application/json; charset=utf-8')
        response.end(
          JSON.stringify({
            contentMatched: multipart.includes(
              'repository-owned synthetic admissions presentation fixture'
            ),
            filenameMatched: multipart.includes('2026') && multipart.includes('.pptx')
          })
        )
      })
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
    if (request.url?.startsWith('/mail-frame')) {
      const crossFrame = request.url.includes('scope=cross')
      const subjectLabel = crossFrame ? 'Cross Frame Subject' : 'Frame Subject'
      const dropLabel = crossFrame ? 'Cross file drop target' : 'Frame file drop target'
      response.end(`<!doctype html>
        <html><body>
          <label for="frame-subject">${subjectLabel}</label>
          <input id="frame-subject" />
          <div id="slow-editor" contenteditable></div>
          <div id="mail-body" contenteditable></div>
          <div id="file-drop" role="region" aria-label="${dropLabel}">Drop fixture file here</div>
          <output id="frame-status">frame-idle</output>
          <output id="subject-value">subject:</output>
          <output id="body-value">body:</output>
          <output id="evaluate-value">evaluate:</output>
          <output id="drop-events">drop-events:</output>
          <output id="drop-selection">drop-selection:</output>
          <output id="drop-value">drop:</output>
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
            document.querySelector('#file-drop').addEventListener('dragover', event => {
              event.preventDefault()
            })
            const dropEvents = []
            for (const eventName of ['dragenter', 'dragover', 'drop']) {
              document.querySelector('#file-drop').addEventListener(eventName, event => {
                dropEvents.push(event.type)
                document.querySelector('#drop-events').textContent =
                  'drop-events:' + dropEvents.join(',')
              })
            }
            document.querySelector('#file-drop').addEventListener('drop', async event => {
              event.preventDefault()
              const file = event.dataTransfer.files[0]
              document.querySelector('#drop-selection').textContent = file
                ? 'drop-selection:' + file.name + ':' + file.size
                : 'drop-selection:no-file'
              try {
                if (!file) {
                  document.querySelector('#drop-value').textContent = 'drop:no-file'
                } else if (file.size > 1024 * 1024) {
                  const bytes = new Uint8Array(await file.arrayBuffer())
                  document.querySelector('#drop-value').textContent =
                    'drop:' + file.name + ':' + file.size + ':' +
                    String.fromCharCode(bytes[0]) + ':' +
                    String.fromCharCode(bytes[bytes.length - 1])
                } else {
                  document.querySelector('#drop-value').textContent =
                    'drop:' + file.name + ':' + await file.text()
                }
              } catch {
                document.querySelector('#drop-value').textContent = file
                  ? 'drop:' + file.name + ':read-failed'
                  : 'drop:no-file'
              }
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
    const listeningAddress = server.address()
    if (!listeningAddress || typeof listeningAddress === 'string') {
      throw new Error('local fixture address unavailable')
    }
    const crossFrameUrl = `http://localhost:${listeningAddress.port}/mail-frame?scope=cross`
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
          <label for="fixture-upload">Fixture upload</label>
          <input id="fixture-upload" type="file" />
          <button id="submit-upload" type="button">Submit fixture upload</button>
          <div id="drag-source" role="button" tabindex="0" draggable="true">Drag source</div>
          <div id="drop-target" role="region" aria-label="Drop target">Drop target</div>
          <ul aria-label="Visible items"><li>Alpha</li><li>Beta</li></ul>
          <output id="output">idle</output>
          <output id="upload-selection">upload-selection:</output>
          <output id="upload-output">upload:</output>
          <output id="upload-submit">upload-submit:</output>
          <output id="network-status">network-pending</output>
          <iframe id="mail-frame" src="/mail-frame"></iframe>
          <iframe id="cross-mail-frame" src="${crossFrameUrl}"></iframe>
        </main>
        <script>
          console.warn('managed fixture warning')
          fetch('/api/ping?token=fixture-query-canary')
            .then(() => { document.querySelector('#network-status').textContent = 'network-ready' })
            .catch(() => undefined)
          document.querySelector('#apply').addEventListener('click', () => {
            document.querySelector('#output').textContent =
              'applied:' + document.querySelector('#message').value
          })
          document.querySelector('#dialog').addEventListener('click', () => {
            alert('managed fixture dialog')
          })
          document.querySelector('#fixture-upload').addEventListener('change', async event => {
            const file = event.target.files[0]
            document.querySelector('#upload-selection').textContent = file
              ? 'upload-selection:' + file.name + ':' + file.size
              : 'upload-selection:no-file'
            try {
              document.querySelector('#upload-output').textContent = file
                ? 'upload:' + file.name + ':' + await file.text()
                : 'upload:no-file'
            } catch {
              document.querySelector('#upload-output').textContent = file
                ? 'upload:' + file.name + ':read-failed'
                : 'upload:no-file'
            }
          })
          document.querySelector('#submit-upload').addEventListener('click', async () => {
            const file = document.querySelector('#fixture-upload').files[0]
            if (!file) {
              document.querySelector('#upload-submit').textContent = 'upload-submit:no-file'
              return
            }
            try {
              const form = new FormData()
              form.append('fixture', file)
              const result = await fetch('/api/upload', { method: 'POST', body: form })
              const receipt = await result.json()
              document.querySelector('#upload-submit').textContent =
                receipt.filenameMatched && receipt.contentMatched
                  ? 'upload-submit:' + file.name + ':content-ok'
                  : 'upload-submit:mismatch'
            } catch {
              document.querySelector('#upload-submit').textContent = 'upload-submit:read-failed'
            }
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
  writeFileSync(
    storageStateFixturePath,
    JSON.stringify({
      cookies: [
        {
          name: 'restored-cookie',
          value: 'repository-owned-storage-cookie',
          domain: '127.0.0.1',
          path: '/',
          expires: -1,
          httpOnly: false,
          secure: false,
          sameSite: 'Lax'
        }
      ],
      origins: [
        {
          origin: `http://127.0.0.1:${address.port}`,
          localStorage: [{ name: 'restored-local', value: 'repository-owned-storage-local-value' }]
        }
      ]
    }),
    { mode: 0o600 }
  )

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
  const surfaceInstances = new Map<string, { guest: WebContents; surfaceInstanceId: string }>()
  let rendererSelectionRevision = Math.max(1, Date.now())
  let ensureCommands = 0
  let closeCommands = 0
  const probeExactSurfaceInstance = async (
    surfaceId: string,
    exactGuest: WebContents
  ): Promise<string> => {
    await waitForFixtureGuestDocumentReady(exactGuest, 10_000)
    const deadline = Date.now() + 10_000
    while (Date.now() < deadline) {
      if (exactGuest.isDestroyed() || guests.get(surfaceId) !== exactGuest) {
        throw new Error('fixture surface was replaced before Renderer acknowledgement')
      }
      const selectionRevision = Math.max(rendererSelectionRevision + 1, Date.now())
      rendererSelectionRevision = selectionRevision
      const output = manager.selectManualSurface(window.webContents, {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        surfaceId,
        surfaceInstanceId: null,
        selectionRevision
      })
      rendererSelectionRevision = Math.max(rendererSelectionRevision, output.authoritativeRevision)
      if (output.reason === 'instance_required' && output.surfaceInstanceId) {
        surfaceInstances.set(surfaceId, {
          guest: exactGuest,
          surfaceInstanceId: output.surfaceInstanceId
        })
        return output.surfaceInstanceId
      }
      if (!output.retryable && output.reason !== 'stale_revision') {
        throw new Error(`fixture surface instance probe failed: ${output.reason}`)
      }
      await waitForFixtureTurn(20)
    }
    throw new Error('fixture surface instance probe timed out')
  }
  const acknowledgeFixtureSurface = async (
    command: Exclude<BrowserSurfaceCommand, { kind: 'closeSurface' }>,
    exactGuest: WebContents,
    viewport?: { height: number; width: number }
  ): Promise<void> => {
    const deadline = Date.now() + 10_000
    while (Date.now() < deadline) {
      const surfaceInstanceId = await probeExactSurfaceInstance(command.surfaceId, exactGuest)
      const output = manager.attach(window.webContents, {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        requestId: command.requestId,
        surfaceId: command.surfaceId,
        surfaceInstanceId,
        ...(viewport ? { viewport } : {})
      })
      if (!output.retryable) return
      await waitForFixtureTurn(20)
    }
    throw new Error('fixture surface readiness acknowledgement timed out')
  }
  const manager = new BrowserSurfaceManager({
    attachTimeoutMs: 10_000,
    broker,
    networkGuard,
    releaseSurfaceResources: (input) => fileBroker.releaseSurface(input),
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
              if (!existingGuest || existingGuest.isDestroyed()) return
              void acknowledgeFixtureSurface(command, existingGuest).then(
                () => {
                  if (pendingEnsure?.requestId === command.requestId) pendingEnsure = undefined
                },
                () => undefined
              )
            }
          },
          () => undefined
        )
        return
      }
      if (command.kind === 'selectSurface' || command.kind === 'resizeSurface') {
        void applyFixtureSurfaceCommand(window, command).then(async (viewport) => {
          const exactGuest = guests.get(command.surfaceId)
          if (!exactGuest || exactGuest.isDestroyed()) {
            throw new Error('fixture selected surface is unavailable')
          }
          await acknowledgeFixtureSurface(command, exactGuest, viewport)
        })
        return
      }
      closeCommands += 1
      const binding = surfaceInstances.get(command.surfaceId)
      if (
        !command.surfaceInstanceId ||
        !binding ||
        binding.surfaceInstanceId !== command.surfaceInstanceId ||
        guests.get(command.surfaceId) !== binding.guest
      ) {
        return
      }
      surfaceInstances.delete(command.surfaceId)
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
    attachedGuest.once('destroyed', () => {
      if (guests.get(command.surfaceId) !== attachedGuest) return
      guests.delete(command.surfaceId)
      const binding = surfaceInstances.get(command.surfaceId)
      if (binding?.guest === attachedGuest) surfaceInstances.delete(command.surfaceId)
    })
    void acknowledgeFixtureSurface(command, attachedGuest).then(
      () => {
        if (pendingEnsure?.requestId === command.requestId) pendingEnsure = undefined
      },
      () => undefined
    )
  })

  const sensitiveTargetBindings = new ManagedPlaywrightSensitiveTargetBindingBroker({
    beginDispatchFence: (target) => manager.beginSensitiveDispatchFence(target),
    getActiveTarget: () => manager.getSensitiveTargetIdentity(),
    releasePreparedFiles: ({ runId, callId }) => {
      void fileBroker.releaseToolCall({ runId, toolCallId: callId })
    }
  })
  const bridgeHost = new ManagedPlaywrightBridgeHost({
    core,
    fileBroker,
    sensitiveTargetBindings,
    createHost: createManagedPlaywrightHostFactory({
      getBrowserContext: () => manager.getBrowserContext({ createVisiblePage: false }),
      closeSurface: () => manager.closeSurface(),
      detachAutomation: () => manager.detachAutomation(),
      beginNetworkOperation: (input) => manager.beginNetworkOperation(input),
      beginTargetCreationOperation: async (input) => {
        const lease = networkGuard.beginTargetCreationOperation(input)
        await lease.ready()
        return lease
      },
      artifactBroker,
      fileBroker,
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
    await fileBroker.shutdown().catch(() => undefined)
    await artifactBroker.shutdown().catch(() => undefined)
    await writeProtocolLine(
      `${RESULT_MARKER}${JSON.stringify({
        broker: broker.snapshot(),
        closeCommands,
        ensureCommands,
        fileSelectionCount,
        fileBroker: fileBroker.snapshot(),
        guestCount: guests.size,
        mainWindowAlive: !window.isDestroyed(),
        riskGuard: networkGuard.snapshot(),
        surfaceCount: manager.snapshot().surfaces,
        targetClosed: !guest || guest.isDestroyed()
      })}`
    ).catch((error: unknown) => {
      console.error(
        `managed Playwright RESULT write failed: ${error instanceof Error ? error.name : 'Error'}`
      )
    })
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

async function waitForFixtureGuestDocumentReady(
  guest: WebContents,
  timeoutMs: number
): Promise<void> {
  if (guest.isDestroyed()) throw new Error('fixture surface was destroyed before document-ready')
  if (!guest.isLoadingMainFrame() && guest.getURL() !== '') return

  await new Promise<void>((resolve, reject) => {
    let settled = false
    const finish = (error?: Error): void => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      guest.removeListener('dom-ready', handleReady)
      guest.removeListener('did-finish-load', handleReady)
      guest.removeListener('destroyed', handleDestroyed)
      if (error) reject(error)
      else resolve()
    }
    const handleReady = (): void => finish()
    const handleDestroyed = (): void =>
      finish(new Error('fixture surface was destroyed before document-ready'))
    const timer = setTimeout(
      () => finish(new Error('fixture surface document-ready timed out')),
      timeoutMs
    )
    guest.once('dom-ready', handleReady)
    guest.once('did-finish-load', handleReady)
    guest.once('destroyed', handleDestroyed)
    if (!guest.isLoadingMainFrame() && guest.getURL() !== '') finish()
  })
}

async function waitForFixtureTurn(delayMs: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, delayMs))
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
