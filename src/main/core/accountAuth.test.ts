import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import { AuthService } from '../auth/AuthService'
import { AuthFailure, type AuthDriver, type CloudSession } from '../auth/CloudBaseAuthDriver'
import { parseAccountProfile } from '../auth/AccountApiClient'
import { SessionStore, type SessionStorage } from '../auth/SessionStore'
import { ACCOUNT_CONFIG, ACCOUNT_PAGES, ACCOUNT_SESSION_SCOPE } from '../auth/accountConfig'

const session: CloudSession = {
  access_token: 'test-access',
  refresh_token: 'test-refresh',
  email: 'test@example.com'
}
const profile = {
  userId: 'usr_test',
  displayName: 'Captain',
  email: session.email,
  avatarDataUrl: null,
  occupation: '',
  organization: ''
}
function setup() {
  const driver = {
    login: vi.fn().mockResolvedValue(session),
    restore: vi.fn().mockResolvedValue(session),
    refresh: vi.fn().mockResolvedValue(session),
    sendCode: vi.fn().mockResolvedValue(undefined),
    verifyCode: vi.fn().mockResolvedValue(session),
    logout: vi.fn().mockResolvedValue(undefined)
  } satisfies AuthDriver
  const store = {
    read: vi.fn().mockReturnValue(null),
    write: vi.fn().mockReturnValue(true),
    clear: vi.fn()
  } satisfies SessionStorage
  const fetchProfile = vi.fn().mockResolvedValue(profile)
  const service = new AuthService(driver, store, fetchProfile)
  return { service, driver, store, fetchProfile }
}
const login = (service: AuthService) =>
  service.login({ email: ' Test@Example.com ', password: 'test-password' })

