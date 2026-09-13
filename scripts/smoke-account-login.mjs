// Run after `pnpm build`. Uses disposable app data; does not log in or send verification mail.
import { _electron } from 'playwright'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const data = await mkdtemp(join(tmpdir(), 'captain-who-login-smoke-'))
let application
try {
  application = await _electron.launch({
    executablePath: require('electron'),
    args: [resolve('scripts/fixtures/account-login-smoke.cjs')],
    env: { ...process.env, CAPTAIN_WHO_AUTH_SMOKE_DATA: data },
    timeout: 30_000
  })
  const page = await application.firstWindow()
  await page.locator('.account-login').waitFor({ timeout: 30_000 })
  await page.locator('.account-login input[type="email"]').waitFor()
  const state = await page.evaluate(() => window.mycopilot.host.auth.getState())
  if (state.status !== 'signedOut' || state.profile !== null)
    throw new Error('Fresh installation must require login')
  const license = await page.evaluate(() => window.mycopilot.host.license.getState())
  if (license.status !== 'signedOut')
    throw new Error('Signed-out installation must not hold a license grant')
  await page.screenshot({ path: join(data, 'login-password.png') })
  await page.locator('.account-login__modes button').nth(1).click()
  await page.screenshot({ path: join(data, 'login-code.png') })
  const usage = await page.evaluate(async () => {
    await window.mycopilot.host.app.whenReady()
    return window.mycopilot.host.agent.getLocalTokenUsage({ from: '2026-01-01', to: '2026-12-31' })
  })
  if (usage.totalTokens !== '0' || usage.days.length !== 0 || usage.timezone !== 'Asia/Shanghai')
    throw new Error('Fresh local statistics must be empty and use the agreed timezone')
  console.log(
    JSON.stringify(
      {
        status: state.status,
        license: license.status,
        localTokens: usage.totalTokens,
        screenshots: [join(data, 'login-password.png'), join(data, 'login-code.png')]
      },
      null,
      2
    )
  )
} finally {
  if (application) await application.close()
}
