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
  await page.screenshot({ path: join(data, 'login-password.png') })
  await page.locator('.account-login__modes button').nth(1).click()
  await page.screenshot({ path: join(data, 'login-code.png') })
  console.log(
    JSON.stringify(
      {
        status: state.status,
        screenshots: [join(data, 'login-password.png'), join(data, 'login-code.png')]
      },
      null,
      2
    )
  )
} finally {
  if (application) await application.close()
}