describe('cloud account service', () => {
  it('does not accept an anonymous publishable identity as a login', async () => {
    const { service, driver } = setup()
    await service.restoreSession()
    expect(service.getState().status).toBe('signedOut')
    expect(driver.restore).not.toHaveBeenCalled()
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
  })
  it('requires active profile validation before granting new turns and only exposes profile DTOs', async () => {
    const { service, driver, fetchProfile } = setup()
    let finish!: (value: typeof profile) => void
    fetchProfile.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve
      })
    )
    const pending = login(service)
    await vi.waitFor(() => expect(fetchProfile).toHaveBeenCalled())
    expect(() => service.assertCanStartTurn()).toThrow()
    finish(profile)
    expect(await pending).toEqual({ ok: true })
    expect(driver.login).toHaveBeenCalledWith('test@example.com', 'test-password')
    expect(service.getState().profile).toEqual(profile)
    expect(() => service.assertCanStartTurn()).not.toThrow()
    expect(JSON.stringify(service.getState())).not.toContain('test-access')
    expect(JSON.stringify(service.getState())).not.toContain('test-password')
  })
  it('restores and validates encrypted sessions at startup', async () => {
    const { service, store, driver } = setup()
    store.read.mockReturnValue(session)
    expect(await service.restoreSession()).toEqual({ ok: true })
    expect(driver.restore).toHaveBeenCalledWith(session)
    expect(service.getState().status).toBe('signedIn')
  })
  it('preserves saved credentials on a transient startup outage', async () => {
    const { service, store, driver } = setup()
    store.read.mockReturnValue(session)
    driver.restore.mockRejectedValue(new AuthFailure('network'))
    expect(await service.restoreSession()).toEqual({ ok: false, error: 'network' })
    expect(store.clear).not.toHaveBeenCalled()
    expect(() => service.assertCanStartTurn()).toThrow()
  })
  it('refreshes once when the profile API rejects an expired token', async () => {
    const { service, driver, fetchProfile } = setup()
    fetchProfile.mockRejectedValueOnce(new AuthFailure('expired'))
    expect(await login(service)).toEqual({ ok: true })
    expect(driver.refresh).toHaveBeenCalledTimes(1)
    expect(fetchProfile).toHaveBeenCalledTimes(2)
  })
  it.each(['inactive', 'expired'] as const)(
    'clears definitively invalid %s sessions',
    async (reason) => {
      const { service, store, fetchProfile } = setup()
      fetchProfile.mockRejectedValue(new AuthFailure(reason))
      expect(await login(service)).toEqual({ ok: false, error: reason })
      expect(service.getState().profile).toBeNull()
      expect(store.clear).toHaveBeenCalled()
    }
  )
  it('does not sign out a running app for a transient profile outage', async () => {
    const { service, fetchProfile } = setup()
    await login(service)
    fetchProfile.mockRejectedValue(new AuthFailure('network'))
    await service.refreshProfile()
    expect(service.getState().status).toBe('signedIn')
  })

  it('blocks new turns when a previously valid account loses its required profile', async () => {
    const { service, fetchProfile } = setup()
    await login(service)
    fetchProfile.mockRejectedValue(new AuthFailure('profile'))
    await service.refreshProfile()
    expect(service.getState().status).toBe('error')
    expect(service.getState().profile).toBeNull()
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
  })
  it('logout immediately invalidates a pending login without any task lifecycle dependency', async () => {
    const { service, fetchProfile, store } = setup()
    let finish!: (value: typeof profile) => void
    fetchProfile.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve
      })
    )
    const pending = login(service)
    await vi.waitFor(() => expect(fetchProfile).toHaveBeenCalled())
    await service.logout()
    const writesBeforeFinish = store.write.mock.calls.length
    finish(profile)
    await pending
    expect(service.getState().status).toBe('signedOut')
    expect(service.getState().profile).toBeNull()
    expect(store.write).toHaveBeenCalledTimes(writesBeforeFinish)
  })
  it('keeps a pending refresh from restoring a logged-out account', async () => {
    const { service, driver, store } = setup()
    await login(service)
    let finish!: (value: CloudSession) => void
    driver.refresh.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve
      })
    )
    const pending = service.refreshProfile()
    await vi.waitFor(() => expect(driver.refresh).toHaveBeenCalled())
    await service.logout()
    const writes = store.write.mock.calls.length
    finish(session)
    await pending
    expect(service.getState().status).toBe('signedOut')
    expect(store.write).toHaveBeenCalledTimes(writes)
  })
  it('supports memory-only sessions and clears older account credentials before switching', async () => {
    const { service, store } = setup()
    store.write.mockReturnValue(false)
    expect(await login(service)).toEqual({ ok: true })
    expect(store.clear).toHaveBeenCalledBefore(store.write)
    expect(service.getState().remembered).toBe(false)
  })
  it('rate-limits code sending in Main, not just the button', async () => {
    const { service, driver } = setup()
    expect(await service.sendEmailCode('test@example.com')).toEqual({ ok: true })
    expect(await service.sendEmailCode('test@example.com')).toEqual({
      ok: false,
      error: 'rateLimit'
    })
    expect(driver.sendCode).toHaveBeenCalledTimes(1)
  })
  it('does not rate-limit a failed code request', async () => {
    const { service, driver } = setup()
    driver.sendCode.mockRejectedValueOnce(new AuthFailure('verificationUnavailable'))

    expect(await service.sendEmailCode('test@example.com')).toEqual({
      ok: false,
      error: 'verificationUnavailable'
    })
    expect(await service.sendEmailCode('test@example.com')).toEqual({ ok: true })
    expect(await service.sendEmailCode('test@example.com')).toEqual({
      ok: false,
      error: 'rateLimit'
    })
    expect(driver.sendCode).toHaveBeenCalledTimes(2)
  })
  it('verifies the code then checks the same account API gate', async () => {
    const { service, driver, fetchProfile } = setup()
    expect(await service.verifyEmailCode({ email: ' TEST@example.com ', code: '123456' })).toEqual({
      ok: true
    })
    expect(driver.verifyCode).toHaveBeenCalledWith('test@example.com', '123456')
    expect(fetchProfile).toHaveBeenCalled()
  })
})

describe('account profile contract', () => {
  const body = (avatarDataUrl: unknown = null) => ({
    data: {
      userId: 'usr_test',
      profile: { userId: 'usr_test', status: 'active', displayName: 'Captain', avatarDataUrl }
    }
  })
  it('uses identity email and accepts inline PNG/JPEG/WebP only', () => {
    for (const kind of ['png', 'jpeg', 'webp']) {
      const avatar = `data:image/${kind};base64,YQ==`
      expect(parseAccountProfile(body(avatar), session.email)).toMatchObject({
        email: session.email,
        avatarDataUrl: avatar
      })
    }
  })
  it.each([
    null,
    'https://example.com/avatar.png',
    'data:image/svg+xml;base64,YQ==',
    'data:image/gif;base64,YQ==',
    `data:image/png;base64,${'A'.repeat(20_000)}`
  ])('rejects unsafe or oversized avatars without blocking login', (avatar) => {
    expect(parseAccountProfile(body(avatar), session.email).avatarDataUrl).toBeNull()
  })
  it('rejects absent, mismatched and inactive profiles', () => {
    expect(() => parseAccountProfile({ data: { profile: null } }, session.email)).toThrow('profile')
    const mismatch = body()
    mismatch.data.profile.userId = 'other'
    expect(() => parseAccountProfile(mismatch, session.email)).toThrow('profile')
    const inactive = body()
    inactive.data.profile.status = 'disabled'
    expect(() => parseAccountProfile(inactive, session.email)).toThrow('inactive')
  })
})

