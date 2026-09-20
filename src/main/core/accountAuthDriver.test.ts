import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AuthError } from '@cloudbase/js-sdk/oauth'
const sdk = vi.hoisted(() => ({
  init: vi.fn(),
  login: vi.fn(),
  restore: vi.fn(),
  refresh: vi.fn(),
  sendCode: vi.fn(),
  verify: vi.fn(),
  getUser: vi.fn(),
  logout: vi.fn()
}))
vi.mock('@cloudbase/js-sdk', () => ({ default: { init: sdk.init } }))
import { CloudBaseAuthDriver, sdkFailure } from '../auth/CloudBaseAuthDriver'
import { AuthService } from '../auth/AuthService'

const tokens = { access_token: 'test-access', refresh_token: 'test-refresh' }
const result = { data: { session: tokens }, error: null }
beforeEach(() => {
  vi.resetAllMocks()
  sdk.init.mockReturnValue({
    auth: () => ({
      signInWithPassword: sdk.login,
      setSession: sdk.restore,
      refreshSession: sdk.refresh,
      signInWithOtp: sdk.sendCode,
      getUser: sdk.getUser,
      signOut: sdk.logout
    })
  })
  sdk.login.mockResolvedValue(result)
  sdk.restore.mockResolvedValue(result)
  sdk.refresh.mockResolvedValue(result)
  sdk.getUser.mockResolvedValue({
    data: { user: { email: 'identity@example.com', is_anonymous: false } },
    error: null
  })
  sdk.sendCode.mockResolvedValue({ data: { verifyOtp: sdk.verify }, error: null })
  sdk.verify.mockResolvedValue(result)
  sdk.logout.mockResolvedValue({ error: null })
})
describe('CloudBase 3.9.2 adapter', () => {
  it('uses memory-only SDK persistence and obtains email from the authenticated identity', async () => {
    const driver = new CloudBaseAuthDriver()
    expect(sdk.init).toHaveBeenCalledWith(
      expect.objectContaining({ persistence: 'none', auth: { detectSessionInUrl: false } })
    )
    expect(await driver.login('submitted@example.com', 'password')).toEqual({
      ...tokens,
      email: 'identity@example.com'
    })
  })
  it('uses OTP callback bound by the SDK and does not auto-register users', async () => {
    const driver = new CloudBaseAuthDriver()
    await driver.sendCode('identity@example.com')
    expect(sdk.sendCode).toHaveBeenCalledWith({
      email: 'identity@example.com',
      options: { shouldCreateUser: false }
    })
    await expect(driver.verifyCode('someone-else@example.com', '123456')).rejects.toThrow('code')
    expect(sdk.verify).not.toHaveBeenCalled()
    await expect(driver.verifyCode('identity@example.com', '123456')).resolves.toMatchObject(tokens)
    expect(sdk.verify).toHaveBeenCalledWith({ token: '123456' })
    await expect(driver.verifyCode('identity@example.com', '123456')).rejects.toThrow('code')
  })
  it('clears OTP challenge on logout and revokes only the current session', async () => {
    const driver = new CloudBaseAuthDriver()
    await driver.sendCode('identity@example.com')
    await driver.logout()
    expect(sdk.logout).toHaveBeenCalledWith()
    await expect(driver.verifyCode('identity@example.com', '123456')).rejects.toThrow('code')
  })
  it('rejects anonymous identities even if the SDK returns a token', async () => {
    sdk.getUser.mockResolvedValue({
      data: { user: { email: 'identity@example.com', is_anonymous: true } },
      error: null
    })
    await expect(
      new CloudBaseAuthDriver().login('identity@example.com', 'password')
    ).rejects.toThrow('profile')
  })
  it('surfaces a safe error if captcha or additional verification is required', async () => {
    sdk.login.mockResolvedValue({
      error: { errorCode: 4001, message: 'internal request with password' }
    })
    await expect(
      new CloudBaseAuthDriver().login('identity@example.com', 'password')
    ).rejects.toThrow('verificationUnavailable')
  })
})

const profile = {
  userId: 'usr_test',
  email: 'identity@example.com',
  displayName: 'Captain',
  avatarDataUrl: null,
  occupation: '',
  organization: ''
}

function setupService() {
  const store = { read: vi.fn(() => tokens), write: vi.fn(() => true), clear: vi.fn() }
  const fetchProfile = vi.fn(async () => profile)
  const service = new AuthService(new CloudBaseAuthDriver(), store, fetchProfile)
  return { service, store, fetchProfile }
}

const login = (service: AuthService) =>
  service.login({ email: profile.email, password: 'synthetic-password' })

