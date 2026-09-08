import { createServer } from 'node:http'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  BROWSER_WEBVIEW_PARTITION,
  createBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceBootstrapUrl,
  type BrowserSurfaceCommand
} from '@mycopilot/protocol'
import { app, BrowserWindow, session } from 'electron'
import type { WebContents } from 'electron'
import { BrowserSurfaceManager } from '../BrowserSurfaceManager'
import { BrowserTargetBroker } from '../BrowserTargetBroker'
import { BrowserInternalPageStore } from '../BrowserInternalPageStore'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from '../../webviews/managedWebviewSecurity'

const RESULT_MARKER = 'MYCOPILOT_BROWSER_SURFACE_RESULT='
const SURFACE_ID = 'right-sidebar-browser-electron-fixture'
const SECOND_SURFACE_ID = 'right-sidebar-browser-electron-fixture-second'
const POPUP_SURFACE_ID = 'right-sidebar-browser-electron-fixture-popup'
const REPLACEMENT_SURFACE_ID = 'right-sidebar-browser-electron-fixture-replacement'
const FIXTURE_READINESS_TIMEOUT_MS = 20_000
const PROFILE_DIRECTORY = mkdtempSync(join(tmpdir(), 'mycopilot-browser-surface-profile-'))

app.setPath('userData', PROFILE_DIRECTORY)

