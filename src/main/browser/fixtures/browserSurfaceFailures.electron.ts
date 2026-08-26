import { createServer } from 'node:http'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  BROWSER_WEBVIEW_PARTITION,
  createBrowserSurfaceBootstrapUrl,
  type BrowserSurfaceState
} from '@mycopilot/protocol'
import { app, BrowserWindow, nativeImage, nativeTheme, session } from 'electron'
import type { WebContents } from 'electron'

import { BrowserNetworkGuard } from '../BrowserNetworkGuard'
import { BrowserNetworkPolicy, ElectronSessionDnsResolver } from '../BrowserNetworkPolicy'
import { BrowserRiskCoordinator } from '../BrowserRiskCoordinator'
import { BrowserSurfaceManager } from '../BrowserSurfaceManager'
import { BrowserTargetBroker } from '../BrowserTargetBroker'
import { BrowserInternalPageStore } from '../BrowserInternalPageStore'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from '../../webviews/managedWebviewSecurity'

const RESULT_MARKER = 'MYCOPILOT_BROWSER_FAILURE_RESULT='
const PROFILE_DIRECTORY = mkdtempSync(join(tmpdir(), 'mycopilot-browser-failure-profile-'))
const SURFACE_ID = 'right-sidebar-browser-electron-failure-fixture'

app.disableHardwareAcceleration()
app.setPath('userData', PROFILE_DIRECTORY)

