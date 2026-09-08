import { createServer } from 'node:http'
import { once } from 'node:events'
import { createRequire } from 'node:module'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { app, BrowserWindow, type WebContents } from 'electron'
import type { Browser, chromium as ChromiumType } from 'playwright'
import { ElectronGuestCdpTransport } from '../ElectronGuestCdpTransport'
import { GuestPdfError, printManagedGuestToPdf } from '../ElectronGuestPdfPrinter'

// The test owns this directory, so it also removes the profile if this process is force-killed.
const profile = process.argv[3]
if (!profile) throw new Error('fixture profile destination missing')
mkdirSync(profile, { recursive: true, mode: 0o700 })
app.setPath('userData', profile)
app.commandLine.appendSwitch('site-per-process')
app.once('quit', () => rmSync(profile, { force: true, recursive: true }))

async function main(): Promise<void> {
  await app.whenReady()
  let accountRequests = 0
  const server = createServer((request, response) => {
    response.setHeader('content-type', 'text/html; charset=utf-8')
    if (request.url === '/login') {
      response.setHeader('set-cookie', 'pdf_fixture_login=yes; HttpOnly; Path=/')
      response.writeHead(302, { location: '/account' }).end()
    } else if (request.url === '/account') {
      accountRequests += 1
      const loggedIn = request.headers.cookie?.includes('pdf_fixture_login=yes')
      response.end(`<!doctype html><html><style>
        @media print { .screen-only { display:none } }
        input, textarea { width: 500px }
      </style><body><h1>${loggedIn ? 'AUTHENTICATED_ACCOUNT' : 'ANONYMOUS'}</h1>
        <div class="screen-only">SCREEN_ONLY_NOT_PRINTED</div>
        <label>Input<input id="input" value="ORIGINAL_INPUT_VALUE"></label>
        <label>Notes<textarea id="notes">ORIGINAL_NOTES_VALUE</textarea></label>
        <p id="dynamic"></p><iframe id="same" src="/frame"></iframe>
      </body></html>`)
    } else response.end('<h2>SAME_FRAME_CONTENT</h2>')
  })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('fixture address unavailable')
  const base = `http://127.0.0.1:${address.port}`
  const remoteServer = createServer((_request, response) => response.end('<h2>REMOTE_FRAME</h2>'))
  remoteServer.listen(0, '::1')
  await once(remoteServer, 'listening')
  const remoteAddress = remoteServer.address()
  if (!remoteAddress || typeof remoteAddress === 'string')
    throw new Error('remote address unavailable')

  const window = new BrowserWindow({
    show: true,
    opacity: 0,
    skipTaskbar: true,
    width: 900,
    height: 700,
    webPreferences: {
      webviewTag: true,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  })
  let browser: Browser | undefined
  let transport: ElectronGuestCdpTransport | undefined
  let recoveryWindow: BrowserWindow | undefined
  try {
    const attached = once(window.webContents, 'did-attach-webview')
    await window.loadURL(
      `data:text/html,${encodeURIComponent(
        `<webview partition="pdf-fixture" style="width:850px;height:650px" src="${base}/login"></webview>`
      )}`
    )
    const guest = (await attached)[1] as WebContents
    if (guest.isLoading()) await once(guest, 'did-stop-loading')
    transport = new ElectronGuestCdpTransport(guest, () => undefined)
    await transport.attach()
    const requireFromWorkspace = createRequire(join(process.cwd(), 'package.json'))
    const { chromium } = requireFromWorkspace('playwright') as { chromium: typeof ChromiumType }
    browser = await chromium.connectOverCDP(transport, {
      isLocal: true,
      noDefaults: true,
      timeout: 10_000
    })
    const page = browser.contexts()[0].pages()[0]
    await page.locator('#input').fill('UNSAVED_INPUT_VALUE')
    await page.locator('#notes').fill('UNSAVED_NOTES_VALUE')
    await page.locator('#dynamic').evaluate((element) => {
      element.textContent = 'LIVE_DYNAMIC_CONTENT'
    })
    const before = accountRequests
    const pdf = await page.pdf()
    const pdfPath = process.argv[2]
    if (!pdfPath) throw new Error('fixture PDF destination missing')
    writeFileSync(pdfPath, pdf)
    const currentUrlPreserved = guest.getURL() === `${base}/account`
    const requestsStable = before === accountRequests

    await page.evaluate((remoteUrl) => {
      const iframe = document.createElement('iframe')
      iframe.id = 'remote'
      iframe.src = remoteUrl
      document.body.append(iframe)
    }, `http://localhost:${remoteAddress.port}/remote`)
    const deadline = Date.now() + 5_000
    while (
      !guest.mainFrame.framesInSubtree.some(
        (frame) => frame.processId !== guest.mainFrame.processId
      ) &&
      Date.now() < deadline
    ) {
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    const actualOopif = guest.mainFrame.framesInSubtree.some(
      (frame) => frame.processId !== guest.mainFrame.processId
    )
    const oopifRejected = await page.pdf().then(
      () => false,
      (error: Error) => error.message.includes('browser.pdf_unavailable:cross_process_frame')
    )
    await page.locator('#remote').evaluate((element) => element.remove())
    const retry = await page.pdf()
    await page.evaluate((remoteUrl) => {
      document.defaultView!.addEventListener(
        'beforeprint',
        () => {
          const iframe = document.createElement('iframe')
          iframe.src = remoteUrl
          document.body.append(iframe)
        },
        { once: true }
      )
    }, `http://localhost:${remoteAddress.port}/during-print`)
    const dynamicPrintOutcome = await printManagedGuestToPdf(
      guest,
      { printBackground: false },
      { timeoutMs: 800 }
    ).then(
      () => 'unexpected_success',
      (error: unknown) => (error instanceof GuestPdfError ? error.reason : 'unexpected_error')
    )
    recoveryWindow = new BrowserWindow({ show: false, webPreferences: { sandbox: true } })
    await recoveryWindow.loadURL(`${base}/frame`)
    const pendingPrintRetained =
      dynamicPrintOutcome !== 'timed_out' ||
      (await printManagedGuestToPdf(recoveryWindow.webContents, {}, { timeoutMs: 800 }).then(
        () => false,
        (error: unknown) =>
          error instanceof GuestPdfError && error.reason === 'native_print_pending'
      ))
    // Simulate the user closing the originating page. Production never closes a page to cancel
    // printing; this verifies that the documented recovery releases the actual native reservation.
    window.destroy()
    const recovered = await printManagedGuestToPdf(
      recoveryWindow.webContents,
      {},
      { timeoutMs: 2_000 }
    )
    console.log(
      `MYCOPILOT_GUEST_PDF_RESULT=${JSON.stringify({
        actualOopif,
        currentUrlPreserved,
        requestsStable,
        oopifRejected,
        afterRejectedPrintWorks: retry.subarray(0, 5).toString() === '%PDF-',
        dynamicPrintOutcome,
        pendingPrintRetained,
        recoveredAfterPageClose: recovered.subarray(0, 5).toString() === '%PDF-'
      })}`
    )
  } finally {
    transport?.close()
    await browser?.close().catch(() => undefined)
    window.destroy()
    recoveryWindow?.destroy()
    server.close()
    remoteServer.close()
  }
}

main().then(
  () => app.quit(),
  (error: unknown) => {
    console.error(error)
    app.exit(1)
  }
)
