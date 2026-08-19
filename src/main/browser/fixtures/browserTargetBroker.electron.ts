import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BROWSER_WEBVIEW_PARTITION } from '@mycopilot/protocol'
import { app, BrowserWindow, session } from 'electron'
import type { WebContents } from 'electron'
import type { Browser, chromium as ChromiumType } from 'playwright'
import {
  formatBrowserFrameEditorCandidates,
  probeBrowserFrameEditors
} from '../BrowserFrameEditorProbe'
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
  const crossOriginServer = createServer((request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    response.end(frameFixtureHtml(request.url ?? '/frame'))
  })
  await listen(crossOriginServer, '::1')
  const crossOriginAddress = crossOriginServer.address()
  if (!crossOriginAddress || typeof crossOriginAddress === 'string') {
    throw new Error('cross-origin fixture server did not bind')
  }
  const crossOriginFrameUrl = `http://localhost:${crossOriginAddress.port}/frame?kind=cross`

  const server = createServer((request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    if (request.url?.startsWith('/frame')) {
      response.end(frameFixtureHtml(request.url))
      return
    }
    if (request.url?.startsWith('/nested')) {
      response.end(`<!doctype html><html><body>
        <iframe id="nested-child" src="/frame?kind=nested-child"></iframe>
      </body></html>`)
      return
    }
    if (request.url === '/interactive') {
      response.end(`<!doctype html>
        <html><body>
          <main>
            <h1>Managed Browser Fixture</h1>
            <label for="fixture-input">Message</label>
            <input id="fixture-input" />
            <button id="fixture-button">Apply</button>
            <output id="fixture-output">idle</output>
            <iframe id="same-frame" src="/frame?kind=same"></iframe>
            <iframe id="second-frame" src="/frame?kind=second"></iframe>
            <iframe id="nested-frame" src="/nested"></iframe>
            <iframe id="cross-frame" src="${crossOriginFrameUrl}"></iframe>
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
  await listen(server)
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
    let outOfProcessFrameAttached = false
    let outOfProcessFrameResumed = false
    const outOfProcessSessions = new Set<string>()
    selectedGuest.debugger.on('message', (_event, method, params, sessionId) => {
      if (method === 'Target.attachedToTarget' && isRecord(params)) {
        const targetInfo = params.targetInfo
        if (isRecord(targetInfo) && targetInfo.type === 'iframe') {
          outOfProcessFrameAttached = true
          if (typeof params.sessionId === 'string') outOfProcessSessions.add(params.sessionId)
        }
      }
      if (method === 'Page.frameNavigated' && outOfProcessSessions.has(sessionId)) {
        outOfProcessFrameResumed = true
      }
    })
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
    page.setDefaultNavigationTimeout(10_000)
    if (pages.some((candidate) => candidate.url().includes('/two'))) {
      throw new Error('second guest was exposed')
    }

    await page.goto(`${baseUrl}/interactive`)
    const ariaSnapshot = await page.locator('body').ariaSnapshot()
    if (!ariaSnapshot.includes('Managed Browser Fixture')) {
      throw new Error('accessibility snapshot did not include local fixture')
    }
    const frameEditorCandidates = await probeBrowserFrameEditors(page)
    const frameEditorProjection = formatBrowserFrameEditorCandidates(frameEditorCandidates)
    const frameEditorPaths = new Set(
      frameEditorCandidates.map((candidate) => candidate.framePath.join('.'))
    )
    await Promise.all(frameEditorCandidates.map(async (candidate) => candidate.element.dispose()))
    window.focus()
    selectedGuest.focus()
    await page.getByLabel('Message').click()
    await page.keyboard.type('hello')
    await page.getByRole('button', { name: 'Apply' }).click()
    const finalText = await page.locator('#fixture-output').textContent()
    if (finalText !== 'applied:hello') {
      throw new Error(`interaction result mismatch: ${finalText ?? 'null'}`)
    }

    const sameFrame = page.frameLocator('#same-frame')
    const keyEventInput = sameFrame.locator('#key-event-input')
    await keyEventInput.pressSequentially('ab')
    const keyEventTypes = await sameFrame.locator('body').getAttribute('data-event-types')
    const keyEventTargets = await sameFrame.locator('body').getAttribute('data-event-targets')
    await sameFrame.locator('#reset-events').click()

    const sameInput = sameFrame.locator('#frame-input')
    await sameInput.click()
    await page.keyboard.type('你好')
    const sameInputValue = await sameInput.inputValue()
    const sameEventTypes = await sameFrame.locator('body').getAttribute('data-event-types')
    const sameEventTargets = await sameFrame.locator('body').getAttribute('data-event-targets')

    const sameTextarea = sameFrame.locator('#frame-textarea')
    await sameTextarea.fill('多行\n你好')
    const sameTextareaValue = await sameTextarea.inputValue()

    const sameEditor = sameFrame.locator('#frame-editor')
    await sameEditor.fill('编辑器你好')
    const sameEditorText = await sameEditor.textContent()

    const blankEditor = sameFrame.locator('#blank-editor')
    await blankEditor.click()
    await page.keyboard.insertText('空白你好')
    const blankEditorText = await blankEditor.textContent()

    const secondInput = page.frameLocator('#second-frame').locator('#frame-input')
    await secondInput.fill('第二帧')
    const secondInputValue = await secondInput.inputValue()

    const nestedInput = page
      .frameLocator('#nested-frame')
      .frameLocator('#nested-child')
      .locator('#frame-input')
    await nestedInput.fill('嵌套你好')
    const nestedInputValue = await nestedInput.inputValue()

    const crossFrame = page.frameLocator('#cross-frame')
    const crossInput = crossFrame.locator('#frame-input')
    await crossInput.click()
    await page.keyboard.type('跨域你好')
    await page.keyboard.press('End')
    const crossInputValue = await crossInput.inputValue()
    const crossEventTypes = await crossFrame.locator('body').getAttribute('data-event-types')

    const crossEditor = crossFrame.locator('#frame-editor')
    await crossEditor.scrollIntoViewIfNeeded()
    const crossEditorBounds = await crossEditor.boundingBox()
    if (!crossEditorBounds) throw new Error('cross-frame editor bounds missing')
    await page.mouse.click(
      crossEditorBounds.x + crossEditorBounds.width / 2,
      crossEditorBounds.y + crossEditorBounds.height / 2
    )
    await page.keyboard.insertText('坐标你好')
    const coordinateEditorText = await crossEditor.textContent()

    const shadowEditor = sameFrame.locator('#shadow-editor')
    await shadowEditor.fill('影子你好')
    const shadowEditorText = await shadowEditor.textContent()

    const detachedFrame = page.frames().find((frame) => frame.url().includes('kind=second'))
    if (!detachedFrame) throw new Error('second fixture frame missing')
    await page.locator('#second-frame').evaluate((frame) => frame.remove())
    let detachedFrameRejected = false
    try {
      await detachedFrame.locator('#frame-input').inputValue({ timeout: 1_000 })
    } catch {
      detachedFrameRejected = true
    }
    if (!detachedFrameRejected) throw new Error('detached frame accepted an input operation')

    await page.locator('#same-frame').evaluate((frame: HTMLIFrameElement) => {
      frame.src = '/frame?kind=navigated'
    })
    const navigatedInput = page.frameLocator('#same-frame').locator('#frame-input')
    await navigatedInput.fill('导航后你好')
    const navigatedInputValue = await navigatedInput.inputValue()

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
        frameParity: {
          blankEditorText,
          coordinateEditorText,
          crossEventTypes,
          crossInputValue,
          detachedFrameRejected,
          keyEventTargets,
          keyEventTypes,
          navigatedInputValue,
          nestedInputValue,
          sameEditorText,
          sameEventTargets,
          sameEventTypes,
          sameInputValue,
          sameTextareaValue,
          secondInputValue,
          shadowEditorText
        },
        frameProbe: {
          candidateCount: frameEditorCandidates.length,
          distinctFramePaths: [...frameEditorPaths].sort(),
          safeProjection:
            frameEditorProjection.includes('safe metadata; page content and values omitted') &&
            !frameEditorProjection.includes(baseUrl) &&
            !frameEditorProjection.includes('你好')
        },
        focusSpoofContained: finalText === 'applied:hello',
        finalText,
        isolatedProfile: app.getPath('userData') === USER_DATA_DIRECTORY,
        noRemoteDebuggingPort: true,
        outOfProcessFrameAttached,
        outOfProcessFrameResumed,
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
    await new Promise<void>((resolve) => crossOriginServer.close(() => resolve()))
    app.quit()
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function frameFixtureHtml(requestUrl: string): string {
  const kind = new URL(requestUrl, 'http://fixture.invalid').searchParams.get('kind') ?? 'frame'
  return `<!doctype html><html><body data-kind="${kind}">
    <label for="frame-input">Frame input ${kind}</label>
    <input id="frame-input" />
    <label for="key-event-input">Key event input ${kind}</label>
    <input id="key-event-input" />
    <label for="frame-textarea">Frame textarea ${kind}</label>
    <textarea id="frame-textarea"></textarea>
    <div id="frame-editor" role="textbox" aria-label="Frame editor ${kind}" contenteditable="true"></div>
    <div id="blank-editor" contenteditable></div>
    <div id="shadow-host"></div>
    <button id="reset-events" type="button">Reset events</button>
    <script>
      if (${JSON.stringify(kind)} === 'cross') {
        // A hostile OOPIF must not be able to claim focus and receive text intended for the top
        // page. The managed transport probes a pristine isolated world, never these overrides.
        document.hasFocus = () => true
        Document.prototype.hasFocus = () => true
      }
      document.body.dataset.eventTypes = ''
      document.body.dataset.eventTargets = ''
      const eventTypes = []
      const eventTargets = []
      for (const eventName of ['keydown', 'beforeinput', 'input', 'keyup']) {
        document.addEventListener(eventName, event => {
          eventTypes.push(event.type)
          eventTargets.push(event.target && event.target.id ? event.target.id : 'unknown')
          document.body.dataset.eventTypes = eventTypes.join(',')
          document.body.dataset.eventTargets = eventTargets.join(',')
        }, true)
      }
      document.querySelector('#reset-events').addEventListener('click', () => {
        eventTypes.length = 0
        eventTargets.length = 0
        document.body.dataset.eventTypes = ''
        document.body.dataset.eventTargets = ''
      })
      const shadowRoot = document.querySelector('#shadow-host').attachShadow({ mode: 'open' })
      shadowRoot.innerHTML = '<div id="shadow-editor" role="textbox" aria-label="Shadow editor" contenteditable="true"></div>'
    </script>
  </body></html>`
}

async function listen(server: ReturnType<typeof createServer>, host = '127.0.0.1'): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, host, resolve)
  })
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
      `BrowserTargetBroker fixture failed: ${error instanceof Error ? error.stack : 'unknown'}\n`
    )
    app.exit(1)
  })
  .finally(() => {
    rmSync(USER_DATA_DIRECTORY, { force: true, recursive: true })
  })
