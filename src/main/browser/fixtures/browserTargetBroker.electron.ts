import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'
import { app, BrowserWindow, session } from 'electron'
import type { WebContents } from 'electron'
import type { Browser, chromium as ChromiumType } from 'playwright'
import { BrowserTargetBroker } from '../BrowserTargetBroker'

// Exercise the exact production partition identity. The Electron process still uses a temporary
// userData directory below, so this never opens or mutates the user's real managed-browser profile.
const PARTITION = BROWSER_WEBVIEW_PARTITION
const RESULT_MARKER = 'MYCOPILOT_BROWSER_BROKER_RESULT='
const USER_DATA_DIRECTORY = mkdtempSync(join(tmpdir(), 'mycopilot-browser-broker-profile-'))

app.setPath('userData', USER_DATA_DIRECTORY)

interface PlaywrightModule {
  chromium: typeof ChromiumType
}

async function main(): Promise<void> {
  if (process.argv.some((argument) => argument.startsWith('--remote-debugging-port'))) {
    throw new Error('remote debugging port is forbidden')
  }

  await app.whenReady()
  const server = createServer((request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    if (request.url === '/interactive') {
      response.end(`<!doctype html>
        <html><body>
          <main>
            <h1>Managed Browser Fixture</h1>
            <label for="fixture-input">Message</label>
            <input id="fixture-input" />
            <button id="fixture-button">Apply</button>
            <output id="fixture-output">idle</output>
          </main>
          <script>
            document.querySelector('#fixture-button').addEventListener('click', () => {
              document.querySelector('#fixture-output').textContent =
                'applied:' + document.querySelector('#fixture-input').value
            })
          </script>
        </body></html>`)
      return
    }
    response.end(`<!doctype html><html><body><h1>${request.url}</h1></body></html>`)
  })
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('fixture server did not bind')
  const baseUrl = `http://127.0.0.1:${address.port}`

  const broker = new BrowserTargetBroker(PARTITION, session.fromPartition(PARTITION))
  const guests: WebContents[] = []
  const window = new BrowserWindow({
    height: 240,
    opacity: 0,
    show: true,
    skipTaskbar: true,
    width: 320,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      webviewTag: true
    }
  })

  window.webContents.on('will-attach-webview', (_event, preferences, params) => {
    delete preferences.preload
    preferences.contextIsolation = true
    preferences.nodeIntegration = false
    preferences.partition = PARTITION
    preferences.sandbox = true
    preferences.webSecurity = true
    params.partition = PARTITION
  })
  window.webContents.on('did-attach-webview', (_event, guest) => {
    broker.registerManagedGuest({ guest, host: window.webContents, partition: PARTITION })
    guests.push(guest)
  })

  let browser: Browser | undefined
  try {
    const hostHtml = `<!doctype html><html><body>
      <webview partition="${PARTITION}" src="${baseUrl}/one"></webview>
      <webview partition="${PARTITION}" src="${baseUrl}/two"></webview>
    </body></html>`
    await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(hostHtml)}`)
    await waitFor(() => guests.length === 2, 'managed guests did not attach')
    await Promise.all(guests.map((guest) => waitForGuestLoad(guest)))
    guests.sort((left, right) => left.getURL().localeCompare(right.getURL()))
    const selectedGuest = guests[0]
    const secondGuest = guests[1]
    if (!selectedGuest || !secondGuest) throw new Error('fixture guests missing')

    broker.claimSurface({
      generation: 1,
      guestWebContentsId: selectedGuest.id,
      host: window.webContents,
      surfaceId: 'fixture-selected'
    })
    broker.claimSurface({
      generation: 1,
      guestWebContentsId: secondGuest.id,
      host: window.webContents,
      surfaceId: 'fixture-second'
    })

    const transport = await broker.connect('fixture-selected')
    const requireFromWorkspace = createRequire(join(process.cwd(), 'package.json'))
    const { chromium } = requireFromWorkspace('playwright') as PlaywrightModule
    browser = await chromium.connectOverCDP(transport, {
      isLocal: true,
      noDefaults: true,
      timeout: 15_000
    })
    const contexts = browser.contexts()
    if (contexts.length !== 1) throw new Error('unexpected browser context count')
    const pages = contexts[0]?.pages() ?? []
    if (pages.length !== 1) throw new Error('broker exposed more than one page')
    const page = pages[0]
    if (!page) throw new Error('selected page missing')
    if (pages.some((candidate) => candidate.url().includes('/two'))) {
      throw new Error('second guest was exposed')
    }

    await page.goto(`${baseUrl}/interactive`)
    const ariaSnapshot = await page.locator('body').ariaSnapshot()
    if (!ariaSnapshot.includes('Managed Browser Fixture')) {
      throw new Error('accessibility snapshot did not include local fixture')
    }
    window.focus()
    selectedGuest.focus()
    await page.getByLabel('Message').click()
    await page.keyboard.type('hello')
    await page.getByRole('button', { name: 'Apply' }).click()
    const finalText = await page.locator('#fixture-output').textContent()
    if (finalText !== 'applied:hello') {
      throw new Error(`interaction result mismatch: ${finalText ?? 'null'}`)
    }

    let createTargetRejected = false
    try {
      await contexts[0]?.newPage()
    } catch {
      createTargetRejected = true
    }
    if (!createTargetRejected) throw new Error('Target.createTarget was not rejected')

    selectedGuest.close()
    await waitFor(() => broker.snapshot().activeConnections === 0, 'target did not disconnect')
    let closedTargetRejected = false
    try {
      await page.title()
    } catch {
      closedTargetRejected = true
    }
    if (!closedTargetRejected) throw new Error('closed target still accepted commands')

    const selectedDebuggerDetached =
      selectedGuest.isDestroyed() || !selectedGuest.debugger.isAttached()
    broker.dispose()
    const snapshot = broker.snapshot()
    process.stdout.write(
      `${RESULT_MARKER}${JSON.stringify({
        ariaSnapshot: ariaSnapshot.includes('Managed Browser Fixture'),
        closedTargetRejected,
        createTargetRejected,
        finalText,
        isolatedProfile: app.getPath('userData') === USER_DATA_DIRECTORY,
        noRemoteDebuggingPort: true,
        onlySelectedGuest: true,
        selectedDebuggerDetached,
        snapshot
      })}\n`
    )
  } finally {
    await browser?.close().catch(() => undefined)
    broker.dispose()
    if (!window.isDestroyed()) window.destroy()
    await new Promise<void>((resolve) => server.close(() => resolve()))
    app.quit()
  }
}

async function waitForGuestLoad(guest: WebContents): Promise<void> {
  if (!guest.isLoading()) return
  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => {
      cleanup()
      reject(new Error('guest load timed out'))
    }, 10_000)
    const cleanup = () => {
      clearTimeout(timeout)
      guest.off('did-finish-load', handleLoad)
      guest.off('did-fail-load', handleFailure)
    }
    const handleLoad = () => {
      cleanup()
      resolve()
    }
    const handleFailure = () => {
      cleanup()
      reject(new Error('guest load failed'))
    }
    guest.once('did-finish-load', handleLoad)
    guest.once('did-fail-load', handleFailure)
  })
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
      `BrowserTargetBroker fixture failed: ${error instanceof Error ? error.message : 'unknown'}\n`
    )
    app.exit(1)
  })
  .finally(() => {
    rmSync(USER_DATA_DIRECTORY, { force: true, recursive: true })
  })
