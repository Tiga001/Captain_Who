/* eslint-disable @typescript-eslint/explicit-function-return-type -- Native development smoke test. */

import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { _electron as electron } from 'playwright'

if (process.platform !== 'win32') throw new Error('Run this smoke test on native Windows')

const require = createRequire(import.meta.url)
const repositoryRoot = fileURLToPath(new URL('..', import.meta.url))
const temporaryRoot = await mkdtemp(join(tmpdir(), 'captainwho-windows-startup-'))
const userData = join(temporaryRoot, 'profile')
await mkdir(userData)
const wrapper = join(temporaryRoot, 'main.cjs')
await writeFile(
  wrapper,
  `const { app } = require('electron');\n` +
    `app.setPath('userData', ${JSON.stringify(userData)});\n` +
    `app.setAppPath(${JSON.stringify(repositoryRoot)});\n` +
    `require(${JSON.stringify(join(repositoryRoot, 'out', 'main', 'index.js'))});\n`
)

let application
let stderr = ''
const environment = { ...process.env }
delete environment.ELECTRON_RUN_AS_NODE
delete environment.ELECTRON_RENDERER_URL

async function within(promise, timeoutMs) {
  let timeout
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error('Windows startup smoke timed out')), timeoutMs)
      })
    ])
  } finally {
    clearTimeout(timeout)
  }
}

try {
  application = await electron.launch({
    executablePath: require('electron'),
    args: [wrapper],
    cwd: repositoryRoot,
    env: environment,
    timeout: 120_000
  })
  application.process().stderr.on('data', (chunk) => (stderr += chunk.toString()))
  const window = await application.firstWindow({ timeout: 120_000 })
  const pageErrors = []
  window.on('pageerror', (error) => pageErrors.push(error.message))
  await window.waitForFunction(() => Boolean(window.mycopilot?.host), undefined, {
    timeout: 120_000
  })
  await within(
    window.evaluate(() => window.mycopilot.host.app.whenReady()),
    120_000
  )
  const ping = await window.evaluate(() =>
    window.mycopilot.host.core.ping({ message: 'Windows startup smoke' })
  )
  assert.equal(ping.message, 'pong')
  assert.equal(ping.echo, 'Windows startup smoke')
  assert.equal(await application.evaluate(({ app }) => app.getPath('userData')), userData)
  assert.ok((await window.locator('body').innerText()).trim().length > 0)

  const terminal = await window.evaluate(async () => {
    const api = window.mycopilot.host.terminal
    const session = await api.createSession({ cols: 80, rows: 24 })
    try {
      await new Promise((resolve, reject) => {
        let output = ''
        const timer = setTimeout(() => {
          unsubscribe()
          reject(new Error('Windows terminal output timed out'))
        }, 15_000)
        const unsubscribe = api.subscribeSession(session.sessionId, {
          onOutput(event) {
            output += event.data
            api.acknowledgeOutput(session.sessionId, event.sequence)
            if (output.includes('CAPTAIN_WINDOWS_PTY_OK')) {
              clearTimeout(timer)
              unsubscribe()
              resolve()
            }
          },
          onExit() {
            clearTimeout(timer)
            unsubscribe()
            reject(new Error('Windows terminal exited before producing output'))
          }
        })
        api.writeInput(session.sessionId, "Write-Output ('CAPTAIN_WINDOWS_' + 'PTY_OK')\r")
      })
      return { shell: session.shell, processId: session.processId }
    } finally {
      await api.killSession(session.sessionId)
    }
  })
  assert.match(terminal.shell, /powershell\.exe$/i)
  assert.ok(terminal.processId > 0)
  assert.deepEqual(pageErrors, [])
  const screenshot = join(temporaryRoot, 'startup.png')
  await window.screenshot({ path: screenshot })
  console.log(JSON.stringify({ ready: true, ping: ping.message, terminal, screenshot, userData }))
} catch (error) {
  if (stderr) console.error(stderr)
  throw error
} finally {
  if (application) await application.close()
}
