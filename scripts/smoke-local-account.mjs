/* eslint-disable @typescript-eslint/explicit-function-return-type -- This Electron smoke script uses runtime assertions for its JavaScript helpers. */
// Run after `pnpm build`. Exercises the production bundle with disposable data and no network.
import { _electron } from 'playwright'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createWriteStream } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const data = await mkdtemp(join(tmpdir(), 'captain-who-local-account-smoke-'))
const rounds = []
const startupChecks = []
let application
let attempt = 0

async function launch() {
  application = await _electron.launch({
    executablePath: require('electron'),
    args: [resolve('scripts/fixtures/local-account-smoke.cjs')],
    env: { ...process.env, CAPTAIN_WHO_LOCAL_ACCOUNT_SMOKE_DATA: data },
    timeout: 30_000
  })
  const processLog = createWriteStream(join(data, `main-${++attempt}.log`))
  application.process().stdout?.pipe(processLog, { end: false })
  application.process().stderr?.pipe(processLog, { end: false })
  application.process().once('exit', () => processLog.end())
  const page = await application.firstWindow()
  page.on('pageerror', (error) => processLog.write(`Renderer: ${error.message}\n`))
  await page.waitForFunction(() => Boolean(window.mycopilot?.host?.auth), { timeout: 30_000 })
  return page
}

async function finishRound(name) {
  const blocked = await application.evaluate(() => global.__captainWhoLocalAccountSmoke.blocked)
  assert.deepEqual(blocked, [], `${name} attempted remote networking`)
  rounds.push({ name, remoteRequests: blocked.length })
  await application.close()
  application = undefined
}

async function state(page) {
  return page.evaluate(() => window.mycopilot.host.auth.getState())
}

async function assertSignedIn(page) {
  await page.waitForFunction(async () => {
    const auth = await window.mycopilot.host.auth.getState()
    return auth.status === 'signedIn'
  })
  const startup = await page.evaluate(async () => {
    const hostReady = await Promise.race([
      window.mycopilot.host.app.whenReady().then(
        () => 'ready',
        (error) => `failed: ${error.message}`
      ),
      new Promise((resolve) => setTimeout(() => resolve('pending after 3s'), 3000))
    ])
    return {
      hostReady,
      workspaceInteractive:
        document.querySelector('.app-startup-root')?.getAttribute('data-interactive') === 'true'
    }
  })
  startupChecks.push(startup)
  const auth = await state(page)
  assert.equal(auth.profile.displayName, '大副')
  assert.equal(auth.profile.localAccount.username, 'captainwho')
  assert.equal(auth.remembered, true)
  const license = await page.evaluate(() => window.mycopilot.host.license.getState())
  assert.equal(license.status, 'allowed')
  assert.equal(license.expiresAt, null)
  const image = page.locator('.left-sidebar__account-button img')
  let avatar = null
  if ((await image.count()) > 0) {
    avatar = await image.evaluate((element) => ({
      src: element.src,
      loaded: element.complete && element.naturalWidth > 0
    }))
    assert.equal(avatar.loaded, true)
    assert.match(avatar.src, /(?:file:|data:image\/svg\+xml)/)
    assert.doesNotMatch(avatar.src, /brand-mark/)
    assert.match(await page.locator('.left-sidebar__account-button').textContent(), /大副/)
  }
  startup.avatarRendered = avatar !== null
  return { auth, avatar, startup }
}

try {
  let page = await launch()
  await page.locator('.account-login input[autocomplete="username"]').waitFor()
  assert.equal((await state(page)).status, 'signedOut')
  await page.locator('.account-login input[autocomplete="username"]').fill('captainwho')
  await page.locator('.account-login input[autocomplete="current-password"]').fill('captainwho')
  await page.locator('.account-login__submit').click()
  const initial = await assertSignedIn(page)
  const refresh = await page.evaluate(async () => ({
    auth: await window.mycopilot.host.auth.refreshProfile(),
    license: await window.mycopilot.host.license.refresh()
  }))
  assert.equal(refresh.auth.ok, true)
  assert.equal(refresh.license.status, 'allowed')
  if (initial.startup.workspaceInteractive)
    await page.locator('.left-sidebar__account-button').click()
  await page.screenshot({ path: join(data, 'local-account-signed-in.png') })
  await finishRound('fresh login and refresh')

  page = await launch()
  const restored = await assertSignedIn(page)
  assert.equal(
    restored.auth.profile.localAccount.avatarSeed,
    initial.auth.profile.localAccount.avatarSeed
  )
  if (initial.avatar && restored.avatar) assert.equal(restored.avatar.src, initial.avatar.src)
  if (restored.startup.workspaceInteractive) {
    await page.locator('.left-sidebar__account-button').click()
    await page.locator('.left-sidebar__account-menu [role="menuitem"]').last().click()
  } else {
    const logout = await page.evaluate(() => window.mycopilot.host.auth.logout())
    assert.equal(logout.ok, true)
  }
  await page.waitForFunction(
    async () => (await window.mycopilot.host.auth.getState()).status === 'signedOut'
  )
  const signedOutLicense = await page.evaluate(() => window.mycopilot.host.license.getState())
  assert.equal(signedOutLicense.status, 'signedOut')
  await finishRound('restored session and logout')

  page = await launch()
  await page.locator('.account-login input[autocomplete="username"]').waitFor()
  assert.equal((await state(page)).status, 'signedOut')
  const saved = JSON.parse(await readFile(join(data, 'local-account.json'), 'utf8'))
  assert.equal(saved.signedIn, false)
  assert.equal(saved.avatarSeed, initial.auth.profile.localAccount.avatarSeed)
  await finishRound('signed out after restart')
  const result = {
    authenticationPassed: true,
    rounds,
    startupChecks,
    limitation: startupChecks.every((check) => check.workspaceInteractive && check.avatarRendered)
      ? null
      : 'Production auth/license IPC passed; fresh Core/Host startup did not become ready, so interactive workspace and rendered avatar were not verified.',
    artifacts: data
  }
  await writeFile(join(data, 'result.json'), JSON.stringify(result, null, 2))
  console.log(JSON.stringify(result, null, 2))
} catch (error) {
  if (application) {
    const page = await application.firstWindow().catch(() => null)
    if (page) await page.screenshot({ path: join(data, 'failure.png') }).catch(() => {})
    const diagnostic = await page?.evaluate(async () => ({
      startupText: document.querySelector('.app-startup-screen')?.textContent,
      auth: await window.mycopilot.host.auth.getState(),
      license: await window.mycopilot.host.license.getState()
    }))
    const blocked = await application.evaluate(() => global.__captainWhoLocalAccountSmoke.blocked)
    await writeFile(join(data, 'failure.json'), JSON.stringify({ diagnostic, blocked }, null, 2))
  }
  console.error(`Local account smoke artifacts: ${data}`)
  throw error
} finally {
  if (application) await application.close()
}