async function main(): Promise<void> {
  if (process.argv.some((argument) => argument.startsWith('--remote-debugging-port'))) {
    throw new Error('remote debugging port is forbidden')
  }

  await app.whenReady()
  initializeManagedWebviewSessions()
  const server = createServer((_request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    response.end(`<!doctype html>
      <html><body>
        <main>
          <h1>Browser Surface Fixture</h1>
          <label for="message">Message</label>
          <input id="message" />
          <button id="apply">Apply</button>
          <button id="open-popup">Open popup</button>
          <output id="output">idle</output>
          <div aria-label="Rich Message" contenteditable="true" id="rich-message"></div>
        </main>
        <script>
          document.body.dataset.keydownCount = '0'
          document.addEventListener('keydown', () => {
            document.body.dataset.keydownCount = String(
              Number(document.body.dataset.keydownCount || '0') + 1
            )
          })
          document.querySelector('#apply').addEventListener('click', () => {
            document.querySelector('#output').textContent =
              'applied:' + document.querySelector('#message').value
          })
          document.querySelector('#open-popup').addEventListener('click', () => {
            window.open('/popup', '_blank')
          })
        </script>
      </body></html>`)
  })
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('local fixture did not bind')
  const fixtureUrl = `http://127.0.0.1:${address.port}/interactive`

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

  const managedSession = session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  const browserInternalPageStore = new BrowserInternalPageStore(managedSession)
  browserInternalPageStore.install()
  const broker = new BrowserTargetBroker(BROWSER_WEBVIEW_PARTITION, managedSession)
  let pendingEnsure:
    Extract<BrowserSurfaceCommand, { kind: 'ensureAttached' | 'createSurface' }> | undefined
  const guests = new Map<string, WebContents>()
  const surfaceInstances = new Map<string, { guest: WebContents; surfaceInstanceId: string }>()
  let rendererSelectionRevision = Math.max(1, Date.now())
  const allocatedSurfaceIds = [
    SURFACE_ID,
    SECOND_SURFACE_ID,
    POPUP_SURFACE_ID,
    REPLACEMENT_SURFACE_ID
  ]
  let ensureCommands = 0
  let createCommands = 0
  let closeCommands = 0
  let rendererReadyAcks = 0
  let newRendererReadyAcks = 0
  const probeExactSurfaceInstance = async (
    surfaceId: string,
    exactGuest: WebContents
  ): Promise<string> => {
    // Initial callers arrive only after ensureFixtureSurface observed both did-attach and the
    // exact webview's document-ready event; later callers use that already-acknowledged guest.
    // Re-subscribing through WebContents here can miss the completed event and race Host expiry.
    const deadline = Date.now() + FIXTURE_READINESS_TIMEOUT_MS
    while (Date.now() < deadline) {
      if (exactGuest.isDestroyed() || guests.get(surfaceId) !== exactGuest) {
        throw new Error('fixture surface was replaced before Renderer acknowledgement')
      }
      const selectionRevision = Math.max(rendererSelectionRevision + 1, Date.now())
      rendererSelectionRevision = selectionRevision
      const output = manager.selectManualSurface(window.webContents, {
        schemaVersion: 1,
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
    const deadline = Date.now() + FIXTURE_READINESS_TIMEOUT_MS
    while (Date.now() < deadline) {
      const surfaceInstanceId = await probeExactSurfaceInstance(command.surfaceId, exactGuest)
      const output = manager.attach(window.webContents, {
        schemaVersion: 1,
        requestId: command.requestId,
        surfaceId: command.surfaceId,
        surfaceInstanceId,
        ...(viewport ? { viewport } : {})
      })
      if (output.status === 'applied' || output.reason === 'already_ready') return
      if (!output.retryable) {
        throw new Error(`unexpected Renderer ready result: ${output.reason}`)
      }
      await waitForFixtureTurn(20)
    }
    throw new Error('fixture surface readiness acknowledgement timed out')
  }

  const manager = new BrowserSurfaceManager({
    attachTimeoutMs: FIXTURE_READINESS_TIMEOUT_MS,
    broker,
    closeTimeoutMs: 5_000,
    createSurfaceId: () => allocatedSurfaceIds.shift() ?? `unexpected-${Date.now()}`,
    internalPageStore: browserInternalPageStore,
    resolveHost: () => window.webContents,
    sendCommand: (_host, command) => {
      if (command.kind === 'ensureAttached' || command.kind === 'createSurface') {
        if (command.kind === 'ensureAttached') ensureCommands += 1
        else createCommands += 1
        pendingEnsure = command
        const existingGuest = guests.get(command.surfaceId)
        void ensureFixtureSurface(
          window,
          command.surfaceId,
          Boolean(existingGuest && !existingGuest.isDestroyed())
        )
          .then((alreadyAttached) => {
            if (pendingEnsure?.requestId !== command.requestId) return
            const exactGuest = guests.get(command.surfaceId)
            if (!exactGuest || exactGuest.isDestroyed()) {
              throw new Error('fixture Renderer ready guest is unavailable')
            }
            return acknowledgeFixtureSurface(command, exactGuest).then(() => {
              rendererReadyAcks += 1
              if (!alreadyAttached) newRendererReadyAcks += 1
              if (pendingEnsure?.requestId === command.requestId) pendingEnsure = undefined
            })
          })
          .catch((error: unknown) => {
            process.stderr.write(
              `Renderer readiness fixture failed: ${error instanceof Error ? error.message : 'unknown'}\n`
            )
            app.exit(1)
          })
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
    targetRegistry: manager
  })
  window.webContents.on('did-attach-webview', (_event, attachedGuest) => {
    const surfaceId =
      parseBrowserSurfaceBootstrapUrl(attachedGuest.getURL()) ?? pendingEnsure?.surfaceId
    if (!surfaceId) return
    guests.set(surfaceId, attachedGuest)
    attachedGuest.once('destroyed', () => {
      if (guests.get(surfaceId) !== attachedGuest) return
      guests.delete(surfaceId)
      const binding = surfaceInstances.get(surfaceId)
      if (binding?.guest === attachedGuest) surfaceInstances.delete(surfaceId)
    })
  })

  try {
    const [firstContext, concurrentContext] = await Promise.all([
      manager.getBrowserContext(),
      manager.getBrowserContext()
    ])
    const firstPage = firstContext.pages()[0]
    if (!firstPage) throw new Error('managed page missing')
    const initialPageCount = firstContext.pages().length
    await firstPage.goto(fixtureUrl)
    const snapshot = await firstPage.locator('body').ariaSnapshot()
    await firstPage.getByLabel('Message', { exact: true }).click()
    await firstPage.keyboard.type('hello')
    await firstPage.getByRole('button', { name: 'Apply' }).click()
    const finalText = await firstPage.locator('#output').textContent()

    await setFixtureVisibility(window, SURFACE_ID, false)
    const hiddenTitle = await firstPage.locator('h1').textContent()
    await setFixtureVisibility(window, SURFACE_ID, true)
    const messageInput = firstPage.getByLabel('Message', { exact: true })
    await messageInput.fill('hidden')
    const inputValueAfterFill = await messageInput.inputValue()
    await firstPage.getByRole('button', { name: 'Apply' }).click()
    await firstPage.keyboard.press('End')
    const pressKeyObserved =
      Number(await firstPage.locator('body').getAttribute('data-keydown-count')) > 0
    const hiddenText = await firstPage.locator('#output').textContent()
    await firstPage.waitForFunction(
      () => document.querySelector('#output')?.textContent === 'applied:hidden'
    )
    const waitCompleted = true
    const richMessage = firstPage.getByLabel('Rich Message')
    await richMessage.fill('rich text')
    const richText = await richMessage.textContent()
    window.minimize()
    await waitFor(() => window.isMinimized(), 'fixture window did not minimize')
    const minimizedTitle = await firstPage.locator('h1').textContent()
    window.restore()
    await setFixtureVisibility(window, SURFACE_ID, true)

    await manager.detachAutomation()
    const retainedAfterDetach = Boolean(
      guests.get(SURFACE_ID) && !guests.get(SURFACE_ID)?.isDestroyed()
    )
    const reattachedContext = await manager.getBrowserContext()
    const reattachedPage = reattachedContext.pages()[0]
    if (!reattachedPage) throw new Error('reattached page missing')
    const retainedText = await reattachedPage.locator('#output').textContent()

    const runA = { runId: 'fixture-run-a', activationId: 'fixture-activation-a' }
    const runB = { runId: 'fixture-run-b', activationId: 'fixture-activation-b' }
    manager.prepareRunTarget(runA)
    const created = await manager.createSurface({ url: `${fixtureUrl}?tab=second` })
    manager.prepareRunTarget(runB)
    await setFixtureVisibility(window, SURFACE_ID, false)
    const leaseA = await manager.beginToolSurfaceLease({ owner: runA })
    const boundPageA = (await manager.getBrowserContext()).pages()[await leaseA.resolveIndex()]!
    await boundPageA.bringToFront()
    await boundPageA
      .getByLabel('Message', { exact: true })
      .fill('background-run', { timeout: 5_000 })
    await boundPageA.getByRole('button', { name: 'Apply' }).click({ timeout: 5_000 })
    const backgroundRunText = await boundPageA.locator('#output').textContent()
    const visibleTabStayedSecond =
      manager.listSurfaces().find((surface) => surface.isActive)?.surfaceId === created.surfaceId
    // Restore the fixture's earlier content before continuing its existing lifecycle checks.
    await boundPageA.getByLabel('Message', { exact: true }).fill('hidden')
    await boundPageA.getByRole('button', { name: 'Apply' }).click()
    leaseA.finish()
    const leaseB = await manager.beginToolSurfaceLease({ owner: runB })
    const secondRunTarget = leaseB.surfaceId === created.surfaceId
    leaseB.finish()
    manager.releaseRunTarget(runA.runId)
    manager.releaseRunTarget(runB.runId)
    await setFixtureVisibility(window, SURFACE_ID, true)
    const secondContext = await manager.getBrowserContext()
    const secondPage = secondContext.pages()[0]
    if (!secondPage) throw new Error('second managed page missing')
    const secondTitle = await secondPage.locator('h1').textContent()
    const tabsAfterCreate = manager.listSurfaces()
    const selectedFirst = manager.selectSurface({ index: 0 })
    await selectedFirst
    const selectedContext = await manager.getBrowserContext()
    const selectedPage = selectedContext.pages()[0]
    if (!selectedPage) throw new Error('reselected managed page missing')
    const selectedRetainedText = await selectedPage.locator('#output').textContent()
    await manager.closeSurfaceByIndex(1)
    const tabsAfterBackgroundClose = manager.listSurfaces()

    await selectedPage.getByRole('button', { name: 'Open popup' }).click()
    await waitFor(
      () => manager.listSurfaces().some((surface) => surface.surfaceId === POPUP_SURFACE_ID),
      'window.open did not create a managed background surface'
    )
    const tabsAfterPopup = manager.listSurfaces()
    const popupKeptOpenerActive =
      manager.getActiveSurfaceIdentity()?.surfaceId === SURFACE_ID &&
      (await selectedPage.locator('#output').textContent()) === 'applied:hidden'
    await manager.selectSurface({ surfaceId: POPUP_SURFACE_ID })
    const popupContext = await manager.getBrowserContext()
    const popupPage = popupContext.pages()[0]
    if (!popupPage) throw new Error('managed popup page missing')
    const popupTitle = await popupPage.locator('h1').textContent()
    await manager.closeSurface(POPUP_SURFACE_ID)

    // This fixture removes webviews directly and does not run the Renderer tab-state fallback.
    // Main intentionally no longer guesses the next visible tab after closing the popup.
    await manager.closeSurface(SURFACE_ID)
    await waitFor(() => !guests.has(SURFACE_ID), 'explicit close did not destroy target')
    const mainWindowAliveAfterClose = !window.isDestroyed()
    let oldTargetRejected = false
    try {
      await reattachedPage.title()
    } catch {
      oldTargetRejected = true
    }

    await manager.ensureActiveSurface()
    await manager.detachAutomation()
    const replacementContext = await manager.getBrowserContext()
    const replacementPage = replacementContext.pages()[0]
    if (!replacementPage) throw new Error('replacement page missing')
    const replacementIsInert = replacementPage.url().startsWith('about:blank')

    await manager.shutdown()
    process.stdout.write(
      `${RESULT_MARKER}${JSON.stringify({
        ariaSnapshot: snapshot.includes('Browser Surface Fixture'),
        closeCommands,
        createCommands,
        concurrentSingleFlight: firstContext === concurrentContext,
        ensureCommands,
        finalText,
        hiddenTitle,
        hiddenText,
        isolatedProfile: app.getPath('userData') === PROFILE_DIRECTORY,
        inputValueAfterFill,
        mainWindowAliveAfterClose,
        multiTab: {
          backgroundRunText,
          visibleTabStayedSecond,
          secondRunTarget,
          createdSurfaceId: created.surfaceId,
          selectedRetainedText,
          secondTitle,
          popupKeptOpenerActive,
          popupTitle,
          tabsAfterBackgroundClose: tabsAfterBackgroundClose.length,
          tabsAfterCreate: tabsAfterCreate.length,
          tabsAfterPopup: tabsAfterPopup.length
        },
        minimizedTitle,
        noRemoteDebuggingPort: true,
        newRendererReadyAcks,
        oldTargetRejected,
        pageCount: initialPageCount,
        pressKeyObserved,
        replacementIsInert,
        retainedAfterDetach,
        retainedText,
        rendererReadyAcks,
        richText,
        snapshot: broker.snapshot(),
        waitCompleted
      })}\n`
    )
  } finally {
    await manager.shutdown().catch(() => undefined)
    if (!window.isDestroyed()) window.destroy()
    await new Promise<void>((resolve) => server.close(() => resolve()))
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
      return Promise.resolve(true)
    }
    return new Promise((resolve, reject) => {
      const webview = document.createElement('webview')
      let attached = false
      let documentReady = false
      let settled = false
      const cleanup = () => {
        clearTimeout(timeout)
        webview.removeEventListener('did-attach', handleAttached)
        webview.removeEventListener('dom-ready', handleDocumentReady)
        webview.removeEventListener('did-finish-load', handleDocumentReady)
      }
      const complete = () => {
        if (settled || !attached || !documentReady) return
        settled = true
        cleanup()
        resolve(${JSON.stringify(isAlreadyAttached)})
      }
      const handleAttached = () => {
        attached = true
        complete()
      }
      const handleDocumentReady = () => {
        documentReady = true
        complete()
      }
      const timeout = setTimeout(() => {
        if (settled) return
        settled = true
        cleanup()
        reject(new Error('fixture webview did not reach Renderer document readiness'))
      }, 5000)
      webview.addEventListener('did-attach', handleAttached)
      webview.addEventListener('dom-ready', handleDocumentReady)
      webview.addEventListener('did-finish-load', handleDocumentReady)
      webview.dataset.surfaceId = ${JSON.stringify(surfaceId)}
      webview.setAttribute('partition', ${JSON.stringify(BROWSER_WEBVIEW_PARTITION)})
      webview.setAttribute('allowpopups', '')
      webview.setAttribute('src', ${JSON.stringify(bootstrapUrl)})
      webview.style.width = '320px'
      webview.style.height = '220px'
      const wrapper = document.createElement('section')
      wrapper.className = 'fixture-page'
      wrapper.style.cssText = 'position:absolute;inset:0;width:320px;height:220px'
      wrapper.append(webview)
      document.querySelector('#host').appendChild(wrapper)
    })
  })()`)
}

async function removeFixtureSurface(window: BrowserWindow, surfaceId: string): Promise<void> {
  await window.webContents.executeJavaScript(`(() => {
    document.querySelector('webview[data-surface-id=${JSON.stringify(surfaceId)}]')?.remove()
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

async function waitForFixtureTurn(delayMs: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, delayMs))
}

async function setFixtureVisibility(
  window: BrowserWindow,
  surfaceId: string,
  visible: boolean
): Promise<void> {
  await window.webContents.executeJavaScript(`(() => {
    const webview = document.querySelector('webview[data-surface-id=${JSON.stringify(surfaceId)}]')
    if (!webview) return
    const wrapper = webview.closest('.fixture-page')
    if (!wrapper) throw new Error('fixture page wrapper missing')
    // Match the production keep-alive sidebar's background-page rendering rules.
    wrapper.style.visibility = 'visible'
    wrapper.style.opacity = ${JSON.stringify(visible ? '1' : '0')}
    wrapper.style.pointerEvents = ${JSON.stringify(visible ? 'auto' : 'none')}
    wrapper.style.contentVisibility = 'visible'
    wrapper.inert = ${JSON.stringify(!visible)}
  })()`)
}

async function waitFor(predicate: () => boolean, errorMessage: string): Promise<void> {
  const deadline = Date.now() + 10_000
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error(errorMessage)
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

void main()
  .catch((error: unknown) => {
    process.stderr.write(
      `BrowserSurfaceManager fixture failed: ${error instanceof Error ? error.stack : 'unknown'}\n`
    )
    app.exit(1)
  })
  .finally(() => {
    rmSync(PROFILE_DIRECTORY, { force: true, recursive: true })
  })