// Exercise the real SDK error representation through both the adapter and service. Only SDK
// requests and local storage are mocked; no account credentials or network requests are used.
describe('SDK authentication failures at the account gate', () => {
  it.each([
    { error: 'unauthenticated', expected: 'expired' },
    { error: 'invalid_grant', expected: 'expired' },
    { error: 'user_blocked', expected: 'inactive' },
    { error: 'unauthenticated', error_code: 16, expected: 'expired' },
    { error: 'unknown', error_code: '16', expected: 'expired' },
    { error: 'unknown', error_code: 401, expected: 'expired' },
    { error: '401', expected: 'expired' },
    { error: 'invalid_grant', error_code: 7, expected: 'expired' },
    { error: 'user_blocked', error_code: 7, expected: 'inactive' },
    { error: 'user_blocked', error_code: 16, expected: 'inactive' }
  ])('closes the gate for $error / $error_code', async ({ expected, ...rawError }) => {
    const { service, store, fetchProfile } = setupService()
    expect(await login(service)).toEqual({ ok: true })
    store.clear.mockClear()
    const error = new AuthError({ ...rawError, error_description: 'synthetic account rejection' })
    if (!rawError.error_code) expect(error.errorCode).toBeUndefined()
    sdk.refresh.mockResolvedValue({ data: { session: null }, error })

    expect(await service.refreshProfile()).toEqual({ ok: false, error: expected })
    expect(service.getState()).toMatchObject({
      status: 'signedOut',
      profile: null,
      remembered: false,
      error: expected
    })
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
    expect(() => service.getAccessToken(profile.userId)).toThrow('ACCOUNT_LOGIN_REQUIRED')
    expect(store.clear).toHaveBeenCalledOnce()
    expect(fetchProfile).toHaveBeenCalledTimes(1)
  })

  it.each(['refresh', 'getUser'] as const)(
    'clears the session when %s rejects with a terminal SDK error',
    async (operation) => {
      const { service, store } = setupService()
      await login(service)
      store.clear.mockClear()
      sdk[operation].mockRejectedValue(
        new AuthError({ error: 'unauthenticated', error_description: 'synthetic expiry' })
      )
      expect(await service.refreshProfile()).toEqual({ ok: false, error: 'expired' })
      expect(service.getState().status).toBe('signedOut')
      expect(store.clear).toHaveBeenCalledOnce()
      expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
    }
  )

  it('rejects a blocked identity even when token refresh succeeds', async () => {
    const { service, store } = setupService()
    await login(service)
    store.clear.mockClear()
    sdk.getUser.mockResolvedValue({
      data: { user: null },
      error: new AuthError({ error: 'user_blocked', error_description: 'synthetic blocked user' })
    })
    expect(await service.refreshProfile()).toEqual({ ok: false, error: 'inactive' })
    expect(service.getState().status).toBe('signedOut')
    expect(store.clear).toHaveBeenCalledOnce()
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
  })

  it.each(['refresh', 'getUser'] as const)(
    'preserves a validated session during a transient %s outage, then recovers',
    async (operation) => {
      const { service, store } = setupService()
      await login(service)
      store.clear.mockClear()
      sdk[operation].mockResolvedValueOnce({
        error: new AuthError({ error: 'unavailable', error_code: '14' })
      })
      expect(await service.refreshProfile()).toEqual({ ok: false, error: 'network' })
      expect(service.getState().status).toBe('signedIn')
      expect(() => service.assertCanStartTurn()).not.toThrow()
      expect(service.getAccessToken(profile.userId)).toBe(tokens.access_token)
      expect(store.clear).not.toHaveBeenCalled()
      expect(await service.refreshProfile()).toEqual({ ok: true })
    }
  )

  it.each([
    { error: 'unauthenticated', expected: 'expired', cleared: true },
    { error: 'user_blocked', expected: 'inactive', cleared: true },
    { error: 'unavailable', expected: 'network', cleared: false }
  ])('fails closed on startup for $error', async ({ error, expected, cleared }) => {
    const { service, store, fetchProfile } = setupService()
    sdk.restore.mockResolvedValue({ error: new AuthError({ error }) })
    expect(await service.restoreSession()).toEqual({ ok: false, error: expected })
    expect(service.getState().status).toBe('signedOut')
    expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
    expect(fetchProfile).not.toHaveBeenCalled()
    expect(store.clear).toHaveBeenCalledTimes(cleared ? 1 : 0)
  })

  it.each(['success', 'expired'] as const)(
    'ignores a late SDK refresh %s after logout',
    async (outcome) => {
      const { service, store } = setupService()
      await login(service)
      let finish!: (value: unknown) => void
      sdk.refresh.mockReturnValueOnce(
        new Promise((resolve) => {
          finish = resolve
        })
      )
      const pending = service.refreshProfile()
      await vi.waitFor(() => expect(sdk.refresh).toHaveBeenCalledOnce())
      await service.logout()
      store.write.mockClear()
      store.clear.mockClear()
      const loggedOutState = service.getState()
      finish(
        outcome === 'success' ? result : { error: new AuthError({ error: 'unauthenticated' }) }
      )
      expect(await pending).toEqual({ ok: false, error: 'expired' })
      expect(service.getState()).toEqual(loggedOutState)
      expect(store.write).not.toHaveBeenCalled()
      expect(store.clear).not.toHaveBeenCalled()
      expect(() => service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
    }
  )
})

describe('safe SDK error normalization', () => {
  it.each([
    { code: '16' },
    { code: 401 },
    { errorCode: '401' },
    { code: 'invalid_grant', errorCode: '7' },
    { code: 'UNAUTHENTICATED', errorCode: '14' }
  ])('normalizes terminal string and numeric fields: %j', (error) => {
    expect(
      sdkFailure({ ...error, message: 'network request with private details' }, 'network')
    ).toMatchObject({ code: 'expired', message: 'expired' })
  })

  it.each([
    { code: '429', expected: 'rateLimit' },
    { errorCode: '8', expected: 'rateLimit' },
    { errorCode: '4001', expected: 'verificationUnavailable' },
    { errorCode: '14', expected: 'network' },
    { message: 'fetch failed', expected: 'network' },
    { code: 'invalid_password', expected: 'credentials' }
  ])('preserves actionable non-terminal errors: %j', ({ expected, ...error }) => {
    expect(sdkFailure(error, 'credentials').code).toBe(expected)
  })

  it('does not present a password-provider configuration error as a wrong password', () => {
    expect(
      sdkFailure(
        {
          code: 'UNKNOWN',
          message: '当前使用「用户名密码登录」，需确保控制台已开启「用户名密码登录」'
        },
        'credentials'
      ).code
    ).toBe('unknown')
  })
})