async function main(): Promise<void> {
  await app.whenReady()
  const managedSession = session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  const internalPageStore = new BrowserInternalPageStore(managedSession)
  internalPageStore.install()
  const networkPolicy = new BrowserNetworkPolicy({
    dnsResolver: new ElectronSessionDnsResolver(managedSession)
  })
  const networkGuard = new BrowserNetworkGuard({
    accessPolicy: 'host_boundaries_only',
    coordinator: new BrowserRiskCoordinator({
      authorizer: {
        authorize: async () => ({ decision: 'approved', grantId: 'fixture-grant' })
      },
      policy: networkPolicy
    }),
    expectedSession: managedSession,
    policy: networkPolicy
  })
  initializeManagedWebviewSessions({ networkGuard })

  const closedPort = await reserveClosedPort()
  const server = createServer((request, response) => {
    const requestUrl = new URL(request.url ?? '/', 'http://127.0.0.1')
    response.setHeader('cache-control', 'no-store')
    response.setHeader('content-type', 'text/html; charset=utf-8')
    const pageName = requestUrl.pathname.slice(1) || 'root'
    const iframe =
      requestUrl.pathname === '/iframe'
        ? `<iframe title="Rejected child" src="http://127.0.0.1:${closedPort}/child"></iframe>`
        : ''
    response.end(`<!doctype html>
      <html><head><title>Fixture ${escapeFixtureHtml(pageName)}</title></head>
      <body><main><h1 id="page-name">${escapeFixtureHtml(pageName)}</h1>${iframe}</main></body></html>`)
  })
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const serverAddress = server.address()
  if (!serverAddress || typeof serverAddress === 'string') throw new Error('fixture server missing')
  const fixtureProxy = {
    mode: 'fixed_servers' as const,
    proxyRules: `http=127.0.0.1:${serverAddress.port}`
  }
  await managedSession.setProxy(fixtureProxy)
  const baseUrl = 'http://browser-surface-fixture.test'
  const pageA = `${baseUrl}/a`
  const pageB = `${baseUrl}/b?token=must-not-enter-internal-html`
  const pageBSecond = `${baseUrl}/b?retry=second-secret`
  const pageC = `${baseUrl}/c`
  const iframePage = `${baseUrl}/iframe`
  const refusedUrl = `http://127.0.0.1:${closedPort}/refused?token=private`
  const visualDirectory = process.env.MYCOPILOT_BROWSER_VISUAL_DIR
  if (visualDirectory) mkdirSync(visualDirectory, { recursive: true })

  let locale = 'zh-CN'
  nativeTheme.themeSource = 'light'
  const window = new BrowserWindow({
    backgroundColor: '#ffffff',
    height: 720,
    show: true,
    skipTaskbar: true,
    width: 380,
    x: -10_000,
    y: -10_000,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      webviewTag: true
    }
  })
  await window.loadURL(
    'data:text/html;charset=utf-8,<html><body style="margin:0"><main id="host"></main></body></html>'
  )

  const broker = new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, managedSession)
  const diagnostics: unknown[] = []
  const manager = new BrowserSurfaceManager({
    broker,
    getLocale: () => locale,
    internalPageStore,
    networkGuard,
    recordDiagnostic: (diagnostic) => diagnostics.push(diagnostic),
    resolveHost: () => window.webContents,
    sendCommand: () => undefined
  })
  configureManagedWebviewHost(window.webContents, { networkGuard, targetRegistry: manager })

  let resolveGuest!: (guest: WebContents) => void
  const guestPromise = new Promise<WebContents>((resolve) => {
    resolveGuest = resolve
  })
  window.webContents.on('did-attach-webview', (_event, guest) => {
    resolveGuest(guest)
  })

  await window.webContents.executeJavaScript(`(() => {
    const webview = document.createElement('webview')
    webview.dataset.surfaceId = ${JSON.stringify(SURFACE_ID)}
    webview.setAttribute('partition', ${JSON.stringify(BROWSER_WEBVIEW_PARTITION)})
    webview.setAttribute('src', ${JSON.stringify(createBrowserSurfaceBootstrapUrl(SURFACE_ID))})
    webview.setAttribute('width', '340')
    webview.setAttribute('height', '680')
    webview.style.display = 'flex'
    webview.style.width = '340px'
    webview.style.height = '680px'
    document.querySelector('#host').appendChild(webview)
  })()`)
  const guest = await withTimeout(guestPromise, 10_000, 'managed guest attachment timed out')

  try {
    const surfaceInstanceId = await waitForSurfaceInstance(manager, window, guest)
    const stateInput = {
      schemaVersion: 1 as const,
      surfaceId: SURFACE_ID,
      surfaceInstanceId
    }
    manager.selectManualSurface(window.webContents, {
      schemaVersion: 1,
      selectionRevision: 2,
      surfaceId: SURFACE_ID,
      surfaceInstanceId
    })

    await navigateAndWait(manager, window, stateInput, pageA)
    const context = await manager.getBrowserContext()
    const page = context.pages()[0]
    if (!page) throw new Error('managed Playwright page missing')
    const generationBefore = manager
      .listSurfaces()
      .find((surface) => surface.surfaceId === SURFACE_ID)?.generation
    if (!generationBefore) throw new Error('surface generation missing')

    managedSession.enableNetworkEmulation({ offline: true })
    await context.setOffline(true)
    await delay(100)
    manager.performSurfaceAction(window.webContents, {
      ...stateInput,
      action: 'navigate',
      url: pageB
    })
    const offlineState = await waitForSurfaceState(manager, window, stateInput, (state) =>
      Boolean(
        state.presentation === 'error-page' &&
        state.loadError?.kind === 'offline' &&
        !state.isLoading
      )
    )
    const offlinePhysicalUrl = guest.getURL()
    const offlineHeading = await page.locator('h1').textContent()
    const offlineLayout = await inspectInternalPageLayout(page)
    const offlineScreenshot = await page.screenshot({
      ...(visualDirectory ? { path: join(visualDirectory, 'offline-zh-light-narrow.png') } : {})
    })
    const automationUrlsDuringError = context.pages().map((candidate) => candidate.url())
    const targetStableDuringOffline = context.pages()[0] === page
    const targetUrlIsLogicalDuringOffline = page.url() === pageB
    await page.locator('#primary-action').focus()
    const keyboardFocus = await page.evaluate(() => document.activeElement?.id === 'primary-action')

    managedSession.enableNetworkEmulation({ offline: false })
    await context.setOffline(false)
    await delay(180)
    const remainedFailedUntilRetry =
      manager.getSurfaceState(window.webContents, stateInput).presentation === 'error-page'

    manager.performSurfaceAction(window.webContents, { ...stateInput, action: 'goBack' })
    await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) => state.presentation === 'content' && state.url === pageA && !state.isLoading
    )
    manager.performSurfaceAction(window.webContents, { ...stateInput, action: 'goForward' })
    const forwardFailure = await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) => state.presentation === 'error-page' && state.url === pageB && !state.isLoading
    )
    await page.locator('#primary-action').click()
    const recoveredB = await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) => state.presentation === 'content' && state.url === pageB && !state.isLoading
    )

    managedSession.enableNetworkEmulation({ offline: true })
    await context.setOffline(true)
    await delay(100)
    manager.performSurfaceAction(window.webContents, {
      ...stateInput,
      action: 'navigate',
      url: pageBSecond
    })
    await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) =>
        state.presentation === 'error-page' && state.url === pageBSecond && !state.isLoading
    )
    managedSession.enableNetworkEmulation({ offline: false })
    await context.setOffline(false)
    await navigateAndWait(manager, window, stateInput, pageC)
    manager.performSurfaceAction(window.webContents, { ...stateInput, action: 'goBack' })
    const addressBarBackState = await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) =>
        state.presentation === 'error-page' && state.url === pageBSecond && !state.isLoading
    )
    manager.performSurfaceAction(window.webContents, { ...stateInput, action: 'goForward' })
    const addressBarForwardState = await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) => state.presentation === 'content' && state.url === pageC && !state.isLoading
    )

    await navigateAndWait(manager, window, stateInput, iframePage)
    await delay(250)
    const iframeState = manager.getSurfaceState(window.webContents, stateInput)

    locale = 'en-US'
    nativeTheme.themeSource = 'dark'
    await managedSession.setProxy({ mode: 'direct' })
    window.setSize(820, 760)
    await window.webContents.executeJavaScript(`(() => {
      const webview = document.querySelector('webview[data-surface-id=${JSON.stringify(SURFACE_ID)}]')
      webview.setAttribute('width', '780')
      webview.setAttribute('height', '720')
      webview.style.width = '780px'
      webview.style.height = '720px'
    })()`)
    manager.performSurfaceAction(window.webContents, {
      ...stateInput,
      action: 'navigate',
      url: refusedUrl
    })
    const refusedState = await waitForSurfaceState(manager, window, stateInput, (state) =>
      Boolean(
        state.presentation === 'error-page' &&
        state.loadError?.kind === 'connection_refused' &&
        !state.isLoading
      )
    )
    const refusedHeading = await page.locator('h1').textContent()
    const refusedLayout = await inspectInternalPageLayout(page)
    const refusedScreenshot = await page.screenshot({
      ...(visualDirectory ? { path: join(visualDirectory, 'refused-en-dark-wide.png') } : {})
    })
    const forgedUrl = 'data:text/html;charset=utf-8,%3Ch1%3Eforged%3C%2Fh1%3E'
    const forgedAuthorized = manager.isInternalNavigationAllowed(guest, forgedUrl)
    let forgedRejected = false
    try {
      manager.performSurfaceAction(window.webContents, {
        ...stateInput,
        action: 'navigate',
        url: forgedUrl
      })
    } catch {
      forgedRejected = true
    }
    const stateAfterForgery = manager.getSurfaceState(window.webContents, stateInput)

    await managedSession.setProxy(fixtureProxy)
    await navigateAndWait(manager, window, stateInput, pageA)
    await manager.detachAutomation()
    guest.debugger.attach('1.3')
    void guest.debugger.sendCommand('Page.crash').catch(() => undefined)
    const crashState = await waitForSurfaceState(manager, window, stateInput, (state) =>
      Boolean(
        state.presentation === 'crash-page' &&
        state.crashError?.kind === 'renderer_crashed' &&
        !state.isLoading
      )
    )
    const crashHeading = await guest.executeJavaScript('document.querySelector("h1")?.textContent')
    manager.performSurfaceAction(window.webContents, { ...stateInput, action: 'reload' })
    const recoveredCrash = await waitForSurfaceState(
      manager,
      window,
      stateInput,
      (state) => state.presentation === 'content' && state.url === pageA && !state.isLoading
    )

    const historyAfterRecovery = guest.navigationHistory.getAllEntries()
    const retainedInternalHistory = historyAfterRecovery.filter((entry) =>
      entry.url.startsWith('mycopilot-browser-internal:')
    )
    process.stdout.write(
      `${RESULT_MARKER}${JSON.stringify({
        addressBarHistory: {
          backReturnedToFailure: addressBarBackState.loadError?.failedUrl === pageBSecond,
          forwardReturnedToC:
            addressBarForwardState.presentation === 'content' &&
            addressBarForwardState.url === pageC
        },
        crash: {
          diagnosticRedacted:
            diagnostics.length === 1 && !JSON.stringify(diagnostics).includes('token='),
          generationStable:
            manager.listSurfaces().find((surface) => surface.surfaceId === SURFACE_ID)
              ?.generation === generationBefore,
          heading: crashHeading,
          kind: crashState.crashError?.kind,
          logicalUrl: crashState.url,
          recovered: recoveredCrash.presentation === 'content'
        },
        forgedInternalUrl: {
          authorized: forgedAuthorized,
          rejected: forgedRejected,
          statePreserved:
            stateAfterForgery.presentation === 'error-page' && stateAfterForgery.url === refusedUrl
        },
        iframeFailureIgnored:
          iframeState.presentation === 'content' && iframeState.loadError === null,
        offline: {
          cdpHistorySanitized: !JSON.stringify(automationUrlsDuringError).includes(
            'mycopilot-browser-internal:'
          ),
          failedUrl: offlineState.loadError?.failedUrl,
          heading: offlineHeading,
          keyboardFocus,
          kind: offlineState.loadError?.kind,
          layout: offlineLayout,
          logicalUrl: offlineState.url,
          physicalInternal: offlinePhysicalUrl.startsWith('mycopilot-browser-internal://page/'),
          physicalOmitsSecret: !offlinePhysicalUrl.includes('must-not-enter'),
          pixelVariation: screenshotHasPixelVariation(offlineScreenshot),
          remainedFailedUntilRetry,
          retryRecovered: recoveredB.presentation === 'content',
          targetStable: targetStableDuringOffline,
          targetUrlIsLogical: targetUrlIsLogicalDuringOffline,
          forwardRestoredFailure: forwardFailure.loadError?.failedUrl === pageB
        },
        physicalHistory: {
          currentEntryIsInternal: guest.getURL().startsWith('mycopilot-browser-internal:'),
          internalCount: retainedInternalHistory.length,
          retainedEntriesAuthorized: retainedInternalHistory.every((entry) =>
            manager.isInternalNavigationAllowed(guest, entry.url)
          )
        },
        refused: {
          heading: refusedHeading,
          kind: refusedState.loadError?.kind,
          layout: refusedLayout,
          logicalUrl: refusedState.url,
          pixelVariation: screenshotHasPixelVariation(refusedScreenshot)
        },
        surfaceId: SURFACE_ID
      })}\n`
    )
  } finally {
    managedSession.enableNetworkEmulation({ offline: false })
    await manager.shutdown().catch(() => undefined)
    await networkGuard.shutdown().catch(() => undefined)
    if (!window.isDestroyed()) window.destroy()
    await new Promise<void>((resolve) => server.close(() => resolve()))
    app.quit()
  }
}

