import { createServer } from 'node:http'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  BROWSER_WEBVIEW_PARTITION,
  createBrowserSurfaceBootstrapUrl,
  type BrowserSurfaceCommand
} from '@mycopilot/protocol'
import { app, BrowserWindow, session } from 'electron'
import type { WebContents } from 'electron'
import { BrowserSurfaceManager } from '../BrowserSurfaceManager'
import { BrowserTargetBroker } from '../BrowserTargetBroker'
import {
  configureManagedWebviewHost,
  initializeManagedWebviewSessions
} from '../../webviews/managedWebviewSecurity'

const RESULT_MARKER = 'MYCOPILOT_BROWSER_SURFACE_RESULT='
const SURFACE_ID = 'right-sidebar-browser-electron-fixture'
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

  const broker = new BrowserTargetBroker(
    BROWSER_WEBVIEW_PARTITION,
    session.fromPartition(BROWSER_WEBVIEW_PARTITION)
  )
  let pendingEnsure: Extract<BrowserSurfaceCommand, { kind: 'ensureAttached' }> | undefined
  let guest: WebContents | undefined
  let ensureCommands = 0
  let closeCommands = 0

  const manager = new BrowserSurfaceManager({
    attachTimeoutMs: 10_000,
    broker,
    closeTimeoutMs: 5_000,
    resolveHost: () => window.webContents,
    sendCommand: (_host, command) => {
      if (command.kind === 'ensureAttached') {
        ensureCommands += 1
        pendingEnsure = command
        void ensureFixtureSurface(window, Boolean(guest && !guest.isDestroyed())).then(
          (alreadyAttached) => {
            if (alreadyAttached && pendingEnsure?.requestId === command.requestId) {
              manager.attach(window.webContents, {
                schemaVersion: 1,
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
  configureManagedWebviewHost(window.webContents, {
    targetRegistry: manager
  })
  window.webContents.on('did-attach-webview', (_event, attachedGuest) => {
    guest = attachedGuest
    const command = pendingEnsure
    if (!command) return
    manager.attach(window.webContents, {
      schemaVersion: 1,
      requestId: command.requestId,
      surfaceId: SURFACE_ID
    })
    pendingEnsure = undefined
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

    await setFixtureVisibility(window, false)
    const hiddenTitle = await firstPage.locator('h1').textContent()
    await setFixtureVisibility(window, true)
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
    await setFixtureVisibility(window, true)

    await manager.detachAutomation()
    const retainedAfterDetach = Boolean(guest && !guest.isDestroyed())
    const reattachedContext = await manager.getBrowserContext()
    const reattachedPage = reattachedContext.pages()[0]
    if (!reattachedPage) throw new Error('reattached page missing')
    const retainedText = await reattachedPage.locator('#output').textContent()

    await manager.closeSurface()
    await waitFor(() => Boolean(guest?.isDestroyed()), 'explicit close did not destroy target')
    const mainWindowAliveAfterClose = !window.isDestroyed()
    let oldTargetRejected = false
    try {
      await reattachedPage.title()
    } catch {
      oldTargetRejected = true
    }

    const replacementContext = await manager.getBrowserContext()
    const replacementPage = replacementContext.pages()[0]
    if (!replacementPage) throw new Error('replacement page missing')
    const replacementIsInert = replacementPage.url().startsWith('about:blank')

    await manager.shutdown()
    process.stdout.write(
      `${RESULT_MARKER}${JSON.stringify({
        ariaSnapshot: snapshot.includes('Browser Surface Fixture'),
        closeCommands,
        concurrentSingleFlight: firstContext === concurrentContext,
        ensureCommands,
        finalText,
        hiddenTitle,
        hiddenText,
        isolatedProfile: app.getPath('userData') === PROFILE_DIRECTORY,
        inputValueAfterFill,
        mainWindowAliveAfterClose,
        minimizedTitle,
        noRemoteDebuggingPort: true,
        oldTargetRejected,
        pageCount: initialPageCount,
        pressKeyObserved,
        replacementIsInert,
        retainedAfterDetach,
        retainedText,
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

async function setFixtureVisibility(window: BrowserWindow, visible: boolean): Promise<void> {
  await window.webContents.executeJavaScript(`(() => {
    const webview = document.querySelector('webview[data-surface-id=${JSON.stringify(SURFACE_ID)}]')
    if (webview) webview.hidden = ${JSON.stringify(!visible)}
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