describe('production account configuration', () => {
  it('uses a consistent production environment, public client key and website', () => {
    expect(ACCOUNT_CONFIG.env).toBe('captainwho-prod-d7f1jv0p4d37c981')
    expect(ACCOUNT_CONFIG.region).toBe('ap-shanghai')
    expect(ACCOUNT_CONFIG.api).toBe(
      `https://${ACCOUNT_CONFIG.env}-1479807831.ap-shanghai.app.tcloudbase.com/account-api`
    )
    const payload = JSON.parse(
      Buffer.from(ACCOUNT_CONFIG.publishableKey.split('.')[1], 'base64url').toString()
    )
    expect(payload.aud).toBe(ACCOUNT_CONFIG.env)
    expect(payload.role).toBe('anon')
    expect(payload.meta.platform).toBe('PublishableKey')
    expect(payload.is_system_admin).toBe(false)
    expect(JSON.parse(ACCOUNT_SESSION_SCOPE)).toEqual([
      ACCOUNT_CONFIG.env,
      ACCOUNT_CONFIG.region,
      ACCOUNT_CONFIG.api
    ])
    for (const url of Object.values(ACCOUNT_PAGES)) {
      expect(new URL(url).origin).toBe('https://captainwhoagent.com')
    }
  })
})

describe('encrypted session storage', () => {
  it.each([undefined, 'another-environment'])(
    'discards a session with scope %s without touching local user data',
    (scope) => {
      const directory = mkdtempSync(join(tmpdir(), 'captain-who-auth-test-'))
      const xor = (bytes: Buffer) => Buffer.from(bytes.map((byte) => byte ^ 0xaa))
      const encryption = {
        isEncryptionAvailable: () => true,
        encryptString: (value: string) => xor(Buffer.from(value)),
        decryptString: (value: Buffer) => xor(value).toString()
      }
      try {
        const encrypted = encryption.encryptString(JSON.stringify({ ...session, scope }))
        writeFileSync(join(directory, 'account-session.enc'), encrypted)
        writeFileSync(join(directory, 'account-session.enc.tmp'), encrypted)
        const localData = join(directory, 'local-data.sqlite')
        writeFileSync(localData, 'unchanged local chats and model configuration')
        const store = new SessionStore(directory, encryption, ACCOUNT_SESSION_SCOPE)
        expect(store.read()).toBeNull()
        expect(readdirSync(directory)).toEqual(['local-data.sqlite'])
        expect(readFileSync(localData, 'utf8')).toBe(
          'unchanged local chats and model configuration'
        )
        expect(store.read()).toBeNull()
      } finally {
        rmSync(directory, { recursive: true })
      }
    }
  )

  it('round trips scoped tokens atomically, without writing plaintext or profile data', () => {
    const directory = mkdtempSync(join(tmpdir(), 'captain-who-auth-test-'))
    const xor = (bytes: Buffer) => Buffer.from(bytes.map((byte) => byte ^ 0xaa))
    try {
      const encryption = {
        isEncryptionAvailable: () => true,
        encryptString: (value: string) => xor(Buffer.from(value)),
        decryptString: (value: Buffer) => xor(value).toString()
      }
      const store = new SessionStore(directory, encryption, ACCOUNT_SESSION_SCOPE)
      expect(store.read()).toBeNull()
      expect(store.write(session)).toBe(true)
      expect(store.read()).toEqual({
        access_token: session.access_token,
        refresh_token: session.refresh_token
      })
      const file = join(directory, 'account-session.enc')
      expect(JSON.parse(encryption.decryptString(readFileSync(file)))).toEqual({
        scope: ACCOUNT_SESSION_SCOPE,
        access_token: session.access_token,
        refresh_token: session.refresh_token
      })
      expect(new SessionStore(directory, encryption, ACCOUNT_SESSION_SCOPE).read()).toEqual({
        access_token: session.access_token,
        refresh_token: session.refresh_token
      })
      expect(readFileSync(file).includes(Buffer.from(session.access_token))).toBe(false)
      expect(statSync(file).mode & 0o777).toBe(0o600)
      expect(readdirSync(directory)).toEqual(['account-session.enc'])
      store.clear()
      expect(store.read()).toBeNull()
    } finally {
      rmSync(directory, { recursive: true })
    }
  })
})