async function navigateAndWait(
  manager: BrowserSurfaceManager,
  window: BrowserWindow,
  stateInput: { schemaVersion: 1; surfaceId: string; surfaceInstanceId: string },
  url: string
): Promise<BrowserSurfaceState> {
  manager.performSurfaceAction(window.webContents, { ...stateInput, action: 'navigate', url })
  return await waitForSurfaceState(
    manager,
    window,
    stateInput,
    (state) => state.presentation === 'content' && state.url === url && !state.isLoading
  )
}

async function waitForSurfaceInstance(
  manager: BrowserSurfaceManager,
  window: BrowserWindow,
  guest: WebContents
): Promise<string> {
  const deadline = Date.now() + 10_000
  while (Date.now() < deadline) {
    if (guest.isDestroyed()) throw new Error('managed guest closed before registration')
    const result = manager.selectManualSurface(window.webContents, {
      schemaVersion: 1,
      selectionRevision: 1,
      surfaceId: SURFACE_ID,
      surfaceInstanceId: null
    })
    if (result.reason === 'instance_required' && result.surfaceInstanceId) {
      return result.surfaceInstanceId
    }
    await delay(20)
  }
  throw new Error('surface registration timed out')
}

async function waitForSurfaceState(
  manager: BrowserSurfaceManager,
  window: BrowserWindow,
  stateInput: { schemaVersion: 1; surfaceId: string; surfaceInstanceId: string },
  predicate: (state: BrowserSurfaceState) => boolean
): Promise<BrowserSurfaceState> {
  const deadline = Date.now() + 12_000
  let latest: BrowserSurfaceState | undefined
  while (Date.now() < deadline) {
    latest = manager.getSurfaceState(window.webContents, stateInput)
    if (predicate(latest)) return latest
    await delay(20)
  }
  throw new Error(`surface state timed out: ${JSON.stringify(latest)}`)
}

