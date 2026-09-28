import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AuthService } from '../auth/AuthService'
import { AuthFailure, type AuthDriver, type CloudSession } from '../auth/CloudBaseAuthDriver'
import { LocalAccountStore } from '../auth/LocalAccountStore'
import type { SessionStorage } from '../auth/SessionStore'

const credentials = { email: 'captainwho', password: 'captainwho' }
const cloudSession: CloudSession = {
  access_token: 'cloud-access',
  refresh_token: 'cloud-refresh',
  email: 'test@example.com'
}
const cloudProfile = {
  userId: 'cloud-user',
  displayName: 'Cloud user',
  email: cloudSession.email,
  avatarDataUrl: null,
  occupation: '',
  organization: ''
}
const directories: string[] = []
function setup() {
  const directory = mkdtempSync(join(tmpdir(), 'captain-who-local-account-'))
  directories.push(directory)
  const localStore = new LocalAccountStore(directory)
  const driver = {
    login: vi.fn().mockRejectedValue(new AuthFailure('network')),
    restore: vi.fn().mockRejectedValue(new AuthFailure('network')),
    refresh: vi.fn().mockRejectedValue(new AuthFailure('network')),
    sendCode: vi.fn().mockRejectedValue(new AuthFailure('network')),
    verifyCode: vi.fn().mockRejectedValue(new AuthFailure('network')),
    logout: vi.fn().mockRejectedValue(new AuthFailure('network'))
  } satisfies AuthDriver
  const cloudStore = {
    read: vi.fn().mockReturnValue(null),
    write: vi.fn().mockReturnValue(true),
    clear: vi.fn()
  } satisfies SessionStorage
  const fetchProfile = vi.fn().mockResolvedValue(cloudProfile)
  const createService = () =>
    new AuthService(driver, cloudStore, fetchProfile, new LocalAccountStore(directory))
  return {
    directory,
    localStore,
    driver,
    cloudStore,
    fetchProfile,
    createService,
    service: createService()
  }
}

afterEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true })
})

