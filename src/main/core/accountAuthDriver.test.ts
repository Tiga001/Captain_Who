import { beforeEach, describe, expect, it, vi } from 'vitest'
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
import { CloudBaseAuthDriver } from '../auth/CloudBaseAuthDriver'

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