async function inspectInternalPageLayout(page: {
  evaluate<T>(expression: () => T): Promise<T>
}): Promise<{
  actionVisible: boolean
  height: number
  headingVisible: boolean
  horizontalOverflow: boolean
  verticalOverflow: boolean
  width: number
}> {
  return await page.evaluate(() => {
    const heading = document.querySelector('h1')?.getBoundingClientRect()
    const action = document.querySelector('button')?.getBoundingClientRect()
    const elementIsInsideViewport = (bounds: DOMRect | undefined): boolean =>
      Boolean(
        bounds &&
        bounds.width > 0 &&
        bounds.height > 0 &&
        bounds.top >= 0 &&
        bounds.left >= 0 &&
        bounds.right <= window.innerWidth &&
        bounds.bottom <= window.innerHeight
      )
    return {
      actionVisible: elementIsInsideViewport(action),
      height: window.innerHeight,
      headingVisible: elementIsInsideViewport(heading),
      horizontalOverflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      verticalOverflow: document.documentElement.scrollHeight > window.innerHeight + 1,
      width: window.innerWidth
    }
  })
}

function screenshotHasPixelVariation(png: Uint8Array): boolean {
  const image = nativeImage.createFromBuffer(Buffer.from(png))
  const bitmap = image.toBitmap()
  if (bitmap.length < 16) return false
  const first = bitmap.subarray(0, 4).toString('hex')
  for (let index = 4; index + 4 <= bitmap.length; index += 4 * 257) {
    if (bitmap.subarray(index, index + 4).toString('hex') !== first) return true
  }
  return false
}

async function reserveClosedPort(): Promise<number> {
  const server = createServer()
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('closed port reservation failed')
  await new Promise<void>((resolve) => server.close(() => resolve()))
  return address.port
}

function escapeFixtureHtml(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
}

async function withTimeout<T>(promise: Promise<T>, timeoutMs: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs)
      })
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

async function delay(ms: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, ms))
}

void main()
  .catch((error: unknown) => {
    process.stderr.write(
      `Browser failure fixture failed: ${error instanceof Error ? error.stack : 'unknown'}\n`
    )
    app.exit(1)
  })
  .finally(() => {
    rmSync(PROFILE_DIRECTORY, { force: true, recursive: true })
  })