describe('built-in offline account', () => {
  it('logs in, refreshes, restores and logs out without invoking cloud auth or profile APIs', async () => {
    const { service, driver, cloudStore, fetchProfile } = setup()
    expect(await service.login(credentials)).toEqual({ ok: true })
    expect(service.isLocalAccount()).toBe(true)
    expect(service.getState()).toMatchObject({
      status: 'signedIn',
      remembered: true,
      error: null,
      profile: {
        userId: 'local:captainwho',
        displayName: '大副',
        email: '',
        localAccount: { username: 'captainwho' }
      }
    })
    expect(() => service.assertCanStartTurn()).not.toThrow()
    // No fake bearer is ever sent to a cloud API, even for the current local user.
    expect(() => service.getAccessToken('local:captainwho')).toThrow('ACCOUNT_LOGIN_REQUIRED')
    expect(await service.refreshProfile()).toEqual({ ok: true })
    expect(await service.refreshProfile(false)).toEqual({ ok: true })
    expect(await service.restoreSession()).toEqual({ ok: true })
    expect(await service.logout()).toEqual({ ok: true })
    expect(service.isLocalAccount()).toBe(false)
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
    for (const operation of Object.values(driver)) expect(operation).not.toHaveBeenCalled()
    expect(fetchProfile).not.toHaveBeenCalled()
    expect(cloudStore.read).not.toHaveBeenCalled()
    expect(cloudStore.write).not.toHaveBeenCalled()
  })

  it('restores after a cold restart years later with the same random avatar, without Keychain or cloud', async () => {
    const { service, createService, cloudStore, driver, fetchProfile, directory } = setup()
    await service.login(credentials)
    const firstProfile = service.getState().profile
    expect(firstProfile?.localAccount?.avatarSeed).toMatch(/^[0-9a-f-]{36}$/)
    cloudStore.read.mockImplementation(() => {
      throw new Error('Keychain unavailable')
    })
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2046-09-28T00:00:00Z'))
    const restarted = createService()
    expect(await restarted.restoreSession()).toEqual({ ok: true })
    expect(restarted.getState().profile).toEqual(firstProfile)
    expect(restarted.isLocalAccount()).toBe(true)
    expect(cloudStore.read).not.toHaveBeenCalled()
    expect(driver.restore).not.toHaveBeenCalled()
    expect(fetchProfile).not.toHaveBeenCalled()
    const saved = JSON.parse(readFileSync(join(directory, 'local-account.json'), 'utf8'))
    expect(Object.keys(saved).sort()).toEqual(['avatarSeed', 'signedIn', 'username', 'version'])
    expect(statSync(join(directory, 'local-account.json')).mode & 0o777).toBe(0o600)
    expect(readdirSync(directory)).toEqual(['local-account.json'])
  })

  it.each(['', 'wrong-password', 'CaptainWho', 'captainwho '])(
    'rejects wrong local password %j without network',
    async (password) => {
      const { service, driver, localStore } = setup()
      expect(await service.login({ email: 'captainwho', password })).toEqual({
        ok: false,
        error: 'credentials'
      })
      expect(service.isLocalAccount()).toBe(false)
      expect(localStore.read()).toBeNull()
      expect(driver.login).not.toHaveBeenCalled()
    }
  )

  it('does not treat arbitrary names or email-code requests as the local account', async () => {
    const { service, driver } = setup()
    expect(await service.login({ email: 'someone', password: 'captainwho' })).toEqual({
      ok: false,
      error: 'credentials'
    })
    expect(await service.sendEmailCode('captainwho')).toEqual({ ok: false, error: 'credentials' })
    expect(await service.verifyEmailCode({ email: 'captainwho', code: '123456' })).toEqual({
      ok: false,
      error: 'credentials'
    })
    for (const operation of Object.values(driver)) expect(operation).not.toHaveBeenCalled()
  })

  it('can sign in immediately while cloud restoration is stuck, and ignores its late result', async () => {
    const { service, driver, cloudStore, fetchProfile } = setup()
    cloudStore.read.mockReturnValue(cloudSession)
    let finish!: (session: CloudSession) => void
    driver.restore.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve
      })
    )
    const restore = service.restoreSession()
    await vi.waitFor(() => expect(driver.restore).toHaveBeenCalled())
    expect(await service.login(credentials)).toEqual({ ok: true })
    const localProfile = service.getState().profile
    finish(cloudSession)
    expect(await restore).toEqual({ ok: false, error: 'expired' })
    expect(service.getState().profile).toEqual(localProfile)
    expect(service.isLocalAccount()).toBe(true)
    expect(fetchProfile).not.toHaveBeenCalled()
    expect(cloudStore.write).not.toHaveBeenCalled()
  })

  it('fences a late cloud refresh failure after switching to the local account', async () => {
    const { service, driver, localStore } = setup()
    driver.login.mockResolvedValue(cloudSession)
    await service.login({ email: cloudSession.email, password: 'cloud-password' })
    let reject!: (error: Error) => void
    driver.refresh.mockReturnValue(
      new Promise((_resolve, fail) => {
        reject = fail
      })
    )
    const refresh = service.refreshProfile()
    await vi.waitFor(() => expect(driver.refresh).toHaveBeenCalled())
    await service.login(credentials)
    reject(new AuthFailure('expired'))
    await refresh
    expect(service.isLocalAccount()).toBe(true)
    expect(localStore.read()).not.toBeNull()
    expect(await service.refreshProfile()).toEqual({ ok: true })
  })

  it('serializes a later cloud login behind a detached cloud restore while local login remains immediate', async () => {
    const { service, driver, cloudStore, fetchProfile } = setup()
    cloudStore.read.mockReturnValue(cloudSession)
    let finishRestore!: (session: CloudSession) => void
    driver.restore.mockReturnValue(
      new Promise((resolve) => {
        finishRestore = resolve
      })
    )
    driver.login.mockResolvedValue(cloudSession)
    const restore = service.restoreSession()
    await vi.waitFor(() => expect(driver.restore).toHaveBeenCalledOnce())
    expect(await service.login(credentials)).toEqual({ ok: true })
    expect(await service.logout()).toEqual({ ok: true })

    const login = service.login({ email: cloudSession.email, password: 'cloud-password' })
    // The later cloud operation has entered AuthService, but cannot mutate the shared SDK yet.
    await vi.waitFor(() => expect(cloudStore.clear).toHaveBeenCalledTimes(3))
    expect(driver.login).not.toHaveBeenCalled()
    expect(driver.logout).not.toHaveBeenCalled()
    finishRestore(cloudSession)
    expect(await restore).toEqual({ ok: false, error: 'expired' })
    expect(await login).toEqual({ ok: true })
    expect(driver.login).toHaveBeenCalledOnce()
    expect(fetchProfile).toHaveBeenCalledOnce()
    expect(service.getState().profile).toEqual(cloudProfile)
  })

  it('queues cloud revocation before a new cloud login even if the old profile request is still pending', async () => {
    const { service, driver, fetchProfile } = setup()
    const operations: string[] = []
    driver.login.mockImplementation(async () => {
      operations.push('login')
      return cloudSession
    })
    driver.logout.mockImplementation(async () => {
      operations.push('logout')
    })
    let finishProfile!: (profile: typeof cloudProfile) => void
    fetchProfile.mockReturnValueOnce(
      new Promise((resolve) => {
        finishProfile = resolve
      })
    )
    const previousLogin = service.login({ email: cloudSession.email, password: 'cloud-password' })
    await vi.waitFor(() => expect(fetchProfile).toHaveBeenCalledOnce())
    await service.logout()
    expect(await service.login(credentials)).toEqual({ ok: true })
    expect(await service.login({ email: cloudSession.email, password: 'cloud-password' })).toEqual({
      ok: true
    })
    expect(operations).toEqual(['login', 'logout', 'login'])

    finishProfile(cloudProfile)
    expect(await previousLogin).toEqual({ ok: false, error: 'expired' })
    expect(operations).toEqual(['login', 'logout', 'login'])
    expect(service.getState().profile).toEqual(cloudProfile)
    expect(service.getAccessToken(cloudProfile.userId)).toBe(cloudSession.access_token)
  })

  it('skips an obsolete cloud operation queued behind an older restore after another local login', async () => {
    const { service, driver, cloudStore } = setup()
    cloudStore.read.mockReturnValue(cloudSession)
    let finishRestore!: (session: CloudSession) => void
    driver.restore.mockReturnValue(
      new Promise((resolve) => {
        finishRestore = resolve
      })
    )
    const restore = service.restoreSession()
    await vi.waitFor(() => expect(driver.restore).toHaveBeenCalledOnce())
    await service.login(credentials)
    const cloudLogin = service.login({ email: cloudSession.email, password: 'cloud-password' })
    await vi.waitFor(() => expect(cloudStore.clear).toHaveBeenCalledTimes(2))
    expect(await service.login(credentials)).toEqual({ ok: true })
    finishRestore(cloudSession)
    expect(await restore).toEqual({ ok: false, error: 'expired' })
    expect(await cloudLogin).toEqual({ ok: false, error: 'expired' })
    expect(driver.login).not.toHaveBeenCalled()
    expect(service.isLocalAccount()).toBe(true)
  })

  it('logout is persistent while retaining the assigned avatar for the next explicit login', async () => {
    const { service, createService } = setup()
    await service.login(credentials)
    const profile = service.getState().profile
    await service.logout()
    const restarted = createService()
    expect(await restarted.restoreSession()).toEqual({ ok: true })
    expect(restarted.getState().status).toBe('signedOut')
    expect(await restarted.login(credentials)).toEqual({ ok: true })
    expect(restarted.getState().profile).toEqual(profile)
  })

  it('switching back to a cloud account clears permanent local admission even when cloud login fails', async () => {
    const { service, createService, localStore, driver } = setup()
    await service.login(credentials)
    expect(await service.login({ email: cloudSession.email, password: 'cloud-password' })).toEqual({
      ok: false,
      error: 'network'
    })
    expect(service.isLocalAccount()).toBe(false)
    expect(localStore.read()).toBeNull()
    const restarted = createService()
    await restarted.restoreSession()
    expect(restarted.getState().status).toBe('signedOut')
    expect(driver.login).toHaveBeenCalledWith(cloudSession.email, 'cloud-password')
  })

  it('does not derive local authority from a cloud profile with a matching id or local metadata', async () => {
    const { service, driver, fetchProfile } = setup()
    await service.login(credentials)
    const profile = service.getState().profile!
    driver.login.mockResolvedValue(cloudSession)
    fetchProfile.mockResolvedValue(profile)
    await service.login({ email: cloudSession.email, password: 'cloud-password' })
    expect(service.isLocalAccount()).toBe(false)
    expect(service.getAccessToken(profile.userId)).toBe(cloudSession.access_token)
  })

  it('refuses remembered login if saving local state fails', async () => {
    const { driver, cloudStore, fetchProfile } = setup()
    const service = new AuthService(driver, cloudStore, fetchProfile, {
      read: () => null,
      clear: () => {},
      activate: () => {
        throw new Error('Read-only disk')
      }
    })
    expect(await service.login(credentials)).toEqual({ ok: false, error: 'storage' })
    expect(service.isLocalAccount()).toBe(false)
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
  })

  it('discards malformed local session data and repairs it only after correct credentials', async () => {
    const { service, directory, localStore } = setup()
    writeFileSync(join(directory, 'local-account.json'), '{ broken')
    expect(await service.restoreSession()).toEqual({ ok: true })
    expect(service.getState().status).toBe('signedOut')
    expect(await service.login(credentials)).toEqual({ ok: true })
    expect(localStore.read()).not.toBeNull()
  })

  it('allocates independent random avatar seeds on separate installations', async () => {
    const first = setup()
    const second = setup()
    await first.service.login(credentials)
    await second.service.login(credentials)
    expect(first.localStore.read()?.avatarSeed).not.toBe(second.localStore.read()?.avatarSeed)
  })
})
